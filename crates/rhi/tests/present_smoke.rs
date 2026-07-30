//! Present smoke: open a real window, render a handful of triangle
//! frames through the swapchain, and exit clean. `harness = false`
//! because the OS event loop must own the main thread.
//!
//! Skips (exit 0 with a SKIP line) when the environment cannot run it:
//! no display server, no Vulkan runtime, or no present support. CI
//! proves the offscreen path; this proves the glass path where glass
//! exists.

use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowError, WindowEvent, WindowRef, run_window_app,
};
use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, PipelineDesc, PresentOutcome, TargetError,
    Validation, WindowTarget, builtin,
};

const FRAMES_WANTED: u32 = 10;
/// Poll-loop iterations before declaring the run wedged.
const UPDATE_BUDGET: u32 = 20_000;

struct SmokeApp {
    device: Option<Device>,
    target: Option<WindowTarget>,
    pipeline: Option<renew_rhi::RenderPipeline>,
    size: Extent,
    frames: u32,
    updates: u32,
    skip: Option<String>,
    failure: Option<String>,
}

impl SmokeApp {
    fn new() -> Self {
        Self {
            device: None,
            target: None,
            pipeline: None,
            size: Extent {
                width: 0,
                height: 0,
            },
            frames: 0,
            updates: 0,
            skip: None,
            failure: None,
        }
    }

    fn done(&self) -> bool {
        self.skip.is_some() || self.failure.is_some() || self.frames >= FRAMES_WANTED
    }
}

impl WindowApp for SmokeApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let device = match Device::new(&DeviceDesc {
            app_name: "renew-present-smoke",
            validation: Validation::IfAvailable,
        }) {
            Ok(device) => device,
            Err(DeviceError::LoaderUnavailable { message }) => {
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
            Err(TargetError::PresentUnsupported { reason }) => {
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
                match target.render(clear, self.pipeline.as_ref()) {
                    Ok(PresentOutcome::Presented) => self.frames += 1,
                    Ok(PresentOutcome::NeedsResize) => {
                        if let Err(error) = target.resize(self.size) {
                            self.failure = Some(format!("resize failed: {error}"));
                        }
                    }
                    Err(error) => {
                        self.failure = Some(format!("render failed: {error}"));
                    }
                }
            }
            _ => {}
        }
    }

    fn update(&mut self, control: &mut LoopControl) {
        self.updates += 1;
        if self.updates > UPDATE_BUDGET && !self.done() {
            self.failure = Some(format!(
                "wedged: {} frames after {} updates",
                self.frames, self.updates
            ));
        }
        if self.done() {
            control.exit();
        } else {
            control.request_redraw();
        }
    }
}

fn main() {
    let mut app = SmokeApp::new();
    let config = WindowConfig {
        title: "renew present smoke".to_string(),
        logical_width: 640.0,
        logical_height: 360.0,
        resizable: true,
    };
    let run = run_window_app(&config, &mut app);
    // Tear down GPU objects BEFORE reading the report so validation
    // findings from destruction are counted in the verdict.
    drop(app.target.take());
    drop(app.pipeline.take());
    let report = app.device.as_ref().map(Device::validation_report);

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
    if let Some(report) = report
        && report.errors > 0
    {
        eprintln!(
            "FAIL: {} validation errors; first: {:?}",
            report.errors, report.first_messages
        );
        std::process::exit(1);
    }
    assert!(app.frames >= FRAMES_WANTED);
    // State the oracle in the output: "validation on" means the frames
    // above ran under the layer (with synchronization checking) and the
    // zero-error verdict is meaningful, not vacuous.
    let validation = app.device.as_ref().map(Device::validation_active);
    println!(
        "OK: presented {} frames (validation {})",
        app.frames,
        match validation {
            Some(true) => "on",
            _ => "OFF — zero-error verdict vacuous",
        }
    );
}
