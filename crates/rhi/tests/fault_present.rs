//! Fault-injection scenarios for the presentation path: swapchain
//! creation, acquire, submit, and present failures, each driven through
//! a real window.
//!
//! `harness = false` because the OS event loop must own the main
//! thread. Scenarios run one per redraw so the window is fully mapped
//! before any of them touches a surface; each builds its own device and
//! target, because the fault layer arms at instance creation and fires
//! exactly once.
//!
//! Skips (exit 0, SKIP line) without a display, a Vulkan runtime,
//! presentation support, or the fault layer; under
//! `RENEW_FAULT_STRICT=1` every one of those becomes a failure.

// Arming a fault is an environment write, `unsafe` since the 2024
// edition. Sound here: this binary is single-threaded (the event loop
// owns the main thread and every scenario runs on it), so nothing reads
// or writes the environment concurrently.
#![allow(unsafe_code)]

use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowError, WindowEvent, WindowRef, run_window_app,
};
use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, PresentOutcome, TargetError, Validation,
    WindowTarget,
};

const CLEAR: Color = Color::new(0.1, 0.2, 0.3, 1.0);
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

/// Implicit layers (overlays, vendor shims) sit above this suite and
/// some of them RETRY calls that fail — an injected fault would then be
/// papered over by a layer nobody asked for, and the scenario would
/// silently measure the overlay instead of the backend. Observed on a
/// desktop with a vendor present layer and a capture hook installed.
/// The loader reads this when an instance is created, so setting it
/// before the first device is enough.
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

fn creation_named(call: &str, got: &TargetError) -> Verdict {
    match got {
        TargetError::Creation { call: got_call, .. } if *got_call == call => Ok(()),
        other => Err(wrong(&format!("Creation({call})"), other)),
    }
}

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

/// Every scenario: arm the fault, build a device and (usually) a
/// target, run the body, then check validation and tear down.
fn present_case(
    size: Extent,
    window: &renew_platform::window::NativeWindow,
    fault: &str,
    body: impl FnOnce(&Device, Result<WindowTarget, TargetError>) -> Verdict,
) -> Verdict {
    with_fault(fault, || {
        let device = new_device().map_err(|error| format!("device bring-up: {error}"))?;
        let target = device.create_window_target(window.clone(), size);
        let outcome = body(&device, target);
        let clean = validation_clean(&device);
        outcome.and(clean)
    })
}

struct FaultApp {
    window: Option<renew_platform::window::NativeWindow>,
    size: Extent,
    next: usize,
    passed: usize,
    updates: u32,
    skip: Option<String>,
    failures: Vec<String>,
}

/// How many scenarios the table below holds.
const SCENARIOS: usize = 7;

