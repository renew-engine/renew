//! The headless render target: an RGBA8 image rendered synchronously
//! and read back to host memory. This is the correctness spine — the
//! golden tests prove pixels without a window or a display server.

use std::rc::Rc;

use ash::vk;

use crate::config::Extent;
use crate::error::TargetError;
use crate::vk::device::{Device, DeviceShared, FENCE_TIMEOUT_NS};
use crate::vk::pipeline::{RenderDesc, TargetFormat};

/// Bytes per pixel of the fixed RGBA8 format.
const BPP: u64 = 4;

fn creation(call: &'static str, code: vk::Result) -> TargetError {
    match code {
        vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => TargetError::OutOfDeviceMemory { call },
        _ => TargetError::Creation {
            call,
            code: code.as_raw(),
        },
    }
}

/// Locate a memory type index satisfying `type_bits` and `flags`.
fn pick_memory_type(
    properties: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    // `memory_types` is a fixed-size array and `type_bits` has one bit
    // per type: a driver over-reporting the count would index out of
    // bounds and shift out of range. Clamp rather than trust it.
    let count = properties
        .memory_type_count
        .min(vk::MAX_MEMORY_TYPES.try_into().unwrap_or(u32::MAX));
    (0..count).find(|&index| {
        let supported = type_bits & (1 << index) != 0;
        let type_flags = properties.memory_types[index as usize].property_flags;
        supported && type_flags.contains(flags)
    })
}

