//! The window render target: surface, swapchain, and the
//! clear-and-draw-then-present loop, one frame in flight.
//!
//! The target owns a keep-alive handle to the native window (boxed,
//! opaque) — the platform window cannot be torn down under a live
//! surface, by ownership rather than by discipline.
//!
//! Error discipline: a mid-frame driver failure quiesces the GPU and
//! tears the swapchain down (the target goes dormant, exactly like a
//! minimized window), so no semaphore or fence is ever left carrying a
//! stale pending operation into the next frame. A later
//! [`WindowTarget::resize`] rebuilds everything fresh.

use std::rc::Rc;

use ash::vk;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::config::Extent;
use crate::error::TargetError;
use crate::vk::device::{Device, DeviceShared, FENCE_TIMEOUT_NS};
use crate::vk::pipeline::{RenderDesc, TargetFormat};

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
    /// The swapchain no longer matches the surface (resize, minimize),
    /// or the target is dormant; call [`WindowTarget::resize`] with the
    /// current size. Nothing was presented.
    NeedsResize,
}

/// The live swapchain and everything sized or bound to it; absent
/// while the target is dormant (zero-sized window, or a mid-frame
/// failure that forced a teardown).
struct Chain {
    swapchain: vk::SwapchainKHR,
    extent: Extent,
    views: Vec<vk::ImageView>,
    images: Vec<vk::Image>,
    /// One acquire semaphore **per frame in flight**, not per image.
    ///
    /// `vkAcquireNextImageKHR` signals this *before* the image index is
    /// known, so indexing it by image would be circular: it belongs to
    /// the frame slot.
    ///
    /// Chain-owned, as the single semaphore was and for the same
    /// reason: a teardown after a mid-frame failure retires every
    /// pending signal operation along with it, and the next chain starts
    /// with fresh unsignaled semaphores. Moving the ring onto the target
    /// so it survives a rebuild reopens that question — a semaphore
    /// signalled against a destroyed swapchain is not simply reusable.
    image_available: [vk::Semaphore; FRAMES_IN_FLIGHT],
    /// One per swapchain image: the present engine may still wait on
    /// the semaphore for image N after our fence signals, so per-image
    /// signaling is the simplest correct scheme.
    render_finished: Vec<vk::Semaphore>,
}

/// How many frames the CPU may have submitted and not yet waited for.
///
/// **Not the swapchain image count.** That is chosen by the driver and
/// the surface; this is chosen by us, and the two are different numbers
/// that happen to be small. Every per-frame resource below multiplies by
/// this one; `render_finished` multiplies by the other.
///
/// Two rather than three: presentation is FIFO, so a third frame buys
/// queueing depth that shows up as latency rather than throughput at this
/// scale. A capability claim, not a measured one — no frame-time budget
/// exists to justify a specific number yet.
const FRAMES_IN_FLIGHT: usize = 2;

/// The same count as Vulkan wants it. Derived rather than written twice,
/// and checked where a mistake costs nothing: a second literal is a
/// hand-maintained pair, and a runtime conversion would put a panic on
/// the creation path for a question the compiler can answer.
const FRAMES_IN_FLIGHT_U32: u32 = {
    assert!(FRAMES_IN_FLIGHT <= u32::MAX as usize);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the assertion above rejects any value that would truncate, at compile time"
    )]
    {
        FRAMES_IN_FLIGHT as u32
    }
};

