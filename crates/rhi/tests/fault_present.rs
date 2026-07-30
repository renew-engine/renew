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
//! A second family arms `RENEW_QUIRK` rather than `RENEW_FAULT`:
//! *response mutations*, where the driver succeeds while reporting
//! something this machine's hardware never reports — no swapchain
//! extension, a queue that cannot present, no usable surface format or
//! only an RGBA one, no swapchain images, an "application chooses" or
//! zero extent, an acquired index past the end of the swapchain. They
//! reach the driver-diversity branches that otherwise only run on other
//! hardware. A quirk is a standing property of the mutated device
//! rather than a one-shot, so those scenarios assert *repeatability*
//! where the fault scenarios assert recovery.
//!
//! Two scenarios arm nothing at all: their trigger is an argument (a
//! zero extent, a window the WSI cannot make a surface from), not a
//! driver failure.
//!
//! Skips (exit 0, SKIP line) without a display, a Vulkan runtime,
//! presentation support, or the fault layer; under
//! `RENEW_FAULT_STRICT=1` every one of those becomes a failure.

// Arming a fault or a quirk is an environment write, `unsafe` since the
// 2024 edition. Sound here: this binary is single-threaded (the event
// loop owns the main thread and every scenario runs on it), so nothing
// reads or writes the environment concurrently.
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

/// Set `name` to `value`, or remove it when `value` is empty.
fn set_or_clear(name: &str, value: &str) {
    if value.is_empty() {
        // SAFETY: single-threaded binary; see the module note.
        unsafe { std::env::remove_var(name) };
    } else {
        // SAFETY: as above.
        unsafe { std::env::set_var(name, value) };
    }
}

/// Arm exactly `fault` and `quirk` — either one empty meaning "not
/// armed" — for the duration of `run`, then clear both. Every scenario
/// arms through here, so none can inherit an arming from the one
/// before it, whatever order they run in.
fn armed<T>(fault: &str, quirk: &str, run: impl FnOnce() -> T) -> T {
    set_or_clear("RENEW_FAULT", fault);
    set_or_clear("RENEW_QUIRK", quirk);
    let outcome = run();
    set_or_clear("RENEW_FAULT", "");
    set_or_clear("RENEW_QUIRK", "");
    outcome
}

/// Arm `spec` as the one fault for the duration of `run`, then disarm.
fn with_fault<T>(spec: &str, run: impl FnOnce() -> T) -> T {
    armed(spec, "", run)
}

/// Arm the `RENEW_QUIRK` list `spec` for the duration of `run`, then
/// disarm. A quirk is never spent by firing: it shapes every matching
/// call the mutated device makes for as long as it is armed.
fn with_quirk<T>(spec: &str, run: impl FnOnce() -> T) -> T {
    armed("", spec, run)
}

