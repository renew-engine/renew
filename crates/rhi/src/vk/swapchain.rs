//! The window render target: surface, swapchain, and the
//! record-passes-then-present loop, `FRAMES_IN_FLIGHT` frames in
//! flight.
//!
//! The target owns a keep-alive handle to the native window — so no
//! window is torn down under a live surface while its surface epoch
//! lasts; a platform ending the epoch has the owner drop the target.
//!
//! Error discipline: a mid-frame driver failure quiesces the GPU and
//! tears the swapchain down (the target goes dormant, exactly like a
//! minimized window), so no semaphore or fence is ever left carrying a
//! stale pending operation into the next frame. A later
//! [`WindowTarget::resize`] rebuilds everything fresh.

use std::rc::Rc;

use ash::vk;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::config::{Extent, SurfaceTransform};
use crate::error::TargetError;
use crate::vk::depth::{self, DepthResources};
use crate::vk::device::{Device, DeviceShared, FENCE_TIMEOUT_NS};
use crate::vk::pass::{self, MAX_RETAINED_RESOURCES, RenderDesc, Retained};
use crate::vk::pipeline::{INSTANCE_BINDING, TargetFormat, VERTEX_BINDING};
use crate::vk::transition;

/// The engine's spelling of a surface transform.
///
/// Anything that is not one of the four rotations — the mirrored
/// variants, or an inherit that a driver leaves to the platform — reads
/// as the identity, because a renderer that cannot fold it must not be
/// told it has been folded. The narrowing is deliberate and stated: the
/// four rotations are what handheld panels report, and the rest would
/// be a promise no consumer here can keep.
const fn from_vk_transform(transform: vk::SurfaceTransformFlagsKHR) -> SurfaceTransform {
    if transform.as_raw() == vk::SurfaceTransformFlagsKHR::ROTATE_90.as_raw() {
        SurfaceTransform::Rotate90
    } else if transform.as_raw() == vk::SurfaceTransformFlagsKHR::ROTATE_180.as_raw() {
        SurfaceTransform::Rotate180
    } else if transform.as_raw() == vk::SurfaceTransformFlagsKHR::ROTATE_270.as_raw() {
        SurfaceTransform::Rotate270
    } else {
        SurfaceTransform::Identity
    }
}

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
    /// The transform this chain was built declaring, which is the one
    /// the surface reported at build time. Kept because a caller that
    /// draws unrotated content into a chain declaring a rotation gets a
    /// sideways image, and it can only avoid that if it can ask.
    transform: SurfaceTransform,
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
    /// One depth image **per frame slot**, when the adapter offers a
    /// format: two frames in flight write depth concurrently, so a
    /// shared image would race. Chain-scoped like the semaphores —
    /// created in `build_chain`, destroyed in `destroy_chain` — so all
    /// layout and first-use state dies with the chain, which is what
    /// makes the transition-from-UNDEFINED-every-frame claim true
    /// across resize and dormancy.
    depth: Vec<DepthResources>,
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

