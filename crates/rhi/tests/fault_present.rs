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
    Attachment, BufferUsage, ClearValue, Color, Device, DeviceDesc, DeviceError, Extent, FrameData,
    Item, LoadOp, Pass, PipelineDesc, PresentOutcome, RenderDesc, StoreOp, SurfaceTransform,
    TargetError, Validation, WindowTarget, builtin,
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

/// The one color attachment these frames render into: cleared, stored.
fn clear(color: Color) -> [Attachment; 1] {
    [Attachment::new(
        LoadOp::Clear(ClearValue::Color(color)),
        StoreOp::Store,
    )]
}

fn strict() -> bool {
    std::env::var_os("RENEW_FAULT_STRICT").is_some_and(|value| value == "1")
}

/// Records the diag channel: a teardown wait-idle that fails reports
/// nowhere else — the last caller is gone, so the diag record *is* the
/// observable (the offscreen fault suite's shape, for this suite's one
/// teardown case).
struct Capture;

static CAPTURED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static CAPTURE: Capture = Capture;

impl renew_diag::Sink for Capture {
    fn write(&self, record: &renew_diag::Record<'_>) {
        if let Ok(mut captured) = CAPTURED.lock() {
            captured.push(format!("{} {}", record.level(), record.message()));
        }
    }
}

/// Forget everything recorded so far, so the scenario reads only what
/// it caused.
fn clear_records() {
    if let Ok(mut captured) = CAPTURED.lock() {
        captured.clear();
    }
}

/// Assert some record since the last [`clear_records`] contains
/// `needle`.
fn recorded(needle: &str) -> Verdict {
    match CAPTURED.lock() {
        Ok(captured) if captured.iter().any(|line| line.contains(needle)) => Ok(()),
        Ok(captured) => Err(format!(
            "no diagnostic contains {needle:?}; the channel carried {captured:?}"
        )),
        Err(_) => Err("the capture sink is poisoned".to_string()),
    }
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
    SlotReuseFails(Expect),
    /// Device loss surfacing from the fence wait, which does not happen
    /// until a slot is reused. Distinct from `DeviceLostOnFrame` because
    /// the frame number is a function of the ring depth rather than a
    /// constant, and a constant here silently stops testing the fence
    /// the day the depth changes.
    DeviceLostOnSlotReuse,
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
        // The fence ring is built one at a time, so a failure part-way
        // must destroy the ones already made. Failing the FIRST call
        // leaves nothing to clean up and never runs that path -- this
        // fails the second, which is the only ordinal that exercises it.
        "Q5b fence-ring-partial",
        "vkCreateFence=ERROR_OUT_OF_HOST_MEMORY@2",
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
        // RETARGETED 2026-08-01. This was named per-image and aimed at
        // ordinal 2, which was the second per-image semaphore while the
        // chain made exactly one acquire semaphore. The acquire ring is
        // created first and is FRAMES_IN_FLIGHT long, so ordinal 2 is
        // now the second acquire semaphore -- the scenario kept passing
        // while testing a different call, which is what a process-global
        // ordinal does when new calls appear ahead of it.
        "Q12 acquire-ring-semaphore",
        "vkCreateSemaphore=ERROR_OUT_OF_HOST_MEMORY@2",
        Shape::BuildFails(Expect::Creation("vkCreateSemaphore")),
    ),
    (
        // The first PER-IMAGE semaphore, which now sits after the
        // acquire ring. Kept as its own scenario rather than moving the
        // one above, so both sides of that boundary stay covered.
        "Q12b per-image-semaphore",
        "vkCreateSemaphore=ERROR_OUT_OF_HOST_MEMORY@3",
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
    // ---- the frame fence: waited only when a slot is REUSED --------
    //
    // With a ring of N frames in flight, the first N frames each take a
    // fresh slot with nothing outstanding on it and wait no fence at
    // all. The wait happens on frame N+1, when the ring wraps. These
    // said "second frame" until 2026-08-01, which was the same statement
    // only while N was one.
    (
        "P8 fence-timeout",
        "vkWaitForFences=TIMEOUT",
        Shape::SlotReuseFails(Expect::Timeout("vkWaitForFences")),
    ),
    (
        "P9 fence-failure",
        "vkWaitForFences=ERROR_OUT_OF_HOST_MEMORY",
        Shape::SlotReuseFails(Expect::Creation("vkWaitForFences")),
    ),
    (
        "P10 reset-fences",
        "vkResetFences=ERROR_OUT_OF_HOST_MEMORY",
        Shape::SlotReuseFails(Expect::Creation("vkResetFences")),
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
        Shape::DeviceLostOnSlotReuse,
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
    /// The surface has no drawable area at all — a minimized window —
    /// so the target is born dormant and a resize leaves it dormant
    /// rather than failing.
    AlwaysDormant,
    /// The surface allows no more images than its own minimum, so the
    /// engine's min-plus-one choice has to clamp. Nothing about that is
    /// exceptional to a caller: the target must build and present
    /// exactly as it always does, which is the whole assertion.
    ClampsImageCount,
    /// The surface reports a quarter turn — the shape a handheld panel
    /// presents and no desktop ever does. The target must build,
    /// present, and **report the rotation it declared**, because a
    /// renderer can only fold what it can ask about, and a target that
    /// swallowed the transform would leave every caller drawing
    /// sideways with no way to notice.
    ReportsRotation,
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
        QuirkShape::NeverPresentable("no 8-bit sRGB surface format"),
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
    (
        "D9 max-image-count-at-minimum",
        "max-image-count-at-minimum",
        QuirkShape::ClampsImageCount,
    ),
    (
        "D10 surface-rotated-90",
        "surface-rotated-90",
        QuirkShape::ReportsRotation,
    ),
];

/// The scenarios past both tables. S1 and S2 run unfaulted; S3 arms a
// COMPOUND fault through the same path the ladder uses, but does not
// fit the table's one-call shape; S4 arms the teardown wait-idle fault
// and reads the diag channel, the only observable a Drop has; S5-S7
// are the window half of the depth creation ladder, guarded on the
// adapter offering a depth format.
const UNFAULTED_SCENARIOS: usize = 7;
/// How many scenarios there are in total.
const SCENARIOS: usize = LADDER.len() + QUIRKS.len() + UNFAULTED_SCENARIOS;

/// The target must be dormant: no size, and rendering asks for a
/// resize instead of presenting.
fn assert_dormant(target: &mut WindowTarget) -> Verdict {
    let extent = target.extent();
    if extent.width != 0 || extent.height != 0 {
        return Err(format!("expected a dormant target, got extent {extent:?}"));
    }
    let color = clear(CLEAR);
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
        Ok(PresentOutcome::NeedsResize) => Ok(()),
        other => Err(wrong("NeedsResize from a dormant target", &other)),
    }
}