/// A window-backed render target. `FRAMES_IN_FLIGHT` frames in flight;
/// presentation is FIFO (vsync) — the guaranteed-available mode.
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
    /// One recording per frame slot: a buffer must not be re-recorded
    /// while a submit reading it is still outstanding.
    cmds: [vk::CommandBuffer; FRAMES_IN_FLIGHT],
    /// Created unsignaled; `pending[i]` tracks whether a submit is
    /// outstanding on fence `i`. The invariant at every public-call
    /// boundary, now per slot: **`pending[i] == false` implies fence `i`
    /// is unsignaled with nothing outstanding.**
    ///
    /// Two distinct questions come off this, and they were one question
    /// when there was one fence. *"May I record into slot i?"* is
    /// `!pending[i]`, and it is the frame path. *"Is anything
    /// outstanding at all?"* is `pending.iter().any(..)`, and it is the
    /// teardown path — retiring one fence where several may be pending
    /// is the defect this split exists to prevent.
    fences: [vk::Fence; FRAMES_IN_FLIGHT],
    pending: [bool; FRAMES_IN_FLIGHT],
    /// The slot the next frame records into; advances after each submit.
    frame: usize,
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
        if shared.lost.poisoned() {
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
            return Err(TargetError::PresentUnsupported {
                reason: "the device does not offer the swapchain extension",
            });
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
        // instance live via the spine. Handle validity: the raw handles
        // were just produced through raw-window-handle's borrow-scoped
        // accessors on `window`, which the target takes ownership of
        // below and keeps for the surface's whole life — and no safe
        // implementation of the handle traits can produce a handle that
        // outlives the window it borrows from without itself using
        // `unsafe` incorrectly. (For the intended argument — the
        // platform's `NativeWindow`, an owning keep-alive — validity is
        // direct.)
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
            return Err(fail(
                self,
                TargetError::PresentUnsupported {
                    reason: "the graphics queue cannot present to this surface",
                },
            ));
        }

        // SAFETY: category 2: as above.
        let formats =
            unsafe { surface_loader.get_physical_device_surface_formats(shared.physical, surface) }
                .map_err(|code| {
                    fail(self, creation("vkGetPhysicalDeviceSurfaceFormatsKHR", code))
                })?;
        let Some((surface_format, format)) = choose_surface_format(&formats) else {
            return Err(fail(
                self,
                TargetError::PresentUnsupported {
                    reason: "no 8-bit UNORM sRGB surface format",
                },
            ));
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
            .command_buffer_count(FRAMES_IN_FLIGHT_U32);
        // SAFETY: category 2: pool live; info local.
        // ash sizes the vector it returns from the create info's own
        // `command_buffer_count`, so a success always carries exactly
        // the buffers asked for; a short success would be a broken ash
        // rather than a broken driver, and it reports through the same
        // return as a driver failure rather than costing a branch of its
        // own.
        let allocated =
            unsafe { shared.device.allocate_command_buffers(&cmd_info) }.and_then(|buffers| {
                <[vk::CommandBuffer; FRAMES_IN_FLIGHT]>::try_from(buffers)
                    .map_err(|_| vk::Result::ERROR_UNKNOWN)
            });
        let cmds = match allocated {
            Ok(cmds) => cmds,
            Err(code) => {
                return Err(fail_pool(self, creation("vkAllocateCommandBuffers", code)));
            }
        };

        // Unsignaled: every `pending` entry starts false, and the
        // protocol only waits when a submit is actually outstanding.
        // Built one at a time so a failure part-way destroys the ones
        // already made -- `Partial` cannot express a half-built ring.
        let mut fences = [vk::Fence::null(); FRAMES_IN_FLIGHT];
        for slot in 0..FRAMES_IN_FLIGHT {
            // SAFETY: category 2: device live; default info local.
            match unsafe {
                shared
                    .device
                    .create_fence(&vk::FenceCreateInfo::default(), Some(&shared.alloc_cbs()))
            } {
                Ok(fence) => fences[slot] = fence,
                Err(code) => {
                    for made in &fences[..slot] {
                        // SAFETY: category 2: device live; each handle
                        // was created just above with these callbacks
                        // and nothing has been submitted against it.
                        unsafe {
                            shared
                                .device
                                .destroy_fence(*made, Some(&shared.alloc_cbs()));
                        }
                    }
                    return Err(fail_pool(self, creation("vkCreateFence", code)));
                }
            }
        }

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
            cmds,
            fences,
            pending: [false; FRAMES_IN_FLIGHT],
            frame: 0,
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

    /// The current drawable size; zero while dormant.
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
        if self.shared.lost.poisoned() {
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
        self.retire_fence_after_idle();
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
    /// otherwise — any such mid-frame failure also tears the swapchain
    /// down (the target goes dormant until resized). A stale swapchain
    /// is not an error — it is [`PresentOutcome::NeedsResize`].
    #[expect(
        clippy::too_many_lines,
        reason = "one recorded frame; wait, acquire, record, submit, present read top to bottom"
    )]
    pub fn render(&mut self, desc: &RenderDesc<'_>) -> Result<PresentOutcome, TargetError> {
        let RenderDesc {
            clear, pipeline, ..
        } = *desc;
        if self.shared.lost.poisoned() {
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
        // Wait for the previous frame if one is outstanding; after
        // this, the fence is unsignaled with nothing pending, so every
        // early return below leaves the protocol consistent.
        // SAFETY: category 2 (ash dispatch) for every call below: all
        // handles live and owned by this target or its chain;
        // single-threaded by the crate contract; every info struct is a
        // local outliving its call.
        unsafe {
            let frame = self.frame;
            if self.pending[frame] {
                match self.shared.device.wait_for_fences(
                    &[self.fences[frame]],
                    true,
                    FENCE_TIMEOUT_NS,
                ) {
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
                if let Err(code) = self.shared.device.reset_fences(&[self.fences[frame]]) {
                    return Err(creation("vkResetFences", code));
                }
                self.pending[frame] = false;
            }

            let Some(chain) = self.chain.as_ref() else {
                return Ok(PresentOutcome::NeedsResize);
            };
            let acquired = self.swapchain_loader.acquire_next_image(
                chain.swapchain,
                FENCE_TIMEOUT_NS,
                chain.image_available[frame],
                vk::Fence::null(),
            );
            let (index, suboptimal) = match acquired {
                Ok(pair) => pair,
                // No semaphore signal is queued on any of these
                // returns, so the chain stays consistent.
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
            // From here to the submit, `image_available` carries a
            // pending signal that only the submit will retire: any
            // failure must tear the chain down (which quiesces first),
            // never merely return.
            // The SWAPCHAIN IMAGE index -- not the frame slot, and not
            // bounded by `FRAMES_IN_FLIGHT`. It was called `slot` until
            // 2026-08-01, when introducing the frame ring let it shadow
            // the frame's own slot and index frame-sized arrays by
            // image: three images into two slots, caught by the present
            // suites. Both names now say which they are.
            let image_index = index as usize;
            let (Some(&image), Some(&view), Some(&finished)) = (
                chain.images.get(image_index),
                chain.views.get(image_index),
                chain.render_finished.get(image_index),
            ) else {
                return Err(self.abort_frame(TargetError::Creation {
                    call: "vkAcquireNextImageKHR(index)",
                    code: 0,
                }));
            };
            let image_available = chain.image_available[frame];
            let cmd = self.cmds[frame];
            let swapchain = chain.swapchain;
            let chain_extent = chain.extent;
            let device = &self.shared.device;

            if let Err(code) =
                device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
            {
                return Err(self.abort_frame(creation("vkResetCommandBuffer", code)));
            }
            if let Err(code) = device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            ) {
                return Err(self.abort_frame(creation("vkBeginCommandBuffer", code)));
            }

            // UNDEFINED → COLOR_ATTACHMENT_OPTIMAL. The source stage is
            // COLOR_ATTACHMENT_OUTPUT — the same stage the acquire
            // semaphore wait is scoped to — which chains this barrier
            // after the semaphore and orders the layout transition
            // against the presentation engine's outstanding reads of
            // this image (the classic write-after-present hazard).
            let to_color = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::NONE)
                .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .image(image)
                .subresource_range(color_range());
            let barriers = [to_color];
            device.cmd_pipeline_barrier2(
                cmd,
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
                    width: chain_extent.width,
                    height: chain_extent.height,
                },
            };
            device.cmd_begin_rendering(
                cmd,
                &vk::RenderingInfo::default()
                    .render_area(area)
                    .layer_count(1)
                    .color_attachments(&attachments),
            );
            if let Some(pipeline) = pipeline {
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline.pipeline);
                // Extents are far below f32's exact-integer range; the
                // casts are lossless in practice.
                #[allow(clippy::cast_precision_loss)]
                let viewport = vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: chain_extent.width as f32,
                    height: chain_extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                };
                device.cmd_set_viewport(cmd, 0, &[viewport]);
                device.cmd_set_scissor(cmd, 0, &[area]);
                pipeline.bind_descriptors(cmd);
                device.cmd_draw(cmd, pipeline.vertex_count, 1, 0, 0);
            }
            device.cmd_end_rendering(cmd);

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
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&barriers),
            );

            if let Err(code) = device.end_command_buffer(cmd) {
                return Err(self.abort_frame(creation("vkEndCommandBuffer", code)));
            }

            let wait_infos = [vk::SemaphoreSubmitInfo::default()
                .semaphore(image_available)
                .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
            let cmd_infos = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
            let signal_infos = [vk::SemaphoreSubmitInfo::default()
                .semaphore(finished)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            let submit = vk::SubmitInfo2::default()
                .wait_semaphore_infos(&wait_infos)
                .command_buffer_infos(&cmd_infos)
                .signal_semaphore_infos(&signal_infos);
            if let Err(code) =
                device.queue_submit2(self.shared.queue, &[submit], self.fences[frame])
            {
                self.shared.note_result(code);
                let error = if code == vk::Result::ERROR_DEVICE_LOST {
                    TargetError::DeviceLost
                } else {
                    creation("vkQueueSubmit2", code)
                };
                return Err(self.abort_frame(error));
            }
            self.pending[frame] = true;
            // Advance only after a successful submit: a frame that failed
            // to submit left its slot unused, and skipping it would leak a
            // slot per failure.
            self.frame = (self.frame + 1) % FRAMES_IN_FLIGHT;

            let swapchains = [swapchain];
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
                    let error = if code == vk::Result::ERROR_DEVICE_LOST {
                        TargetError::DeviceLost
                    } else {
                        creation("vkQueuePresentKHR", code)
                    };
                    Err(self.abort_frame(error))
                }
            }
        }
    }

    /// Mid-frame failure path: quiesce the GPU, retire the fence, and
    /// tear the chain down so no pending semaphore or fence operation
    /// survives into the next frame. Returns the error it was handed,
    /// for `return Err(self.abort_frame(error))` call sites.
    fn abort_frame(&mut self, error: TargetError) -> TargetError {
        // SAFETY: category 2: device live; best-effort — the target is
        // going dormant regardless.
        unsafe {
            let _ = self.shared.device.device_wait_idle();
        }
        self.retire_fence_after_idle();
        self.destroy_chain();
        error
    }

    /// After a successful (or best-effort) wait-idle, a pending fence
    /// is signaled: reset it so the unsignaled-when-not-pending
    /// invariant holds.
    fn retire_fence_after_idle(&mut self) {
        for slot in 0..FRAMES_IN_FLIGHT {
            if self.pending[slot] {
                // SAFETY: category 2: fence live; nothing outstanding
                // after the caller's quiesce.
                unsafe {
                    let _ = self.shared.device.reset_fences(&[self.fences[slot]]);
                }
                self.pending[slot] = false;
            }
        }
    }

    /// How many frames this target may have in flight at once.
    ///
    /// **Public because a consumer needs it, not for testing.** The
    /// resource model requires the caller to double-buffer any buffer a
    /// live frame may read -- the RHI deliberately owns no scratch -- and
    /// the correct number of copies is exactly this. A consumer that
    /// guesses two while the target runs three writes into memory the GPU
    /// is reading.
    #[must_use]
    pub fn frames_in_flight(&self) -> usize {
        FRAMES_IN_FLIGHT
    }

    /// Which slot the next frame will use, in `0..frames_in_flight()`.
    ///
    /// The other half of what a double-buffering consumer needs: knowing
    /// how many copies to keep is useless without knowing which one this
    /// frame belongs to. Advances after each successful submit.
    ///
    /// # Ordering — read this before writing the copy this names
    ///
    /// **Knowing the slot is not permission to write it yet.** The slot
    /// returned here may still have a submit in flight: its fence is
    /// waited at the *start* of the [`render`](Self::render) that uses
    /// it, and the slot advances at the *end* of the previous one. So
    /// between two calls, the slot this names is generally the one whose
    /// submit has not been waited for — at a depth of two, the first two
    /// frames are clear and every frame after that is not.
    ///
    /// A consumer that writes its copy for this slot between frames is
    /// therefore writing memory the GPU may be reading. **Keeping
    /// [`frames_in_flight`](Self::frames_in_flight) copies is necessary
    /// and it is not sufficient**; nothing in this API currently reports
    /// when a slot's previous submit has completed, so per-frame data
    /// that a draw reads has no safe write point through these two
    /// accessors alone.
    ///
    /// They remain correct for anything that does not race a submit —
    /// choosing which of N caller-owned resources to *create* or label,
    /// or reading back after a frame the caller has otherwise
    /// synchronised.
    #[must_use]
    pub fn frame_slot(&self) -> usize {
        self.frame
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
            image_available: [vk::Semaphore::null(); FRAMES_IN_FLIGHT],
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

    /// Fill in the acquire semaphore and per-image views and semaphores
    /// for a fresh swapchain.
    fn populate_chain(&self, chain: &mut Chain) -> Result<(), TargetError> {
        let shared = &self.shared;
        // One per frame slot. Built in a loop rather than as an array
        // expression because each is fallible, and a partial ring must
        // be torn down by `destroy_chain` rather than left half-made:
        // the nulls this started as are what makes that safe, since
        // destroying a null handle is defined as doing nothing.
        for slot in 0..FRAMES_IN_FLIGHT {
            // SAFETY: category 2: device live; default info local.
            chain.image_available[slot] = unsafe {
                shared.device.create_semaphore(
                    &vk::SemaphoreCreateInfo::default(),
                    Some(&shared.alloc_cbs()),
                )
            }
            .map_err(|code| creation("vkCreateSemaphore", code))?;
        }
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

    /// Destroy the chain's semaphores, views, and swapchain. The caller
    /// quiesces the GPU first (resize, `abort_frame`, and Drop all
    /// wait-idle).
    fn destroy_chain(&mut self) {
        let Some(chain) = self.chain.take() else {
            return;
        };
        // SAFETY: category 2: every handle live and created with these
        // callbacks; the GPU is idle per the caller contract; a null
        // acquire semaphore (populate failed before creating it) is a
        // legal no-op destroy.
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
            for semaphore in chain.image_available {
                self.shared
                    .device
                    .destroy_semaphore(semaphore, Some(&self.shared.alloc_cbs()));
            }
            self.swapchain_loader
                .destroy_swapchain(chain.swapchain, Some(&self.shared.alloc_cbs()));
        }
    }
}

