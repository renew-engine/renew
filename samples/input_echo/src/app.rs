//! The windowed driver: real input, the same state machine.
//!
//! Two properties of the window seam decide the shape of everything
//! here, and a session that has not met them will "fix" them by
//! accident.
//!
//! - `update` runs *after* the event phase, so every event of an
//!   iteration is already in the world by the time the frame is planned.
//!   That is the order the scripted trace reproduces.
//! - `WindowApp::event` receives no loop control, so a close request
//!   cannot exit the loop where it arrives. It is latched in the world,
//!   and `update` acts on it. That is the seam's requirement, not a
//!   sample quirk.

use renew_frame::{FrameLoop, FrameStats, StepBudget, Timestamp, Timestep};
use renew_platform::Clock;
use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowError, WindowEvent, WindowRef, run_window_app,
};

use crate::cli::{Options, Report};
use crate::error::SampleError;
use crate::world::EchoWorld;

/// Open a window and echo what arrives in it.
///
/// # Errors
///
/// [`SampleError::Unavailable`] when there is no display server;
/// [`SampleError::Failed`] when the loop itself fails.
pub fn run(options: &Options) -> Result<Report, SampleError> {
    let mut app = EchoApp::new(options);
    let config = WindowConfig {
        title: "renew — input echo".to_string(),
        logical_width: 640.0,
        logical_height: 360.0,
        resizable: true,
    };
    let outcome = run_window_app(&config, &mut app);
    verdict(outcome, app.report())
}

/// One event, in one line a person can read.
///
/// The sample's visible half: run it, press things, watch them arrive.
#[must_use]
pub fn describe(event: WindowEvent) -> String {
    match event {
        WindowEvent::Key {
            code,
            pressed,
            repeat,
        } => {
            let action = if pressed { "down" } else { "up" };
            let repeated = if repeat { " (repeat)" } else { "" };
            format!("key {code:?} {action}{repeated}")
        }
        WindowEvent::PointerMoved { x, y } => format!("pointer {x:.0},{y:.0}"),
        WindowEvent::PointerButton { button, pressed } => {
            let action = if pressed { "down" } else { "up" };
            format!("button {button:?} {action}")
        }
        WindowEvent::Wheel { dx, dy } => format!("wheel {dx:.0},{dy:.0}"),
        WindowEvent::Focused(focused) => format!("focus {focused}"),
        WindowEvent::Resized { width, height } => format!("resized {width}x{height}"),
        WindowEvent::CloseRequested => "close requested".to_string(),
        // Everything this sample does not act on — a scale change, a
        // repaint request it has nothing to paint for, and whatever the
        // seam's vocabulary grows next — says what it was and no more.
        // One arm rather than one per variant, so a new event is shown
        // rather than swallowed.
        other => format!("{other:?}"),
    }
}

/// The sample as the window seam sees it.
pub struct EchoApp {
    clock: Clock,
    seed: u64,
    frames_wanted: u64,
    world: EchoWorld,
    stats: FrameStats,
    /// Anchored in `ready`: time spent bringing a window up is not time
    /// the simulation owes.
    frame: Option<FrameLoop>,
}

impl EchoApp {
    #[must_use]
    pub fn new(options: &Options) -> Self {
        Self {
            clock: Clock::start(),
            seed: options.seed,
            frames_wanted: options.frames,
            world: EchoWorld::new(options.seed),
            stats: FrameStats::new(),
            frame: None,
        }
    }

    /// Whether there is any reason left to keep the loop running.
    ///
    /// Bounded by simulation steps rather than loop iterations: nothing
    /// here presents, so nothing throttles the poll loop, and a run
    /// bounded by iterations would be over before a hand reached the
    /// keyboard. Six hundred steps is ten seconds at 60 Hz — and in
    /// headless mode, where every frame is exactly one step, the two
    /// readings are the same number.
    fn done(&self) -> bool {
        self.world.close_requested() || self.world.ticks() >= self.frames_wanted
    }