/// Rebuild after dormancy and prove the target presents again.
fn assert_recovers(target: &mut WindowTarget, size: Extent) -> Verdict {
    target
        .resize(size)
        .map_err(|error| format!("recovery resize failed: {error}"))?;
    let color = clear(CLEAR);
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
        Ok(PresentOutcome::Presented) => Ok(()),
        other => Err(wrong("Presented after recovery", &other)),
    }
}

/// The failed-quiesce corner, which is the retention design's whole
/// reason to exist: a frame fails, the abort's recovery quiesce ALSO
/// fails (host OOM, not a lost device), and the pending flags clear with
/// work possibly still executing. Retained buffer memory must survive
/// that moment — releasing it there is a device-side use-after-free —
/// and release only at the next PROVEN quiesce, which is the resize
/// rebirth already requires. The caller's handle is dropped in the
/// middle, so retention is the only thing keeping the memory alive; the
/// validation layer, consulted after the case, is what proves no freed
/// memory was still referenced.
fn failed_quiesce_retains_frame_buffers(
    device: &Device,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
) -> Verdict {
    let mut target = built(target)?;
    let pipeline = device
        .create_pipeline(
            &PipelineDesc::new(builtin::INSTANCED, target.format())
                .instance_input(builtin::INSTANCED_LAYOUT),
        )
        .map_err(|error| format!("instanced pipeline failed: {error}"))?;
    let buffer = device
        .create_buffer(64, BufferUsage::PerFrame)
        .map_err(|error| format!("per-frame buffer failed: {error}"))?;
    let bytes = [0u8; 24];
    let color = clear(CLEAR);

    // Frame 1 presents and its slot retains the buffer.
    let items = [Item::new(&pipeline).frame_data(FrameData::new(&buffer, &bytes, 1))];
    match target.render(&RenderDesc::new(&[Pass::new(&color, &items)])) {
        Ok(PresentOutcome::Presented) => {}
        other => return Err(wrong("Presented on the first frame", &other)),
    }

    // Frame 2: the submit fails, the abort's quiesce fails too.
    let items = [Item::new(&pipeline).frame_data(FrameData::new(&buffer, &bytes, 1))];
    match target.render(&RenderDesc::new(&[Pass::new(&color, &items)])) {
        Err(error) => Expect::Creation("vkQueueSubmit2").matched(&error)?,
        Ok(outcome) => return Err(wrong("an error", &outcome)),
    }

    // The caller walks away. Retention is now the only owner, and the
    // flags cleared under a FAILED quiesce — the exact corner where
    // releasing would free memory frame 1's submit may still read.
    drop(buffer);
    assert_dormant(&mut target)?;

    // Rebirth: resize's wait-idle is the proof (its fault was spent on
    // the abort), retention releases, and a plain frame presents.
    assert_recovers(&mut target, size)
}

