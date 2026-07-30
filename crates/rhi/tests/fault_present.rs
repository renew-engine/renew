//! Fault-injection scenarios for the presentation path: every rung of
//! the window target's bring-up ladder, then acquire, submit, and
//! present failures, each driven through a real window.
//!
//! `harness = false` because the OS event loop must own the main
//! thread. Scenarios run one per redraw so the window is fully mapped
//! before any of them touches a surface; each builds its own device and
//! target, because the fault layer arms at instance creation and fires
//! exactly once.
//!
//! Two scenarios arm nothing at all: their trigger is an argument (a
//! zero extent, a window the WSI cannot make a surface from), not a
//! driver failure.
//!
//! Skips (exit 0, SKIP line) without a display, a Vulkan runtime,
//! presentation support, or the fault layer; under
//! `RENEW_FAULT_STRICT=1` every one of those becomes a failure.

// Arming a fault is an environment write, `unsafe` since the 2024
// edition. Sound here: this binary is single-threaded (the event loop
// owns the main thread and every scenario runs on it), so nothing reads
// or writes the environment concurrently.
#![allow(unsafe_code)]

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
    WebWindowHandle, WindowHandle,
};
use renew_platform::window::{
    LoopControl, NativeWindow, WindowApp, WindowConfig, WindowError, WindowEvent, WindowRef,
    run_window_app,
};
use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, PresentOutcome, TargetError, Validation,
    WindowTarget,
};

const CLEAR: Color = Color::new(0.1, 0.2, 0.3, 1.0);
/// The dormant size: a minimized window's extent.
const ZERO: Extent = Extent {
    width: 0,
    height: 0,
};
/// Poll-loop iterations before declaring the run wedged.
const UPDATE_BUDGET: u32 = 20_000;

type Verdict = Result<(), String>;

fn strict() -> bool {
    std::env::var_os("RENEW_FAULT_STRICT").is_some_and(|value| value == "1")
}

/// Arm `spec` for the duration of `run`, then disarm.
fn with_fault<T>(spec: &str, run: impl FnOnce() -> T) -> T {
    // SAFETY: single-threaded binary; see the module note.
    unsafe { std::env::set_var("RENEW_FAULT", spec) };
    let outcome = run();
    // SAFETY: as above.
    unsafe { std::env::remove_var("RENEW_FAULT") };
    outcome
}

/// Run with nothing armed: the layer re-reads the environment at every
/// instance creation, so an unset variable arms no fault at all.
fn without_fault<T>(run: impl FnOnce() -> T) -> T {
    // SAFETY: single-threaded binary; see the module note.
    unsafe { std::env::remove_var("RENEW_FAULT") };
    run()
}

