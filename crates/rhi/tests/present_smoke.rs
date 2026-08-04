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
use std::rc::Rc;

use renew_rhi::{
    Attachment, ClearValue, Color, DepthState, Device, DeviceDesc, DeviceError, Extent, Item,
    LoadOp, Pass, PipelineDesc, PresentOutcome, RenderDesc, SamplerDesc, StoreOp, TargetError,
    TextureDesc, Validation, WindowTarget, builtin,
};

/// The one color attachment these frames render into: cleared, stored.
fn clear(color: Color) -> [Attachment; 1] {
    [Attachment::new(
        LoadOp::Clear(ClearValue::Color(color)),
        StoreOp::Store,
    )]
}

const FRAMES_WANTED: u32 = 10;
/// Poll-loop iterations before declaring the run wedged.
const UPDATE_BUDGET: u32 = 20_000;

struct SmokeApp {
    device: Option<Device>,
    target: Option<WindowTarget>,
    pipeline: Option<renew_rhi::RenderPipeline>,
    /// A depth-testing triangle for the frame's second pass, `None`
    /// when the adapter offers no depth format (depth-free presenting
    /// still runs; the device suite's probe is what makes the strict
    /// lane refuse such an adapter).
    depth_pipeline: Option<renew_rhi::RenderPipeline>,
    size: Extent,
    frames: u32,
    updates: u32,
    cycled: bool,
    /// Bitmask of the frame slots observed across the run.
    slots_seen: u32,
    skip: Option<String>,
    failure: Option<String>,
}

