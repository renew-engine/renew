//! The allocation contract on the **window** path.
//!
//! `zero_alloc.rs` pins the offscreen path and stops there, so the path
//! that acquires, submits and presents — the one a real game runs every
//! frame — had no allocation gate at all. That is also the path the frame
//! signature and frames-in-flight work land on, so the oracle has to
//! exist before the work rather than after it.
//!
//! `harness = false` because winit's event loop owns the main thread, and
//! a global allocator is per-binary, so this cannot live inside the
//! offscreen suite.
//!
//! **The counter is `renew_memory::CountingAllocator`, not a local one.**
//! A test-local counting shim was written first and then deleted: it
//! would have been the sixth copy of a helper already recorded as
//! duplicated five times, and — because implementing a global allocator
//! needs `unsafe impl` — it would have enlarged the crate's recorded
//! unsafe surface for a counter the engine already ships. Reusing the
//! engine's costs no removability cell either: that crate is ratified
//! minimal core, so nothing removes it.
//!
//! Validation stays off: the debug messenger allocates when it speaks,
//! and it must not speak into the measurement. Driver-side host
//! allocations are invisible here by design — they route through the
//! instrumented Vulkan callbacks and their own ledger; this pins the
//! engine side of the line, exactly as the offscreen gate does.

use renew_memory::{CountingAllocator, counters};
use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowError, WindowEvent, WindowRef, run_window_app,
};
use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, PipelineDesc, PresentOutcome, TargetError,
    Validation, WindowTarget, builtin,
};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Allocations recorded so far, process-wide.
fn allocations() -> u64 {
    counters::snapshot().allocations
}

/// Frames per measurement window.
const WINDOW_FRAMES: u32 = 16;
/// Warmup frames before measuring: the first present lazily initializes
/// driver state, and a swapchain may be rebuilt once on first show.
const WARMUP_FRAMES: u32 = 8;
/// Retries. The counter is process-wide, so a neighbour's one-shot
/// allocation must be allowed to ride out while a real render-path
/// allocation reproduces in every window. Same protocol as the offscreen
/// gate, and the reason is the same.
const ATTEMPTS: u32 = 5;
/// Poll-loop iterations before declaring the run wedged.
const UPDATE_BUDGET: u32 = 20_000;

struct GateApp {
    device: Option<Device>,
    target: Option<WindowTarget>,
    pipeline: Option<renew_rhi::RenderPipeline>,
    size: Extent,
    presented: u32,
    updates: u32,
    /// Allocations charged to `render` calls inside the current window.
    /// Summed per call rather than measured across the window, so the
    /// event loop's own work between frames is never counted.
    window_spent: u64,
    /// Frames presented inside the current window.
    window_frames: u32,
    attempts: u32,
    last_delta: u64,
    observed_zero: bool,
    done: bool,
    skip: Option<String>,
    failure: Option<String>,
}

impl GateApp {
    fn new() -> Self {
        Self {
            device: None,
            target: None,
            pipeline: None,
            size: Extent {
                width: 0,
                height: 0,
            },
            presented: 0,
            updates: 0,
            window_spent: 0,
            window_frames: 0,
            attempts: 0,
            last_delta: 0,
            observed_zero: false,
            done: false,
            skip: None,
            failure: None,
        }
    }
}

impl GateApp {
    /// Nothing left to do: measured, skipped, or broken.
    fn finished(&self) -> bool {
        self.done || self.skip.is_some() || self.failure.is_some()
    }
}