/// The memory type for the render image: device-local when this image
/// can live there, any type it accepts otherwise.
///
/// The spec guarantees a device-local memory type exists, but not that
/// a given image's `memoryTypeBits` includes one — the fallback is what
/// keeps bring-up working on an implementation that does not, and it
/// lives here, pure, because no test machine can be made into one.
fn image_memory_type(
    properties: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
) -> Option<u32> {
    pick_memory_type(properties, type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
        .or_else(|| pick_memory_type(properties, type_bits, vk::MemoryPropertyFlags::empty()))
}

/// The non-zero-extent contract as a value, so the release-build
/// verdict is provable: a dev build asserts on a zero extent long
/// before the returned error could be observed, and every test runs
/// with assertions on.
fn check_extent(extent: Extent) -> Result<(), TargetError> {
    if extent.width == 0 || extent.height == 0 {
        return Err(TargetError::Creation {
            call: "create_offscreen_target(zero extent)",
            code: 0,
        });
    }
    Ok(())
}

/// Everything created so far during bring-up, destroyed in reverse
/// order when a later step fails.
///
/// The fence has no slot here: it is the last handle [`build`] creates
/// and nothing fallible follows it, so an unwinder could never observe
/// one. A new fallible step after the fence must add it back.
#[derive(Default)]
struct Partial {
    image: Option<vk::Image>,
    image_memory: Option<vk::DeviceMemory>,
    view: Option<vk::ImageView>,
    buffer: Option<vk::Buffer>,
    buffer_memory: Option<vk::DeviceMemory>,
    mapped: bool,
    pool: Option<vk::CommandPool>,
}

impl Partial {
    fn destroy(&mut self, shared: &DeviceShared) {
        // SAFETY: category 2: every present handle was created with
        // these callbacks and nothing submitted work against them yet.
        unsafe {
            if let Some(pool) = self.pool.take() {
                shared
                    .device
                    .destroy_command_pool(pool, Some(&shared.alloc_cbs()));
            }
            if let Some(memory) = self.buffer_memory.take() {
                if self.mapped {
                    shared.device.unmap_memory(memory);
                }
                shared.device.free_memory(memory, Some(&shared.alloc_cbs()));
            }
            if let Some(buffer) = self.buffer.take() {
                shared
                    .device
                    .destroy_buffer(buffer, Some(&shared.alloc_cbs()));
            }
            if let Some(view) = self.view.take() {
                shared
                    .device
                    .destroy_image_view(view, Some(&shared.alloc_cbs()));
            }
            if let Some(memory) = self.image_memory.take() {
                shared.device.free_memory(memory, Some(&shared.alloc_cbs()));
            }
            if let Some(image) = self.image.take() {
                shared
                    .device
                    .destroy_image(image, Some(&shared.alloc_cbs()));
            }
        }
    }
}

/// A fixed-size RGBA8 render target with synchronous CPU readback.
///
/// Pixels are tightly packed, row-major, RGBA byte order. Before the
/// first [`render`](Self::render), the readback contents are
/// unspecified.
pub struct OffscreenTarget {
    shared: Rc<DeviceShared>,
    extent: Extent,
    image: vk::Image,
    image_memory: vk::DeviceMemory,
    view: vk::ImageView,
    buffer: vk::Buffer,
    buffer_memory: vk::DeviceMemory,
    mapped: *const u8,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    /// Set when a submitted frame never provably completed (a fence
    /// wait timed out or failed): GPU work may still be writing the
    /// readback buffer, so reading it would be a data race, and the
    /// command buffer and fence carry unknown pending state. A wedged
    /// target refuses further work; drop it (Drop quiesces).
    wedged: bool,
}

impl Device {
    /// Create an offscreen target. Zero-sized extents are a contract
    /// violation (there is no minimized-window story headless).
    ///
    /// # Errors
    ///
    /// Creation and memory errors from the driver;
    /// [`TargetError::DeviceLost`] on a poisoned device.
    pub fn create_offscreen_target(&self, extent: Extent) -> Result<OffscreenTarget, TargetError> {
        // Fatal in dev builds; in release, where the assertion is
        // compiled out, the same verdict is returned instead.
        let checked = check_extent(extent);
        debug_assert!(
            checked.is_ok(),
            "offscreen targets need a non-zero extent, got {extent:?}"
        );
        checked?;
        if self.shared.lost.poisoned() {
            return Err(TargetError::DeviceLost);
        }
        let shared = &self.shared;
        let mut partial = Partial::default();
        let result = build(shared, extent, &mut partial);
        if result.is_err() {
            partial.destroy(shared);
        }
        result
    }
}

/// The fallible bring-up body; `partial` records what exists so the
/// caller can unwind it on failure.
#[expect(
    clippy::too_many_lines,
    reason = "one linear bring-up ladder; splitting it hides the creation order the unwinder must mirror"
)]
fn build(
    shared: &Rc<DeviceShared>,
    extent: Extent,
    partial: &mut Partial,
) -> Result<OffscreenTarget, TargetError> {
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(TargetFormat::Rgba8Unorm.to_vk())
        .extent(vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
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
    let image_requirements = unsafe { shared.device.get_image_memory_requirements(image) };
    // SAFETY: instance and physical device live via the spine. Read
    // once: the table is a property of the adapter, not of the moment.
    let memory_properties = unsafe {
        shared
            .instance
            .get_physical_device_memory_properties(shared.physical)
    };
    let image_type = image_memory_type(&memory_properties, image_requirements.memory_type_bits)
        .ok_or(TargetError::Creation {
            call: "image memory type",
            code: 0,
        })?;
    let image_alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(image_requirements.size)
        .memory_type_index(image_type);
    // SAFETY: device live; info local.
    let image_memory = unsafe {
        shared
            .device
            .allocate_memory(&image_alloc, Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkAllocateMemory(image)", code))?;
    partial.image_memory = Some(image_memory);
    // SAFETY: image and memory live; offset 0 within the allocation
    // sized from this image's own requirements.
    unsafe { shared.device.bind_image_memory(image, image_memory, 0) }
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

    let byte_len = u64::from(extent.width) * u64::from(extent.height) * BPP;
    let buffer_info = vk::BufferCreateInfo::default()
        .size(byte_len)
        .usage(vk::BufferUsageFlags::TRANSFER_DST)
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
    .ok_or(TargetError::Creation {
        call: "readback memory type",
        code: 0,
    })?;
    let buffer_alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(buffer_requirements.size)
        .memory_type_index(buffer_type);
    // SAFETY: device live; info local.
    let buffer_memory = unsafe {
        shared
            .device
            .allocate_memory(&buffer_alloc, Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkAllocateMemory(readback)", code))?;
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

    let pool_info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
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
    // SAFETY: pool live; info local.
    let cmd = unsafe { shared.device.allocate_command_buffers(&cmd_info) }
        .map_err(|code| creation("vkAllocateCommandBuffers", code))?
        .into_iter()
        .next()
        .ok_or(TargetError::Creation {
            call: "vkAllocateCommandBuffers",
            code: 0,
        })?;

    // SAFETY: device live; default info (unsignaled) local.
    let fence = unsafe {
        shared
            .device
            .create_fence(&vk::FenceCreateInfo::default(), Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkCreateFence", code))?;

    // Ownership transfers to the target; disarm the unwinder.
    *partial = Partial::default();
    Ok(OffscreenTarget {
        shared: Rc::clone(shared),
        extent,
        image,
        image_memory,
        view,
        buffer,
        buffer_memory,
        mapped: mapped.cast::<u8>().cast_const(),
        pool,
        cmd,
        fence,
        wedged: false,
    })
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1)
}

impl OffscreenTarget {
    /// The target size.
    #[must_use]
    pub fn extent(&self) -> Extent {
        self.extent
    }

    /// The size of one full readback: `width * height * 4` bytes.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        usize::try_from(u64::from(self.extent.width) * u64::from(self.extent.height) * BPP)
            .unwrap_or(usize::MAX)
    }

    /// Clear, optionally draw one 3-vertex call with `pipeline`, and
    /// wait for completion. On return, the pixels are readable via
    /// [`read_back_into`](Self::read_back_into).
    ///
    /// The pipeline must come from this target's device and target
    /// [`TargetFormat::Rgba8Unorm`] — contract violations, checked in
    /// dev builds.
    ///
    /// # Errors
    ///
    /// [`TargetError::Timeout`] when the GPU exceeds the watchdog —
    /// the target is then wedged (submitted work never provably
    /// finished) and refuses further use; drop and recreate it.
    /// [`TargetError::DeviceLost`] on device loss (the device is then
    /// poisoned); command/submission failures otherwise.
    #[expect(
        clippy::too_many_lines,
        reason = "one recorded command stream; the barrier ordering reads top to bottom"
    )]
    pub fn render(&mut self, desc: &RenderDesc<'_>) -> Result<(), TargetError> {
        let RenderDesc {
            clear, pipeline, ..
        } = *desc;
        if self.wedged {
            return Err(TargetError::Timeout {
                call: "target wedged by an earlier incomplete frame",
            });
        }
        if self.shared.lost.poisoned() {
            return Err(TargetError::DeviceLost);
        }
        if let Some(pipeline) = pipeline {
            debug_assert!(
                Rc::ptr_eq(&self.shared, &pipeline.shared),
                "pipeline and target come from different devices"
            );
            debug_assert!(
                pipeline.format == TargetFormat::Rgba8Unorm,
                "pipeline targets {:?}, offscreen is Rgba8Unorm",
                pipeline.format
            );
        }
        let device = &self.shared.device;

        // SAFETY: category 2 (ash dispatch) for every call below:
        // device, image, view, buffer, pool, cmd, fence all live and
        // owned by this target; single-threaded by the crate contract;
        // every info struct is a local outliving its call. Recording
        // matches the layout transitions it declares.
        unsafe {
            device
                .reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())
                .map_err(|code| creation("vkResetCommandBuffer", code))?;
            device
                .begin_command_buffer(
                    self.cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|code| creation("vkBeginCommandBuffer", code))?;

            // UNDEFINED → COLOR_ATTACHMENT_OPTIMAL: nothing to wait on,
            // block the color-output stage that follows.
            let to_color = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::NONE)
                .src_access_mask(vk::AccessFlags2::NONE)
                .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .image(self.image)
                .subresource_range(color_range());
            let barriers = [to_color];
            device.cmd_pipeline_barrier2(
                self.cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&barriers),
            );

            let clear_value = vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [clear.r, clear.g, clear.b, clear.a],
                },
            };
            let attachment = vk::RenderingAttachmentInfo::default()
                .image_view(self.view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(clear_value);
            let attachments = [attachment];
            let area = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: self.extent.width,
                    height: self.extent.height,
                },
            };
            device.cmd_begin_rendering(
                self.cmd,
                &vk::RenderingInfo::default()
                    .render_area(area)
                    .layer_count(1)
                    .color_attachments(&attachments),
            );
            if let Some(pipeline) = pipeline {
                device.cmd_bind_pipeline(
                    self.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipeline.pipeline,
                );
                // Extents are far below f32's exact-integer range; the
                // casts are lossless in practice.
                #[allow(clippy::cast_precision_loss)]
                let viewport = vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: self.extent.width as f32,
                    height: self.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                };
                device.cmd_set_viewport(self.cmd, 0, &[viewport]);
                device.cmd_set_scissor(self.cmd, 0, &[area]);
                device.cmd_draw(self.cmd, 3, 1, 0, 0);
            }
            device.cmd_end_rendering(self.cmd);

            // COLOR_ATTACHMENT_OPTIMAL → TRANSFER_SRC_OPTIMAL for the
            // copy.
            let to_transfer = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .image(self.image)
                .subresource_range(color_range());
            let barriers = [to_transfer];
            device.cmd_pipeline_barrier2(
                self.cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&barriers),
            );

            let region = vk::BufferImageCopy::default()
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
                    width: self.extent.width,
                    height: self.extent.height,
                    depth: 1,
                });
            device.cmd_copy_image_to_buffer(
                self.cmd,
                self.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.buffer,
                &[region],
            );

            // Make the copy visible to host reads through the mapping.
            let host_barrier = vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ)
                .buffer(self.buffer)
                .offset(0)
                .size(vk::WHOLE_SIZE);
            let buffer_barriers = [host_barrier];
            device.cmd_pipeline_barrier2(
                self.cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&buffer_barriers),
            );

            device
                .end_command_buffer(self.cmd)
                .map_err(|code| creation("vkEndCommandBuffer", code))?;

            let cmd_infos = [vk::CommandBufferSubmitInfo::default().command_buffer(self.cmd)];
            let submit = vk::SubmitInfo2::default().command_buffer_infos(&cmd_infos);
            device
                .queue_submit2(self.shared.queue, &[submit], self.fence)
                .map_err(|code| {
                    self.shared.note_result(code);
                    if code == vk::Result::ERROR_DEVICE_LOST {
                        TargetError::DeviceLost
                    } else {
                        creation("vkQueueSubmit2", code)
                    }
                })?;

            match device.wait_for_fences(&[self.fence], true, FENCE_TIMEOUT_NS) {
                Ok(()) => {}
                Err(vk::Result::TIMEOUT) => {
                    // The submit is still in flight: the readback
                    // buffer may yet be written and the fence/command
                    // buffer carry pending state. Wedge the target so
                    // no safe call can touch either.
                    self.wedged = true;
                    return Err(TargetError::Timeout {
                        call: "vkWaitForFences",
                    });
                }
                Err(code) => {
                    self.shared.note_result(code);
                    self.wedged = true;
                    return Err(if code == vk::Result::ERROR_DEVICE_LOST {
                        TargetError::DeviceLost
                    } else {
                        creation("vkWaitForFences", code)
                    });
                }
            }
            if let Err(code) = device.reset_fences(&[self.fence]) {
                // The frame completed but the fence state is now
                // unknown; a next submit would be invalid usage.
                self.wedged = true;
                return Err(creation("vkResetFences", code));
            }
        }
        Ok(())
    }

    /// Copy the last rendered pixels into `out`, whose length must be
    /// exactly [`byte_len`](Self::byte_len). Both checks below are
    /// retained in release builds — each is a memory-safety boundary,
    /// not merely a logic bug.
    ///
    /// # Panics
    ///
    /// When `out.len() != self.byte_len()`, or when the target is
    /// wedged (an earlier frame never provably completed, so the GPU
    /// may still be writing the readback buffer) — contract violations.
    pub fn read_back_into(&self, out: &mut [u8]) {
        assert!(
            !self.wedged,
            "readback from a wedged target would race in-flight GPU work"
        );
        assert_eq!(
            out.len(),
            self.byte_len(),
            "readback buffer length must equal byte_len()"
        );
        // SAFETY: category 6 (the one mapped-memory read site): the
        // mapping is live for the target's whole life (mapped at
        // creation, unmapped in Drop); the wedge assert above proves the
        // last submit's fence completed, so no GPU write is in flight;
        // HOST_COHERENT plus the host barrier in `render` makes the
        // bytes visible; the copy length equals the buffer's created
        // size and `out`'s asserted length; the regions cannot overlap
        // (device mapping vs caller slice).
        unsafe {
            core::ptr::copy_nonoverlapping(self.mapped, out.as_mut_ptr(), out.len());
        }
    }
}