    /// What the run has to say for itself. The source is the window
    /// rather than a trace name, so a digest line never claims a
    /// reproducibility its input does not have.
    #[must_use]
    pub fn report(&self) -> Report {
        Report {
            seed: self.seed,
            source: "window",
            stats: self.stats,
            world: self.world,
        }
    }
}

/// Fold the two ways a windowed run can end into one answer.
///
/// A free function so both are exercised without a window — the loop
/// cannot be made to fail from inside a test.
fn verdict(outcome: Result<(), WindowError>, report: Report) -> Result<Report, SampleError> {
    match outcome {
        Ok(()) => Ok(report),
        Err(WindowError::LoopUnavailable { message }) => {
            Err(SampleError::Unavailable(format!("no window: {message}")))
        }
        Err(error) => Err(SampleError::failed("window loop", &error)),
    }
}

impl WindowApp for EchoApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let (width, height) = window.physical_size();
        self.world.event(WindowEvent::Resized { width, height });
        let now = Timestamp::from_nanos(self.clock.elapsed_nanos());
        self.frame = Some(FrameLoop::new(Timestep::HZ_60, StepBudget::DEFAULT, now));
    }

    fn event(&mut self, event: WindowEvent) {
        println!("{}", describe(event));
        self.world.event(event);
    }

    fn update(&mut self, control: &mut LoopControl) {
        // The one clock read on the path, and the whole of this method:
        // everything downstream is `update_at`, which is handed the
        // timestamp rather than reading one.
        let now = Timestamp::from_nanos(self.clock.elapsed_nanos());
        self.update_at(now, control);
    }
}

