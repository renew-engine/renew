//! The window render target: surface, swapchain, and the
//! clear-and-draw-then-present loop, one frame in flight.
//!
//! The target owns a keep-alive handle to the native window (boxed,
//! opaque) — the platform window cannot be torn down under a live
//! surface, by construction rather than by discipline.

use std::rc::Rc;

use ash::vk;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::config::{Color, Extent};
use crate::error::TargetError;
use crate::vk::device::{Device, DeviceShared, FENCE_TIMEOUT_NS};
use crate::vk::pipeline::{RenderPipeline, TargetFormat};

fn creation(call: &'static str, code: vk::Result) -> TargetError {
    match code {
        vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => TargetError::OutOfDeviceMemory { call },
        _ => TargetError::Creation {
            call,
            code: code.as_raw(),
        },
    }
}

/// What a [`WindowTarget::render`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentOutcome {
    /// The frame is on its way to the screen.
    Presented,
    /// The swapchain no longer matches the surface (resize, minimize);
    /// call [`WindowTarget::resize`] with the current size. Nothing was
    /// presented.
    NeedsResize,
}

/// The live swapchain and everything sized to it; absent while the
/// window is zero-sized (minimized).
struct Chain {
    swapchain: vk::SwapchainKHR,
    extent: Extent,
    views: Vec<vk::ImageView>,
    images: Vec<vk::Image>,
    /// One per swapchain image: the present engine may still wait on
    /// the semaphore for image N after our fence signals, so per-image
    /// signaling is the simplest correct scheme.
    render_finished: Vec<vk::Semaphore>,
}

/// A window-backed render target. One frame in flight; presentation is
/// FIFO (vsync) — the guaranteed-available mode.
pub struct WindowTarget {
    shared: Rc<DeviceShared>,
    /// Keep-alive for the platform window backing `surface`.
    _window: Box<dyn core::any::Any>,
    surface_loader: ash::khr::surface::Instance,
    swapchain_loader: ash::khr::swapchain::Device,
    surface: vk::SurfaceKHR,
    format: TargetFormat,
    vk_format: vk::Format,
    color_space: vk::ColorSpaceKHR,
    chain: Option<Chain>,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    image_available: vk::Semaphore,
}