impl Drop for OffscreenTarget {
    fn drop(&mut self) {
        // SAFETY: category 2: wait-idle (best-effort) quiesces any
        // submitted work; then reverse creation order, each handle
        // created with these callbacks; the mapping is unmapped before
        // its memory is freed.
        unsafe {
            let _ = self.shared.device.device_wait_idle();
            self.shared
                .device
                .destroy_fence(self.fence, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .destroy_command_pool(self.pool, Some(&self.shared.alloc_cbs()));
            self.shared.device.unmap_memory(self.buffer_memory);
            self.shared
                .device
                .free_memory(self.buffer_memory, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .destroy_buffer(self.buffer, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .destroy_image_view(self.view, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .free_memory(self.image_memory, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .destroy_image(self.image, Some(&self.shared.alloc_cbs()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A memory-properties table with one heap and the given types, in
    /// order — the shape the driver reports, without a driver.
    fn table(types: &[vk::MemoryPropertyFlags]) -> vk::PhysicalDeviceMemoryProperties {
        let mut properties = vk::PhysicalDeviceMemoryProperties {
            memory_type_count: u32::try_from(types.len()).expect("a plausible type count"),
            memory_heap_count: 1,
            ..Default::default()
        };
        for (slot, flags) in properties.memory_types.iter_mut().zip(types) {
            slot.property_flags = *flags;
        }
        properties
    }

    const DEVICE_LOCAL: vk::MemoryPropertyFlags = vk::MemoryPropertyFlags::DEVICE_LOCAL;
    const HOST: vk::MemoryPropertyFlags = vk::MemoryPropertyFlags::from_raw(
        vk::MemoryPropertyFlags::HOST_VISIBLE.as_raw()
            | vk::MemoryPropertyFlags::HOST_COHERENT.as_raw(),
    );

    #[test]
    fn the_render_image_prefers_device_local_memory() {
        let properties = table(&[HOST, DEVICE_LOCAL]);
        assert_eq!(
            image_memory_type(&properties, 0b11),
            Some(1),
            "both types allowed: the device-local one wins"
        );
    }

    // The spec guarantees a device-local memory type exists; it does not
    // guarantee a given image may live in one. No test machine can be
    // made to report that, so the fallback is proven here instead.
    #[test]
    fn the_render_image_falls_back_when_no_device_local_type_is_allowed() {
        let properties = table(&[HOST, DEVICE_LOCAL]);
        assert_eq!(
            image_memory_type(&properties, 0b01),
            Some(0),
            "the image excludes the device-local type: take what it allows"
        );
    }

    #[test]
    fn a_memory_type_nothing_satisfies_is_reported_not_guessed() {
        let properties = table(&[DEVICE_LOCAL]);
        assert_eq!(
            image_memory_type(&properties, 0),
            None,
            "an image allowing no type at all has nowhere to live"
        );
        assert_eq!(
            pick_memory_type(&properties, 0b1, HOST),
            None,
            "the readback buffer needs host-visible memory, not any memory"
        );
    }

    /// Every requested flag must be present, not merely one of them.
    /// A type offering `HOST_VISIBLE` without `HOST_COHERENT` would be
    /// accepted by an "intersects" test and would break readback
    /// silently: this code never invalidates mapped ranges, so it would
    /// hand back stale bytes rather than fail.
    #[test]
    fn a_partial_flag_match_is_not_a_match() {
        let properties = table(&[vk::MemoryPropertyFlags::HOST_VISIBLE]);
        assert_eq!(
            pick_memory_type(&properties, 0b1, HOST),
            None,
            "host-visible alone does not satisfy host-visible AND coherent"
        );
        assert_eq!(
            pick_memory_type(&properties, 0b1, vk::MemoryPropertyFlags::HOST_VISIBLE),
            Some(0),
            "the same type satisfies the weaker request"
        );
    }

    // There is no minimized-window story headless, so a zero extent is
    // a caller bug. It is fatal in dev builds; this pins what release
    // builds answer instead, which no test run can otherwise reach.
    #[test]
    fn a_zero_extent_is_refused_in_either_dimension() {
        for extent in [
            Extent {
                width: 0,
                height: 8,
            },
            Extent {
                width: 8,
                height: 0,
            },
            Extent {
                width: 0,
                height: 0,
            },
        ] {
            let refusal = check_extent(extent).expect_err("a zero extent has no target");
            assert!(
                matches!(refusal, TargetError::Creation { call, code: 0 }
                    if call == "create_offscreen_target(zero extent)"),
                "{extent:?} refused as {refusal:?}, which names no cause"
            );
        }
        assert!(
            check_extent(Extent {
                width: 1,
                height: 1
            })
            .is_ok(),
            "one pixel is a real target"
        );
    }

    #[test]
    fn type_bits_are_matched_per_index_not_in_bulk() {
        let properties = table(&[DEVICE_LOCAL, DEVICE_LOCAL, DEVICE_LOCAL]);
        assert_eq!(pick_memory_type(&properties, 0b100, DEVICE_LOCAL), Some(2));
        // Types past `memory_type_count` are unreported, so a bit set
        // there selects nothing.
        assert_eq!(pick_memory_type(&properties, 0b1000, DEVICE_LOCAL), None);
    }
}