/// The ring this target indexes must be at least as deep as the frames it
/// keeps in flight.
///
/// **Checked at build time, because the alternative is silent corruption.**
/// `frame` indexes per-frame buffers' slot regions, whose count is
/// `MAX_FRAME_SLOTS` in another module — two independent literals with no
/// relation between them until this line. Raising this constant alone
/// reads as a local change and turns every per-frame copy into a write
/// past the allocation; a runtime assert would catch it only in a debug
/// build, which is not where the damage happens.
const _: () = assert!(
    FRAMES_IN_FLIGHT <= crate::vk::buffer::MAX_FRAME_SLOTS,
    "the presentation ring is deeper than the per-frame buffers it indexes"
);

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
    /// Buffers a slot's recorded work references, retained so a caller
    /// dropping its handle cannot free memory the submit still reads.
    /// Fixed-width per slot — the frame contract refuses more distinct
    /// buffers than fit. Released only when the work has PROVABLY
    /// ended: a successful fence wait, a successful quiesce, a lost
    /// device, or the top of a copy phase whose slot either had its
    /// fence waited this call or never submitted (the acquire-failure
    /// early returns leave stale entries with `pending` false — the
    /// field invariant on `pending` is what makes releasing them
    /// sound). Deliberately NOT cleared beside `pending` after a failed
    /// non-lost quiesce: `pending` answers "may I record?", retention
    /// answers "may memory die?", and the failed-quiesce corner is
    /// where those two questions have different answers.
    retained: [[Option<Retained>; MAX_RETAINED_RESOURCES]; FRAMES_IN_FLIGHT],
    /// Set when a frame aborted and the recovery quiesce FAILED without
    /// a lost device: the GPU may still be executing, so fences were not
    /// reset, flags were not cleared, the chain was not destroyed and
    /// retained memory was not released — the validation layer wrote
    /// three VUIDs against the path that used to do all four. Everything
    /// waits, intact, for the next PROVEN quiesce: `resize`'s, whose
    /// wait-idle must succeed before it retires anything.
    dormant: bool,
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
        // platform's `NativeWindow`, an owning keep-alive within its
        // surface epoch — validity is direct for the surface's life.)
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
                    reason: "no 8-bit sRGB surface format",
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
            retained: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            dormant: false,
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

    /// The rotation this target's swapchain declares.
    ///
    /// **The presentation engine believes it**: content drawn without
    /// the matching counter-rotation arrives on screen turned by this
    /// much. Identity on every desktop surface and while dormant, so a
    /// caller that folds it correctly is correct everywhere and a
    /// caller that ignores it is only wrong where a panel rotates.
    #[must_use]
    pub fn transform(&self) -> SurfaceTransform {
        if self.dormant {
            return SurfaceTransform::Identity;
        }
        self.chain
            .as_ref()
            .map_or(SurfaceTransform::Identity, |chain| chain.transform)
    }

    /// The current drawable size; zero while dormant.
    #[must_use]
    pub fn extent(&self) -> Extent {
        // A parked target keeps its chain object alive purely so nothing
        // GPU-referenced is destroyed without proof; publicly it is as
        // dormant as one whose chain is gone, and it reports the same.
        if self.dormant {
            return Extent {
                width: 0,
                height: 0,
            };
        }
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
        self.dormant = false;
        if extent.width == 0 || extent.height == 0 {
            return Ok(());
        }
        self.build_chain(extent)
    }

    /// Record the frame's passes, submit once, and present.
    ///
    /// Every item's pipeline must come from this target's device and
    /// target [`format`](Self::format) — contract violations, checked
    /// in dev builds.
    ///
    /// # Panics
    ///
    /// The frame-shape contract is asserted before any fence is waited
    /// or written — among its refusals: a frame needs at least one
    /// pass, and at least one surface pass; a surface pass carries
    /// exactly one color attachment, an image pass none (its one
    /// attachment rides its target); `LoadOp::Load` is refused on each
    /// identity's first use in the frame, and on a render image whose
    /// last targeting pass discarded; clear values must match their
    /// attachment's kind and a depth clear its documented range; an
    /// item's pipeline depth state must match its pass, its format an
    /// image pass's kind, and a depth-only pipeline draws only into
    /// depth images; one buffer carries one write per frame, whichever
    /// channel wrote it (pointer-identical data may repeat across items;
    /// differing data is refused); an item names geometry exactly when its
    /// pipeline declares per-vertex input, and a mesh's vertex stride
    /// equals the stride that pipeline's layout packs to; an item names
    /// bindings exactly when its pipeline declares any descriptor slot,
    /// exactly as many as it declares, and of the classes it declares —
    /// sampled slots first, a uniform block last; an item carries uniform
    /// data exactly when its pipeline declares a block and exactly that
    /// many bytes, and the buffer its block binding reads holds exactly
    /// that many per frame; the per-image walk is one-way — a frame
    /// writes an image before reading it, never in the same pass, never
    /// re-targeting after a read, storing whatever a later pass loads
    /// or samples — over at most
    /// [`MAX_FRAME_RENDER_IMAGES`](crate::MAX_FRAME_RENDER_IMAGES)
    /// distinct images; and a frame carries at most the retention
    /// table's width of distinct resources — per-frame buffers, meshes,
    /// bindings and pass-target images together, the repeatable classes
    /// counting once however many mentions. Frame data longer than its
    /// buffer's per-frame capacity also panics through a retained
    /// assertion: the length bounds a copy into mapped device memory,
    /// which makes it a memory-safety boundary rather than a contract
    /// nicety.
    ///
    /// # Errors
    ///
    /// [`TargetError::DepthUnsupported`] when a pass carries depth and
    /// the adapter refused the whole format chain — returned before any
    /// frame work begins, so the target is untouched.
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
        if self.shared.lost.poisoned() {
            return Err(TargetError::DeviceLost);
        }
        // Before any fence is waited or written: a dormant target's ring
        // state is deliberately frozen mid-flight, and only `resize` may
        // thaw it.
        if self.dormant {
            return Ok(PresentOutcome::NeedsResize);
        }
        pass::check_frame_contract(desc);
        let wants_depth = desc.passes.iter().any(|pass| pass.depth.is_some());
        if wants_depth && self.shared.depth_format.is_none() {
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
                    !surface_pass || item.pipeline.format == self.format,
                    "pipeline targets {:?}, swapchain is {:?}",
                    item.pipeline.format,
                    self.format
                );
            }
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
                // The wait succeeded, so slot `frame`'s previous work has
                // provably ended: the memory it read may die.
                for slot in &mut self.retained[frame] {
                    *slot = None;
                }
            }

            let Some(chain) = self.chain.as_ref() else {
                return Ok(PresentOutcome::NeedsResize);
            };
            // Per-frame bytes land here and not one line earlier: after
            // the fence wait AND the chain check. A dormant target's
            // flags can claim nothing is outstanding after a failed
            // quiesce, and the chain check is what stands between that
            // state and a copy into memory the GPU may still read.
            //
            // The copy phase opens by releasing the slot's table — a
            // release site beside the three proven ones, and its proof
            // is the field invariant, not a wait: on the path where the
            // fence WAS waited this call, the wait's own release above
            // already emptied the row and this is a no-op; a live entry
            // here is a stale one left by the acquire-failure early
            // returns below, where no submit was ever made, and
            // `pending[frame] == false` means fence `frame` is
            // unsignaled with nothing outstanding — the same footing as
            // the overwrite this table's predecessor performed from this
            // position. Clearing first also scopes the table to exactly
            // this frame's fills.
            for slot in &mut self.retained[frame] {
                *slot = None;
            }
            let mut retained_count = 0usize;
            // One per render, beside the retention counter and for the
            // same reason: a resource named by several items is handled
            // once, not once per mention.
            let mut blocks_written = crate::vk::buffer::BlockWrites::new();
            for pass in desc.passes {
                // A pass-target image is retained by the pass itself,
                // the offscreen loop's reasoning on the asynchronous
                // path — where a skipped hold is memory freed under a
                // live submit.
                if let pass::PassTarget::Image(image, _) = &pass.target {
                    debug_assert!(
                        Rc::ptr_eq(&image.inner.shared, &self.shared),
                        "render image and target come from different devices"
                    );
                    let resource = Retained::Image(Rc::clone(&image.inner));
                    if !pass::already_retained(&resource, &self.retained[frame][..retained_count]) {
                        self.retained[frame][retained_count] = Some(resource);
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
                    // Retention is enumerated by one shared function with
                    // a total match over the item's shape, so a resource
                    // class added to `Item` cannot be skipped here
                    // silently — which on this path would free memory
                    // under a live submit. A mesh or binding named by
                    // several items is retained once; the frame contract
                    // bounded the distinct count, and the recognition
                    // rule is one shared definition.
                    for resource in pass::retained_of(item).into_iter().flatten() {
                        if pass::already_retained(
                            &resource,
                            &self.retained[frame][..retained_count],
                        ) {
                            continue;
                        }
                        self.retained[frame][retained_count] = Some(resource);
                        retained_count += 1;
                    }
                    // The uniform block's bytes, into this frame's
                    // region of whichever buffer the item's uniform
                    // binding reads.
                    //
                    // SAFETY: the wait above proved slot `frame`'s
                    // previous work ended, and the retention loop above
                    // recorded every binding this item names — which is
                    // what holds the block's buffer alive past the
                    // submit.
                    // **Not an ash dispatch, unlike everything else the
                    // blanket above covers** — a raw write into mapped
                    // memory, with preconditions of its own. Its offscreen
                    // twin sits in a block of its own; this one cannot,
                    // because a nested `unsafe` inside the blanket is a
                    // lint error, so the comment carries the whole weight
                    // here.
                    //
                    // SAFETY: the wait above proved slot `frame`'s
                    // previous work ended; `frame < FRAMES_IN_FLIGHT`, and
                    // a build-time assert holds that inside the ring's
                    // depth; and the retention loop above recorded every
                    // binding this item names, which is what holds the
                    // block's buffer alive past the submit.
                    crate::vk::buffer::write_uniform_blocks(
                        item,
                        &self.shared,
                        std::ptr::from_ref::<Self>(self) as usize,
                        frame,
                        &mut blocks_written,
                    );
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
                    // below, which makes it a memory-safety boundary,
                    // not a contract nicety.
                    assert!(
                        data.bytes.len() <= inner.capacity,
                        "frame data exceeds the buffer's per-frame capacity"
                    );
                    // SAFETY: the mapping covers `slot_stride *
                    // MAX_FRAME_SLOTS` bytes; `frame < MAX_FRAME_SLOTS`
                    // and the assert above bounds the length within one
                    // slot region, so the write stays inside the
                    // allocation and cannot touch a neighbouring slot.
                    // The wait above proved no submit reads this region;
                    // the memory is HOST_COHERENT, so no flush. The
                    // stride is 64-aligned and doubled, far inside usize
                    // on every supported target; the cast is a lint
                    // formality, not a risk.
                    #[allow(clippy::cast_possible_truncation)]
                    let slot_byte_offset = (inner.slot_stride * frame as u64) as usize;
                    std::ptr::copy_nonoverlapping(
                        data.bytes.as_ptr(),
                        inner.mapped.add(slot_byte_offset),
                        data.bytes.len(),
                    );
                    // Retention for this buffer was recorded above,
                    // before the copy: the submit this frame records will
                    // read the region until its fence retires, and the
                    // memory must outlive any caller drop until then.
                }
            }
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

            // UNDEFINED → COLOR_ATTACHMENT_OPTIMAL. Deliberately NOT
            // the pure core's first-use masks: the source stage is
            // COLOR_ATTACHMENT_OUTPUT — the same stage the acquire
            // semaphore wait is scoped to — which chains this barrier
            // after the semaphore and orders the layout transition
            // against the presentation engine's outstanding reads of
            // this image (the classic write-after-present hazard). The
            // literal is pinned by unit test beside the core.
            let masks = transition::acquire_chained_color_first_use();
            let to_color = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(masks.src_stage)
                .src_access_mask(masks.src_access)
                .dst_stage_mask(masks.dst_stage)
                .dst_access_mask(masks.dst_access)
                .old_layout(masks.old_layout)
                .new_layout(masks.new_layout)
                .image(image)
                .subresource_range(color_range());
            let barriers = [to_color];
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&barriers),
            );

            // The walk: for each pass, its boundary barriers from the
            // shared frame walk (the same state machine the contract
            // proved), then its attachments, then its items — every one
            // following the pass's target. One deliberate exception:
            // the surface's FIRST use was already transitioned by the
            // acquire-chained barrier above, whose source stage exists
            // for semaphore chaining — so the walk's surface first-use
            // pair is recognised and skipped, never re-emitted.
            let mut walk = pass::FrameWalk::new();
            for (index, pass) in desc.passes.iter().enumerate() {
                let uses = walk.advance_target(index, pass);
                let mut barriers = [vk::ImageMemoryBarrier2::default(); pass::MAX_PASS_BARRIERS];
                let mut barrier_count = 0usize;
                if let Some((from, to)) = uses.color {
                    // The acquire-chained exception: exactly the
                    // surface's first use, nothing else.
                    let acquire_chained =
                        matches!(from, transition::ImageUse::ColorAttachmentFirstUse);
                    if !acquire_chained {
                        let masks = transition::pass_boundary(from, to);
                        let barrier_image = match &pass.target {
                            pass::PassTarget::Surface => image,
                            pass::PassTarget::Image(target_image, _) => target_image.inner.image,
                        };
                        barriers[barrier_count] = vk::ImageMemoryBarrier2::default()
                            .src_stage_mask(masks.src_stage)
                            .src_access_mask(masks.src_access)
                            .dst_stage_mask(masks.dst_stage)
                            .dst_access_mask(masks.dst_access)
                            .old_layout(masks.old_layout)
                            .new_layout(masks.new_layout)
                            .image(barrier_image)
                            .subresource_range(color_range());
                        barrier_count += 1;
                    }
                }
                if let Some((from, to)) = uses.depth {
                    let masks = transition::pass_boundary(from, to);
                    // Total for surface passes by the depth-availability
                    // check before recording began; an image pass
                    // carries its own depth image in its target.
                    let (barrier_image, format) = match &pass.target {
                        pass::PassTarget::Surface => {
                            let depth_slot = &chain.depth[frame];
                            (depth_slot.image, depth_slot.format)
                        }
                        pass::PassTarget::Image(target_image, _) => {
                            (target_image.inner.image, target_image.inner.format)
                        }
                    };
                    barriers[barrier_count] = vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(masks.src_stage)
                        .src_access_mask(masks.src_access)
                        .dst_stage_mask(masks.dst_stage)
                        .dst_access_mask(masks.dst_access)
                        .old_layout(masks.old_layout)
                        .new_layout(masks.new_layout)
                        .image(barrier_image)
                        .subresource_range(depth::barrier_range(format));
                    barrier_count += 1;
                }
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
                if barrier_count > 0 {
                    device.cmd_pipeline_barrier2(
                        cmd,
                        &vk::DependencyInfo::default()
                            .image_memory_barriers(&barriers[..barrier_count]),
                    );
                }

                // Attachments and geometry follow the target, the
                // offscreen loop's shape with this chain's own images.
                let mut color_attachments: [vk::RenderingAttachmentInfo<'_>; 1] =
                    [vk::RenderingAttachmentInfo::default()];
                let mut color_attachment_count = 0usize;
                let mut depth_attachment = None;
                let extent = match &pass.target {
                    pass::PassTarget::Surface => {
                        let color = &pass.color[0];
                        color_attachments[0] = vk::RenderingAttachmentInfo::default()
                            .image_view(view)
                            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                            .load_op(color.load.to_vk())
                            .store_op(color.store.to_vk())
                            .clear_value(pass::vk_clear_color(color));
                        color_attachment_count = 1;
                        depth_attachment = pass.depth.as_ref().map(|attachment| {
                            let depth_slot = &chain.depth[frame];
                            vk::RenderingAttachmentInfo::default()
                                .image_view(depth_slot.view)
                                .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                                .load_op(attachment.load.to_vk())
                                .store_op(attachment.store.to_vk())
                                .clear_value(pass::vk_clear_depth(attachment))
                        });
                        chain_extent
                    }
                    pass::PassTarget::Image(target_image, attachment) => {
                        if target_image.kind() == crate::RenderImageKind::Depth {
                            depth_attachment = Some(
                                vk::RenderingAttachmentInfo::default()
                                    .image_view(target_image.inner.view)
                                    .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                                    .load_op(attachment.load.to_vk())
                                    .store_op(attachment.store.to_vk())
                                    .clear_value(pass::vk_clear_depth(attachment)),
                            );
                        } else {
                            color_attachments[0] = vk::RenderingAttachmentInfo::default()
                                .image_view(target_image.inner.view)
                                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                                .load_op(attachment.load.to_vk())
                                .store_op(attachment.store.to_vk())
                                .clear_value(pass::vk_clear_color(attachment));
                            color_attachment_count = 1;
                        }
                        target_image.extent()
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
                device.cmd_begin_rendering(cmd, &rendering_info);
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
                    device.cmd_set_viewport(cmd, 0, &[viewport]);
                    device.cmd_set_scissor(cmd, 0, &[area]);
                }
                for item in pass.items {
                    device.cmd_bind_pipeline(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        item.pipeline.pipeline,
                    );
                    if let Some(bindings) = &item.bindings {
                        item.pipeline.bind_bindings(cmd, bindings, frame);
                    }
                    if let Some(bytes) = item.push_data {
                        // The contract proved presence and length match
                        // the pipeline's declared range; the bytes are
                        // copied into the command stream here, so no
                        // retention slot is spent.
                        item.pipeline.push_frame_constants(cmd, bytes);
                    }
                    if let Some(mesh) = item.mesh {
                        // No slot arithmetic: a mesh was written once at
                        // creation and has one region, so this bind is
                        // identical on both targets. The asymmetry below
                        // is the per-frame ring's alone.
                        device.cmd_bind_vertex_buffers(
                            cmd,
                            VERTEX_BINDING,
                            &[mesh.inner.buffer],
                            &[0],
                        );
                        device.cmd_bind_index_buffer(
                            cmd,
                            mesh.inner.buffer,
                            mesh.inner.index_offset,
                            vk::IndexType::UINT32,
                        );
                    }
                    let instances = match &item.frame_data {
                        Some(data) => {
                            // A plain record-time offset — sound only
                            // because this buffer is re-recorded every
                            // frame, so each recording bakes in its own
                            // slot's region.
                            device.cmd_bind_vertex_buffers(
                                cmd,
                                INSTANCE_BINDING,
                                &[data.buffer.inner.buffer],
                                &[data.buffer.inner.slot_stride * frame as u64],
                            );
                            data.instances
                        }
                        None => 1,
                    };
                    // The count comes from whichever half owns it: the
                    // geometry for a mesh draw, the shader for a stage
                    // that writes its own vertex list.
                    if let Some(mesh) = item.mesh {
                        device.cmd_draw_indexed(cmd, mesh.inner.index_count, instances, 0, 0, 0);
                    } else {
                        device.cmd_draw(cmd, item.pipeline.vertex_count, instances, 0, 0);
                    }
                }
                device.cmd_end_rendering(cmd);
            }

            // COLOR_ATTACHMENT_OPTIMAL → PRESENT_SRC; not a pass — the
            // terminal literal, pinned by unit test beside the core it
            // is excluded from. The signal semaphore orders
            // presentation, so no destination stage.
            let masks = transition::terminal_present();
            let to_present = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(masks.src_stage)
                .src_access_mask(masks.src_access)
                .dst_stage_mask(masks.dst_stage)
                .dst_access_mask(masks.dst_access)
                .old_layout(masks.old_layout)
                .new_layout(masks.new_layout)
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
        let idle = unsafe { self.shared.device.device_wait_idle() };
        // Best effort about *recovering*, not about *reporting*. Every
        // other quiesce in this crate records a lost device on the
        // shared spine, and this one discarded the result entirely — so
        // a device lost while tearing a frame down left the flag clear
        // and nothing afterwards reported it.
        //
        // Handed the outcome unconditionally rather than through a
        // branch: `note_result` ignores every code but the lost-device
        // one, so this records exactly what matters and stays a line
        // that always runs.
        self.shared
            .note_result(idle.err().unwrap_or(vk::Result::SUCCESS));
        // Work has provably ended if the quiesce succeeded OR the device
        // is lost — a lost device terminates execution as finally as an
        // idle one. A failed non-lost quiesce proves nothing, and
        // retained buffer memory then survives until the next successful
        // wait-idle (`resize`'s, which rebirth already requires) or this
        // target's own teardown.
        let quiesced = match idle {
            Ok(()) => true,
            Err(code) => code == vk::Result::ERROR_DEVICE_LOST,
        };
        if quiesced {
            self.retire_fence_after_idle();
            self.destroy_chain();
        } else {
            // No proof the GPU stopped, so nothing it may reference is
            // touched: resetting a pending fence or destroying the
            // chain's semaphores here is exactly the spec violation the
            // first test to drive this corner caught. Park everything.
            self.dormant = true;
        }
        error
    }

    /// After a successful (or best-effort) wait-idle, a pending fence
    /// is signaled: reset it so the unsignaled-when-not-pending
    /// invariant holds.
    fn retire_fence_after_idle(&mut self) {
        // Called only after PROOF that work ended — a successful
        // wait-idle or a lost device. An unproven quiesce parks the
        // target dormant instead, touching nothing: fences stay
        // unreset, flags stay pending, retained memory stays alive.
        // One rule, one place; the failed-quiesce corner is handled by
        // never arriving here.
        for row in &mut self.retained {
            for slot in row {
                *slot = None;
            }
        }
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
    /// therefore writing memory the GPU may be reading. **Per-frame data
    /// a draw reads does not go through these accessors at all**: it
    /// rides [`RenderDesc`](crate::RenderDesc) as
    /// [`FrameData`](crate::FrameData), and the target copies it into
    /// the right slot region *after* that slot's fence wait — the one
    /// point where the region is provably not being read. There is no
    /// caller-side write to time correctly, which is the entire design.
    ///
    /// These accessors remain correct for anything that does not race a
    /// submit — choosing which of N caller-owned resources to *create*
    /// or label, or reading back after a frame the caller has otherwise
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
            transform: from_vk_transform(caps.current_transform),
            views: Vec::new(),
            images: Vec::new(),
            image_available: [vk::Semaphore::null(); FRAMES_IN_FLIGHT],
            render_finished: Vec::new(),
            depth: Vec::new(),
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
        // One depth image per frame slot, when the adapter offers a
        // format; a partial vector on failure is torn down by
        // `destroy_chain` like every other chain resource.
        if let Some(format) = self.shared.depth_format {
            for _ in 0..FRAMES_IN_FLIGHT {
                chain
                    .depth
                    .push(DepthResources::create(&self.shared, chain.extent, format)?);
            }
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
        for depth in &chain.depth {
            depth.destroy(&self.shared);
        }
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
    let chosen = [vk::Format::B8G8R8A8_SRGB, vk::Format::R8G8B8A8_SRGB]
        .into_iter()
        .find_map(|want| {
            formats
                .iter()
                .find(|f| f.format == want && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        })?;
    let format = match chosen.format {
        vk::Format::R8G8B8A8_SRGB => TargetFormat::Rgba8Srgb,
        _ => TargetFormat::Bgra8Srgb,
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
            // Best-effort quiesce; failure is logged, never a panic
            // — the diag record is the only observable this path has.
            if let Err(code) = self.shared.device.device_wait_idle() {
                renew_diag::error!(
                    target: "renew-rhi",
                    "wait-idle at window-target teardown failed: {code:?}"
                );
            }
        }
        // Teardown releases retained memory unconditionally: this target
        // and its ring die here, and holding memory past the owner that
        // guaranteed its fences would be a leak, not a safety net. The
        // best-effort wait above is the same one every cold teardown
        // path in this crate already accepts.
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
    /// Every transform a surface can report, mapped.
    ///
    /// The four rotations are the ones a handheld panel produces and
    /// the only ones a renderer can fold; everything else — the
    /// mirrored variants a display wall might report, and the
    /// `INHERIT` a driver uses to say "ask the platform" — reads as
    /// the identity, because telling a caller a fold happened when it
    /// cannot fold is worse than telling it nothing. Two of the four
    /// rotations are unreachable in every lane here, which is exactly
    /// why they are pinned by construction rather than by a surface.
    #[test]
    fn every_reported_transform_maps_to_what_a_renderer_can_fold() {
        use super::from_vk_transform;
        use crate::config::SurfaceTransform;
        use ash::vk::SurfaceTransformFlagsKHR as Vk;

        for (reported, expected) in [
            (Vk::IDENTITY, SurfaceTransform::Identity),
            (Vk::ROTATE_90, SurfaceTransform::Rotate90),
            (Vk::ROTATE_180, SurfaceTransform::Rotate180),
            (Vk::ROTATE_270, SurfaceTransform::Rotate270),
        ] {
            assert_eq!(from_vk_transform(reported), expected, "{reported:?}");
        }

        for unfoldable in [
            Vk::HORIZONTAL_MIRROR,
            Vk::HORIZONTAL_MIRROR_ROTATE_90,
            Vk::HORIZONTAL_MIRROR_ROTATE_180,
            Vk::HORIZONTAL_MIRROR_ROTATE_270,
            Vk::INHERIT,
        ] {
            assert_eq!(
                from_vk_transform(unfoldable),
                SurfaceTransform::Identity,
                "{unfoldable:?} is not foldable and must not be reported as a rotation"
            );
        }
    }

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
            offered(vk::Format::R8G8B8A8_SRGB, SRGB),
            offered(vk::Format::B8G8R8A8_SRGB, SRGB),
        ];
        let (chosen, format) = choose_surface_format(&formats).expect("both are acceptable");
        assert_eq!(chosen.format, vk::Format::B8G8R8A8_SRGB);
        assert_eq!(format, TargetFormat::Bgra8Srgb);
    }

    #[test]
    fn rgba_is_taken_when_it_is_the_only_one_offered() {
        // Reachable on real hardware, not on every machine: proving it
        // through a driver would only prove what this machine reports.
        let formats = [
            offered(vk::Format::B8G8R8A8_UNORM, SRGB),
            offered(vk::Format::R8G8B8A8_SRGB, SRGB),
        ];
        let (chosen, format) = choose_surface_format(&formats).expect("rgba is acceptable");
        assert_eq!(chosen.format, vk::Format::R8G8B8A8_SRGB);
        assert_eq!(format, TargetFormat::Rgba8Srgb);
    }

    /// A surface offering only formats that store what was written is
    /// refused rather than accepted and drawn into wrongly.
    ///
    /// **The refusal is the whole fallback story, and it is deliberate.**
    /// The alternative — take the UNORM surface and apply the transfer
    /// function in the shader — needs a second variant of every shader
    /// that reaches a surface, selected at pipeline creation. That is a
    /// permutation mechanism, and building a bespoke one here would mean
    /// writing a path that no machine available to test it can reach:
    /// the only hardware that takes the fallback is hardware without an
    /// sRGB surface, and any machine that can run the check has one.
    ///
    /// An unexercised path is the failure this repository has paid for
    /// most often, so this refuses in terms until the permutation
    /// compiler exists to make the variant cheap and testable.
    #[test]
    fn a_surface_offering_no_srgb_format_is_refused() {
        let formats = [
            offered(vk::Format::B8G8R8A8_UNORM, SRGB),
            offered(vk::Format::R8G8B8A8_UNORM, SRGB),
        ];
        assert!(choose_surface_format(&formats).is_none());
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