/// Once the device is lost the poison is total: the target, a resize, a
/// fresh target, and the device itself all fail fast and stay failed.
fn assert_poison_sticks(
    device: &Device,
    target: &mut WindowTarget,
    window: &NativeWindow,
    size: Extent,
) -> Verdict {
    let color = clear(CLEAR);
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
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

/// One rung of the window half of the depth creation ladder: the named
/// call fails during chain build — on this path the depth image's
/// create, allocate and bind are the first of their names, so no
/// ordinal is needed — target creation reports the depth call by name,
/// and a second build on the same device proves the partial-chain
/// unwinder left it whole. Skips (Ok) on an adapter with no depth
/// format, where the guarded calls never happen.
fn depth_creation_case(extent: Extent, window: &NativeWindow, fault: &str) -> Verdict {
    present_case(extent, window, fault, |device, target| {
        if device.depth_format_name().is_none() {
            eprintln!("SKIP: adapter offers no chain depth format");
            return Ok(());
        }
        match target {
            Err(TargetError::Creation { call, .. }) if call.contains("(depth)") => {}
            Err(TargetError::OutOfDeviceMemory { call }) if call.contains("(depth)") => {}
            Ok(_) => return Err("the build succeeded despite the fault".to_string()),
            Err(other) => return Err(wrong("a depth-named creation failure", &other)),
        }
        // The fault is spent: the unwinder left a device that builds.
        let rebuilt = device
            .create_window_target(window.clone(), extent)
            .map_err(|error| format!("recovery build failed: {error}"))?;
        drop(rebuilt);
        Ok(())
    })
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
        Shape::SlotReuseFails(expect) => slot_reuse_fails(expect, target, size),
        Shape::DeviceLostOnSlotReuse => {
            // The ring must be full before the fence is waited at all,
            // so the losing frame is depth + 1. Asked of a freshly built
            // target rather than assumed.
            let depth = match &target {
                Ok(built) => built.frames_in_flight(),
                Err(_) => 1,
            };
            let frame = u32::try_from(depth).unwrap_or(u32::MAX).saturating_add(1);
            device_lost_on_frame(frame, device, target, size, window)
        }
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
        QuirkShape::ClampsImageCount => clamps_image_count(target, size),
        QuirkShape::ReportsRotation => reports_rotation(target, size),
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
    let color = clear(CLEAR);
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
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
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
        Err(error) => expect.matched(&error)?,
        Ok(outcome) => return Err(wrong("an error from the rebuilt chain", &outcome)),
    }
    assert_dormant(&mut target)
}

/// The target must be sized exactly `size` and present a frame at it.
/// A surface that allows exactly its own minimum number of images. The
/// engine asks for one more than the minimum and must clamp to what the
/// surface permits; a swapchain built from an unclamped count would be
/// rejected outright, so a target that builds and presents is the proof
/// that the clamp ran. The rebuild repeats it, because the count is
/// chosen afresh every time a chain is built.
fn clamps_image_count(target: Result<WindowTarget, TargetError>, size: Extent) -> Verdict {
    let mut target = built(target)?;
    presents_at(&mut target, size, "on the first build")?;
    target
        .resize(size)
        .map_err(|error| format!("resize failed: {error}"))?;
    presents_at(&mut target, size, "after a rebuild")
}

/// A rotated surface builds and presents like any other, and the
/// target says so: the transform it declared when building the chain
/// is the transform it reports, across a rebuild too. (The layer
/// restores a supported transform below us before the driver sees it —
/// a desktop driver cannot be made to rotate, and what is under test
/// here is the caller's handling of a surface that says it is.) This is
/// the only lane anywhere that sees a non-identity transform — every
/// real surface here is a desktop one — so it is also the only proof
/// that the reported value tracks the surface rather than a default.
fn reports_rotation(target: Result<WindowTarget, TargetError>, size: Extent) -> Verdict {
    let mut target = built(target)?;
    if target.transform() != SurfaceTransform::Rotate90 {
        return Err(format!(
            "expected the declared quarter turn to be reported, got {:?}",
            target.transform()
        ));
    }
    presents_at(&mut target, size, "on a rotated surface")?;
    target
        .resize(size)
        .map_err(|error| format!("resize failed: {error}"))?;
    if target.transform() != SurfaceTransform::Rotate90 {
        return Err(format!(
            "the rebuild lost the rotation: {:?}",
            target.transform()
        ));
    }
    presents_at(&mut target, size, "after a rebuild on a rotated surface")
}

fn presents_at(target: &mut WindowTarget, size: Extent, stage: &str) -> Verdict {
    let extent = target.extent();
    if extent != size {
        return Err(format!(
            "expected the chosen extent {size:?} {stage}, got {extent:?}"
        ));
    }
    let color = clear(CLEAR);
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
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
    let color = clear(CLEAR);
    match recovered.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
        Ok(PresentOutcome::Presented) => Ok(()),
        other => Err(wrong("Presented after recovery", &other)),
    }
}

