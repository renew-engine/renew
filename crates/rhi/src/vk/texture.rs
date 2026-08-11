//! A sampled image: created, filled once from host bytes, then read-only
//! to the GPU for the rest of its life.
//!
//! Upload is synchronous and happens entirely inside creation. That is a
//! deliberate narrowing rather than a simplification waiting to be
//! fixed: a texture that can never be written after it exists cannot be
//! written while a submit that reads it is in flight, so the whole class
//! of "who is allowed to touch this, and when" questions never arises.
//! An animated atlas needs the opposite property and will need a
//! different type; see the contract on [`Texture`].
//!
//! **The format is UNORM, so the bytes handed in are the values
//! sampled, with no hardware colour conversion on the way.** That is
//! the only choice a byte-comparing reference image can be written
//! against today, and it is stated because it is a real constraint
//! rather than an oversight: an atlas authored in an image editor is
//! sRGB-encoded, and sampling it as UNORM reads the encoded values as
//! though they were linear. Giving the descriptor an explicit format,
//! so such an atlas can be decoded by the hardware on read, belongs
//! with the wider colour-handling decision and not ahead of it.

use std::rc::Rc;

use ash::vk;

use crate::config::Extent;
use crate::error::TargetError;
use crate::vk::device::{Device, DeviceShared, FENCE_TIMEOUT_NS};
use crate::vk::offscreen::{BPP, color_range, creation, image_memory_type, pick_memory_type};
use crate::vk::pipeline::TargetFormat;

/// The pixels and dimensions of a sampled image.
///
/// **`#[non_exhaustive]` with a constructor, per the descriptor pattern
/// this crate uses for its other descriptors** — format, mip levels and
/// array layers arrive as builders touching no existing caller.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct TextureDesc<'a> {
    /// Dimensions in texels. Neither may be zero.
    pub extent: Extent,
    /// Tightly packed RGBA8 rows, top row first.
    ///
    /// Length must be exactly `extent.width * extent.height * 4`.
    pub rgba8: &'a [u8],
}

impl<'a> TextureDesc<'a> {
    /// A texture of `extent`, filled from `rgba8`.
    #[must_use]
    pub fn new(extent: Extent, rgba8: &'a [u8]) -> Self {
        Self { extent, rgba8 }
    }
}

/// No memory type satisfies a resource's requirements.
///
/// Not a driver refusal, so it carries no result code — the driver
/// answered, and the answer was that nothing fits.
///
/// **A named function rather than a struct literal at each site**, so
/// the `?` that propagates it shares a line with a call made on every
/// pass. `ok_or` takes its argument eagerly, so a literal spelled out
/// inline can leave the closing `)?;` holding nothing but the
/// propagation — a region reachable only on a machine where no memory
/// type fits, which is to say not reachable at all.
pub(crate) fn no_memory_type(call: &'static str) -> TargetError {
    TargetError::Creation { call, code: 0 }
}

/// The byte length `extent` demands, or `None` if it does not fit a
/// `u64` — a pure function so the overflow case is provable without a
/// device, and without allocating the terabytes that would reach it.
fn required_bytes(extent: Extent) -> Option<u64> {
    u64::from(extent.width)
        .checked_mul(u64::from(extent.height))?
        .checked_mul(BPP)
}

/// The size contract as a value, so the release-build verdict is
/// provable: a dev build asserts long before the returned error could be
/// observed, and every test runs with assertions on.
///
/// Both halves matter. A zero extent is rejected because Vulkan forbids
/// it. A mismatched slice is rejected because the copy below reads
/// exactly `width * height * 4` bytes from the mapping, and a short
/// slice would leave the tail of the image as whatever the staging
/// allocation happened to contain — a bug that renders as plausible
/// garbage rather than as a failure.
fn check_desc(desc: &TextureDesc<'_>, max_dimension: u32) -> Result<u64, TargetError> {
    if desc.extent.width == 0 || desc.extent.height == 0 {
        return Err(TargetError::Creation {
            call: "create_texture(zero extent)",
            code: 0,
        });
    }
    // Handed in, not queried -- see the note in `check_extent`.
    if desc.extent.width > max_dimension || desc.extent.height > max_dimension {
        return Err(TargetError::Creation {
            call: "create_texture(extent exceeds the device limit)",
            code: 0,
        });
    }
    let needed = required_bytes(desc.extent).ok_or(TargetError::Creation {
        call: "create_texture(extent overflows)",
        code: 0,
    })?;
    if u64::try_from(desc.rgba8.len()) != Ok(needed) {
        return Err(TargetError::Creation {
            call: "create_texture(pixel length)",
            code: 0,
        });
    }
    Ok(needed)
}

