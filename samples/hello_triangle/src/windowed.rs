//! The windowed driver: the OS owns the loop, the sample owns the rest.
//!
//! Two properties of the window seam decide the shape of everything
//! here, and a session that has not met them will "fix" them by
//! accident.
//!
//! - `update` runs *after* the event phase, so a redraw requested in
//!   iteration N arrives in iteration N+1. The render therefore lags the
//!   step phase by one iteration and draws with the alpha stored then.
//!   Harmless — alpha is a hint, and an OS repaint with no intervening
//!   update correctly re-renders at the same alpha — but it has to be
//!   said out loud.
//! - `WindowApp::event` receives no loop control, so a close request
//!   cannot exit the loop where it arrives. It is latched, and `update`
//!   acts on it. That is the seam's requirement, not a sample quirk.

use renew_frame::{FrameLoop, FrameStats, FrameTiming, Nanos, StepBudget, Timestamp, Timestep};
use renew_math::Alpha;
use renew_platform::Clock;
use renew_platform::window::{
    LoopControl, NativeWindow, WindowApp, WindowConfig, WindowError, WindowEvent, WindowRef,
    run_window_app,
};
use renew_rhi::{
    Device, DeviceDesc, Extent, Item, Pass, PipelineDesc, RenderDesc, RenderPipeline, TargetError,
    builtin,
};

use crate::cli::{Options, Report};
use crate::error::{SampleError, device_error, pipeline_error, render_error, target_error};
use crate::readout::{self, Readout};
use crate::render::{Surface, clear_attachment, clear_color};
use crate::world::World;

/// What the window is called before any measurement exists, and the text
/// every frame-time reading is appended to.
const TITLE: &str = "renew — hello triangle";

/// How long a run may go without presenting anything before it is
/// declared wedged.
///
/// Measured against the last frame that reached the screen rather than
/// against the whole run: a long run is not a stuck one, and a count of
/// poll iterations would mean five seconds on one machine and ten
/// milliseconds on another, because nothing here sleeps. An unattended
/// run must always end, and it must end by saying what went wrong.
const WEDGE_AFTER: Nanos = Nanos::from_nanos(5_000_000_000);

/// Open a window and run the sample in it.
///
/// # Errors
///
/// [`SampleError::Unavailable`] when there is no display server or no
/// GPU runtime; [`SampleError::Failed`] for anything that should have
/// worked.
pub fn run(options: &Options) -> Result<Report, SampleError> {
    let mut app = TriangleApp::new(options);
    let config = WindowConfig {
        title: TITLE.to_string(),
        logical_width: 640.0,
        logical_height: 360.0,
        resizable: true,
    };
    let outcome = run_window_app(&config, &mut app);
    app.finish(outcome)
}

/// The sample as the window seam sees it.
///
/// The renderer's objects are `Option`s because the app is constructed
/// before a window exists: `ready` is the only place a live window is
/// available, so it is the only place they can be built. That is the
/// price of the seam's inverted control, and it is paid here in a
/// sample rather than anywhere in the engine.
/// The frame after which the swapchain is rebuilt once. Early enough
/// that even a short run reaches it, and not the first frame, so the
/// rebuild happens against a chain that has already presented.
const REBUILD_AFTER_FRAME: u64 = 2;

pub struct TriangleApp {
    clock: Clock,
    seed: u64,
    frames_wanted: u64,
    world: World,
    stats: FrameStats,
    timing: FrameTiming,
    /// The on-screen half of the frame-time capture: the window title,
    /// because the engine renders no text yet. Folds every frame's cost
    /// in and answers with a new title four times a second.
    readout: Readout,
    /// Anchored in `ready`, after bring-up.
    frame: Option<FrameLoop>,
    /// A second handle on the same OS window, kept so the title can be
    /// relabelled. The renderer owns the one it was given; this one
    /// costs a reference count and keeps the window seam the only place
    /// that knows what a window is.
    window: Option<NativeWindow>,
    device: Option<Device>,
    surface: Option<Surface>,
    pipeline: Option<RenderPipeline>,
    size: Extent,
    /// The last plan's interpolation factor, consumed by the draw that
    /// follows it.
    alpha: Alpha,
    presented: u64,
    /// Latched here, acted on in `update`.
    close_requested: bool,
    /// Start of the current frame, for the measured CPU cost.
    last_update: Timestamp,
    /// When a frame last reached the screen: the run's progress, and
    /// what the stall check measures against.
    last_progress: Timestamp,
    /// How long the run may stall before it is declared wedged. A field
    /// rather than the constant so the stall path is exercised without
    /// a five-second test.
    wedge_after: Nanos,
    drawn_since_update: bool,
    skip: Option<String>,
    failure: Option<String>,
}