fn frame_fails_chain_survives(
    expect: Expect,
    target: Result<WindowTarget, TargetError>,
) -> Verdict {
    let mut target = built(target)?;
    let color = clear(CLEAR);
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
        Err(error) => expect.matched(&error)?,
        Ok(outcome) => return Err(wrong("an error", &outcome)),
    }
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
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
    let color = clear(CLEAR);
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
        Err(error) => expect.matched(&error)?,
        Ok(outcome) => return Err(wrong("an error", &outcome)),
    }
    assert_dormant(&mut target)?;
    assert_recovers(&mut target, size)
}

/// Fill the ring, then fail on the frame that reuses a slot.
///
/// Asked of the target rather than hardcoded: the fence wait first
/// happens on frame `frames_in_flight() + 1`, and writing that as a
/// literal would silently stop testing the fence the day the ring
/// changes depth -- it would just pass, on a frame that waits nothing.
fn slot_reuse_fails(
    expect: Expect,
    target: Result<WindowTarget, TargetError>,
    size: Extent,
) -> Verdict {
    let mut target = built(target)?;
    let depth = target.frames_in_flight();
    let color = clear(CLEAR);
    for frame in 0..depth {
        match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
            Ok(PresentOutcome::Presented) => {}
            other => {
                return Err(wrong(
                    &format!("Presented on frame {} of the first {depth}", frame + 1),
                    &other,
                ));
            }
        }
    }
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
        Err(error) => expect.matched(&error)?,
        Ok(outcome) => {
            return Err(wrong(
                &format!("an error on frame {}, the first to reuse a slot", depth + 1),
                &outcome,
            ));
        }
    }
    assert_recovers(&mut target, size)
}

fn stale_swapchain(target: Result<WindowTarget, TargetError>, size: Extent) -> Verdict {
    let mut target = built(target)?;
    let color = clear(CLEAR);
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
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
    let color = clear(CLEAR);
    for earlier in 1..frame {
        match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
            Ok(PresentOutcome::Presented) => {}
            other => return Err(wrong(&format!("Presented on frame {earlier}"), &other)),
        }
    }
    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
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
            1 => ("S2 unsupported-window-handles", unsupported_handles()),
            2 => (
                "S3 failed-quiesce-retention",
                present_case(
                    size,
                    window,
                    "vkQueueSubmit2=ERROR_OUT_OF_HOST_MEMORY@2,                     vkDeviceWaitIdle=ERROR_OUT_OF_HOST_MEMORY",
                    |device, target| failed_quiesce_retains_frame_buffers(device, target, size),
                ),
            ),
            // S4: the window target's teardown wait-idle fails — the
            // diag record is the only observable. The no-ordinal spec
            // is first-match, and nothing before the drop performs a
            // wait-idle (bring-up and target creation make none; no
            // frame is rendered, no resize runs), so the target's own
            // Drop owns this window's first — the faulted one; the
            // spine's follows, unfaulted. R1's resize case and S3's
            // compound arm each own separate windows, untouched.
            3 => (
                "S4 target-teardown/wait-idle-failure",
                present_case(
                    size,
                    window,
                    "vkDeviceWaitIdle=ERROR_OUT_OF_HOST_MEMORY",
                    |_, target| {
                        let target = built(target)?;
                        clear_records();
                        drop(target);
                        recorded("wait-idle at window-target teardown failed")
                    },
                ),
            ),
            // S5-S7: the window half of the depth creation ladder. On
            // this path the chain's own images come from the swapchain,
            // so the depth image's vkCreateImage, vkAllocateMemory and
            // vkBindImageMemory are the FIRST of their names — no
            // ordinal needed (the depth view is not pinnable here: its
            // vkCreateImageView ordinal floats with the driver-chosen
            // swapchain image count, and its failure arm is driven on
            // the offscreen twin). Each proves creation fails cleanly
            // AND the partial chain's unwinder leaves the device
            // rebuilding: a second target on the same device builds and
            // goes dormant-or-usable per protocol, validation clean.
            4 => (
                "S5 depth-image-creation-fails",
                depth_creation_case(size, window, "vkCreateImage=ERROR_OUT_OF_HOST_MEMORY"),
            ),
            5 => (
                "S6 depth-memory-allocation-fails",
                depth_creation_case(size, window, "vkAllocateMemory=ERROR_OUT_OF_DEVICE_MEMORY"),
            ),
            _ => (
                "S7 depth-image-bind-fails",
                depth_creation_case(size, window, "vkBindImageMemory=ERROR_OUT_OF_HOST_MEMORY"),
            ),
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
    renew_diag::install(&CAPTURE);
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