/// Pick the surface format to present in, preferring BGRA because it is
/// what desktop compositors overwhelmingly report first.
///
/// Pure, and separated from the surface query for that reason: which
/// formats a surface offers is a property of the machine, so testing
/// the CHOICE through a real driver only ever proves what this one
/// machine happens to report. Given the list, the answer is fixed.
fn choose_surface_format(
    formats: &[vk::SurfaceFormatKHR],
) -> Option<(vk::SurfaceFormatKHR, TargetFormat)> {
    let chosen = [vk::Format::B8G8R8A8_UNORM, vk::Format::R8G8B8A8_UNORM]
        .into_iter()
        .find_map(|want| {
            formats
                .iter()
                .find(|f| f.format == want && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        })?;
    let format = match chosen.format {
        vk::Format::R8G8B8A8_UNORM => TargetFormat::Rgba8Unorm,
        _ => TargetFormat::Bgra8Unorm,
    };
    Some((*chosen, format))
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
        self.retire_fence_after_idle();
        self.destroy_chain();
        // SAFETY: as above.
        unsafe {
            for fence in self.fences {
                self.shared
                    .device
                    .destroy_fence(fence, Some(&self.shared.alloc_cbs()));
            }
            self.shared
                .device
                .destroy_command_pool(self.pool, Some(&self.shared.alloc_cbs()));
            self.surface_loader
                .destroy_surface(self.surface, Some(&self.shared.alloc_cbs()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TargetFormat, choose_surface_format};
    use ash::vk;

    fn offered(format: vk::Format, color_space: vk::ColorSpaceKHR) -> vk::SurfaceFormatKHR {
        vk::SurfaceFormatKHR {
            format,
            color_space,
        }
    }

    const SRGB: vk::ColorSpaceKHR = vk::ColorSpaceKHR::SRGB_NONLINEAR;

    #[test]
    fn bgra_wins_when_a_surface_offers_both() {
        let formats = [
            offered(vk::Format::R8G8B8A8_UNORM, SRGB),
            offered(vk::Format::B8G8R8A8_UNORM, SRGB),
        ];
        let (chosen, format) = choose_surface_format(&formats).expect("both are acceptable");
        assert_eq!(chosen.format, vk::Format::B8G8R8A8_UNORM);
        assert_eq!(format, TargetFormat::Bgra8Unorm);
    }

    #[test]
    fn rgba_is_taken_when_it_is_the_only_one_offered() {
        // Reachable on real hardware, not on every machine: proving it
        // through a driver would only prove what this machine reports.
        let formats = [
            offered(vk::Format::R8G8B8A8_SRGB, SRGB),
            offered(vk::Format::R8G8B8A8_UNORM, SRGB),
        ];
        let (chosen, format) = choose_surface_format(&formats).expect("rgba is acceptable");
        assert_eq!(chosen.format, vk::Format::R8G8B8A8_UNORM);
        assert_eq!(format, TargetFormat::Rgba8Unorm);
    }

    #[test]
    fn the_colour_space_must_match_too() {
        let formats = [offered(
            vk::Format::B8G8R8A8_UNORM,
            vk::ColorSpaceKHR::DISPLAY_P3_NONLINEAR_EXT,
        )];
        assert!(
            choose_surface_format(&formats).is_none(),
            "an 8-bit format in the wrong colour space is not acceptable"
        );
    }

    #[test]
    fn a_surface_offering_nothing_acceptable_is_refused() {
        assert!(choose_surface_format(&[]).is_none(), "no formats at all");
        let unusable = [offered(vk::Format::R5G6B5_UNORM_PACK16, SRGB)];
        assert!(
            choose_surface_format(&unusable).is_none(),
            "only formats the engine cannot present in"
        );
    }
}