impl TriangleApp {
    #[must_use]
    pub fn new(options: &Options) -> Self {
        Self {
            clock: Clock::start(),
            seed: options.seed,
            frames_wanted: options.frames,
            world: World::new(options.seed),
            stats: FrameStats::new(),
            timing: FrameTiming::new(),
            readout: Readout::new(TITLE, readout::INTERVAL),
            frame: None,
            window: None,
            device: None,
            surface: None,
            pipeline: None,
            size: Extent {
                width: 0,
                height: 0,
            },
            alpha: Alpha::ZERO,
            presented: 0,
            close_requested: false,
            last_update: Timestamp::from_nanos(0),
            last_progress: Timestamp::from_nanos(0),
            wedge_after: WEDGE_AFTER,
            drawn_since_update: false,
            skip: None,
            failure: None,
        }
    }

    /// Everything the renderer needs, built against the live window.
    fn bring_up(&mut self, window: NativeWindow, size: Extent) -> Result<(), SampleError> {
        let device = Device::new(&DeviceDesc {
            app_name: "renew-hello-triangle",
            validation: crate::validation_policy(),
        })
        .map_err(device_error)?;
        let target = device
            .create_window_target(window, size)
            .map_err(target_error)?;
        let surface = Surface::Window(target);
        let pipeline = device
            .create_pipeline(&PipelineDesc::new(builtin::TRIANGLE, surface.format()))
            .map_err(pipeline_error)?;
        self.device = Some(device);
        self.surface = Some(surface);
        self.pipeline = Some(pipeline);
        Ok(())
    }

    /// Record an outcome: an environment that cannot host this run is a
    /// skip, anything else is a failure. One place, so the two verdicts
    /// can never drift apart.
    fn record(&mut self, outcome: Result<(), SampleError>) {
        match outcome {
            Ok(()) => {}
            Err(SampleError::Unavailable(reason)) => self.skip = Some(reason),
            Err(error) => self.failure = Some(error.to_string()),
        }
    }

    /// Draw the world as it stands, interpolated by the stored alpha.
    fn draw(&mut self) {
        // Surface and pipeline are built together in `bring_up`, so
        // either both exist or neither does — one pattern says so.
        let (Some(surface), Some(pipeline)) = (&mut self.surface, self.pipeline.as_ref()) else {
            return;
        };
        let clear = clear_color(&self.world, self.alpha);
        // The frame, composed on this stack; the borrows end at the
        // render call.
        let color = [clear_attachment(clear)];
        let items = [Item::new(pipeline)];
        let passes = [Pass::new(&color, &items)];
        let outcome = surface.render(&RenderDesc::new(&passes));
        self.record_draw(outcome);
    }

    /// What a draw outcome means for the run.
    ///
    /// A dormant window is not an error and not a reason to stop
    /// stepping: the simulation keeps its own time, and the frame is
    /// counted as skipped so it cannot inflate the frame-time summary.
    fn record_draw(&mut self, outcome: Result<bool, TargetError>) {
        match outcome {
            Ok(true) => {
                self.presented = self.presented.saturating_add(1);
                self.drawn_since_update = true;
                // Rebuild the chain once, mid-run, at the size it
                // already has. A sample exists to exercise the engine
                // end to end, and a swapchain that is only ever built
                // once has not been shown to survive being rebuilt
                // under a live surface — the thing that actually
                // happens every time a user drags a window edge. Doing
                // it deliberately also keeps this path covered wherever
                // the sample runs, rather than only on a desktop with a
                // window manager to send the event: a virtual display
                // has none, and a path that is exercised on one machine
                // and not another is a path nobody is really watching.
                if self.presented == REBUILD_AFTER_FRAME {
                    let size = self.size;
                    self.resize(size);
                }
            }
            Ok(false) => {
                let size = self.size;
                self.resize(size);
            }
            Err(error) => self.record(Err(render_error(error))),
        }
    }

    /// Follow the window's size. The renderer tears the swapchain down
    /// and rebuilds it; the loop never learns the word swapchain.
    fn resize(&mut self, size: Extent) {
        self.size = size;
        if let Some(Surface::Window(target)) = &mut self.surface {
            let outcome = target.resize(size).map_err(target_error);
            self.record(outcome);
        }
    }

    /// Whether there is any reason left to keep the loop running.
    fn done(&self) -> bool {
        self.skip.is_some()
            || self.failure.is_some()
            || self.close_requested
            || self.presented >= self.frames_wanted
    }