impl EchoApp {
    /// The update the seam asks for, as a pure function of the frame's
    /// timestamp: the same state machine the scripted trace drives.
    ///
    /// Split from [`WindowApp::update`] so tests can say what time it is.
    /// Driving it through the real clock made the number of simulation
    /// steps a fact about how fast the machine was — at poll speed,
    /// usually none — which left the step body exercised only by luck.
    fn update_at(&mut self, now: Timestamp, control: &mut LoopControl) {
        if let Some(frame) = &mut self.frame {
            let plan = frame.begin_frame(now);
            for step in plan.steps() {
                self.world.step(step);
            }
            self.stats.absorb(&plan);
        }
        if self.done() {
            control.exit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EchoApp, describe, verdict};
    use crate::cli::{Options, Report};
    use crate::error::SampleError;
    use crate::world::EchoWorld;
    use renew_frame::{FrameLoop, FrameStats, Nanos, Step, StepBudget, Timestamp, Timestep};
    use renew_platform::window::{
        KeyCode, LoopControl, PointerButton, WindowApp, WindowError, WindowEvent,
    };

    /// An app with a schedule but no window: every callback below is the
    /// one the seam calls, driven directly. Only `ready` is out of reach
    /// — it borrows a live OS window — and the windowed CI lane covers
    /// that.
    fn fresh(frames: u64) -> EchoApp {
        let mut app = EchoApp::new(&Options {
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

    #[test]
    fn an_update_plans_a_frame_whether_or_not_a_step_is_due() {
        let mut app = fresh(1_000);
        let mut control = LoopControl::default();
        // The anchor itself: a plan is made, but no step is due yet.
        app.update_at(Timestamp::from_nanos(0), &mut control);
        assert_eq!(app.world.ticks(), 0, "nothing is due at the anchor");
        // One timestep later exactly one step is due, so the frame count
        // and the step count move independently — which is the thing
        // this test is named for.
        app.update_at(ONE_STEP, &mut control);
        assert_eq!(app.stats.frames(), 2);
        assert_eq!(app.world.ticks(), 1);
        assert!(!app.done());
    }

    /// The seam's own callback, which reads the clock and hands the
    /// result to `update_at`. What time it finds is the machine's
    /// business, so this asserts only what holds at any speed.
    #[test]
    fn the_seam_callback_reads_the_clock_and_drives_the_update() {
        let mut app = fresh(1_000);
        let mut control = LoopControl::default();
        app.update(&mut control);
        assert_eq!(app.stats.frames(), 1);
        assert!(!app.done());
    }

    /// The run is bounded by simulation steps, not by loop iterations:
    /// with nothing presenting, iterations are free and a run counted in
    /// them would end before anyone could press a key.
    #[test]
    fn the_run_ends_when_the_simulation_has_advanced_as_far_as_it_was_asked() {
        let mut app = fresh(2);
        for tick in 0..2 {
            assert!(!app.done());
            app.world.step(Step {
                tick,
                dt: Nanos::from_nanos(16_666_667),
                sim_time: Nanos::from_nanos(tick * 16_666_667),
            });
        }
        assert!(app.done(), "two steps were asked for and two were run");
    }

    #[test]
    fn an_event_reaches_the_world_and_a_close_request_ends_the_run() {
        let mut app = fresh(1_000);
        app.event(WindowEvent::Key {
            code: KeyCode::ArrowRight,
            pressed: true,
            repeat: false,
        });
        // How far the key moves the world is the schedule's business and
        // a real clock's; that the event arrived is this one's.
        assert_eq!(app.report().world.keys(), (1, 0, 0));
        app.update_at(ONE_STEP, &mut LoopControl::default());
        assert!(!app.done());
        app.event(WindowEvent::CloseRequested);
        assert!(app.done(), "the close request is latched in the world");
    }

    #[test]
    fn an_update_before_the_window_is_ready_plans_nothing() {
        let mut app = EchoApp::new(&Options::default());
        app.update_at(ONE_STEP, &mut LoopControl::default());
        assert_eq!(app.stats.frames(), 0);
    }

    #[test]
    fn a_windowed_report_says_the_window_was_its_input() {
        let report = fresh(10).report();
        assert_eq!(report.source, "window");
        assert!(report.digest_line().contains("source=window"));
    }

    #[test]
    fn every_event_the_seam_can_deliver_prints_something_a_person_can_read() {
        let lines = [
            (
                describe(WindowEvent::Key {
                    code: KeyCode::Space,
                    pressed: true,
                    repeat: false,
                }),
                "key Space down",
            ),
            (
                describe(WindowEvent::Key {
                    code: KeyCode::Space,
                    pressed: false,
                    repeat: true,
                }),
                "key Space up (repeat)",
            ),
            (
                describe(WindowEvent::PointerMoved { x: 10.4, y: 20.6 }),
                "pointer 10,21",
            ),
            (
                describe(WindowEvent::PointerButton {
                    button: PointerButton::Left,
                    pressed: true,
                }),
                "button Left down",
            ),
            (
                describe(WindowEvent::PointerButton {
                    button: PointerButton::Right,
                    pressed: false,
                }),
                "button Right up",
            ),
            (
                describe(WindowEvent::Wheel { dx: 0.0, dy: 16.0 }),
                "wheel 0,16",
            ),
            (describe(WindowEvent::Focused(true)), "focus true"),
            (
                describe(WindowEvent::Resized {
                    width: 800,
                    height: 600,
                }),
                "resized 800x600",
            ),
            (describe(WindowEvent::CloseRequested), "close requested"),
            // The events the sample does not act on still say what they
            // were, through the one arm that catches all of them.
            (
                describe(WindowEvent::ScaleFactorChanged { scale: 2.0 }),
                "ScaleFactorChanged { scale: 2.0 }",
            ),
            (describe(WindowEvent::RedrawRequested), "RedrawRequested"),
        ];
        for (produced, expected) in lines {
            assert_eq!(produced, expected);
        }
    }

    #[test]
    fn the_verdict_tells_a_missing_display_from_a_failed_loop() {
        let report = Report {
            seed: 0,
            source: "window",
            stats: FrameStats::new(),
            world: EchoWorld::new(0),
        };
        assert!(verdict(Ok(()), report).is_ok());
        let unavailable = verdict(
            Err(WindowError::LoopUnavailable {
                message: "no display".to_string(),
            }),
            report,
        );
        assert!(matches!(unavailable, Err(SampleError::Unavailable(_))));
        let failed = verdict(
            Err(WindowError::Loop {
                message: "backend fell over".to_string(),
            }),
            report,
        );
        assert!(matches!(failed, Err(SampleError::Failed(_))), "{failed:?}");
    }
}