/// Implicit layers (overlays, vendor shims) sit above this suite and
/// some of them RETRY calls that fail — an injected fault would then be
/// papered over by a layer nobody asked for, and the scenario would
/// silently measure the overlay instead of the backend. They also issue
/// calls of their own, which would shift the ordinals the scenarios
/// below pick out. Observed on a desktop with a vendor present layer
/// and a capture hook installed. The loader reads this when an instance
/// is created, so setting it before the first device is enough.
fn silence_implicit_layers() {
    // SAFETY: single-threaded; see the module note.
    unsafe { std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~") };
}

fn new_device() -> Result<Device, DeviceError> {
    Device::new(&DeviceDesc {
        app_name: "renew-rhi-fault-present",
        validation: Validation::IfAvailable,
    })
}

fn wrong<T: std::fmt::Debug>(expected: &str, got: &T) -> String {
    format!("expected {expected}, got {got:?}")
}

/// The error a scenario's failing call must surface, and the call it
/// must name.
#[derive(Clone, Copy)]
enum Expect {
    Creation(&'static str),
    Timeout(&'static str),
    OutOfDeviceMemory(&'static str),
}

impl Expect {
    fn describe(self) -> String {
        match self {
            Self::Creation(call) => format!("Creation({call})"),
            Self::Timeout(call) => format!("Timeout({call})"),
            Self::OutOfDeviceMemory(call) => format!("OutOfDeviceMemory({call})"),
        }
    }

    /// `Ok(())` when `got` is exactly this variant naming this call.
    fn matched(self, got: &TargetError) -> Verdict {
        let held = match (self, got) {
            (Self::Creation(call), TargetError::Creation { call: got, .. })
            | (Self::Timeout(call), TargetError::Timeout { call: got })
            | (Self::OutOfDeviceMemory(call), TargetError::OutOfDeviceMemory { call: got }) => {
                *got == call
            }
            _ => false,
        };
        if held {
            Ok(())
        } else {
            Err(wrong(&self.describe(), got))
        }
    }
}

/// The protocol a scenario walks — one variant per distinct
/// failure-and-recovery contract the presentation path promises.
#[derive(Clone, Copy)]
enum Shape {
    /// The target must fail to build; a fresh build then succeeds and
    /// presents, proving the unwinder left device and window usable.
    BuildFails(Expect),
    /// The frame fails but queues no semaphore signal, so the chain
    /// survives and the next frame presents.
    FrameFailsChainSurvives(Expect),
    /// The frame fails with a signal outstanding, so the target tears
    /// the chain down and goes dormant; a resize rebuilds it.
    FrameFailsGoesDormant(Expect),
    /// The first frame presents and the second fails on the frame
    /// fence; a resize quiesces, retires the fence, and presents again.
    SecondFrameFails(Expect),
    /// Not an error at all: the swapchain is stale, so the frame asks
    /// to be resized.
    StaleSwapchain,
    /// Frame `n` reports device loss; the poison then sticks to every
    /// later operation on the device.
    DeviceLostOnFrame(u32),
    /// A resize fails without losing the device; a second resize
    /// recovers.
    ResizeFails(Expect),
    /// A resize loses the device; the poison sticks.
    ResizeLosesDevice,
}

/// Every faulted scenario: the name printed, the fault armed, and the
/// protocol the target must then honor.
///
/// Ordinals (`@n`) count invocations, both halves of a two-call idiom
/// included; `Q12` picks the first per-image semaphore, the acquire
/// semaphore being the chain's first.
const LADDER: &[(&str, &str, Shape)] = &[
    // ---- bring-up: create_window_target, rung by rung --------------
    (
        "Q1 surface-support",
        "vkGetPhysicalDeviceSurfaceSupportKHR=ERROR_OUT_OF_HOST_MEMORY",
        Shape::BuildFails(Expect::Creation("vkGetPhysicalDeviceSurfaceSupportKHR")),
    ),
    (
        "Q2 surface-formats",
        "vkGetPhysicalDeviceSurfaceFormatsKHR=ERROR_OUT_OF_HOST_MEMORY",
        Shape::BuildFails(Expect::Creation("vkGetPhysicalDeviceSurfaceFormatsKHR")),
    ),
    (
        "Q3 command-pool",
        "vkCreateCommandPool=ERROR_OUT_OF_HOST_MEMORY",
        Shape::BuildFails(Expect::Creation("vkCreateCommandPool")),
    ),
    (
        "Q4 command-buffers",
        "vkAllocateCommandBuffers=ERROR_OUT_OF_HOST_MEMORY",
        Shape::BuildFails(Expect::Creation("vkAllocateCommandBuffers")),
    ),
    (
        "Q5 fence",
        "vkCreateFence=ERROR_OUT_OF_HOST_MEMORY",
        Shape::BuildFails(Expect::Creation("vkCreateFence")),
    ),
    (
        "Q6 surface-capabilities",
        "vkGetPhysicalDeviceSurfaceCapabilitiesKHR=ERROR_OUT_OF_HOST_MEMORY",
        Shape::BuildFails(Expect::Creation(
            "vkGetPhysicalDeviceSurfaceCapabilitiesKHR",
        )),
    ),
    (
        "Q7 swapchain",
        "vkCreateSwapchainKHR=ERROR_OUT_OF_HOST_MEMORY",
        Shape::BuildFails(Expect::Creation("vkCreateSwapchainKHR")),
    ),
    (
        "Q8 swapchain/device-memory",
        "vkCreateSwapchainKHR=ERROR_OUT_OF_DEVICE_MEMORY",
        Shape::BuildFails(Expect::OutOfDeviceMemory("vkCreateSwapchainKHR")),
    ),
    (
        "Q9 acquire-semaphore",
        "vkCreateSemaphore=ERROR_OUT_OF_HOST_MEMORY",
        Shape::BuildFails(Expect::Creation("vkCreateSemaphore")),
    ),
    (
        "Q10 swapchain-images",
        "vkGetSwapchainImagesKHR=ERROR_OUT_OF_HOST_MEMORY",
        Shape::BuildFails(Expect::Creation("vkGetSwapchainImagesKHR")),
    ),
    (
        "Q11 image-view",
        "vkCreateImageView=ERROR_OUT_OF_HOST_MEMORY",
        Shape::BuildFails(Expect::Creation("vkCreateImageView")),
    ),
    (
        "Q12 per-image-semaphore",
        "vkCreateSemaphore=ERROR_OUT_OF_HOST_MEMORY@2",
        Shape::BuildFails(Expect::Creation("vkCreateSemaphore")),
    ),
    // ---- the frame: failures the chain survives --------------------
    (
        "P1 acquire-timeout",
        "vkAcquireNextImageKHR=TIMEOUT",
        Shape::FrameFailsChainSurvives(Expect::Timeout("vkAcquireNextImageKHR")),
    ),
    (
        "P2 acquire-surface-lost",
        "vkAcquireNextImageKHR=ERROR_SURFACE_LOST_KHR",
        Shape::FrameFailsChainSurvives(Expect::Creation("vkAcquireNextImageKHR")),
    ),
    // ---- the frame: failures that tear the chain down --------------
    (
        "P3 reset-command-buffer",
        "vkResetCommandBuffer=ERROR_OUT_OF_HOST_MEMORY",
        Shape::FrameFailsGoesDormant(Expect::Creation("vkResetCommandBuffer")),
    ),
    (
        "P4 begin-command-buffer",
        "vkBeginCommandBuffer=ERROR_OUT_OF_HOST_MEMORY",
        Shape::FrameFailsGoesDormant(Expect::Creation("vkBeginCommandBuffer")),
    ),
    (
        "P5 end-command-buffer",
        "vkEndCommandBuffer=ERROR_OUT_OF_HOST_MEMORY",
        Shape::FrameFailsGoesDormant(Expect::Creation("vkEndCommandBuffer")),
    ),
    (
        "P6 submit",
        "vkQueueSubmit2=ERROR_OUT_OF_HOST_MEMORY",
        Shape::FrameFailsGoesDormant(Expect::Creation("vkQueueSubmit2")),
    ),
    (
        "P7 present-surface-lost",
        "vkQueuePresentKHR=ERROR_SURFACE_LOST_KHR",
        Shape::FrameFailsGoesDormant(Expect::Creation("vkQueuePresentKHR")),
    ),
    // ---- the frame fence: only waited on from frame two on ---------
    (
        "P8 fence-timeout",
        "vkWaitForFences=TIMEOUT",
        Shape::SecondFrameFails(Expect::Timeout("vkWaitForFences")),
    ),
    (
        "P9 fence-failure",
        "vkWaitForFences=ERROR_OUT_OF_HOST_MEMORY",
        Shape::SecondFrameFails(Expect::Creation("vkWaitForFences")),
    ),
    (
        "P10 reset-fences",
        "vkResetFences=ERROR_OUT_OF_HOST_MEMORY",
        Shape::SecondFrameFails(Expect::Creation("vkResetFences")),
    ),
    // ---- stale swapchain: protocol outcomes, not errors ------------
    (
        "P11 acquire-out-of-date",
        "vkAcquireNextImageKHR=ERROR_OUT_OF_DATE_KHR",
        Shape::StaleSwapchain,
    ),
    (
        "P12 present-out-of-date",
        "vkQueuePresentKHR=ERROR_OUT_OF_DATE_KHR",
        Shape::StaleSwapchain,
    ),
    // ---- device loss, from every call that can report it -----------
    (
        "L1 acquire/device-lost",
        "vkAcquireNextImageKHR=ERROR_DEVICE_LOST",
        Shape::DeviceLostOnFrame(1),
    ),
    (
        "L2 submit/device-lost",
        "vkQueueSubmit2=ERROR_DEVICE_LOST",
        Shape::DeviceLostOnFrame(1),
    ),
    (
        "L3 present/device-lost",
        "vkQueuePresentKHR=ERROR_DEVICE_LOST",
        Shape::DeviceLostOnFrame(1),
    ),
    (
        "L4 fence/device-lost",
        "vkWaitForFences=ERROR_DEVICE_LOST",
        Shape::DeviceLostOnFrame(2),
    ),
    // ---- resize: the only caller of wait-idle in this path ---------
    (
        "L5 resize/device-lost",
        "vkDeviceWaitIdle=ERROR_DEVICE_LOST",
        Shape::ResizeLosesDevice,
    ),
    (
        "R1 resize/wait-idle-failure",
        "vkDeviceWaitIdle=ERROR_OUT_OF_HOST_MEMORY",
        Shape::ResizeFails(Expect::Creation("vkDeviceWaitIdle")),
    ),
];

/// The two scenarios that arm nothing, run after the table.
const UNFAULTED_SCENARIOS: usize = 2;
/// How many scenarios there are in total.
const SCENARIOS: usize = LADDER.len() + UNFAULTED_SCENARIOS;

/// The target must be dormant: no size, and rendering asks for a
/// resize instead of presenting.
fn assert_dormant(target: &mut WindowTarget) -> Verdict {
    let extent = target.extent();
    if extent.width != 0 || extent.height != 0 {
        return Err(format!("expected a dormant target, got extent {extent:?}"));
    }
    match target.render(CLEAR, None) {
        Ok(PresentOutcome::NeedsResize) => Ok(()),
        other => Err(wrong("NeedsResize from a dormant target", &other)),
    }
}

/// Rebuild after dormancy and prove the target presents again.
fn assert_recovers(target: &mut WindowTarget, size: Extent) -> Verdict {
    target
        .resize(size)
        .map_err(|error| format!("recovery resize failed: {error}"))?;
    match target.render(CLEAR, None) {
        Ok(PresentOutcome::Presented) => Ok(()),
        other => Err(wrong("Presented after recovery", &other)),
    }
}

/// Once the device is lost the poison is total: the target, a resize, a
/// fresh target, and the device itself all fail fast and stay failed.
fn assert_poison_sticks(
    device: &Device,
    target: &mut WindowTarget,
    window: &NativeWindow,
    size: Extent,
) -> Verdict {
    match target.render(CLEAR, None) {
        Err(TargetError::DeviceLost) => {}
        other => return Err(wrong("DeviceLost from the next frame", &other)),
    }
    match target.resize(size) {
        Err(TargetError::DeviceLost) => {}
        other => return Err(wrong("DeviceLost from a resize", &other)),
    }
    match device.create_window_target(window.clone(), size) {
        Err(TargetError::DeviceLost) => {}
        Err(other) => return Err(wrong("DeviceLost from a fresh target", &other)),
        Ok(unexpected) => {
            return Err(format!(
                "a lost device still built a target (extent {:?})",
                unexpected.extent()
            ));
        }
    }
    match device.wait_idle() {
        Err(DeviceError::DeviceLost) => Ok(()),
        other => Err(wrong("DeviceLost from wait_idle", &other)),
    }
}

fn validation_clean(device: &Device) -> Verdict {
    let report = device.validation_report();
    if report.errors == 0 {
        Ok(())
    } else {
        Err(format!(
            "{} validation error(s); first: {:?}",
            report.errors, report.first_messages
        ))
    }
}

/// A device plus a target built for `extent`, handed to `body`;
/// validation must be clean whatever the body concludes.
fn target_case(
    extent: Extent,
    window: &NativeWindow,
    body: impl FnOnce(&Device, Result<WindowTarget, TargetError>) -> Verdict,
) -> Verdict {
    let device = new_device().map_err(|error| format!("device bring-up: {error}"))?;
    let target = device.create_window_target(window.clone(), extent);
    let outcome = body(&device, target);
    let clean = validation_clean(&device);
    outcome.and(clean)
}

/// Every faulted scenario: arm the fault, then run the case.
fn present_case(
    extent: Extent,
    window: &NativeWindow,
    fault: &str,
    body: impl FnOnce(&Device, Result<WindowTarget, TargetError>) -> Verdict,
) -> Verdict {
    with_fault(fault, || target_case(extent, window, body))
}

/// The target the body of a frame scenario needs, or a verdict blaming
/// the build that should have succeeded.
fn built(target: Result<WindowTarget, TargetError>) -> Result<WindowTarget, String> {
    target.map_err(|error| format!("target: {error}"))
}

fn walk(
    shape: Shape,
    device: &Device,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
    window: &NativeWindow,
) -> Verdict {
    match shape {
        Shape::BuildFails(expect) => build_fails(expect, device, target, size, window),
        Shape::FrameFailsChainSurvives(expect) => frame_fails_chain_survives(expect, target),
        Shape::FrameFailsGoesDormant(expect) => frame_fails_goes_dormant(expect, target, size),
        Shape::SecondFrameFails(expect) => second_frame_fails(expect, target, size),
        Shape::StaleSwapchain => stale_swapchain(target, size),
        Shape::DeviceLostOnFrame(frame) => {
            device_lost_on_frame(frame, device, target, size, window)
        }
        Shape::ResizeFails(expect) => resize_fails(expect, target, size),
        Shape::ResizeLosesDevice => resize_loses_device(device, target, size, window),
    }
}

fn build_fails(
    expect: Expect,
    device: &Device,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
    window: &NativeWindow,
) -> Verdict {
    match target {
        Err(error) => expect.matched(&error)?,
        Ok(unexpected) => {
            return Err(format!(
                "the target built despite the fault (extent {:?})",
                unexpected.extent()
            ));
        }
    }
    // The fault is spent: the unwinder left the device usable and the
    // window free of a half-built surface.
    let mut recovered = device
        .create_window_target(window.clone(), size)
        .map_err(|error| format!("recovery build failed: {error}"))?;
    match recovered.render(CLEAR, None) {
        Ok(PresentOutcome::Presented) => Ok(()),
        other => Err(wrong("Presented after recovery", &other)),
    }
}

fn frame_fails_chain_survives(
    expect: Expect,
    target: Result<WindowTarget, TargetError>,
) -> Verdict {
    let mut target = built(target)?;
    match target.render(CLEAR, None) {
        Err(error) => expect.matched(&error)?,
        Ok(outcome) => return Err(wrong("an error", &outcome)),
    }
    match target.render(CLEAR, None) {
        Ok(PresentOutcome::Presented) => Ok(()),
        other => Err(wrong("Presented on the next frame", &other)),
    }
}

fn frame_fails_goes_dormant(
    expect: Expect,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
) -> Verdict {
    let mut target = built(target)?;
    match target.render(CLEAR, None) {
        Err(error) => expect.matched(&error)?,
        Ok(outcome) => return Err(wrong("an error", &outcome)),
    }
    assert_dormant(&mut target)?;
    assert_recovers(&mut target, size)
}

fn second_frame_fails(
    expect: Expect,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
) -> Verdict {
    let mut target = built(target)?;
    match target.render(CLEAR, None) {
        Ok(PresentOutcome::Presented) => {}
        other => return Err(wrong("Presented on the first frame", &other)),
    }
    match target.render(CLEAR, None) {
        Err(error) => expect.matched(&error)?,
        Ok(outcome) => return Err(wrong("an error on the second frame", &outcome)),
    }
    assert_recovers(&mut target, size)
}

fn stale_swapchain(target: Result<WindowTarget, TargetError>, size: Extent) -> Verdict {
    let mut target = built(target)?;
    match target.render(CLEAR, None) {
        Ok(PresentOutcome::NeedsResize) => {}
        other => return Err(wrong("NeedsResize", &other)),
    }
    assert_recovers(&mut target, size)
}

fn device_lost_on_frame(
    frame: u32,
    device: &Device,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
    window: &NativeWindow,
) -> Verdict {
    let mut target = built(target)?;
    for earlier in 1..frame {
        match target.render(CLEAR, None) {
            Ok(PresentOutcome::Presented) => {}
            other => return Err(wrong(&format!("Presented on frame {earlier}"), &other)),
        }
    }
    match target.render(CLEAR, None) {
        Err(TargetError::DeviceLost) => {}
        other => return Err(wrong(&format!("DeviceLost on frame {frame}"), &other)),
    }
    assert_poison_sticks(device, &mut target, window, size)
}

fn resize_fails(
    expect: Expect,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
) -> Verdict {
    let mut target = built(target)?;
    match target.resize(size) {
        Err(error) => expect.matched(&error)?,
        Ok(()) => return Err("the resize succeeded despite the fault".to_string()),
    }
    // Not a device loss: the fault is spent and the target rebuilds.
    assert_recovers(&mut target, size)
}

fn resize_loses_device(
    device: &Device,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
    window: &NativeWindow,
) -> Verdict {
    let mut target = built(target)?;
    match target.resize(size) {
        Err(TargetError::DeviceLost) => {}
        other => return Err(wrong("DeviceLost from the resize", &other)),
    }
    assert_poison_sticks(device, &mut target, window, size)
}

/// A zero extent builds a target that is dormant from birth — the
/// minimized-window case — and a resize wakes it.
fn born_dormant(window: &NativeWindow, size: Extent) -> Verdict {
    without_fault(|| {
        target_case(ZERO, window, |_device, target| {
            let mut target =
                target.map_err(|error| format!("a zero-extent target failed to build: {error}"))?;
            assert_dormant(&mut target)?;
            assert_recovers(&mut target, size)
        })
    })
}

/// A window whose handles name a windowing system no desktop WSI can
/// build a surface from: an error, never a panic, and nothing left
/// behind.
fn unsupported_handles() -> Verdict {
    without_fault(|| {
        let device = new_device().map_err(|error| format!("device bring-up: {error}"))?;
        let outcome = match device.create_window_target(
            UnsupportedWindow,
            Extent {
                width: 16,
                height: 16,
            },
        ) {
            Err(TargetError::SurfaceCreation { .. }) => Ok(()),
            Err(other) => Err(wrong("SurfaceCreation", &other)),
            Ok(unexpected) => Err(format!(
                "a surface came out of handles no WSI supports (extent {:?})",
                unexpected.extent()
            )),
        };
        let clean = validation_clean(&device);
        outcome.and(clean)
    })
}

/// Web handles: structurally valid, and unsupported by every desktop
/// surface extension.
struct UnsupportedWindow;

impl HasDisplayHandle for UnsupportedWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::web())
    }
}

impl HasWindowHandle for UnsupportedWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: `borrow_raw`'s obligation covers pointers, and its
        // documentation excludes window ids explicitly — a web handle
        // is nothing but a `u32` id. Nothing dereferences it: the
        // surface code rejects the handle pair before any use.
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Web(WebWindowHandle::new(1))) })
    }
}