    fn report(&self) -> Report {
        Report {
            seed: self.seed,
            stats: self.stats,
            timing: self.timing,
            state_hash: self.world.state_hash(),
        }
    }

    /// The run's verdict, after the loop has returned.
    fn finish(mut self, outcome: Result<(), WindowError>) -> Result<Report, SampleError> {
        // Teardown before the verdict: the renderer's objects go away
        // while the device is still here to destroy them cleanly.
        drop(self.surface.take());
        drop(self.pipeline.take());
        drop(self.device.take());
        verdict(
            outcome,
            self.skip.take(),
            self.failure.take(),
            self.report(),
        )
    }
}

/// Whether a run that is not finished has stopped making progress.
///
/// A free function so the stall verdict is exercised directly: the state
/// that produces it needs a window no test has.
const fn wedged(since_progress: Nanos, budget: Nanos, done: bool) -> bool {
    !done && since_progress.get() > budget.get()
}

/// Fold the three ways a windowed run can end into one answer: the loop
/// itself failed, a callback recorded something, or the report stands.
///
/// A free function so every combination is exercised without a window —
/// the callbacks that produce those states cannot be driven without one.
fn verdict(
    outcome: Result<(), WindowError>,
    skip: Option<String>,
    failure: Option<String>,
    report: Report,
) -> Result<Report, SampleError> {
    match outcome {
        Err(WindowError::LoopUnavailable { message }) => {
            return Err(SampleError::Unavailable(format!("no window: {message}")));
        }
        Err(error) => return Err(SampleError::failed("window loop", &error)),
        Ok(()) => {}
    }
    if let Some(reason) = skip {
        return Err(SampleError::Unavailable(reason));
    }
    if let Some(message) = failure {
        return Err(SampleError::Failed(message));
    }
    Ok(report)
}

impl WindowApp for TriangleApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let (width, height) = window.physical_size();
        self.size = Extent { width, height };
        // Two handles on one window: the renderer consumes one for its
        // surface, and the readout keeps the other. Cloning is a
        // reference count, and the window outlives both by construction.
        let native = window.native();
        self.window = Some(native.clone());
        let outcome = self.bring_up(native, self.size);
        self.record(outcome);
        // Anchor AFTER bring-up: device creation costs on the order of
        // a hundred milliseconds, and banking it as frame one would open
        // the run with a clamped burst.
        let now = Timestamp::from_nanos(self.clock.elapsed_nanos());
        self.frame = Some(FrameLoop::new(Timestep::HZ_60, StepBudget::DEFAULT, now));
        self.last_update = now;
    }

    fn event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized { width, height } => self.resize(Extent { width, height }),
            WindowEvent::CloseRequested => self.close_requested = true,
            _ => {}
        }
    }

    fn update(&mut self, control: &mut LoopControl) {
        // The one clock read on the path, and the whole of this method:
        // everything downstream is `update_at`, which is handed the
        // timestamp rather than reading one.
        let now = Timestamp::from_nanos(self.clock.elapsed_nanos());
        self.update_at(now, control);
    }
}

