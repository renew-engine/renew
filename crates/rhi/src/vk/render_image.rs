//! Render images: what one pass writes and a later pass samples.
//!
//! A [`RenderImage`] is one physical image with attachment and sampled
//! usage, kinded Color or Depth at creation. Its *contents* are
//! frame-scoped — nothing rendered into it survives a frame boundary,
//! which is what keeps the frame contract pure — while the image
//! itself lives as long as its handle does. It has no host path: no
//! upload, no readback, no resize (recreate instead). What it is for
//! arrives with pass targets; this module owns creation, ownership,
//! and the format pre-check that refuses an unsampleable depth format
//! by type before any frame exists to prove it.

use std::rc::Rc;

use ash::vk;

use crate::config::Extent;
use crate::error::TargetError;
use crate::vk::device::{Device, DeviceShared};
use crate::vk::offscreen::{creation, image_memory_type};

/// What a render image is for: which attachment slot of a pass it can
/// be, which decides its format and usage at creation.
///
/// An input enum, so `#[non_exhaustive]`: a later kind must not break
/// downstream matchers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RenderImageKind {
    /// A color target: `Rgba8Unorm`, drawable and sampleable.
    Color,
    /// A depth target: the device's chosen depth format, drawable by a
    /// depth-only pass and sampleable by a later one.
    Depth,
}

/// Everything a render image needs: its kind and its size.
///
/// `#[non_exhaustive]` with a constructor, per the descriptor pattern
/// this crate uses everywhere.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RenderImageDesc {
    /// Which attachment slot this image can fill.
    pub kind: RenderImageKind,
    /// The image's size in pixels.
    pub extent: Extent,
    /// Whether the image's contents persist across frames. See
    /// [`RenderImage`]'s contract for what a kept image may do that a
    /// frame-scoped one may not.
    pub kept: bool,
}

impl RenderImageDesc {
    /// A render image of `kind`, sized `extent`, frame-scoped.
    ///
    /// Positional because neither has a meaningful default: an image
    /// with no kind has no format, and one with no size is not an
    /// image. Frame-scoped by default because that is the cheaper
    /// contract; [`Self::kept`] opts into persistence.
    #[must_use]
    pub fn new(kind: RenderImageKind, extent: Extent) -> Self {
        Self {
            kind,
            extent,
            kept: false,
        }
    }

    /// The same image, keeping its contents across frames.
    #[must_use]
    pub fn kept(mut self) -> Self {
        self.kept = true;
        self
    }
}

/// What an earlier frame left in a **kept** render image - the
/// cross-frame half of the frame walk's state machine, stored on the
/// image because the image is the thing that outlives frames.
///
/// Written back only when a frame is actually recorded (the contract's
/// dry walk reads it and must not move it), and only ever read and
/// written on the one thread the crate contract already requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeptContents {
    /// No frame has rendered the image yet: contents undefined, exactly
    /// a frame-scoped image's every-frame state. Also every non-kept
    /// image's permanent value - the walk never writes those back.
    Undefined,
    /// The last frame to touch it wrote and stored: contents live, the
    /// image sits in its attachment layout.
    Stored,
    /// The last frame to touch it wrote and discarded: the layout is
    /// the attachment layout but the pixels are gone, so loading or
    /// sampling them is refused by name.
    Discarded,
    /// The last frame to touch it left it sampled: contents live, the
    /// image sits in the sampled layout.
    Sampled,
}

/// Refuse a malformed render-image extent. A pure function so both
/// rules are unit-tested without a device, like the texture's own
/// check.
fn check_extent(extent: Extent, max_dimension: u32) -> Result<(), TargetError> {
    if extent.width == 0 || extent.height == 0 {
        return Err(TargetError::Creation {
            call: "create_render_image(zero extent)",
            code: 0,
        });
    }
    if extent.width > max_dimension || extent.height > max_dimension {
        return Err(TargetError::Creation {
            call: "create_render_image(extent exceeds the device limit)",
            code: 0,
        });
    }
    Ok(())
}

/// The render image's owning half: the mesh split, applied here so the
/// retention table and a binding can keep the image alive with `Rc`
/// clones while callers pass plain borrows. The contract lives on
/// [`RenderImage`], where its reader is.
pub(crate) struct RenderImageInner {
    pub(crate) shared: Rc<DeviceShared>,
    pub(crate) image: vk::Image,
    memory: vk::DeviceMemory,
    pub(crate) view: vk::ImageView,
    pub(crate) format: vk::Format,
    pub(crate) kind: RenderImageKind,
    /// Whether contents persist across frames (creation-time choice).
    pub(crate) kept: bool,
    /// The cross-frame contents state; [`KeptContents::Undefined`]
    /// forever on a frame-scoped image.
    pub(crate) contents: core::cell::Cell<KeptContents>,
    extent: Extent,
}