struct FaultApp {
    window: Option<NativeWindow>,
    size: Extent,
    next: usize,
    passed: usize,
    updates: u32,
    skip: Option<String>,
    failures: Vec<String>,
}

impl FaultApp {
    fn new() -> Self {
        Self {
            window: None,
            size: ZERO,
            next: 0,
            passed: 0,
            updates: 0,
            skip: None,
            failures: Vec::new(),
        }
    }

    fn done(&self) -> bool {
        self.skip.is_some() || self.next >= SCENARIOS
    }

    /// Is the fault layer actually in the chain? Uses an offscreen
    /// target so the answer does not depend on presentation.
    fn canary() -> Result<bool, String> {
        with_fault("vkCreateFence=ERROR_OUT_OF_HOST_MEMORY", || {
            let device = match new_device() {
                Ok(device) => device,
                Err(DeviceError::LoaderUnavailable { message }) => {
                    return Err(format!("no Vulkan runtime: {message}"));
                }
                Err(error) => return Err(format!("device bring-up failed: {error}")),
            };
            match device.create_offscreen_target(Extent {
                width: 8,
                height: 8,
            }) {
                Err(TargetError::Creation {
                    call: "vkCreateFence",
                    ..
                }) => Ok(true),
                _ => Ok(false),
            }
        })
    }