impl Device {
    /// Create a render target over a window. `window` is any owner of
    /// native window handles — the platform's `NativeWindow` (cloned)
    /// is the intended argument — and is held alive by the target.
    ///
    /// # Errors
    ///
    /// [`TargetError::PresentUnsupported`] when the device cannot
    /// present to this surface (no swapchain extension, no queue
    /// support, or no compatible surface format);
    /// [`TargetError::SurfaceCreation`] when the surface itself cannot
    /// be created; creation errors otherwise.
    #[expect(
        clippy::too_many_lines,
        reason = "one linear bring-up ladder; splitting it hides the creation order the failure paths must mirror"
    )]
    pub fn create_window_target<W>(
        &self,
        window: W,
        extent: Extent,
    ) -> Result<WindowTarget, TargetError>
    where
        W: HasDisplayHandle + HasWindowHandle + 'static,
    {
        let shared = &self.shared;
        if shared.lost.get() {
            return Err(TargetError::DeviceLost);
        }

        // The swapchain device extension is only enabled when the
        // physical device offers it; re-check rather than assume.
        // SAFETY: category 2 (ash dispatch): instance and physical
        // device live via the spine.
        let extensions = unsafe {
            shared
                .instance
                .enumerate_device_extension_properties(shared.physical)
        }
        .map_err(|code| creation("vkEnumerateDeviceExtensionProperties", code))?;
        let has_swapchain = extensions.iter().any(|ext| {
            ext.extension_name_as_c_str()
                .is_ok_and(|name| name == ash::khr::swapchain::NAME)
        });
        if !has_swapchain {
            return Err(TargetError::PresentUnsupported);
        }

        let display = window
            .display_handle()
            .map_err(|_| TargetError::SurfaceCreation { code: 0 })?
            .as_raw();
        let raw_window = window
            .window_handle()
            .map_err(|_| TargetError::SurfaceCreation { code: 0 })?
            .as_raw();
        // SAFETY: category 3 (the one surface-creation site): entry and
        // instance live via the spine; the raw handles were produced
        // moments ago from a window the target takes ownership of below
        // — the platform window outlives the surface by construction.
        let surface = unsafe {
            ash_window::create_surface(
                &shared.entry,
                &shared.instance,
                display,
                raw_window,
                Some(&shared.alloc_cbs()),
            )
        }
        .map_err(|code| TargetError::SurfaceCreation {
            code: code.as_raw(),
        })?;
        let surface_loader = ash::khr::surface::Instance::new(&shared.entry, &shared.instance);
        let swapchain_loader = ash::khr::swapchain::Device::new(&shared.instance, &shared.device);
        // From here on, failure must destroy the surface.
        let fail = |target: &Self, error: TargetError| {
            // SAFETY: surface live, nothing else references it yet.
            unsafe {
                surface_loader.destroy_surface(surface, Some(&target.shared.alloc_cbs()));
            }
            error
        };

        // SAFETY: category 2: physical device and surface live.
        let supported = unsafe {
            surface_loader.get_physical_device_surface_support(
                shared.physical,
                shared.queue_family,
                surface,
            )
        }
        .map_err(|code| fail(self, creation("vkGetPhysicalDeviceSurfaceSupportKHR", code)))?;
        if !supported {
            return Err(fail(self, TargetError::PresentUnsupported));
        }

        // SAFETY: category 2: as above.
        let formats =
            unsafe { surface_loader.get_physical_device_surface_formats(shared.physical, surface) }
                .map_err(|code| {
                    fail(self, creation("vkGetPhysicalDeviceSurfaceFormatsKHR", code))
                })?;
        let chosen = [vk::Format::B8G8R8A8_UNORM, vk::Format::R8G8B8A8_UNORM]
            .into_iter()
            .find_map(|want| {
                formats.iter().find(|f| {
                    f.format == want && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
                })
            });
        let Some(surface_format) = chosen else {
            return Err(fail(self, TargetError::PresentUnsupported));
        };
        let format = match surface_format.format {
            vk::Format::R8G8B8A8_UNORM => TargetFormat::Rgba8Unorm,
            _ => TargetFormat::Bgra8Unorm,
        };

        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(shared.queue_family);
        // SAFETY: category 2: device live; info local.
        let pool = unsafe {
            shared
                .device
                .create_command_pool(&pool_info, Some(&shared.alloc_cbs()))
        }
        .map_err(|code| fail(self, creation("vkCreateCommandPool", code)))?;
        let fail_pool = |target: &Self, error: TargetError| {
            // SAFETY: pool then surface, both live and unshared on this
            // failure path.
            unsafe {
                target
                    .shared
                    .device
                    .destroy_command_pool(pool, Some(&target.shared.alloc_cbs()));
            }
            fail(target, error)
        };

        let cmd_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: category 2: pool live; info local.
        let cmd = match unsafe { shared.device.allocate_command_buffers(&cmd_info) } {
            Ok(buffers) => match buffers.into_iter().next() {
                Some(cmd) => cmd,
                None => {
                    return Err(fail_pool(
                        self,
                        TargetError::Creation {
                            call: "vkAllocateCommandBuffers",
                            code: 0,
                        },
                    ));
                }
            },
            Err(code) => {
                return Err(fail_pool(self, creation("vkAllocateCommandBuffers", code)));
            }
        };

        // Signaled: the first frame's wait must pass immediately.
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        // SAFETY: category 2: device live; info local.
        let fence = match unsafe {
            shared
                .device
                .create_fence(&fence_info, Some(&shared.alloc_cbs()))
        } {
            Ok(fence) => fence,
            Err(code) => return Err(fail_pool(self, creation("vkCreateFence", code))),
        };
        // SAFETY: category 2: device live; default info local.
        let image_available = match unsafe {
            shared.device.create_semaphore(
                &vk::SemaphoreCreateInfo::default(),
                Some(&shared.alloc_cbs()),
            )
        } {
            Ok(semaphore) => semaphore,
            Err(code) => {
                // SAFETY: fence live and unshared.
                unsafe {
                    shared
                        .device
                        .destroy_fence(fence, Some(&shared.alloc_cbs()));
                }
                return Err(fail_pool(self, creation("vkCreateSemaphore", code)));
            }
        };

        let mut target = WindowTarget {
            shared: Rc::clone(shared),
            _window: Box::new(window),
            surface_loader,
            swapchain_loader,
            surface,
            format,
            vk_format: surface_format.format,
            color_space: surface_format.color_space,
            chain: None,
            pool,
            cmd,
            fence,
            image_available,
        };
        // Build the initial chain; zero extents stay dormant. From here
        // the target's Drop owns cleanup on failure.
        if extent.width > 0 && extent.height > 0 {
            target.build_chain(extent)?;
        }
        Ok(target)
    }
}