impl FaultApp {
    fn new() -> Self {
        Self {
            window: None,
            size: Extent {
                width: 0,
                height: 0,
            },
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
    #[expect(
        clippy::too_many_lines,
        reason = "one scenario table; splitting it would scatter the protocol it walks"
    )]
    fn run_scenario(&self, index: usize) -> (&'static str, Verdict) {
        let size = self.size;
        let Some(window) = self.window.as_ref() else {
            return ("no window", Err("the window vanished".to_string()));
        };
        match index {
            // Swapchain creation failing is a plain creation error.
            0 => (
                "P1 swapchain-creation",
                present_case(
                    size,
                    window,
                    "vkCreateSwapchainKHR=ERROR_OUT_OF_HOST_MEMORY",
                    |_device, target| match target {
                        Err(error) => creation_named("vkCreateSwapchainKHR", &error),
                        Ok(built) => Err(format!(
                            "the target built despite the fault (extent {:?})",
                            built.extent()
                        )),
                    },
                ),
            ),
            // An out-of-date acquire is a protocol outcome, not an
            // error: resize and carry on.
            1 => (
                "P2 acquire-out-of-date",
                present_case(
                    size,
                    window,
                    "vkAcquireNextImageKHR=ERROR_OUT_OF_DATE_KHR",
                    |_device, target| {
                        let mut target = target.map_err(|error| format!("target: {error}"))?;
                        match target.render(CLEAR, None) {
                            Ok(PresentOutcome::NeedsResize) => {}
                            other => return Err(wrong("NeedsResize", &other)),
                        }
                        assert_recovers(&mut target, size)
                    },
                ),
            ),
            // A stalled acquire is an error, but the chain survives.
            2 => (
                "P3 acquire-timeout",
                present_case(
                    size,
                    window,
                    "vkAcquireNextImageKHR=TIMEOUT",
                    |_device, target| {
                        let mut target = target.map_err(|error| format!("target: {error}"))?;
                        match target.render(CLEAR, None) {
                            Err(TargetError::Timeout {
                                call: "vkAcquireNextImageKHR",
                            }) => {}
                            other => {
                                return Err(wrong("Timeout(vkAcquireNextImageKHR)", &other));
                            }
                        }
                        match target.render(CLEAR, None) {
                            Ok(PresentOutcome::Presented) => Ok(()),
                            other => Err(wrong("Presented on the next frame", &other)),
                        }
                    },
                ),
            ),
            // A failed submit leaves an acquire signal outstanding, so
            // the target must go dormant rather than continue.
            3 => (
                "P4 submit-failure-dormancy",
                present_case(
                    size,
                    window,
                    "vkQueueSubmit2=ERROR_OUT_OF_HOST_MEMORY",
                    |_device, target| {
                        let mut target = target.map_err(|error| format!("target: {error}"))?;
                        match target.render(CLEAR, None) {
                            Err(error) => creation_named("vkQueueSubmit2", &error)?,
                            Ok(outcome) => return Err(wrong("an error", &outcome)),
                        }
                        assert_dormant(&mut target)?;
                        assert_recovers(&mut target, size)
                    },
                ),
            ),
            // An out-of-date present is a protocol outcome.
            4 => (
                "P5 present-out-of-date",
                present_case(
                    size,
                    window,
                    "vkQueuePresentKHR=ERROR_OUT_OF_DATE_KHR",
                    |_device, target| {
                        let mut target = target.map_err(|error| format!("target: {error}"))?;
                        match target.render(CLEAR, None) {
                            Ok(PresentOutcome::NeedsResize) => {}
                            other => return Err(wrong("NeedsResize", &other)),
                        }
                        assert_recovers(&mut target, size)
                    },
                ),
            ),
            // A lost surface is a real error, and the target goes
            // dormant behind it.
            5 => (
                "P6 present-surface-lost",
                present_case(
                    size,
                    window,
                    "vkQueuePresentKHR=ERROR_SURFACE_LOST_KHR",
                    |_device, target| {
                        let mut target = target.map_err(|error| format!("target: {error}"))?;
                        match target.render(CLEAR, None) {
                            Err(error) => creation_named("vkQueuePresentKHR", &error)?,
                            Ok(outcome) => return Err(wrong("an error", &outcome)),
                        }
                        assert_dormant(&mut target)
                    },
                ),
            ),
            // The frame fence only gets waited on from the second frame
            // onward: a stall there is an error, and a resize (which
            // quiesces and retires the fence) recovers.
            _ => (
                "P7 frame-fence-timeout",
                present_case(
                    size,
                    window,
                    "vkWaitForFences=TIMEOUT",
                    |_device, target| {
                        let mut target = target.map_err(|error| format!("target: {error}"))?;
                        match target.render(CLEAR, None) {
                            Ok(PresentOutcome::Presented) => {}
                            other => return Err(wrong("Presented on the first frame", &other)),
                        }
                        match target.render(CLEAR, None) {
                            Err(TargetError::Timeout {
                                call: "vkWaitForFences",
                            }) => {}
                            other => return Err(wrong("Timeout(vkWaitForFences)", &other)),
                        }
                        assert_recovers(&mut target, size)
                    },
                ),
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