    /// Run scenario `index`; `Ok(())` means it held.
    fn run_scenario(&self, index: usize) -> (&'static str, Verdict) {
        let size = self.size;
        let Some(window) = self.window.as_ref() else {
            return ("no window", Err("the window vanished".to_string()));
        };
        if let Some(&(name, fault, shape)) = LADDER.get(index) {
            let verdict = present_case(size, window, fault, |device, target| {
                walk(shape, device, target, size, window)
            });
            return (name, verdict);
        }
        match index - LADDER.len() {
            0 => ("S1 born-dormant", born_dormant(window, size)),
            _ => ("S2 unsupported-window-handles", unsupported_handles()),
        }
    }
}

impl WindowApp for FaultApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        match Self::canary() {
            Ok(true) => {}
            Ok(false) => {
                self.skip = Some("fault injection not active".to_string());
                return;
            }
            Err(message) => {
                self.skip = Some(message);
                return;
            }
        }
        let (width, height) = window.physical_size();
        self.size = Extent { width, height };
        self.window = Some(window.native());
    }

    fn event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::Resized { width, height } => {
                // Scenarios build their own targets; just track the size
                // they should build for.
                if width > 0 && height > 0 {
                    self.size = Extent { width, height };
                }
            }
            WindowEvent::RedrawRequested => {
                if self.done() {
                    return;
                }
                let index = self.next;
                let (name, verdict) = self.run_scenario(index);
                self.next += 1;
                match verdict {
                    Ok(()) => {
                        self.passed += 1;
                        println!("ok: {name}");
                    }
                    Err(message) => {
                        println!("FAIL: {name}: {message}");
                        self.failures.push(format!("{name}: {message}"));
                    }
                }
            }
            _ => {}
        }
    }

    fn update(&mut self, control: &mut LoopControl) {
        self.updates += 1;
        if self.updates > UPDATE_BUDGET && !self.done() {
            self.failures
                .push(format!("wedged after {} updates", self.updates));
            self.next = SCENARIOS;
        }
        if self.done() {
            control.exit();
        } else {
            control.request_redraw();
        }
    }
}