/// Run with nothing armed: the layer re-reads both variables at every
/// instance creation, so unset means an unfaulted, unmutated driver.
fn without_fault<T>(run: impl FnOnce() -> T) -> T {
    armed("", "", run)
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

/// The protocol a quirk scenario walks. Recovery is not among them: a
/// quirk is not spent by firing, so inside one device the mutated
/// driver keeps its behavior, and the contract worth proving is that
/// the engine's answer is stable and leaves nothing behind.
#[derive(Clone, Copy)]
enum QuirkShape {
    /// The device can never present to this surface; the payload is
    /// the exact check that rejected it, and a second attempt on the
    /// same device must give the same rejection.
    NeverPresentable(&'static str),
    /// The chain builds, but every acquire hands back an index that
    /// addresses no image of it: the frame aborts, the target goes
    /// dormant, and a rebuilt chain aborts the same way.
    EveryFrameAborts(Expect),
    /// The surface reports the "you choose" extent sentinel, so the
    /// size the caller asked for is the size it gets — across a
    /// rebuild too — and frames present at it.
    ApplicationChoosesExtent,
    /// The surface offers exactly one format the engine can use, and it
    /// is the second of its two preferences: the target must come up on
    /// that format and present at it, across a rebuild too.
    /// The surface has no drawable area at all — a minimized window —
    /// so the target is born dormant and a resize leaves it dormant
    /// rather than failing.
    AlwaysDormant,
}

/// Every quirk scenario: the name printed, the `RENEW_QUIRK` list
/// armed, and the protocol the target must then honor. Each names a
/// driver behavior that is legal Vulkan and simply absent from this
/// machine's hardware.
const QUIRKS: &[(&str, &str, QuirkShape)] = &[
    (
        "D1 no-swapchain-extension",
        "no-swapchain-extension",
        QuirkShape::NeverPresentable("the device does not offer the swapchain extension"),
    ),
    (
        "D2 no-surface-formats",
        "no-surface-formats",
        QuirkShape::NeverPresentable("no 8-bit UNORM sRGB surface format"),
    ),
    (
        "D3 acquire-out-of-range-index",
        "acquire-out-of-range-index",
        QuirkShape::EveryFrameAborts(Expect::Creation("vkAcquireNextImageKHR(index)")),
    ),
    (
        "D4 no-swapchain-images",
        "no-swapchain-images",
        QuirkShape::EveryFrameAborts(Expect::Creation("vkAcquireNextImageKHR(index)")),
    ),
    (
        "D5 undefined-surface-extent",
        "undefined-surface-extent",
        QuirkShape::ApplicationChoosesExtent,
    ),
    (
        "D6 present-unsupported",
        "present-unsupported",
        QuirkShape::NeverPresentable("the graphics queue cannot present to this surface"),
    ),
    // A scenario forcing the RGBA format arm used to sit here. The
    // quirk behind it selects the surface's own RGBA entry rather than
    // fabricating one — correct, because a fabricated format would make
    // the swapchain invalid — but that makes the outcome depend on what
    // the machine's surface happens to offer: it holds on a desktop GPU
    // and does nothing under the software rasterizer CI runs on. The
    // choice is pure logic over the reported list, so it is proven
    // exhaustively in swapchain.rs's unit tests instead, on every
    // machine. The quirk stays in the layer for investigation.
    (
        "D8 zero-surface-extent",
        "zero-surface-extent",
        QuirkShape::AlwaysDormant,
    ),
];

/// The two scenarios that arm nothing, run after both tables.
const UNFAULTED_SCENARIOS: usize = 2;
/// How many scenarios there are in total.
const SCENARIOS: usize = LADDER.len() + QUIRKS.len() + UNFAULTED_SCENARIOS;

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

/// Every quirk scenario: arm the response mutation, then run the case.
fn quirk_case(
    extent: Extent,
    window: &NativeWindow,
    quirk: &str,
    body: impl FnOnce(&Device, Result<WindowTarget, TargetError>) -> Verdict,
) -> Verdict {
    with_quirk(quirk, || target_case(extent, window, body))
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

fn walk_quirk(
    shape: QuirkShape,
    device: &Device,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
    window: &NativeWindow,
) -> Verdict {
    match shape {
        QuirkShape::NeverPresentable(reason) => {
            never_presentable(reason, device, target, size, window)
        }
        QuirkShape::EveryFrameAborts(expect) => every_frame_aborts(expect, target, size),
        QuirkShape::ApplicationChoosesExtent => application_chooses_extent(target, size),
        QuirkShape::AlwaysDormant => always_dormant(target, size),
    }
}

/// `Ok(())` when the build was refused as unpresentable for exactly
/// `reason` — the string is the whole diagnosis a caller gets, so it is
/// the thing worth asserting.
fn refused(reason: &'static str, target: Result<WindowTarget, TargetError>) -> Verdict {
    match target {
        Err(TargetError::PresentUnsupported { reason: got }) if got == reason => Ok(()),
        Err(other) => Err(wrong(&format!("PresentUnsupported({reason})"), &other)),
        Ok(unexpected) => Err(format!(
            "a target built on a surface the device cannot present to (extent {:?})",
            unexpected.extent()
        )),
    }
}

/// A device whose driver reports away its ability to present: the
/// refusal must name the failing check, and it must repeat. The second
/// attempt is the leak check too — the first unwound past a surface
/// (and, past the format gate, nothing else), so a second surface on
/// the same window has to be creatable.
fn never_presentable(
    reason: &'static str,
    device: &Device,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
    window: &NativeWindow,
) -> Verdict {
    refused(reason, target)?;
    refused(reason, device.create_window_target(window.clone(), size))
}

/// A chain that builds and then cannot be drawn into, because every
/// acquire names an image the chain does not have. The frame aborts and
/// the target goes dormant; the quirk is still armed, so the rebuilt
/// chain must abort identically — the abort path is re-entrant, not a
/// one-shot that corrupts the target on its second use.
fn every_frame_aborts(
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
    target
        .resize(size)
        .map_err(|error| format!("rebuild after the abort failed: {error}"))?;
    if target.extent() != size {
        return Err(format!(
            "expected the rebuilt chain at {size:?}, got {:?}",
            target.extent()
        ));
    }
    match target.render(CLEAR, None) {
        Err(error) => expect.matched(&error)?,
        Ok(outcome) => return Err(wrong("an error from the rebuilt chain", &outcome)),
    }
    assert_dormant(&mut target)
}

/// The target must be sized exactly `size` and present a frame at it.
fn presents_at(target: &mut WindowTarget, size: Extent, stage: &str) -> Verdict {
    let extent = target.extent();
    if extent != size {
        return Err(format!(
            "expected the chosen extent {size:?} {stage}, got {extent:?}"
        ));
    }
    match target.render(CLEAR, None) {
        Ok(PresentOutcome::Presented) => Ok(()),
        other => Err(wrong(&format!("Presented {stage}"), &other)),
    }
}

/// A surface that leaves the extent to the application: the engine's
/// clamp against the reported bounds decides the size, so the size
/// asked for is the size delivered — on the first build and on a
/// rebuild — and frames present at it.
fn application_chooses_extent(target: Result<WindowTarget, TargetError>, size: Extent) -> Verdict {
    let mut target = built(target)?;
    presents_at(&mut target, size, "on the first build")?;
    target
        .resize(size)
        .map_err(|error| format!("resize failed: {error}"))?;
    presents_at(&mut target, size, "after a resize")
}

/// A surface with no drawable area at all — a minimized window. Nothing
/// here is an error: the target must be *born* dormant instead of
/// failing to build, and because the quirk is still armed the resize
/// that would normally wake it finds the same zero extent and must
/// leave it dormant, again without failing.
fn always_dormant(target: Result<WindowTarget, TargetError>, size: Extent) -> Verdict {
    let mut target = built(target)?;
    assert_dormant(&mut target)?;
    target
        .resize(size)
        .map_err(|error| format!("resize of a zero-extent surface failed: {error}"))?;
    assert_dormant(&mut target)
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
            assert!(
                device.validation_active() || !strict(),
                "RENEW_FAULT_STRICT=1 but the validation layer is not active — the                  zero-validation-errors assertion in every scenario below would be                  a claim about nothing."
            );
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
        let after_ladder = index - LADDER.len();
        if let Some(&(name, quirk, shape)) = QUIRKS.get(after_ladder) {
            let verdict = quirk_case(size, window, quirk, |device, target| {
                walk_quirk(shape, device, target, size, window)
            });
            return (name, verdict);
        }
        match after_ladder - QUIRKS.len() {
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
            "FAIL: {} presentation scenario(s):\n{}",
            app.failures.len(),
            app.failures.join("\n")
        );
        std::process::exit(1);
    }
    if app.passed != SCENARIOS {
        eprintln!("FAIL: {} of {SCENARIOS} scenarios ran", app.passed);
        std::process::exit(1);
    }
    println!(
        "OK: {} presentation scenarios ({} faults, {} quirks, {UNFAULTED_SCENARIOS} unarmed)",
        app.passed,
        LADDER.len(),
        QUIRKS.len()
    );
}