impl WindowTarget {
    /// The swapchain's color format — build the draw pipeline against
    /// this.
    #[must_use]
    pub fn format(&self) -> TargetFormat {
        self.format
    }

    /// The current drawable size; zero while minimized/dormant.
    #[must_use]
    pub fn extent(&self) -> Extent {
        self.chain.as_ref().map_or(
            Extent {
                width: 0,
                height: 0,
            },
            |chain| chain.extent,
        )
    }

    /// Recreate the swapchain for a new window size. A zero extent
    /// (minimized window) tears the swapchain down; the target stays
    /// dormant — [`render`](Self::render) reports
    /// [`PresentOutcome::NeedsResize`] — until a non-zero resize.
    ///
    /// # Errors
    ///
    /// Creation errors from the driver; [`TargetError::DeviceLost`] on
    /// a poisoned device.
    pub fn resize(&mut self, extent: Extent) -> Result<(), TargetError> {
        if self.shared.lost.get() {
            return Err(TargetError::DeviceLost);
        }
        // Quiesce before touching anything the GPU might still read.
        // SAFETY: category 2: device live; single-threaded contract.
        let idle = unsafe { self.shared.device.device_wait_idle() };
        if let Err(code) = idle {
            self.shared.note_result(code);
            return Err(if code == vk::Result::ERROR_DEVICE_LOST {
                TargetError::DeviceLost
            } else {
                creation("vkDeviceWaitIdle", code)
            });
        }
        self.destroy_chain();
        if extent.width == 0 || extent.height == 0 {
            return Ok(());
        }
        self.build_chain(extent)
    }

