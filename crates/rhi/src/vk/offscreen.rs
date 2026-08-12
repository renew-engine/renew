//! The headless render target: an RGBA8 image rendered synchronously
//! and read back to host memory. This is the correctness spine — the
//! golden tests prove pixels without a window or a display server.

use std::rc::Rc;

use ash::vk;

use crate::config::Extent;
use crate::error::TargetError;
use crate::vk::depth::{self, DepthResources};
use crate::vk::device::{Device, DeviceShared, FENCE_TIMEOUT_NS};
use crate::vk::pass::{self, MAX_RETAINED_RESOURCES, RenderDesc, Retained};
use crate::vk::pipeline::{INSTANCE_BINDING, TargetFormat, VERTEX_BINDING};
use crate::vk::transition;

/// Bytes per pixel of the fixed RGBA8 format.
pub(crate) const BPP: u64 = 4;

pub(crate) fn creation(call: &'static str, code: vk::Result) -> TargetError {
    match code {
        vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => TargetError::OutOfDeviceMemory { call },
        _ => TargetError::Creation {
            call,
            code: code.as_raw(),
        },
    }
}

/// Locate a memory type index satisfying `type_bits` and `flags`.
pub(crate) fn pick_memory_type(
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
pub(crate) fn image_memory_type(
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
fn check_extent(extent: Extent, max_dimension: u32) -> Result<(), TargetError> {
    if extent.width == 0 || extent.height == 0 {
        return Err(TargetError::Creation {
            call: "create_offscreen_target(zero extent)",
            code: 0,
        });
    }
    // The device's limit, handed in rather than read from the device
    // here. A pure function of the two is testable at any limit; one
    // that asked the device would be reachable only on an adapter whose
    // real maximum a caller could plausibly exceed, which is no adapter
    // any lane runs on -- the shape that ships with its message tested
    // and its trigger never pulled.
    if extent.width > max_dimension || extent.height > max_dimension {
        return Err(TargetError::Creation {
            call: "create_offscreen_target(extent exceeds the device limit)",
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
    /// The target's own depth image, `None` when the adapter offers no
    /// format in the chain. One, not per-slot: this target is
    /// synchronous.
    depth: Option<DepthResources>,
    /// Resources the recorded work references, retained so a caller
    /// dropping its handle cannot free memory the submit still reads.
    /// Cleared at the top of the next render's copy phase — where the
    /// previous render's tail wait has proven the work ended — and in
    /// `Drop`, after its wait-idle. On the wedge arm (a tail wait that
    /// timed out) the entries deliberately survive: the submit may
    /// still be reading them, and the wedged flag keeps every later
    /// call from touching this table until Drop's quiesce.
    retained: [Option<Retained>; MAX_RETAINED_RESOURCES],
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
        let checked = check_extent(extent, self.shared.max_image_dimension_2d);
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
        // TargetFormat::Rgba8Srgb's one Vulkan spelling — the offscreen
        // target's format is a static fact of this module, and the pipeline
        // assertion further down is what keeps it one fact rather than two
        // that can drift into a draw the validation layer rejects.
        .format(vk::Format::R8G8B8A8_SRGB)
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
        // TargetFormat::Rgba8Srgb's one Vulkan spelling — the offscreen
        // target's format is a static fact of this module, and the pipeline
        // assertion further down is what keeps it one fact rather than two
        // that can drift into a draw the validation layer rejects.
        .format(vk::Format::R8G8B8A8_SRGB)
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

    // Last, because it manages its own unwind: on failure it destroys
    // what it created, and `partial` unwinds the rest. A depthless
    // adapter creates no depth image; depth-free rendering never
    // notices.
    let depth = match shared.depth_format {
        Some(format) => Some(DepthResources::create(shared, extent, format)?),
        None => None,
    };

    // SAFETY: device live; default info (unsignaled) local.
    let fence = match unsafe {
        shared
            .device
            .create_fence(&vk::FenceCreateInfo::default(), Some(&shared.alloc_cbs()))
    } {
        Ok(fence) => fence,
        Err(code) => {
            if let Some(depth) = &depth {
                depth.destroy(shared);
            }
            return Err(creation("vkCreateFence", code));
        }
    };

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
        depth,
        retained: Default::default(),
        wedged: false,
    })
}

pub(crate) fn color_range() -> vk::ImageSubresourceRange {
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

    /// Record the frame's passes, submit once, and wait for completion.
    /// On return, the pixels are readable via
    /// [`read_back_into`](Self::read_back_into).
    ///
    /// Every item's pipeline must come from this target's device and
    /// target [`TargetFormat::Rgba8Srgb`] — contract violations,
    /// checked in dev builds.
    ///
    /// # Panics
    ///
    /// The frame-shape contract is asserted before any GPU call —
    /// among its refusals: a frame needs at least one pass, and at
    /// least one surface pass; a surface pass carries exactly one
    /// color attachment, an image pass none (its one attachment rides
    /// its target); `LoadOp::Load` is refused on each identity's first
    /// use in the frame, and on a render image whose last targeting
    /// pass discarded; clear values must match their attachment's kind
    /// and a depth clear its documented range; an item's pipeline
    /// depth state must match its pass, its format an image pass's
    /// kind, and a depth-only pipeline draws only into depth images;
    /// one buffer carries one `FrameData` per frame
    /// (pointer-identical data may repeat across items; differing data
    /// is refused); an item names geometry exactly when its pipeline
    /// declares per-vertex input, and a mesh's vertex stride equals
    /// the stride that pipeline's layout packs to; an item names
    /// bindings exactly when its pipeline declares sampled slots, and
    /// exactly as many as it declares; the per-image walk is one-way —
    /// a frame writes an image before reading it, never in the same
    /// pass, never re-targeting after a read, storing whatever a later
    /// pass loads or samples — over at most
    /// [`MAX_FRAME_RENDER_IMAGES`](crate::MAX_FRAME_RENDER_IMAGES)
    /// distinct images; and a frame carries at most the retention
    /// table's width of distinct resources — per-frame buffers,
    /// meshes, bindings and pass-target images together, the
    /// repeatable classes counting once however many mentions. Frame
    /// data longer than its buffer's per-frame capacity also panics
    /// through a retained assertion: the length bounds a copy into
    /// mapped device memory, which makes it a memory-safety boundary
    /// rather than a contract nicety.
    ///
    /// # Errors
    ///
    /// [`TargetError::DepthUnsupported`] when a pass carries depth and
    /// the adapter refused the whole format chain — returned before any
    /// frame work begins, so the target is untouched;
    /// [`TargetError::Timeout`] when the GPU exceeds the watchdog —
    /// the target is then wedged (submitted work never provably
    /// finished) and refuses further use; drop and recreate it.
    /// [`TargetError::DeviceLost`] on device loss (the device is then
    /// poisoned); command/submission failures otherwise.
    #[expect(
        clippy::too_many_lines,
        reason = "one recorded command stream; the pass walk and barrier ordering read top to bottom"
    )]
    pub fn render(&mut self, desc: &RenderDesc<'_>) -> Result<(), TargetError> {
        if self.wedged {
            return Err(TargetError::Timeout {
                call: "target wedged by an earlier incomplete frame",
            });
        }
        if self.shared.lost.poisoned() {
            return Err(TargetError::DeviceLost);
        }
        pass::check_frame_contract(desc);
        if desc.passes.iter().any(|pass| pass.depth.is_some()) && self.depth.is_none() {
            return Err(TargetError::DepthUnsupported {
                chain: depth::CHAIN_NAMES,
            });
        }
        for pass in desc.passes {
            let surface_pass = matches!(pass.target, pass::PassTarget::Surface);
            for item in pass.items {
                debug_assert!(
                    Rc::ptr_eq(&self.shared, &item.pipeline.shared),
                    "pipeline and target come from different devices"
                );
                // Image passes matched their format against their image
                // in the contract; the surface's own format is this
                // target's to assert.
                debug_assert!(
                    !surface_pass || item.pipeline.format == TargetFormat::Rgba8Srgb,
                    "pipeline targets {:?}, offscreen is Rgba8Srgb",
                    item.pipeline.format
                );
            }
        }
        // The copy phase. Release the previous frame's retained buffers
        // first: the previous render's tail wait proved that work ended
        // (a wedged target — the one case where it did not — never
        // reaches here), so the memory may die, and clearing before
        // filling scopes the table to exactly this frame's fills.
        for slot in &mut self.retained {
            *slot = None;
        }
        let mut retained_count = 0usize;
        for pass in desc.passes {
            // A pass-target image is retained by the pass itself: the
            // recorded attachment references it whether or not any item
            // samples it, so the pass walk is where its hold begins.
            if let pass::PassTarget::Image(image, _) = &pass.target {
                debug_assert!(
                    Rc::ptr_eq(&image.inner.shared, &self.shared),
                    "render image and target come from different devices"
                );
                let resource = Retained::Image(Rc::clone(&image.inner));
                if !pass::already_retained(&resource, &self.retained[..retained_count]) {
                    self.retained[retained_count] = Some(resource);
                    retained_count += 1;
                }
            }
            for item in pass.items {
                if let Some(mesh) = item.mesh {
                    debug_assert!(
                        Rc::ptr_eq(&mesh.inner.shared, &self.shared),
                        "mesh and target come from different devices"
                    );
                }
                if let Some(bindings) = &item.bindings {
                    for binding in bindings.iter() {
                        debug_assert!(
                            Rc::ptr_eq(&binding.inner.shared, &self.shared),
                            "binding and target come from different devices"
                        );
                    }
                }
                // Retention is enumerated by one shared function with a
                // total match over the item's shape, so a resource class
                // added to `Item` cannot be skipped here silently. A mesh
                // or binding named by several items is retained once —
                // the frame contract bounded the distinct count, and the
                // recognition rule is one shared definition.
                for resource in pass::retained_of(item).into_iter().flatten() {
                    if pass::already_retained(&resource, &self.retained[..retained_count]) {
                        continue;
                    }
                    self.retained[retained_count] = Some(resource);
                    retained_count += 1;
                }
                let Some(data) = &item.frame_data else {
                    continue;
                };
                let inner = &data.buffer.inner;
                debug_assert!(
                    Rc::ptr_eq(&inner.shared, &self.shared),
                    "buffer and target come from different devices"
                );
                let me = std::ptr::from_ref::<Self>(self) as usize;
                match inner.owner.get() {
                    None => inner.owner.set(Some(me)),
                    Some(owner) => debug_assert!(
                        owner == me,
                        "a per-frame buffer belongs to one target: its slot regions are \
                         owned by whichever target last submitted against them"
                    ),
                }
                // Retained in release: this length bounds the copy
                // below, which makes it a memory-safety boundary, not a
                // contract nicety.
                assert!(
                    data.bytes.len() <= inner.capacity,
                    "frame data exceeds the buffer's per-frame capacity"
                );
                // SAFETY: the mapping covers every slot region and the
                // assert bounds the length within slot zero's; the tail
                // wait of the previous `render` proved no submit reads
                // it; HOST_COHERENT, so no flush.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.bytes.as_ptr(),
                        inner.mapped,
                        data.bytes.len(),
                    );
                }
                // Retention for this buffer was recorded above, before
                // the copy: the submit this frame records will read the
                // region until the tail wait proves otherwise, and the
                // wedge arm is exactly the path where the caller's borrow
                // ends while the GPU may still read.
            }
        }
        let device = &self.shared.device;

        // SAFETY: category 2 (ash dispatch) for every call below:
        // device, images, views, buffer, pool, cmd, fence all live and
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

            // The walk: for each pass, its boundary barriers from the
            // shared frame walk (the same state machine the contract
            // already proved this frame against), then its attachments,
            // then its items in slice order — every one of them
            // following the pass's target.
            let mut walk = pass::FrameWalk::new();
            for (index, pass) in desc.passes.iter().enumerate() {
                let uses = walk.advance_target(index, pass);
                let mut barriers = [vk::ImageMemoryBarrier2::default(); pass::MAX_PASS_BARRIERS];
                let mut barrier_count = 0usize;
                if let Some((from, to)) = uses.color {
                    let masks = transition::pass_boundary(from, to);
                    let image = match &pass.target {
                        pass::PassTarget::Surface => self.image,
                        pass::PassTarget::Image(image, _) => image.inner.image,
                    };
                    barriers[barrier_count] = vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(masks.src_stage)
                        .src_access_mask(masks.src_access)
                        .dst_stage_mask(masks.dst_stage)
                        .dst_access_mask(masks.dst_access)
                        .old_layout(masks.old_layout)
                        .new_layout(masks.new_layout)
                        .image(image)
                        .subresource_range(color_range());
                    barrier_count += 1;
                }
                if let Some((from, to)) = uses.depth {
                    let masks = transition::pass_boundary(from, to);
                    // Total by the depth-availability check at the top
                    // for surface passes; an image pass carries its own
                    // depth image in its target.
                    let (image, format) = match &pass.target {
                        pass::PassTarget::Surface => match self.depth.as_ref() {
                            Some(depth_resources) => {
                                (depth_resources.image, depth_resources.format)
                            }
                            None => unreachable!(
                                "a depth-carrying surface pass on a depthless target was \
                                 refused before recording began"
                            ),
                        },
                        pass::PassTarget::Image(image, _) => {
                            (image.inner.image, image.inner.format)
                        }
                    };
                    barriers[barrier_count] = vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(masks.src_stage)
                        .src_access_mask(masks.src_access)
                        .dst_stage_mask(masks.dst_stage)
                        .dst_access_mask(masks.dst_access)
                        .old_layout(masks.old_layout)
                        .new_layout(masks.new_layout)
                        .image(image)
                        .subresource_range(depth::barrier_range(format));
                    barrier_count += 1;
                }
                // The sampling transitions this pass forces: each
                // rendered-then-read image crosses to SHADER_READ_ONLY
                // at the first reading pass's boundary, once.
                let (samples, sample_count) = walk.advance_sampling(index, pass);
                for sample in samples.iter().take(sample_count).flatten() {
                    let masks = transition::pass_boundary(sample.uses.0, sample.uses.1);
                    barriers[barrier_count] = vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(masks.src_stage)
                        .src_access_mask(masks.src_access)
                        .dst_stage_mask(masks.dst_stage)
                        .dst_access_mask(masks.dst_access)
                        .old_layout(masks.old_layout)
                        .new_layout(masks.new_layout)
                        .image(sample.image)
                        .subresource_range(sample.range);
                    barrier_count += 1;
                }
                device.cmd_pipeline_barrier2(
                    self.cmd,
                    &vk::DependencyInfo::default()
                        .image_memory_barriers(&barriers[..barrier_count]),
                );

                // Attachments and geometry follow the target: a surface
                // pass renders into this target's own images at its
                // extent; an image pass renders into the image at the
                // image's.
                let mut color_attachments: [vk::RenderingAttachmentInfo<'_>; 1] =
                    [vk::RenderingAttachmentInfo::default()];
                let mut color_attachment_count = 0usize;
                let mut depth_attachment = None;
                let extent = match &pass.target {
                    pass::PassTarget::Surface => {
                        let color = &pass.color[0];
                        color_attachments[0] = vk::RenderingAttachmentInfo::default()
                            .image_view(self.view)
                            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                            .load_op(color.load.to_vk())
                            .store_op(color.store.to_vk())
                            .clear_value(pass::vk_clear_color(color));
                        color_attachment_count = 1;
                        depth_attachment = pass.depth.as_ref().zip(self.depth.as_ref()).map(
                            |(attachment, depth_resources)| {
                                vk::RenderingAttachmentInfo::default()
                                    .image_view(depth_resources.view)
                                    .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                                    .load_op(attachment.load.to_vk())
                                    .store_op(attachment.store.to_vk())
                                    .clear_value(pass::vk_clear_depth(attachment))
                            },
                        );
                        self.extent
                    }
                    pass::PassTarget::Image(image, attachment) => {
                        if image.kind() == crate::RenderImageKind::Depth {
                            depth_attachment = Some(
                                vk::RenderingAttachmentInfo::default()
                                    .image_view(image.inner.view)
                                    .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                                    .load_op(attachment.load.to_vk())
                                    .store_op(attachment.store.to_vk())
                                    .clear_value(pass::vk_clear_depth(attachment)),
                            );
                        } else {
                            color_attachments[0] = vk::RenderingAttachmentInfo::default()
                                .image_view(image.inner.view)
                                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                                .load_op(attachment.load.to_vk())
                                .store_op(attachment.store.to_vk())
                                .clear_value(pass::vk_clear_color(attachment));
                            color_attachment_count = 1;
                        }
                        image.extent()
                    }
                };
                let area = vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: vk::Extent2D {
                        width: extent.width,
                        height: extent.height,
                    },
                };
                let mut rendering_info = vk::RenderingInfo::default()
                    .render_area(area)
                    .layer_count(1)
                    .color_attachments(&color_attachments[..color_attachment_count]);
                if let Some(depth_attachment) = &depth_attachment {
                    rendering_info = rendering_info.depth_attachment(depth_attachment);
                }
                device.cmd_begin_rendering(self.cmd, &rendering_info);
                if !pass.items.is_empty() {
                    // Extents are far below f32's exact-integer range;
                    // the casts are lossless in practice.
                    #[allow(clippy::cast_precision_loss)]
                    let viewport = vk::Viewport {
                        x: 0.0,
                        y: 0.0,
                        width: extent.width as f32,
                        height: extent.height as f32,
                        min_depth: 0.0,
                        max_depth: 1.0,
                    };
                    device.cmd_set_viewport(self.cmd, 0, &[viewport]);
                    device.cmd_set_scissor(self.cmd, 0, &[area]);
                }
                for item in pass.items {
                    device.cmd_bind_pipeline(
                        self.cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        item.pipeline.pipeline,
                    );
                    if let Some(bindings) = &item.bindings {
                        item.pipeline.bind_bindings(self.cmd, bindings);
                    }
                    if let Some(bytes) = item.push_data {
                        // The contract proved presence and length match
                        // the pipeline's declared range; the bytes are
                        // copied into the command stream here, so no
                        // retention slot is spent.
                        item.pipeline.push_frame_constants(self.cmd, bytes);
                    }
                    if let Some(mesh) = item.mesh {
                        // A mesh has no slots — its bytes were written
                        // once at creation and never again — so both
                        // targets bind it at the same offsets, and the
                        // per-frame ring's slot arithmetic simply does
                        // not arise for it.
                        device.cmd_bind_vertex_buffers(
                            self.cmd,
                            VERTEX_BINDING,
                            &[mesh.inner.buffer],
                            &[0],
                        );
                        device.cmd_bind_index_buffer(
                            self.cmd,
                            mesh.inner.buffer,
                            mesh.inner.index_offset,
                            vk::IndexType::UINT32,
                        );
                    }
                    let instances = match &item.frame_data {
                        Some(data) => {
                            // Slot zero always: this target is
                            // synchronous, so one region is in play; the
                            // retention table above holds the memory
                            // alive past any caller drop until the tail
                            // wait proves the read ended.
                            device.cmd_bind_vertex_buffers(
                                self.cmd,
                                INSTANCE_BINDING,
                                &[data.buffer.inner.buffer],
                                &[0],
                            );
                            data.instances
                        }
                        None => 1,
                    };
                    // The count comes from whichever half owns it: the
                    // geometry for a mesh draw, the shader for a stage
                    // that writes its own vertex list. The frame contract
                    // already refused the mismatch.
                    match item.mesh {
                        Some(mesh) => device.cmd_draw_indexed(
                            self.cmd,
                            mesh.inner.index_count,
                            instances,
                            0,
                            0,
                            0,
                        ),
                        None => {
                            device.cmd_draw(self.cmd, item.pipeline.vertex_count, instances, 0, 0);
                        }
                    }
                }
                device.cmd_end_rendering(self.cmd);
            }

            // COLOR_ATTACHMENT_OPTIMAL → TRANSFER_SRC_OPTIMAL for the
            // copy. Not a pass boundary — the terminal literal, pinned
            // by unit test beside the core it is excluded from.
            let masks = transition::terminal_transfer_src();
            let to_transfer = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(masks.src_stage)
                .src_access_mask(masks.src_access)
                .dst_stage_mask(masks.dst_stage)
                .dst_access_mask(masks.dst_access)
                .old_layout(masks.old_layout)
                .new_layout(masks.new_layout)
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
            // A buffer barrier, not an image transition — outside the
            // pure core, its masks pinned by unit test beside it.
            let masks = transition::host_readback();
            let host_barrier = vk::BufferMemoryBarrier2::default()
                .src_stage_mask(masks.src_stage)
                .src_access_mask(masks.src_access)
                .dst_stage_mask(masks.dst_stage)
                .dst_access_mask(masks.dst_access)
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
            // Best-effort quiesce; failure is logged, never a panic (D5)
            // — the diag record is the only observable this path has.
            if let Err(code) = self.shared.device.device_wait_idle() {
                renew_diag::error!(
                    target: "renew-rhi",
                    "wait-idle at offscreen teardown failed: {code:?}"
                );
            }
            // Retained memory may die now: teardown's wait is the same
            // best-effort proof every cold teardown path in this crate
            // accepts.
            for slot in &mut self.retained {
                *slot = None;
            }
            if let Some(depth) = self.depth.take() {
                depth.destroy(&self.shared);
            }
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

    /// An extent past the adapter's `maxImageDimension2D` is refused
    /// here, by name, instead of reaching `vkCreateImage`.
    ///
    /// **Reachable because the limit is a parameter.** A check that read
    /// the device would need an adapter whose real maximum a caller
    /// could plausibly exceed, and every adapter these lanes run on
    /// reports at least 4096 -- so the arm would ship with its message
    /// tested and its trigger never pulled. Handing the limit in makes
    /// both sides of the comparison ordinary to test.
    #[test]
    fn an_extent_past_the_device_limit_is_refused_by_name() {
        for extent in [
            Extent {
                width: 65,
                height: 8,
            },
            Extent {
                width: 8,
                height: 65,
            },
            Extent {
                width: 65,
                height: 65,
            },
        ] {
            let refusal = check_extent(extent, 64).expect_err("past the limit is not a target");
            assert!(
                matches!(refusal, TargetError::Creation { call, code: 0 }
                    if call == "create_offscreen_target(extent exceeds the device limit)"),
                "{extent:?} refused as {refusal:?}, which names no cause"
            );
        }
        // Inclusive: it is a maximum, not a bound to stay under, and an
        // off-by-one here would refuse the largest target the adapter
        // actually supports.
        assert!(
            check_extent(
                Extent {
                    width: 64,
                    height: 64
                },
                64
            )
            .is_ok(),
            "the limit itself is allowed"
        );
    }
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
            let refusal = check_extent(extent, u32::MAX).expect_err("a zero extent has no target");
            assert!(
                matches!(refusal, TargetError::Creation { call, code: 0 }
                    if call == "create_offscreen_target(zero extent)"),
                "{extent:?} refused as {refusal:?}, which names no cause"
            );
        }
        assert!(
            check_extent(
                Extent {
                    width: 1,
                    height: 1
                },
                u32::MAX
            )
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
