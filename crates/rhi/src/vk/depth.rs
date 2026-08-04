//! The target-internal depth image: created from the device's chosen
//! format, sized to the target, never public. One per offscreen
//! target; one per frame slot on the window target, living and dying
//! with its chain.

use std::rc::Rc;

use ash::vk;

use crate::config::Extent;
use crate::error::TargetError;
use crate::vk::device::DeviceShared;
use crate::vk::offscreen::{creation, image_memory_type};

/// The chain the device query walked, for diagnostics on the refusal
/// path.
pub(crate) const CHAIN_NAMES: &str = "D32_SFLOAT, D24_UNORM_S8_UINT";

/// One depth attachment image with its memory and view.
pub(crate) struct DepthResources {
    pub(crate) image: vk::Image,
    pub(crate) memory: vk::DeviceMemory,
    pub(crate) view: vk::ImageView,
    pub(crate) format: vk::Format,
}

/// The aspects a *barrier* on this format must name: both, when the
/// format carries stencil (the whole image transitions together — this
/// crate does not enable separate depth/stencil layouts). The attachment
/// *view* names depth alone either way.
pub(crate) fn barrier_aspect(format: vk::Format) -> vk::ImageAspectFlags {
    if format == vk::Format::D24_UNORM_S8_UINT {
        vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL
    } else {
        vk::ImageAspectFlags::DEPTH
    }
}

/// The subresource range a barrier on this format covers.
pub(crate) fn barrier_range(format: vk::Format) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(barrier_aspect(format))
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1)
}

impl DepthResources {
    /// Create one depth image for `extent` in `format` — the device's
    /// chosen format, which the caller has already checked exists (a
    /// depthless adapter creates no depth images at all).
    ///
    /// The memory figure is journaled from the driver's own
    /// requirements at creation; nothing here does size arithmetic.
    #[expect(
        clippy::too_many_lines,
        reason = "one linear creation ladder; splitting it hides the order the failure paths must mirror"
    )]
    pub(crate) fn create(
        shared: &Rc<DeviceShared>,
        extent: Extent,
        format: vk::Format,
    ) -> Result<Self, TargetError> {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: category 2 (ash dispatch): device live via the spine;
        // the create info is a local; the callbacks' ledger outlives the
        // image. (The same argument covers every dispatch call below.)
        let image = unsafe {
            shared
                .device
                .create_image(&image_info, Some(&shared.alloc_cbs()))
        }
        .map_err(|code| creation("vkCreateImage(depth)", code))?;

        let destroy_image = |shared: &Rc<DeviceShared>| {
            // SAFETY: image live, nothing bound or recorded against it.
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
        let Some(type_index) = image_memory_type(&memory_properties, requirements.memory_type_bits)
        else {
            destroy_image(shared);
            return Err(TargetError::Creation {
                call: "depth image memory type",
                code: 0,
            });
        };
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
            Err(code) => {
                destroy_image(shared);
                return Err(creation("vkAllocateMemory(depth)", code));
            }
        };
        // Driver truth, not host arithmetic: the figure device-memory
        // accounting will want is exactly what was asked of the driver.
        renew_diag::info!(
            target: "renew-rhi",
            "depth image: {}x{}, {} bytes device memory",
            extent.width,
            extent.height,
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
            return Err(creation("vkBindImageMemory(depth)", code));
        }

        // The attachment view names depth alone — stencil is unused even
        // on the combined format.
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
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
                return Err(creation("vkCreateImageView(depth)", code));
            }
        };

        Ok(Self {
            image,
            memory,
            view,
            format,
        })
    }

    /// Destroy the view, memory and image. The caller quiesces first —
    /// every call site sits behind a proven wait-idle or a chain
    /// teardown whose caller contract already demands one.
    pub(crate) fn destroy(&self, shared: &DeviceShared) {
        // SAFETY: category 2: every handle live and created with these
        // callbacks; the GPU is idle per the caller contract.
        unsafe {
            shared
                .device
                .destroy_image_view(self.view, Some(&shared.alloc_cbs()));
            shared
                .device
                .free_memory(self.memory, Some(&shared.alloc_cbs()));
            shared
                .device
                .destroy_image(self.image, Some(&shared.alloc_cbs()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The barrier covers what the format carries: the combined format
    /// transitions both aspects (this crate enables no separate
    /// depth/stencil layouts), the pure-depth format transitions depth
    /// alone.
    #[test]
    fn the_barrier_aspect_follows_the_format() {
        assert_eq!(
            barrier_aspect(vk::Format::D32_SFLOAT),
            vk::ImageAspectFlags::DEPTH
        );
        assert_eq!(
            barrier_aspect(vk::Format::D24_UNORM_S8_UINT),
            vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL
        );
        let range = barrier_range(vk::Format::D32_SFLOAT);
        assert_eq!(range.aspect_mask, vk::ImageAspectFlags::DEPTH);
        assert_eq!(range.level_count, 1);
        assert_eq!(range.layer_count, 1);
    }
}