    /// Clear, optionally draw one 3-vertex call, and present.
    ///
    /// The pipeline must come from this target's device and target
    /// [`format`](Self::format) — contract violations, checked in dev
    /// builds.
    ///
    /// # Errors
    ///
    /// [`TargetError::Timeout`] when the GPU exceeds the watchdog;
    /// [`TargetError::DeviceLost`] on device loss; submission errors
    /// otherwise. A stale swapchain is not an error — it is
    /// [`PresentOutcome::NeedsResize`].
    #[expect(
        clippy::too_many_lines,
        reason = "one recorded frame; wait, acquire, record, submit, present read top to bottom"
    )]
    pub fn render(
        &mut self,
        clear: Color,
        pipeline: Option<&RenderPipeline>,
    ) -> Result<PresentOutcome, TargetError> {
        if self.shared.lost.get() {
            return Err(TargetError::DeviceLost);
        }
        if let Some(pipeline) = pipeline {
            debug_assert!(
                Rc::ptr_eq(&self.shared, &pipeline.shared),
                "pipeline and target come from different devices"
            );
            debug_assert!(
                pipeline.format == self.format,
                "pipeline targets {:?}, swapchain is {:?}",
                pipeline.format,
                self.format
            );
        }
        let Some(chain) = &self.chain else {
            return Ok(PresentOutcome::NeedsResize);
        };
        let device = &self.shared.device;

        // Wait for the previous frame; the fence starts signaled, so
        // the order wait → acquire → reset → submit never deadlocks on
        // an early NeedsResize return.
        // SAFETY: category 2 (ash dispatch) for every call below: all
        // handles live and owned by this target or its chain;
        // single-threaded by the crate contract; every info struct is a
        // local outliving its call.
        unsafe {
            match device.wait_for_fences(&[self.fence], true, FENCE_TIMEOUT_NS) {
                Ok(()) => {}
                Err(vk::Result::TIMEOUT) => {
                    return Err(TargetError::Timeout {
                        call: "vkWaitForFences",
                    });
                }
                Err(code) => {
                    self.shared.note_result(code);
                    return Err(if code == vk::Result::ERROR_DEVICE_LOST {
                        TargetError::DeviceLost
                    } else {
                        creation("vkWaitForFences", code)
                    });
                }
            }

            let acquired = self.swapchain_loader.acquire_next_image(
                chain.swapchain,
                FENCE_TIMEOUT_NS,
                self.image_available,
                vk::Fence::null(),
            );
            let (index, suboptimal) = match acquired {
                Ok(pair) => pair,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    return Ok(PresentOutcome::NeedsResize);
                }
                Err(vk::Result::TIMEOUT | vk::Result::NOT_READY) => {
                    return Err(TargetError::Timeout {
                        call: "vkAcquireNextImageKHR",
                    });
                }
                Err(code) => {
                    self.shared.note_result(code);
                    return Err(if code == vk::Result::ERROR_DEVICE_LOST {
                        TargetError::DeviceLost
                    } else {
                        creation("vkAcquireNextImageKHR", code)
                    });
                }
            };
            let slot = index as usize;
            let (Some(&image), Some(&view), Some(&finished)) = (
                chain.images.get(slot),
                chain.views.get(slot),
                chain.render_finished.get(slot),
            ) else {
                return Err(TargetError::Creation {
                    call: "vkAcquireNextImageKHR(index)",
                    code: 0,
                });
            };

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

            let to_color = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::NONE)
                .src_access_mask(vk::AccessFlags2::NONE)
                .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .image(image)
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
                .image_view(view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(clear_value);
            let attachments = [attachment];
            let area = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: chain.extent.width,
                    height: chain.extent.height,
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
                    width: chain.extent.width as f32,
                    height: chain.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                };
                device.cmd_set_viewport(self.cmd, 0, &[viewport]);
                device.cmd_set_scissor(self.cmd, 0, &[area]);
                device.cmd_draw(self.cmd, 3, 1, 0, 0);
            }
            device.cmd_end_rendering(self.cmd);

            // COLOR_ATTACHMENT_OPTIMAL → PRESENT_SRC; the signal
            // semaphore orders presentation, so no destination stage.
            let to_present = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::NONE)
                .dst_access_mask(vk::AccessFlags2::NONE)
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .image(image)
                .subresource_range(color_range());
            let barriers = [to_present];
            device.cmd_pipeline_barrier2(
                self.cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&barriers),
            );

            device
                .end_command_buffer(self.cmd)
                .map_err(|code| creation("vkEndCommandBuffer", code))?;

            device
                .reset_fences(&[self.fence])
                .map_err(|code| creation("vkResetFences", code))?;
            let wait_infos = [vk::SemaphoreSubmitInfo::default()
                .semaphore(self.image_available)
                .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
            let cmd_infos = [vk::CommandBufferSubmitInfo::default().command_buffer(self.cmd)];
            let signal_infos = [vk::SemaphoreSubmitInfo::default()
                .semaphore(finished)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            let submit = vk::SubmitInfo2::default()
                .wait_semaphore_infos(&wait_infos)
                .command_buffer_infos(&cmd_infos)
                .signal_semaphore_infos(&signal_infos);
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

            let swapchains = [chain.swapchain];
            let indices = [index];
            let wait = [finished];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&wait)
                .swapchains(&swapchains)
                .image_indices(&indices);
            let presented = self
                .swapchain_loader
                .queue_present(self.shared.queue, &present_info);
            match presented {
                Ok(false) if !suboptimal => Ok(PresentOutcome::Presented),
                Ok(_) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(PresentOutcome::NeedsResize),
                Err(code) => {
                    self.shared.note_result(code);
                    Err(if code == vk::Result::ERROR_DEVICE_LOST {
                        TargetError::DeviceLost
                    } else {
                        creation("vkQueuePresentKHR", code)
                    })
                }
            }
        }
    }

    /// Build the swapchain and its per-image resources for `extent`
    /// (clamped to the surface's current limits).
    fn build_chain(&mut self, extent: Extent) -> Result<(), TargetError> {
        let shared = &self.shared;
        // SAFETY: category 2: physical device and surface live.
        let caps = unsafe {
            self.surface_loader
                .get_physical_device_surface_capabilities(shared.physical, self.surface)
        }
        .map_err(|code| creation("vkGetPhysicalDeviceSurfaceCapabilitiesKHR", code))?;

        // The surface dictates the extent when it reports one; the
        // sentinel u32::MAX means "you choose, within bounds".
        let chosen = if caps.current_extent.width == u32::MAX {
            Extent {
                width: extent
                    .width
                    .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                height: extent
                    .height
                    .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
            }
        } else {
            Extent {
                width: caps.current_extent.width,
                height: caps.current_extent.height,
            }
        };
        if chosen.width == 0 || chosen.height == 0 {
            // Surface currently unrenderable (mid-minimize); stay
            // dormant.
            return Ok(());
        }

        let mut image_count = caps.min_image_count.saturating_add(1);
        if caps.max_image_count > 0 {
            image_count = image_count.min(caps.max_image_count);
        }
        let composite = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::INHERIT,
        ]
        .into_iter()
        .find(|&flag| caps.supported_composite_alpha.contains(flag))
        .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);

        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(image_count)
            .image_format(self.vk_format)
            .image_color_space(self.color_space)
            .image_extent(vk::Extent2D {
                width: chosen.width,
                height: chosen.height,
            })
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(composite)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true);
        // SAFETY: category 2: surface live; info local; callbacks'
        // ledger outlives the swapchain.
        let swapchain = unsafe {
            self.swapchain_loader
                .create_swapchain(&info, Some(&shared.alloc_cbs()))
        }
        .map_err(|code| creation("vkCreateSwapchainKHR", code))?;

        let mut chain = Chain {
            swapchain,
            extent: chosen,
            views: Vec::new(),
            images: Vec::new(),
            render_finished: Vec::new(),
        };
        let built = self.populate_chain(&mut chain);
        if let Err(error) = built {
            self.chain = Some(chain);
            self.destroy_chain();
            return Err(error);
        }
        self.chain = Some(chain);
        Ok(())
    }

    /// Fill in per-image views and semaphores for a fresh swapchain.
    fn populate_chain(&self, chain: &mut Chain) -> Result<(), TargetError> {
        let shared = &self.shared;
        // SAFETY: category 2: swapchain live.
        chain.images = unsafe { self.swapchain_loader.get_swapchain_images(chain.swapchain) }
            .map_err(|code| creation("vkGetSwapchainImagesKHR", code))?;
        for &image in &chain.images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(self.vk_format)
                .subresource_range(color_range());
            // SAFETY: category 2: image live (owned by the swapchain).
            let view = unsafe {
                shared
                    .device
                    .create_image_view(&view_info, Some(&shared.alloc_cbs()))
            }
            .map_err(|code| creation("vkCreateImageView", code))?;
            chain.views.push(view);
            // SAFETY: category 2: device live; default info local.
            let semaphore = unsafe {
                shared.device.create_semaphore(
                    &vk::SemaphoreCreateInfo::default(),
                    Some(&shared.alloc_cbs()),
                )
            }
            .map_err(|code| creation("vkCreateSemaphore", code))?;
            chain.render_finished.push(semaphore);
        }
        Ok(())
    }

    /// Destroy the chain's per-image resources and the swapchain. The
    /// caller quiesces the GPU first (resize and Drop both wait-idle).
    fn destroy_chain(&mut self) {
        let Some(chain) = self.chain.take() else {
            return;
        };
        // SAFETY: category 2: every handle live and created with these
        // callbacks; the GPU is idle per the caller contract.
        unsafe {
            for view in chain.views {
                self.shared
                    .device
                    .destroy_image_view(view, Some(&self.shared.alloc_cbs()));
            }
            for semaphore in chain.render_finished {
                self.shared
                    .device
                    .destroy_semaphore(semaphore, Some(&self.shared.alloc_cbs()));
            }
            self.swapchain_loader
                .destroy_swapchain(chain.swapchain, Some(&self.shared.alloc_cbs()));
        }
    }
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1)
}

impl Drop for WindowTarget {
    fn drop(&mut self) {
        // SAFETY: category 2: wait-idle (best-effort) quiesces
        // presentation and submitted work; then reverse creation order,
        // each handle created with these callbacks; the surface goes
        // after the swapchain, the window keep-alive after everything
        // (field drop).
        unsafe {
            let _ = self.shared.device.device_wait_idle();
        }
        self.destroy_chain();
        // SAFETY: as above.
        unsafe {
            self.shared
                .device
                .destroy_semaphore(self.image_available, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .destroy_fence(self.fence, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .destroy_command_pool(self.pool, Some(&self.shared.alloc_cbs()));
            self.surface_loader
                .destroy_surface(self.surface, Some(&self.shared.alloc_cbs()));
        }
    }
}