impl Drop for RenderImageInner {
    fn drop(&mut self) {
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // Rc; every handle was created with these callbacks. No submit
        // still references the image: a recorded frame retains this
        // inner through the target's retention table, released only
        // after the frame's work provably ended — or by the targets'
        // best-effort teardown quiesce, the same corner every retained
        // class shares and the retention fields document.
        unsafe {
            self.shared
                .device
                .destroy_image_view(self.view, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .free_memory(self.memory, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .destroy_image(self.image, Some(&self.shared.alloc_cbs()));
        }
    }
}

/// An image a pass renders into and a later pass samples. Holds its
/// device alive; destroyed on drop.
///
/// # Contract
///
/// By default the image's **contents are frame-scoped**: every frame's
/// first use of it starts from undefined pixels, and nothing rendered
/// into it is promised to a later frame — the frame contract refuses a
/// contents-preserving first-use load exactly as it does for the
/// surface. The image itself outlives every frame that names it, held
/// by the target's retention table until that frame's work provably
/// ended, so a caller may drop the handle mid-frame and take nothing
/// away from the GPU.
///
/// A **kept** image ([`RenderImageDesc::kept`]) persists its contents
/// across frames instead. What that buys, concretely: a frame may open
/// it with `LoadOp::Load` and paint over last frame's pixels, and a
/// frame may **sample it without rendering to it at all** — the shape
/// of every render-to-texture updated on its own cadence: a shadow map
/// under a slow sun, a reflection probe, a minimap, an impostor cache.
/// Two rules survive unchanged: the first frame ever to touch it must
/// render before anything loads or samples it (there is nothing to
/// keep yet), and a frame whose last write discarded leaves nothing
/// for later frames to load or sample — both refused by name. The
/// within-frame order (every writing pass before the first sampling
/// pass) binds kept images exactly as it does frame-scoped ones.
///
/// There is no host path — no upload, no readback — and no resize: an
/// image is its size for its whole life, and a differently-sized frame
/// wants a different image.
pub struct RenderImage {
    pub(crate) inner: Rc<RenderImageInner>,
}

impl RenderImage {
    /// The image's size in pixels.
    #[must_use]
    pub fn extent(&self) -> Extent {
        self.inner.extent
    }

    /// Which attachment slot this image can fill.
    #[must_use]
    pub fn kind(&self) -> RenderImageKind {
        self.inner.kind
    }
}

impl std::fmt::Debug for RenderImage {
    /// Kind and dimensions, not handles — the crate's posture.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderImage")
            .field("kind", &self.inner.kind)
            .field("extent", &self.inner.extent)
            .finish_non_exhaustive()
    }
}

impl Device {
    /// Create a render image: one device-local image with attachment
    /// and sampled usage, its format decided by the kind.
    ///
    /// The format is **pre-checked against the adapter's own feature
    /// report** — `SAMPLED_IMAGE` plus the kind's attachment feature
    /// under optimal tiling — so an adapter whose depth format cannot
    /// be sampled refuses here, at creation, rather than as undefined
    /// sampling behaviour in some later frame.
    ///
    /// # Errors
    ///
    /// [`TargetError::Creation`] for a malformed extent, a format the
    /// adapter cannot sample or attach, or a driver refusal;
    /// [`TargetError::DepthUnsupported`] for a Depth image on an
    /// adapter with no depth format at all;
    /// [`TargetError::OutOfDeviceMemory`] when the allocation fails for
    /// want of device memory; [`TargetError::DeviceLost`] when the
    /// device was already lost.
    #[expect(
        clippy::too_many_lines,
        reason = "one creation ladder with its unwind at every rung; splitting scatters the reverse order"
    )]
    pub fn create_render_image(&self, desc: &RenderImageDesc) -> Result<RenderImage, TargetError> {
        let shared = &self.shared;
        check_extent(desc.extent, shared.max_image_dimension_2d)?;
        if shared.lost.poisoned() {
            return Err(TargetError::DeviceLost);
        }
        let (format, usage, aspect) = match desc.kind {
            RenderImageKind::Color => (
                vk::Format::R8G8B8A8_UNORM,
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
                vk::ImageAspectFlags::COLOR,
            ),
            RenderImageKind::Depth => {
                let format = shared.depth_format.ok_or(TargetError::DepthUnsupported {
                    chain: crate::vk::depth::CHAIN_NAMES,
                })?;
                (
                    format,
                    vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
                    vk::ImageAspectFlags::DEPTH,
                )
            }
        };
        // The pre-check. Color's features are spec-guaranteed for
        // R8G8B8A8_UNORM, but one rule beats one rule and an exemption:
        // both kinds hold their format to what the frame model will ask
        // of it, and the query is a static property read once per call.
        let attachment_feature = match desc.kind {
            RenderImageKind::Color => vk::FormatFeatureFlags::COLOR_ATTACHMENT,
            RenderImageKind::Depth => vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT,
        };
        // SAFETY: category 2 (ash dispatch): instance and physical
        // device live via the spine. (The same argument covers every
        // dispatch call below.)
        let features = unsafe {
            shared
                .instance
                .get_physical_device_format_properties(shared.physical, format)
        }
        .optimal_tiling_features;
        let needed = vk::FormatFeatureFlags::SAMPLED_IMAGE | attachment_feature;
        if !features.contains(needed) {
            return Err(TargetError::Creation {
                call: "create_render_image(the adapter cannot sample this format)",
                code: 0,
            });
        }

        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: desc.extent.width,
                height: desc.extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: device live via the spine; the create info is a
        // local; the callbacks' ledger outlives the image.
        let image = unsafe {
            shared
                .device
                .create_image(&image_info, Some(&shared.alloc_cbs()))
        }
        .map_err(|code| creation("vkCreateImage(render)", code))?;
        let destroy_image = |shared: &Rc<DeviceShared>| {
            // SAFETY: image live; nothing bound or recorded against it
            // on this unwind path.
            unsafe {
                shared
                    .device
                    .destroy_image(image, Some(&shared.alloc_cbs()));
            }
        };

        // SAFETY: image live.
        let requirements = unsafe { shared.device.get_image_memory_requirements(image) };
        // SAFETY: instance and physical device live via the spine.
        let memory_properties = unsafe {
            shared
                .instance
                .get_physical_device_memory_properties(shared.physical)
        };
        let type_index = image_memory_type(&memory_properties, requirements.memory_type_bits)
            .ok_or(TargetError::Creation {
                call: "render image memory type",
                code: 0,
            })
            .inspect_err(|_| destroy_image(shared))?;
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(type_index);
        // SAFETY: device live; info local.
        let memory = match unsafe {
            shared
                .device
                .allocate_memory(&alloc, Some(&shared.alloc_cbs()))
        } {
            Ok(memory) => memory,
            Err(code) if code == vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => {
                destroy_image(shared);
                return Err(TargetError::OutOfDeviceMemory {
                    call: "vkAllocateMemory(render)",
                });
            }
            Err(code) => {
                destroy_image(shared);
                return Err(creation("vkAllocateMemory(render)", code));
            }
        };
        // Driver truth, not host arithmetic, matching the depth image's
        // record.
        renew_diag::info!(
            target: "renew-rhi",
            "render image: {}x{} {:?}, {} bytes device memory",
            desc.extent.width,
            desc.extent.height,
            format,
            requirements.size
        );
        // SAFETY: image and memory live; offset 0 within an allocation
        // sized from this image's own requirements.
        if let Err(code) = unsafe { shared.device.bind_image_memory(image, memory, 0) } {
            // SAFETY: both live, nothing else references them.
            unsafe {
                shared.device.free_memory(memory, Some(&shared.alloc_cbs()));
            }
            destroy_image(shared);
            return Err(creation("vkBindImageMemory(render)", code));
        }

        // One view serves both faces of the image: the attachment names
        // it, and a binding samples through it. Depth views name the
        // depth aspect alone, the depth attachment's own convention.
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(aspect)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        // SAFETY: image live and bound.
        let view = match unsafe {
            shared
                .device
                .create_image_view(&view_info, Some(&shared.alloc_cbs()))
        } {
            Ok(view) => view,
            Err(code) => {
                // SAFETY: both live, nothing else references them.
                unsafe {
                    shared.device.free_memory(memory, Some(&shared.alloc_cbs()));
                }
                destroy_image(shared);
                return Err(creation("vkCreateImageView(render)", code));
            }
        };

        Ok(RenderImage {
            inner: Rc::new(RenderImageInner {
                shared: Rc::clone(shared),
                image,
                memory,
                view,
                format,
                kind: desc.kind,
                kept: desc.kept,
                contents: core::cell::Cell::new(KeptContents::Undefined),
                extent: desc.extent,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extent refusals, both rules, with no device involved.
    #[test]
    fn malformed_extents_are_refused_by_name() {
        let refusal = check_extent(
            Extent {
                width: 0,
                height: 4,
            },
            4096,
        );
        assert!(matches!(
            refusal,
            Err(TargetError::Creation {
                call: "create_render_image(zero extent)",
                ..
            })
        ));
        let refusal = check_extent(
            Extent {
                width: 4,
                height: 8192,
            },
            4096,
        );
        assert!(matches!(
            refusal,
            Err(TargetError::Creation {
                call: "create_render_image(extent exceeds the device limit)",
                ..
            })
        ));
        assert!(
            check_extent(
                Extent {
                    width: 4,
                    height: 4,
                },
                4096,
            )
            .is_ok()
        );
    }
}