impl TriangleApp {
    /// The update the seam asks for, as a pure function of the frame's
    /// timestamp: the same state machine the headless run drives.
    ///
    /// Split from [`WindowApp::update`] so tests can say what time it is.
    /// Driving it through the real clock made the number of simulation
    /// steps a fact about how fast the machine was — usually none, since
    /// a test reaches here microseconds after the schedule is anchored —
    /// which left the step body exercised only by luck.
    fn update_at(&mut self, now: Timestamp, control: &mut LoopControl) {
        if let Some(frame) = &mut self.frame {
            let plan = frame.begin_frame(now);
            for step in plan.steps() {
                self.world.step(step);
            }
            self.alpha = Alpha::new(plan.remainder().get(), plan.timestep().nanos());
            self.stats.absorb(&plan);
            // The measured cost of one loop iteration. Nothing sleeps
            // here (the seam polls), so the interval between updates is
            // what the frame cost.
            let cpu = now.saturating_since(self.last_update);
            self.timing.record(cpu, self.drawn_since_update);
            // The same number, on the window instead of in the summary
            // — deliberately the same one, over the same population of
            // frames (drawn and skipped alike), so the title and
            // `--dump-stats` can never describe different things. The
            // readout decides when a relabel is due from `now`, so the
            // sample never reads a second clock for it, and the OS call
            // happens four times a second rather than every frame.
            if let Some(title) = self.readout.record(cpu, now)
                && let Some(window) = &self.window
            {
                window.set_title(title);
            }
            if self.drawn_since_update {
                self.last_progress = now;
            }
            self.drawn_since_update = false;
        }
        self.last_update = now;
        let stalled = now.saturating_since(self.last_progress);
        if wedged(stalled, self.wedge_after, self.done()) {
            self.failure = Some(format!(
                "wedged: {} of {} frames presented, and none in the last {} ms",
                self.presented,
                self.frames_wanted,
                stalled.get() / 1_000_000
            ));
        }
        if self.done() {
            control.exit();
        } else {
            control.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Nanos, TriangleApp, WEDGE_AFTER, verdict, wedged};
    use crate::cli::{Options, Report};
    use crate::error::SampleError;
    use renew_frame::{FrameLoop, FrameStats, FrameTiming, StepBudget, Timestamp, Timestep};
    use renew_platform::window::{LoopControl, WindowApp, WindowError, WindowEvent};
    use renew_rhi::TargetError;

    /// An app with a schedule but no window: every callback below is the
    /// one the seam calls, driven directly. Only `ready` is out of reach
    /// — it borrows a live OS window — and the windowed CI lane covers
    /// that.
    fn fresh(frames: u64) -> TriangleApp {
        let mut app = TriangleApp::new(&Options {
            frames,
            ..Options::default()
        });
        app.frame = Some(FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(0),
        ));
        app
    }

    /// Exactly one 60 Hz step past the anchor `fresh` sets, so an update
    /// from here has exactly one step to run however fast the machine is.
    /// Derived from the timestep rather than written out, so changing the
    /// rate cannot silently turn this back into zero steps.
    const ONE_STEP: Timestamp = Timestamp::from_nanos(Timestep::HZ_60.nanos().get());

    fn report() -> Report {
        Report {
            seed: 0,
            stats: FrameStats::new(),
            timing: FrameTiming::new(),
            state_hash: 0,
        }
    }

    #[test]
    fn an_update_plans_a_frame_and_asks_for_the_redraw_that_draws_it() {
        let mut app = fresh(10);
        let mut control = LoopControl::default();
        app.update_at(ONE_STEP, &mut control);
        assert_eq!(app.stats.frames(), 1);
        // A step was due, so the world advanced by exactly one. Asserted
        // because the alternative — a plan with no steps — also counts a
        // frame, and would leave the step body untested.
        assert_eq!(app.world.ticks(), 1);
        // Nothing has presented yet, so the loop must keep going.
        assert!(!app.done());
    }

    /// The seam's own callback, which reads the clock and hands the
    /// result to `update_at`. What time it finds is the machine's
    /// business, so this asserts only what holds at any speed.
    #[test]
    fn the_seam_callback_reads_the_clock_and_drives_the_update() {
        let mut app = fresh(10);
        let mut control = LoopControl::default();
        app.update(&mut control);
        assert_eq!(app.stats.frames(), 1);
        assert!(!app.done());
    }

    #[test]
    fn an_update_before_the_window_is_ready_plans_nothing() {
        // `ready` is where the schedule is anchored, and the seam can
        // call `update` before it: with no schedule there is no frame to
        // plan, and nothing to record about one.
        let mut app = TriangleApp::new(&Options::default());
        app.update_at(ONE_STEP, &mut LoopControl::default());
        assert_eq!(app.stats.frames(), 0);
        assert!(app.failure.is_none() && app.skip.is_none());
    }

    #[test]
    fn a_close_request_is_latched_and_ends_the_run_at_the_next_update() {
        let mut app = fresh(1_000);
        app.event(WindowEvent::CloseRequested);
        assert!(app.close_requested, "the request must be latched");
        assert!(app.done());
        // Events with no meaning for this sample are dropped, and they
        // do not end anything.
        let mut quiet = fresh(1_000);
        quiet.event(WindowEvent::Focused(true));
        assert!(!quiet.done());
    }

    #[test]
    fn a_resize_is_remembered_even_before_a_surface_exists() {
        let mut app = fresh(10);
        app.event(WindowEvent::Resized {
            width: 800,
            height: 600,
        });
        assert_eq!(app.size.width, 800);
        assert_eq!(app.size.height, 600);
        assert!(app.failure.is_none());
    }

    #[test]
    fn a_redraw_with_no_surface_does_nothing_at_all() {
        let mut app = fresh(10);
        app.event(WindowEvent::RedrawRequested);
        assert_eq!(app.presented, 0);
        assert!(app.failure.is_none() && app.skip.is_none());
    }