impl SmokeApp {
    fn new() -> Self {
        Self {
            device: None,
            target: None,
            pipeline: None,
            depth_pipeline: None,
            size: Extent {
                width: 0,
                height: 0,
            },
            frames: 0,
            updates: 0,
            cycled: false,
            slots_seen: 0,
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
        // `Required`, not `IfAvailable`: this is the only suite that
        // builds a `WindowTarget`, so it is the only place the present
        // path can be watched by the validation layer at all. Under
        // `IfAvailable` a lane with no layer installed still passed, and
        // every "no validation errors" claim about presenting was a claim
        // about nothing.
        //
        // A missing layer is a skip rather than a failure so a developer
        // without the SDK is not blocked -- and `RENEW_GOLDEN=1` turns
        // that skip back into a failure where the lane exists to run it,
        // which is what stops the skip being invisible. Same shape the
        // device suite already uses.
        let strict = std::env::var_os("RENEW_GOLDEN").is_some_and(|value| value == "1");
        let device = match Device::new(&DeviceDesc {
            app_name: "renew-present-smoke",
            validation: Validation::Required,
        }) {
            Ok(device) => device,
            Err(DeviceError::LoaderUnavailable { message }) => {
                self.skip = Some(format!("no Vulkan runtime: {message}"));
                return;
            }
            Err(DeviceError::ValidationUnavailable) if !strict => {
                self.skip = Some("validation layer not installed".to_string());
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
        // **Textured, so that the window record path actually binds a
        // descriptor set.** The bind is one method shared by both
        // targets, and until now only the offscreen target ever reached
        // its body — on this path it always took the early return, so a
        // bind recorded outside the render pass, or after the draw,
        // would have passed every check. Validation is `Required` here
        // and the run asserts zero errors, which is what makes that
        // reachable at all.
        //
        // The pipeline takes shared ownership of both, so neither needs
        // a field on this struct: the keep-alive is the point of the
        // design and letting them drop here exercises it.
        let texels: [u8; 16] = [
            10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
        ];
        let texture = match device.create_texture(&TextureDesc::new(
            Extent {
                width: 2,
                height: 2,
            },
            &texels,
        )) {
            Ok(texture) => Rc::new(texture),
            Err(error) => {
                self.failure = Some(format!("atlas upload failed: {error}"));
                return;
            }
        };
        let sampler = match device.create_sampler(&SamplerDesc::atlas()) {
            Ok(sampler) => Rc::new(sampler),
            Err(error) => {
                self.failure = Some(format!("sampler failed: {error}"));
                return;
            }
        };
        let pipeline = match device.create_pipeline(
            &PipelineDesc::new(builtin::TEXTURED, target.format()).texture(texture, sampler),
        ) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                self.failure = Some(format!("pipeline failed: {error}"));
                return;
            }
        };
        // The depth exercise's pipeline: every frame's second pass
        // carries the target's depth image, so a run past the in-flight
        // count renders both slots' depth images — the per-slot
        // first-use transitions and between-frame behavior execute
        // where sync validation (Required, above) can judge them.
        let depth_pipeline = if device.depth_format_name().is_some() {
            match device.create_pipeline(
                &PipelineDesc::new(builtin::TRIANGLE, target.format())
                    .depth_state(DepthState::read_write()),
            ) {
                Ok(pipeline) => Some(pipeline),
                Err(error) => {
                    self.failure = Some(format!("depth pipeline failed: {error}"));
                    return;
                }
            }
        } else {
            None
        };
        self.device = Some(device);
        self.target = Some(target);
        self.pipeline = Some(pipeline);
        self.depth_pipeline = depth_pipeline;
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
                let clear_color = Color::new(0.1, 0.2, 0.3, 1.0);
                if self.frames == 5 && !self.cycled {
                    // Dormant cycle mid-run: a zero-extent resize tears
                    // the swapchain down, a dormant render reports
                    // NeedsResize (and presents nothing), a real resize
                    // rebuilds — the minimize protocol, exercised on
                    // real glass.
                    self.cycled = true;
                    if let Err(error) = target.resize(Extent {
                        width: 0,
                        height: 0,
                    }) {
                        self.failure = Some(format!("dormant resize failed: {error}"));
                        return;
                    }
                    if target.extent()
                        != (Extent {
                            width: 0,
                            height: 0,
                        })
                    {
                        self.failure = Some("dormant target reports a size".to_string());
                        return;
                    }
                    let color = clear(clear_color);
                    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
                        Ok(PresentOutcome::NeedsResize) => {}
                        Ok(PresentOutcome::Presented) => {
                            self.failure = Some("dormant target presented".to_string());
                            return;
                        }
                        Err(error) => {
                            self.failure = Some(format!("dormant render failed: {error}"));
                            return;
                        }
                    }
                    if let Err(error) = target.resize(self.size) {
                        self.failure = Some(format!("rebuild resize failed: {error}"));
                        return;
                    }
                }
                let color = clear(clear_color);
                let items_storage;
                let items: &[Item<'_>] = match self.pipeline.as_ref() {
                    Some(pipeline) => {
                        items_storage = [Item::new(pipeline)];
                        &items_storage
                    }
                    None => &[],
                };
                // Two passes wherever a depth format exists: the
                // textured pass, then a pass that Loads its result and
                // draws the depth-tested triangle over it — multi-pass,
                // color Load, and the per-slot depth image, all on the
                // window path under Required validation.
                let load = [Attachment::new(LoadOp::Load, StoreOp::Store)];
                let depth_items_storage;
                let passes_two;
                let passes_one;
                let passes: &[Pass<'_>] = if let Some(depth_pipeline) = self.depth_pipeline.as_ref()
                {
                    depth_items_storage = [Item::new(depth_pipeline)];
                    passes_two = [
                        Pass::new(&color, items),
                        Pass::new(&load, &depth_items_storage).depth(Attachment::new(
                            LoadOp::Clear(ClearValue::Depth(1.0)),
                            StoreOp::Discard,
                        )),
                    ];
                    &passes_two
                } else {
                    passes_one = [Pass::new(&color, items)];
                    &passes_one
                };
                // The ring must actually cycle. A ring stuck on slot
                // zero is still *correct* -- every frame just waits its
                // own fence and the pipeline serialises -- so no other
                // assertion here can tell the difference. Record which
                // slots were used and check at the end that more than
                // one was.
                self.slots_seen |= 1u32 << target.frame_slot();
                match target.render(&RenderDesc::new(passes)) {
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
    drop(app.depth_pipeline.take());
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
    assert!(
        app.slots_seen.count_ones() > 1,
        "the frame ring never advanced past one slot ({:#b}); frames pipeline correctly but          serially, which no other assertion here can detect",
        app.slots_seen
    );
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