/// Everything created so far during upload, destroyed in reverse order
/// when a later step fails.
///
/// The staging buffer and its memory are transient — they are destroyed
/// on the success path too, once the copy has been waited for. They live
/// here so that a failure *between* their creation and that point does
/// not leak them.
struct Partial<'a> {
    shared: &'a Rc<DeviceShared>,
    image: Option<vk::Image>,
    memory: Option<vk::DeviceMemory>,
    view: Option<vk::ImageView>,
    buffer: Option<vk::Buffer>,
    buffer_memory: Option<vk::DeviceMemory>,
    mapped: bool,
    pool: Option<vk::CommandPool>,
    fence: Option<vk::Fence>,
}

impl Drop for Partial<'_> {
    fn drop(&mut self) {
        let device = &self.shared.device;
        let cbs = self.shared.alloc_cbs();
        // Quiesce before destroying anything a submit touched, which is
        // what both other fence sites in this crate already do and what
        // this one originally did not.
        //
        // **Waiting the fence is enough for the specification and not
        // enough in practice.** `vkDestroyFence` asks only that the
        // submissions referring to the fence have completed, and they
        // have. But the validation layer keeps its own fence state,
        // retires it on a thread of its own, and will free that state
        // here while the other thread is still unlocking a lock embedded
        // in it — a race the sanitizer reports against two of the
        // layer's own frames, reproducibly, with no engine frame in the
        // second stack.
        //
        // Guarded on the fence because it is the only handle here tied
        // to a submit: a failure before the fence existed has nothing in
        // flight to wait for, and this runs on every upload.
        if self.fence.is_some() {
            // SAFETY: device live via the spine `Rc`; blocking until
            // every queue is idle. Best-effort quiesce; failure is
            // logged, never a panic (D5) — the diag record is the only
            // observable this path has.
            if let Err(code) = unsafe { device.device_wait_idle() } {
                renew_diag::error!(
                    target: "renew-rhi",
                    "wait-idle at upload teardown failed: {code:?}"
                );
            }
        }
        // SAFETY: category 2 (ash dispatch): every handle in an
        // `Option` here was created by this module with these callbacks
        // and has not been destroyed; the device outlives them via the
        // spine `Rc`. Destroyed in reverse creation order, dependents
        // first.
        unsafe {
            if let Some(fence) = self.fence.take() {
                device.destroy_fence(fence, Some(&cbs));
            }
            if let Some(pool) = self.pool.take() {
                device.destroy_command_pool(pool, Some(&cbs));
            }
            if self.mapped {
                if let Some(memory) = self.buffer_memory {
                    device.unmap_memory(memory);
                }
                self.mapped = false;
            }
            if let Some(buffer) = self.buffer.take() {
                device.destroy_buffer(buffer, Some(&cbs));
            }
            if let Some(memory) = self.buffer_memory.take() {
                device.free_memory(memory, Some(&cbs));
            }
            if let Some(view) = self.view.take() {
                device.destroy_image_view(view, Some(&cbs));
            }
            if let Some(image) = self.image.take() {
                device.destroy_image(image, Some(&cbs));
            }
            if let Some(memory) = self.memory.take() {
                device.free_memory(memory, Some(&cbs));
            }
        }
    }
}

/// The texture's owning half, behind the handle the caller holds —
/// the mesh split, applied here so a binding keeps the image alive
/// with an `Rc` clone while callers pass plain borrows and drop
/// whenever they like. The contract lives on [`Texture`], where its
/// reader is.
pub(crate) struct TextureInner {
    pub(crate) shared: Rc<DeviceShared>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    pub(crate) view: vk::ImageView,
    extent: Extent,
}