impl WindowApp for GateApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let strict = std::env::var_os("RENEW_GOLDEN").is_some_and(|value| value == "1");
        let device = match Device::new(&DeviceDesc {
            app_name: "renew-present-zero-alloc",
            validation: Validation::Off,
        }) {
            Ok(device) => device,
            Err(DeviceError::LoaderUnavailable { message }) if !strict => {
                self.skip = Some(format!("no Vulkan runtime: {message}"));
                return;
            }
            Err(error) => {
                self.failure = Some(format!("device bring-up failed: {error}"));
                return;
            }
        };
        let (width, height) = window.physical_size();
        self.size = Extent { width, height };
        let target = match device.create_window_target(window.native(), self.size) {
            Ok(target) => target,
            Err(TargetError::PresentUnsupported { reason }) if !strict => {
                self.skip = Some(format!("cannot present to this surface: {reason}"));
                return;
            }
            Err(error) => {
                self.failure = Some(format!("window target failed: {error}"));
                return;
            }
        };
        let pipeline = match device.create_pipeline(&PipelineDesc {
            vertex_spirv: builtin::TRIANGLE_VS_SPV,
            fragment_spirv: builtin::TRIANGLE_FS_SPV,
            target_format: target.format(),
        }) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                self.failure = Some(format!("pipeline failed: {error}"));
                return;
            }
        };
        self.device = Some(device);
        self.target = Some(target);
        self.pipeline = Some(pipeline);
    }

    fn event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::Resized { width, height } => {
                self.size = Extent { width, height };
                if let Some(target) = &mut self.target
                    && let Err(error) = target.resize(self.size)
                {
                    self.failure = Some(format!("resize failed: {error}"));
                }
            }
            WindowEvent::RedrawRequested => {
                let Some(target) = &mut self.target else {
                    return;
                };
                let clear = Color::new(0.1, 0.2, 0.3, 1.0);
                // Bracket `render` ITSELF, not the event-handling window
                // around it. Reading the counter at frame boundaries
                // instead put everything the OS event loop does between
                // redraws inside the measurement -- which is nothing on
                // Windows and two allocations per iteration on X11, so
                // the gate passed locally and failed in CI for a reason
                // that was never the engine's. The contract is about the
                // render path; measure the render path.
                let before = allocations();
                let outcome = target.render(clear, self.pipeline.as_ref());
                let spent = allocations() - before;
                match outcome {
                    Ok(PresentOutcome::Presented) => {
                        self.presented += 1;
                        if self.presented > WARMUP_FRAMES {
                            self.window_frames += 1;
                            self.window_spent += spent;
                        }
                    }
                    // A rebuild is not steady state. Restart the window
                    // rather than charging its allocations to the frame
                    // path, which would make the gate report a defect
                    // where the environment merely resized us.
                    Ok(PresentOutcome::NeedsResize) => {
                        if let Err(error) = target.resize(self.size) {
                            self.failure = Some(format!("rebuild resize failed: {error}"));
                            return;
                        }
                        self.window_frames = 0;
                        self.window_spent = 0;
                    }
                    Err(error) => {
                        self.failure = Some(format!("render failed: {error}"));
                        return;
                    }
                }

                // Open the first measurement window exactly when warmup
                // ends, so the transition itself is never inside one.
                if self.presented == WARMUP_FRAMES {
                    self.window_frames = 0;
                    self.window_spent = 0;
                }
                if self.window_frames == WINDOW_FRAMES {
                    self.last_delta = self.window_spent;
                    self.attempts += 1;
                    if self.window_spent == 0 {
                        self.observed_zero = true;
                        self.done = true;
                    } else if self.attempts >= ATTEMPTS {
                        self.done = true;
                    } else {
                        self.window_frames = 0;
                        self.window_spent = 0;
                    }
                }
            }
            _ => {}
        }
    }

    fn update(&mut self, control: &mut LoopControl) {
        self.updates += 1;
        if self.updates > UPDATE_BUDGET && !self.finished() {
            self.failure = Some(format!(
                "wedged: {} updates, {} presented, {} in the current window",
                self.updates, self.presented, self.window_frames
            ));
        }
        if self.finished() {
            control.exit();
        } else {
            control.request_redraw();
        }
    }
}

fn main() {
    let mut app = GateApp::new();
    let config = WindowConfig {
        title: "renew present zero-alloc gate".to_string(),
        logical_width: 320.0,
        logical_height: 240.0,
        resizable: true,
    };
    let run = run_window_app(&config, &mut app);
    // Drop GPU objects before the verdict, matching the sibling suite:
    // teardown is part of the frame path's lifetime, not after it.
    drop(app.target.take());
    drop(app.pipeline.take());

    match run {
        Ok(()) => {}
        Err(WindowError::LoopUnavailable { message }) => {
            println!("SKIP: window loop unavailable: {message}");
            return;
        }
        Err(error) => {
            eprintln!("FAIL: window loop: {error}");
            std::process::exit(1);
        }
    }
    if let Some(message) = app.skip {
        println!("SKIP: {message}");
        return;
    }
    if let Some(message) = app.failure {
        eprintln!("FAIL: {message}");
        std::process::exit(1);
    }
    // The driver-side ledger is printed for the record, never gated:
    // driver host-allocation behaviour is the driver's, not ours.
    if let Some(device) = &app.device {
        let stats = device.host_allocation_stats();
        println!("driver host allocations: {stats:?}");
    }
    assert!(
        app.observed_zero,
        "the window render path allocated on the frame path: {} allocations across {WINDOW_FRAMES} \
         steady frames, reproduced in all {ATTEMPTS} measurement windows. The offscreen gate has \
         covered this contract for the offscreen path only; this is the window path.",
        app.last_delta
    );
    println!(
        "OK: {WINDOW_FRAMES} steady window frames allocated nothing (after {WARMUP_FRAMES} warmup, \
         {} presented total)",
        app.presented
    );
}