fn main() {
    silence_implicit_layers();
    let mut app = FaultApp::new();
    let config = WindowConfig {
        title: "renew presentation faults".to_string(),
        logical_width: 640.0,
        logical_height: 360.0,
        resizable: true,
    };
    let run = run_window_app(&config, &mut app);

    if let Err(error) = run {
        match error {
            WindowError::LoopUnavailable { message } if !strict() => {
                println!("SKIP: window loop unavailable: {message}");
                return;
            }
            other => {
                eprintln!("FAIL: window loop: {other}");
                std::process::exit(1);
            }
        }
    }
    if let Some(message) = app.skip {
        if strict() {
            eprintln!("FAIL: RENEW_FAULT_STRICT=1 but scenarios could not run: {message}");
            std::process::exit(1);
        }
        println!("SKIP: {message}");
        return;
    }
    if !app.failures.is_empty() {
        eprintln!(
            "FAIL: {} presentation fault scenario(s):\n{}",
            app.failures.len(),
            app.failures.join("\n")
        );
        std::process::exit(1);
    }
    if app.passed != SCENARIOS {
        eprintln!("FAIL: {} of {SCENARIOS} scenarios ran", app.passed);
        std::process::exit(1);
    }
    println!("OK: {} presentation fault scenarios", app.passed);
}