/// An image the GPU can sample. Holds its device alive; destroyed on
/// drop.
///
/// # Contract
///
/// A texture must outlive every descriptor set that references it, and
/// its contents never change after creation.
///
/// **Immutability is what makes the first half tractable.** Because no
/// host write can reach the image after it exists, the only ordering
/// question left is destruction, and destruction is owned by the
/// [`Binding`](crate::Binding) whose set references it — by holding the
/// texture's inner half, not by asking the caller to sequence drops.
/// `Drop` therefore needs no quiesce: a submit that reads this image
/// can only have been recorded against a binding that is keeping it
/// alive.
///
/// A texture whose pixels change — an animated or glyph atlas — breaks
/// that argument rather than extending it, and needs a type that states
/// its own rule about when a write is legal.
pub struct Texture {
    pub(crate) inner: Rc<TextureInner>,
}

impl Texture {
    /// The texture's size in texels.
    #[must_use]
    pub fn extent(&self) -> Extent {
        self.inner.extent
    }

    /// The device spine this texture belongs to, for the cross-device
    /// contract check the pipeline makes before writing a descriptor.
    pub(crate) fn shared(&self) -> &Rc<DeviceShared> {
        &self.inner.shared
    }
}

impl core::fmt::Debug for Texture {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Texture")
            .field("extent", &self.inner.extent)
            .finish_non_exhaustive()
    }
}

impl Drop for TextureInner {
    fn drop(&mut self) {
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // `Rc`; all three handles were created here with these callbacks
        // and are destroyed once. No quiesce: see the contract above —
        // anything that could have referenced this image in a submit is
        // holding it, so it cannot be here.
        unsafe {
            let cbs = self.shared.alloc_cbs();
            self.shared.device.destroy_image_view(self.view, Some(&cbs));
            self.shared.device.destroy_image(self.image, Some(&cbs));
            self.shared.device.free_memory(self.memory, Some(&cbs));
        }
    }
}

impl Device {
    /// Create a sampled image and fill it from host bytes.
    ///
    /// The upload is synchronous: this returns once the copy has
    /// completed and the image is readable by a shader.
    ///
    /// # Errors
    ///
    /// [`TargetError::Creation`] if `desc` is malformed — a zero extent,
    /// or a pixel slice whose length is not `width * height * 4` — or if
    /// the driver refuses a creation call.
    /// [`TargetError::OutOfDeviceMemory`] if an allocation fails for
    /// want of device memory. [`TargetError::Timeout`] if the upload
    /// does not complete within the fence timeout.
    /// [`TargetError::DeviceLost`] if the device was already lost, or is
    /// lost by this upload.
    ///
    /// # Panics
    ///
    /// In a dev build, if `desc` is malformed. The returned error is the
    /// release-build verdict for the same condition.
    pub fn create_texture(&self, desc: &TextureDesc<'_>) -> Result<Texture, TargetError> {
        // Fatal in dev builds; in release, where the assertion is
        // compiled out, the same verdict is returned instead.
        //
        // **Every value in the message is bound first and captured
        // inline.** A call left inside the argument list becomes a
        // region that runs only when the assertion fails, which is to
        // say never — and an unreachable region is a hole in the
        // coverage gate rather than a nicety.
        let checked = check_desc(desc, self.shared.max_image_dimension_2d);
        let extent = desc.extent;
        let supplied = desc.rgba8.len();
        debug_assert!(
            checked.is_ok(),
            "a texture needs a non-zero extent and exactly width*height*4 bytes; \
             got {extent:?} with {supplied} bytes"
        );
        let byte_len = checked?;
        if self.shared.lost.poisoned() {
            return Err(TargetError::DeviceLost);
        }
        let shared = &self.shared;
        let mut partial = Partial {
            shared,
            image: None,
            memory: None,
            view: None,
            buffer: None,
            buffer_memory: None,
            mapped: false,
            pool: None,
            fence: None,
        };
        // On the way out, `partial` destroys whatever it still holds:
        // on failure that is everything, and on success only the
        // transient staging state, `upload` having released the three
        // handles the texture takes over.
        upload(shared, desc, byte_len, &mut partial)
    }
}