    #[test]
    fn a_presented_frame_counts_and_a_dormant_one_does_not() {
        let mut app = fresh(2);
        app.record_draw(Ok(true));
        assert_eq!(app.presented, 1);
        assert!(app.drawn_since_update);
        // A dormant window presented nothing: no frame counted, no
        // failure recorded, and the size it will rebuild at is kept.
        app.record_draw(Ok(false));
        assert_eq!(app.presented, 1);
        assert!(app.failure.is_none());
        // A real render failure ends the run.
        app.record_draw(Err(TargetError::DeviceLost));
        assert!(app.failure.is_some());
        assert!(app.done());
    }

    #[test]
    fn an_unavailable_environment_is_a_skip_and_anything_else_is_a_failure() {
        let mut app = fresh(10);
        app.record(Ok(()));
        assert!(app.skip.is_none() && app.failure.is_none());
        app.record(Err(SampleError::Unavailable("no adapter".to_string())));
        assert_eq!(app.skip.as_deref(), Some("no adapter"));
        app.record(Err(SampleError::Failed("boom".to_string())));
        assert_eq!(app.failure.as_deref(), Some("boom"));
    }

    #[test]
    fn a_finished_run_is_never_wedged_and_a_stalled_one_always_is() {
        assert!(!wedged(Nanos::from_nanos(0), WEDGE_AFTER, false));
        assert!(!wedged(Nanos::from_nanos(u64::MAX), WEDGE_AFTER, true));
        assert!(wedged(Nanos::from_nanos(u64::MAX), WEDGE_AFTER, false));
    }

    #[test]
    fn a_run_that_never_presents_is_declared_wedged_rather_than_left_spinning() {
        let mut app = fresh(1);
        let mut control = LoopControl::default();
        app.update_at(ONE_STEP, &mut control);
        assert!(app.failure.is_none(), "a fresh run has not stalled");
        // Nothing has ever reached the screen, and now the budget is
        // spent: the run ends, saying so, instead of spinning forever.
        app.wedge_after = Nanos::ZERO;
        app.update_at(ONE_STEP, &mut control);
        let failure = app.failure.as_deref().unwrap_or_default();
        assert!(failure.starts_with("wedged: 0 of 1 frames"), "{failure}");
        assert!(app.done());
    }

    /// A presented frame is what progress means: it moves the stall
    /// deadline, and nothing else does.
    #[test]
    fn a_presented_frame_moves_the_stall_deadline() {
        let mut app = fresh(1_000);
        app.record_draw(Ok(true));
        app.update_at(ONE_STEP, &mut LoopControl::default());
        // The exact timestamp of the update that saw the presented
        // frame, rather than merely "later than the anchor": with the
        // clock out of the way the deadline is a value, not a bound.
        assert_eq!(app.last_progress, ONE_STEP);
        assert!(app.failure.is_none());
    }

    #[test]
    fn asking_for_no_frames_ends_the_run_immediately() {
        let app = fresh(0);
        assert!(app.done());
    }

    #[test]
    fn the_verdict_puts_the_loops_own_failure_first() {
        let unavailable = verdict(
            Err(WindowError::LoopUnavailable {
                message: "no display".to_string(),
            }),
            None,
            None,
            report(),
        );
        assert!(matches!(unavailable, Err(SampleError::Unavailable(_))));

        let failed = verdict(
            Err(WindowError::Loop {
                message: "backend fell over".to_string(),
            }),
            // Even with a callback skip recorded, the loop's own failure
            // is the one worth reporting.
            Some("no adapter".to_string()),
            None,
            report(),
        );
        assert!(matches!(failed, Err(SampleError::Failed(_))), "{failed:?}");
    }

    #[test]
    fn a_callback_skip_beats_a_callback_failure_and_both_beat_the_report() {
        let skipped = verdict(Ok(()), Some("no adapter".to_string()), None, report());
        assert!(matches!(skipped, Err(SampleError::Unavailable(_))));
        let failed = verdict(Ok(()), None, Some("boom".to_string()), report());
        assert!(matches!(failed, Err(SampleError::Failed(_))));
        let clean = verdict(Ok(()), None, None, report());
        assert!(clean.is_ok());
    }

    #[test]
    fn tearing_down_without_a_window_still_produces_the_report() {
        let mut app = fresh(10);
        let mut control = LoopControl::default();
        app.update_at(ONE_STEP, &mut control);
        let report = app.finish(Ok(())).expect("a run that recorded nothing");
        assert_eq!(report.stats.frames(), 1);
    }
}