/// The upload proper, split out so `create_texture` owns only the
/// contract check and the handover of ownership out of `Partial`.
#[expect(
    clippy::too_many_lines,
    reason = "one linear resource ladder; splitting it would separate each handle from the cleanup slot it must be recorded in"
)]
fn upload(
    shared: &Rc<DeviceShared>,
    desc: &TextureDesc<'_>,
    byte_len: u64,
    partial: &mut Partial<'_>,
) -> Result<Texture, TargetError> {
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(TargetFormat::Rgba8Unorm.to_vk())
        .extent(vk::Extent3D {
            width: desc.extent.width,
            height: desc.extent.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: category 2 (ash dispatch): device live via the spine; the
    // create info is a local; the callbacks' ledger outlives the image.
    // (The same argument covers every dispatch call in this function.)
    let image = unsafe {
        shared
            .device
            .create_image(&image_info, Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkCreateImage", code))?;
    partial.image = Some(image);

    // SAFETY: image live.
    let requirements = unsafe { shared.device.get_image_memory_requirements(image) };
    // SAFETY: instance and physical device live via the spine.
    let memory_properties = unsafe {
        shared
            .instance
            .get_physical_device_memory_properties(shared.physical)
    };
    let image_type = image_memory_type(&memory_properties, requirements.memory_type_bits)
        .ok_or(no_memory_type("texture memory type"))?;
    let alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(image_type);
    // SAFETY: device live; info local.
    let memory = unsafe {
        shared
            .device
            .allocate_memory(&alloc, Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkAllocateMemory(texture)", code))?;
    partial.memory = Some(memory);
    // SAFETY: image and memory live; offset 0 within an allocation sized
    // from this image's own requirements.
    unsafe { shared.device.bind_image_memory(image, memory, 0) }
        .map_err(|code| creation("vkBindImageMemory", code))?;

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(TargetFormat::Rgba8Unorm.to_vk())
        .subresource_range(color_range());
    // SAFETY: image live and bound.
    let view = unsafe {
        shared
            .device
            .create_image_view(&view_info, Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkCreateImageView", code))?;
    partial.view = Some(view);

    // ---- staging: host-visible bytes the copy reads from ------------
    let buffer_info = vk::BufferCreateInfo::default()
        .size(byte_len)
        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: device live; info local.
    let buffer = unsafe {
        shared
            .device
            .create_buffer(&buffer_info, Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkCreateBuffer", code))?;
    partial.buffer = Some(buffer);

    // SAFETY: buffer live.
    let buffer_requirements = unsafe { shared.device.get_buffer_memory_requirements(buffer) };
    let buffer_type = pick_memory_type(
        &memory_properties,
        buffer_requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )
    .ok_or(no_memory_type("staging memory type"))?;
    let buffer_alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(buffer_requirements.size)
        .memory_type_index(buffer_type);
    // SAFETY: device live; info local.
    let buffer_memory = unsafe {
        shared
            .device
            .allocate_memory(&buffer_alloc, Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkAllocateMemory(staging)", code))?;
    partial.buffer_memory = Some(buffer_memory);
    // SAFETY: buffer and memory live; offset 0 within an allocation
    // sized from this buffer's own requirements.
    unsafe { shared.device.bind_buffer_memory(buffer, buffer_memory, 0) }
        .map_err(|code| creation("vkBindBufferMemory", code))?;
    // SAFETY: memory live, HOST_VISIBLE, not already mapped; WHOLE_SIZE
    // maps the full allocation.
    let mapped = unsafe {
        shared.device.map_memory(
            buffer_memory,
            0,
            vk::WHOLE_SIZE,
            vk::MemoryMapFlags::empty(),
        )
    }
    .map_err(|code| creation("vkMapMemory", code))?;
    partial.mapped = true;
    // SAFETY: `mapped` addresses at least `buffer_requirements.size`
    // bytes, which is at least `byte_len`; `check_desc` proved the
    // source slice is exactly `byte_len` long; the regions cannot
    // overlap, the mapping being a fresh device allocation. The memory
    // is HOST_COHERENT, so no explicit flush is needed before the
    // submit below.
    unsafe {
        std::ptr::copy_nonoverlapping(desc.rgba8.as_ptr(), mapped.cast::<u8>(), desc.rgba8.len());
    }

    // ---- the copy, recorded and waited for --------------------------
    let pool_info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::TRANSIENT)
        .queue_family_index(shared.queue_family);
    // SAFETY: device live; info local.
    let pool = unsafe {
        shared
            .device
            .create_command_pool(&pool_info, Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkCreateCommandPool", code))?;
    partial.pool = Some(pool);

    let cmd_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: pool live; info local. The command buffer is freed with
    // the pool, so it needs no slot of its own in `Partial`.
    let cmd = unsafe { shared.device.allocate_command_buffers(&cmd_info) }
        .map_err(|code| creation("vkAllocateCommandBuffers", code))?
        .into_iter()
        .next()
        .ok_or(TargetError::Creation {
            call: "vkAllocateCommandBuffers(empty)",
            code: 0,
        })?;

    // SAFETY: device live; info local.
    let fence = unsafe {
        shared
            .device
            .create_fence(&vk::FenceCreateInfo::default(), Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkCreateFence", code))?;
    partial.fence = Some(fence);

    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the buffer is freshly allocated and not recording.
    unsafe { shared.device.begin_command_buffer(cmd, &begin) }
        .map_err(|code| creation("vkBeginCommandBuffer", code))?;

    let to_transfer_dst = vk::ImageMemoryBarrier2::default()
        // Nothing has touched the image, so there is nothing to wait
        // for; the destination scope is the copy about to be recorded.
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::COPY)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .image(image)
        .subresource_range(color_range());
    let barriers = [to_transfer_dst];
    let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
    // SAFETY: recording; the barrier array outlives the call.
    unsafe { shared.device.cmd_pipeline_barrier2(cmd, &dependency) };

    let region = vk::BufferImageCopy::default()
        // Zero means "tightly packed to the image extent", which is
        // what the descriptor's contract already requires of the slice.
        .buffer_offset(0)
        .buffer_row_length(0)
        .buffer_image_height(0)
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .mip_level(0)
                .base_array_layer(0)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width: desc.extent.width,
            height: desc.extent.height,
            depth: 1,
        });
    let regions = [region];
    // SAFETY: recording; buffer and image live; the image is in
    // TRANSFER_DST_OPTIMAL by the barrier above; the region covers
    // exactly the extent the buffer was sized for.
    unsafe {
        shared.device.cmd_copy_buffer_to_image(
            cmd,
            buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &regions,
        );
    }

    let to_shader_read = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COPY)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        // The destination scope names the fragment stage because that
        // is where this crate samples. A texture read from a vertex or
        // compute stage would need that stage added here.
        .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .image(image)
        .subresource_range(color_range());
    let barriers = [to_shader_read];
    let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
    // SAFETY: recording; the barrier array outlives the call.
    unsafe { shared.device.cmd_pipeline_barrier2(cmd, &dependency) };

    // SAFETY: recording, with every recorded command complete.
    unsafe { shared.device.end_command_buffer(cmd) }
        .map_err(|code| creation("vkEndCommandBuffer", code))?;

    let cmd_infos = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
    let submits = [vk::SubmitInfo2::default().command_buffer_infos(&cmd_infos)];
    // SAFETY: queue live via the spine; the arrays outlive the call; the
    // fence is unsignaled and not in use.
    if let Err(code) = unsafe { shared.device.queue_submit2(shared.queue, &submits, fence) } {
        // Poison the device before returning, exactly as every other
        // submitting path does. A loss reported only as a creation
        // error would leave `lost` unset, and every later render would
        // pass its own guard and submit to a dead device.
        shared.note_result(code);
        if code == vk::Result::ERROR_DEVICE_LOST {
            // The staging buffer this submit reads is destroyed by
            // `Partial::drop`, which quiesces first — so a queue whose
            // execution state is undefined is already accounted for.
            return Err(TargetError::DeviceLost);
        }
        return Err(creation("vkQueueSubmit2", code));
    }

    let fences = [fence];
    // SAFETY: fence live and associated with the submit above.
    let waited = unsafe {
        shared
            .device
            .wait_for_fences(&fences, true, FENCE_TIMEOUT_NS)
    };
    match waited {
        Ok(()) => {}
        Err(vk::Result::TIMEOUT) => {
            // The submit is still running and it reads the staging
            // buffer, so that buffer must not be destroyed yet.
            // `Partial::drop` quiesces before destroying anything, which
            // is what makes returning here safe.
            return Err(TargetError::Timeout {
                call: "vkWaitForFences(texture upload)",
            });
        }
        Err(code) => {
            shared.note_result(code);
            // Same argument as the timeout arm: the submit's state is
            // unknown, and `Partial::drop` is what waits before
            // destroying anything it reads.
            return Err(if code == vk::Result::ERROR_DEVICE_LOST {
                TargetError::DeviceLost
            } else {
                creation("vkWaitForFences(texture upload)", code)
            });
        }
    }

    // Ownership of these three passes to the `Texture` here, at the
    // point it is constructed, so the release and the transfer cannot
    // drift apart. Everything still recorded in `partial` -- the
    // staging buffer, its mapping and memory, the pool, the fence -- is
    // transient and is destroyed by its `Drop`, the wait above having
    // proved the submit that read them is complete.
    partial.image = None;
    partial.memory = None;
    partial.view = None;
    Ok(Texture {
        inner: Rc::new(TextureInner {
            shared: Rc::clone(shared),
            image,
            memory,
            view,
            extent: desc.extent,
        }),
    })
}

#[cfg(test)]
mod tests {

    /// The same refusal on the upload path, for the same reason.
    #[test]
    fn a_texture_past_the_device_limit_is_refused_by_name() {
        let pixels = [0u8; 16];
        let refusal = check_desc(
            &TextureDesc::new(
                Extent {
                    width: 65,
                    height: 2,
                },
                &pixels,
            ),
            64,
        )
        .expect_err("past the limit is not a texture");
        assert!(
            matches!(refusal, TargetError::Creation { call, code: 0 }
                if call == "create_texture(extent exceeds the device limit)"),
            "refused as {refusal:?}, which names no cause"
        );

        // Reported before the pixel-length rule, so a caller whose
        // extent is impossible hears about the extent rather than about
        // a byte count derived from it.
        let refusal = check_desc(
            &TextureDesc::new(
                Extent {
                    width: 65,
                    height: 65,
                },
                &pixels,
            ),
            64,
        )
        .expect_err("past the limit is not a texture");
        assert!(
            matches!(refusal, TargetError::Creation { call, .. }
                if call == "create_texture(extent exceeds the device limit)"),
            "the extent rule must be reported before the length rule: {refusal:?}"
        );

        assert!(
            check_desc(
                &TextureDesc::new(
                    Extent {
                        width: 2,
                        height: 2,
                    },
                    &pixels,
                ),
                2,
            )
            .is_ok(),
            "the limit itself is allowed"
        );
    }
    use super::*;

    /// The byte count is `width * height * 4`, and the multiplication is
    /// checked. Both asserted without a device, because the overflow
    /// case is unreachable through any allocation a test could make.
    #[test]
    fn the_required_byte_count_is_checked_arithmetic() {
        assert_eq!(
            required_bytes(Extent {
                width: 3,
                height: 5,
            }),
            Some(60)
        );
        assert_eq!(
            required_bytes(Extent {
                width: u32::MAX,
                height: u32::MAX,
            }),
            None,
            "u32::MAX squared then quadrupled exceeds u64 and must not wrap"
        );
    }

    /// Each malformed descriptor names itself in the error, so a caller
    /// reading the failure learns which rule they broke rather than that
    /// creation failed.
    #[test]
    fn every_malformed_descriptor_is_rejected_by_name() {
        let good = Extent {
            width: 2,
            height: 2,
        };
        let pixels = [0u8; 16];

        assert!(matches!(
            check_desc(
                &TextureDesc::new(
                    Extent {
                        width: 0,
                        height: 2
                    },
                    &pixels
                ),
                u32::MAX
            ),
            Err(TargetError::Creation {
                call: "create_texture(zero extent)",
                ..
            })
        ));
        assert!(matches!(
            check_desc(
                &TextureDesc::new(
                    Extent {
                        width: 2,
                        height: 0
                    },
                    &pixels
                ),
                u32::MAX
            ),
            Err(TargetError::Creation {
                call: "create_texture(zero extent)",
                ..
            })
        ));
        assert!(matches!(
            check_desc(
                &TextureDesc::new(
                    Extent {
                        width: u32::MAX,
                        height: u32::MAX
                    },
                    &pixels
                ),
                u32::MAX
            ),
            Err(TargetError::Creation {
                call: "create_texture(extent overflows)",
                ..
            })
        ));
        assert!(matches!(
            check_desc(&TextureDesc::new(good, &pixels[..15]), u32::MAX),
            Err(TargetError::Creation {
                call: "create_texture(pixel length)",
                ..
            })
        ));
        assert!(matches!(
            check_desc(&TextureDesc::new(good, &pixels), u32::MAX),
            Ok(16)
        ));
    }
}
