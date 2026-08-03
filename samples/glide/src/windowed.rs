//! The game in a window: the same world, a real clock, a key and a
//! button, the score in the title.
//!
//! The shape is the delegation shell both siblings use — every seam
//! callback is a thin forward to a plain function tests drive with no
//! window; only `ready` needs a live OS window, and the windowed CI
//! lane covers it. The one place the scripted loop is deliberately NOT
//! copied is input: a wall clock plans zero, one, or many steps per
//! frame, so flap edges ride a saturating counter consumed one per
//! step — the closest a real clock gets to the scripted loop's
//! one-edge-one-tick.

use renew_event::WindowEvent;
use renew_frame::{FrameLoop, FrameStats, Nanos, StepBudget, Timestamp, Timestep};
use renew_input::InputMap;
use renew_platform::Clock;
use renew_platform::window::{
    LoopControl, NativeWindow, WindowApp, WindowConfig, WindowError, WindowRef, run_window_app,
};
use renew_render2d::{AtlasDesc, Canvas, Region, Sprite, SpriteRenderer};
use renew_rhi::{Color, Device, DeviceDesc, Extent, PresentOutcome, Validation, WindowTarget};
use renew_sample_glide_world::{Action, VIEW_HEIGHT, VIEW_WIDTH, World};

use crate::cli::{Options, Report};
use crate::scene::{SceneSprite, Tile, scene};
use crate::{SampleError, scripted};

/// The window's base title; the score readout appends to it.
const TITLE: &str = "renew — glide";

/// The sky, the bird, the pipes: the goldens' test-card look, which is
/// the game's look until real art arrives.
const SKY: Color = Color {
    r: 51.0 / 255.0,
    g: 102.0 / 255.0,
    b: 153.0 / 255.0,
    a: 1.0,
};
const BIRD_TEXEL: [u8; 4] = [255, 208, 0, 255];
const PIPE_TEXEL: [u8; 4] = [0, 160, 40, 255];
const ATLAS_EXTENT: Extent = Extent {
    width: 4,
    height: 2,
};
const BIRD_REGION: Region = Region {
    x: 0,
    y: 0,
    width: 2,
    height: 2,
};
const PIPE_REGION: Region = Region {
    x: 2,
    y: 0,
    width: 2,
    height: 2,
};

/// How long the run may present nothing before it is declared wedged.
/// Time-based, not update-counted: the poll loop's iteration rate is a
/// fact about the machine. A field on the app holds it so tests reach
/// the path without waiting real seconds. Second copy of this shape;
/// the third is the cue to extract it.
const WEDGE_AFTER: Nanos = Nanos::from_nanos(5_000_000_000);

/// The presented frame after which the swapchain is rebuilt once, so
/// the rebuild-under-a-live-surface path runs everywhere and not only
/// where a window manager sends resizes.
const REBUILD_AFTER_FRAME: u64 = 2;

/// Sprite capacity: worst case is five pipes as two bars each plus the
/// bird; headroom beyond that.
const SPRITE_CAPACITY: u32 = 32;

/// Open the window and play until closed, wedged, or the tick bound.
pub fn run(options: &Options) -> Result<Report, SampleError> {
    let mut app = GlideApp::new(options);
    let config = WindowConfig {
        title: TITLE.to_string(),
        logical_width: 640.0,
        logical_height: 480.0,
        resizable: true,
    };
    let outcome = run_window_app(&config, &mut app);
    app.finish(outcome)
}

/// The fixed-capacity title buffer: format into it, borrow it, never
/// touch the heap. Score digits and the terminal suffix fit in a
/// fraction of this.
struct Title {
    bytes: [u8; 64],
    length: usize,
}

impl Title {
    fn new() -> Self {
        Self {
            bytes: [0; 64],
            length: 0,
        }
    }

    /// The title for a score, with the game-over suffix once dead.
    /// Returns `None` when the buffer would overflow — unreachable at
    /// this capacity, kept so the refusal is a branch and not a panic.
    fn compose(&mut self, score: u64, alive: bool) -> Option<&str> {
        use core::fmt::Write as _;
        struct Sink<'a> {
            bytes: &'a mut [u8; 64],
            length: &'a mut usize,
        }
        impl core::fmt::Write for Sink<'_> {
            fn write_str(&mut self, text: &str) -> core::fmt::Result {
                let end = self
                    .length
                    .checked_add(text.len())
                    .ok_or(core::fmt::Error)?;
                if end > self.bytes.len() {
                    return Err(core::fmt::Error);
                }
                self.bytes[*self.length..end].copy_from_slice(text.as_bytes());
                *self.length = end;
                Ok(())
            }
        }
        self.length = 0;
        let mut sink = Sink {
            bytes: &mut self.bytes,
            length: &mut self.length,
        };
        let suffix = if alive { "" } else { " — over" };
        write!(sink, "{TITLE} — score {score}{suffix}").ok()?;
        core::str::from_utf8(&self.bytes[..self.length]).ok()
    }
}

/// The game as the window seam sees it.
pub struct GlideApp {
    clock: Clock,
    seed: u64,
    /// `None` = play until closed; `Some(n)` = stop after `n` ticks.
    ticks_wanted: Option<u64>,
    world: World,
    input: InputMap<Action>,
    /// Flap edges not yet consumed by a step. A saturating counter, not
    /// a bit: a press on a zero-step frame plus a press on a catch-up
    /// frame are two flaps with two step intervals to land in, and a
    /// bit would fold them. Only Flap is latched; a second action must
    /// extend this or lose its edges.
    pending_flaps: u8,
    stats: FrameStats,
    scene_scratch: Vec<SceneSprite>,
    title: Title,
    /// The score last written into the title, so relabeling happens on
    /// change only. Death changes the suffix, tracked beside it.
    titled: Option<(u64, bool)>,
    frame: Option<FrameLoop>,
    window: Option<NativeWindow>,
    device: Option<Device>,
    target: Option<WindowTarget>,
    renderer: Option<SpriteRenderer>,
    size: Extent,
    presented: u64,
    close_requested: bool,
    last_progress: Timestamp,
    /// A field rather than the constant so tests reach the wedge path
    /// without waiting real seconds.
    wedge_after: Nanos,
    drawn_since_update: bool,
    failure: Option<SampleError>,
}

impl GlideApp {
    #[must_use]
    pub fn new(options: &Options) -> Self {
        Self {
            clock: Clock::start(),
            seed: options.seed,
            ticks_wanted: options.window_ticks,
            world: World::new(options.seed),
            input: scripted::input_map(),
            pending_flaps: 0,
            stats: FrameStats::new(),
            scene_scratch: Vec::new(),
            title: Title::new(),
            titled: None,
            frame: None,
            window: None,
            device: None,
            target: None,
            renderer: None,
            size: Extent {
                width: 1,
                height: 1,
            },
            presented: 0,
            close_requested: false,
            last_progress: Timestamp::from_nanos(0),
            wedge_after: WEDGE_AFTER,
            drawn_since_update: false,
            failure: None,
        }
    }

    fn atlas_bytes() -> [u8; 32] {
        let mut bytes = [0u8; 32];
        for (index, chunk) in bytes.chunks_exact_mut(4).enumerate() {
            let texel = if index % 4 < 2 {
                BIRD_TEXEL
            } else {
                PIPE_TEXEL
            };
            chunk.copy_from_slice(&texel);
        }
        bytes
    }

    fn bring_up(&mut self, window: NativeWindow, size: Extent) -> Result<(), SampleError> {
        let device = Device::new(&DeviceDesc {
            app_name: "renew-glide",
            validation: Validation::Off,
        })
        .map_err(|error| SampleError::failed("bringing up the device", &error))?;
        let target = device
            .create_window_target(window, size)
            .map_err(|error| SampleError::failed("creating the window target", &error))?;
        let atlas = Self::atlas_bytes();
        let canvas = Canvas::new(VIEW_WIDTH, VIEW_HEIGHT)
            .ok_or_else(|| SampleError::Failed("the view has zero size".to_string()))?;
        let capacity = core::num::NonZeroU32::new(SPRITE_CAPACITY)
            .ok_or_else(|| SampleError::Failed("zero sprite capacity".to_string()))?;
        let renderer = SpriteRenderer::new(
            &device,
            &AtlasDesc::new(ATLAS_EXTENT, &atlas),
            canvas,
            target.format(),
            capacity,
        )
        .map_err(|error| SampleError::failed("building the sprite renderer", &error))?;
        self.device = Some(device);
        self.target = Some(target);
        self.renderer = Some(renderer);
        Ok(())
    }

    /// Draw the world as it stands.
    fn draw(&mut self) {
        let (Some(target), Some(renderer)) = (&mut self.target, &mut self.renderer) else {
            return;
        };
        scene(&self.world, &mut self.scene_scratch);
        renderer.begin();
        for sprite in &self.scene_scratch {
            let region = match sprite.tile {
                Tile::Bird => BIRD_REGION,
                Tile::Pipe => PIPE_REGION,
            };
            renderer
                .push(&Sprite::new(region, sprite.x, sprite.y).size(sprite.width, sprite.height));
        }
        let outcome = target.render(&renderer.desc(SKY));
        self.record_draw(outcome);
    }

    /// What a draw outcome means for the run — split from [`Self::draw`]
    /// so tests drive every arm with constructed outcomes, no window.
    fn record_draw(&mut self, outcome: Result<PresentOutcome, renew_rhi::TargetError>) {
        match outcome {
            Ok(PresentOutcome::Presented) => {
                self.presented = self.presented.saturating_add(1);
                self.drawn_since_update = true;
                if self.presented == REBUILD_AFTER_FRAME {
                    let size = self.size;
                    self.resize(size);
                }
            }
            Ok(PresentOutcome::NeedsResize) => {
                let size = self.size;
                self.resize(size);
            }
            Err(error) => {
                self.failure = Some(SampleError::failed("rendering", &error));
            }
        }
    }

    /// Follow the window's size; the renderer never learns the word
    /// swapchain.
    fn resize(&mut self, size: Extent) {
        self.size = size;
        if let Some(target) = &mut self.target
            && let Err(error) = target.resize(size)
        {
            self.failure = Some(SampleError::failed("resizing", &error));
        }
    }

    /// Whether any reason to keep looping remains.
    fn done(&self) -> bool {
        self.failure.is_some()
            || self.close_requested
            || self
                .ticks_wanted
                .is_some_and(|bound| self.world.tick() >= bound)
    }

    /// The update the seam asks for, as a pure function of the frame's
    /// timestamp — the testable core.
    fn update_at(&mut self, now: Timestamp, control: &mut LoopControl) {
        if let Some(frame) = &mut self.frame {
            let plan = frame.begin_frame(now);
            // Capture the edge before it retires, count it, spend one
            // per step: a press on a zero-step frame survives to the
            // next frame's first step, and two presses with two due
            // steps deliver two flaps.
            self.pending_flaps = self
                .pending_flaps
                .saturating_add(u8::from(self.input.state(Action::Flap).just_pressed));
            for _step in plan.steps() {
                let bound = self.ticks_wanted;
                if bound.is_some_and(|bound| self.world.tick() >= bound) {
                    break;
                }
                self.world.step(self.pending_flaps > 0);
                self.pending_flaps = self.pending_flaps.saturating_sub(1);
            }
            self.input.advance();
            self.stats.absorb(&plan);
            self.relabel();
            if self.drawn_since_update {
                self.last_progress = now;
            }
            self.drawn_since_update = false;
        }
        let stalled = now.saturating_since(self.last_progress);
        if self.failure.is_none() && stalled >= self.wedge_after && !self.done() {
            self.failure = Some(SampleError::Failed(format!(
                "wedged: {} frames presented, none in the last {} ms",
                self.presented,
                stalled.get() / 1_000_000
            )));
        }
        if self.done() {
            control.exit();
        } else {
            control.request_redraw();
        }
    }

    /// Relabel the window when the score or the terminal state changes;
    /// otherwise the OS hears nothing.
    fn relabel(&mut self) {
        let state = (self.world.score(), self.world.alive());
        if self.titled == Some(state) {
            return;
        }
        if let Some(title) = self.title.compose(state.0, state.1)
            && let Some(window) = &self.window
        {
            window.set_title(title);
            self.titled = Some(state);
        }
    }

    /// Turn the loop's outcome into the run's report.
    fn finish(self, outcome: Result<(), WindowError>) -> Result<Report, SampleError> {
        if let Err(error) = outcome {
            return Err(match error {
                WindowError::LoopUnavailable { message } => {
                    SampleError::Unavailable(format!("no window is available here: {message}"))
                }
                other => SampleError::failed("running the window loop", &other),
            });
        }
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        Ok(Report {
            seed: self.seed,
            source: "window".to_string(),
            stats: self.stats,
            world: self.world,
        })
    }
}

impl WindowApp for GlideApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let (width, height) = window.physical_size();
        self.size = Extent { width, height };
        let native = window.native();
        self.window = Some(native.clone());
        if let Err(error) = self.bring_up(native, self.size) {
            self.failure = Some(error);
        }
        // Anchor AFTER bring-up, so device creation is not banked as a
        // clamped burst of catch-up steps.
        let now = Timestamp::from_nanos(self.clock.elapsed_nanos());
        self.frame = Some(FrameLoop::new(Timestep::HZ_60, StepBudget::DEFAULT, now));
        self.last_progress = now;
    }

    fn event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized { width, height } => self.resize(Extent { width, height }),
            WindowEvent::CloseRequested => self.close_requested = true,
            WindowEvent::Focused(false) => self.input.release_all(),
            other => self.input.handle(other),
        }
    }

    fn update(&mut self, control: &mut LoopControl) {
        let now = Timestamp::from_nanos(self.clock.elapsed_nanos());
        self.update_at(now, control);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> GlideApp {
        let options = Options {
            seed: 7,
            frames: 2_000,
            input_trace: "soar".to_string(),
            record_trace: None,
            replay_trace: None,
            window: true,
            window_ticks: None,
        };
        let mut app = GlideApp::new(&options);
        // Anchor the schedule by hand: these tests say what time it is,
        // and no window exists.
        app.frame = Some(FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(0),
        ));
        app
    }

    /// One 60Hz step interval in nanoseconds.
    const STEP: u64 = Timestep::HZ_60.nanos().get();

    fn press(app: &mut GlideApp) {
        app.input.handle(WindowEvent::Key {
            code: renew_event::KeyCode::Space,
            pressed: true,
            repeat: false,
        });
    }

    fn release(app: &mut GlideApp) {
        app.input.handle(WindowEvent::Key {
            code: renew_event::KeyCode::Space,
            pressed: false,
            repeat: false,
        });
    }

    #[test]
    fn one_press_on_a_multi_step_frame_flaps_once() {
        let mut app = app();
        let mut control = LoopControl::default();
        press(&mut app);
        // Three step intervals elapse in one frame: three steps run,
        // exactly one may flap.
        app.update_at(Timestamp::from_nanos(3 * STEP), &mut control);
        assert_eq!(app.world.tick(), 3, "three steps ran");
        assert_eq!(app.pending_flaps, 0, "the one edge was consumed");
        // The flap reached the world: velocity is upward (negative) at
        // some point only if a flap fired; after three ticks with one
        // flap the world differs from a flapless one.
        let mut flapless = World::new(7);
        for _ in 0..3 {
            flapless.step(false);
        }
        assert_ne!(
            app.world.digest(),
            flapless.digest(),
            "the press must have reached exactly one step"
        );
    }

    #[test]
    fn a_press_on_a_zero_step_frame_survives_to_the_next_step() {
        let mut app = app();
        let mut control = LoopControl::default();
        press(&mut app);
        // No step is due yet: the edge retires but the counter holds it.
        app.update_at(Timestamp::from_nanos(STEP / 4), &mut control);
        assert_eq!(app.world.tick(), 0, "no step was due");
        assert_eq!(app.pending_flaps, 1, "the edge survived in the counter");
        app.update_at(Timestamp::from_nanos(STEP), &mut control);
        assert_eq!(app.world.tick(), 1);
        assert_eq!(app.pending_flaps, 0, "the held edge was consumed");
    }

    #[test]
    fn two_presses_across_a_catch_up_frame_deliver_two_flaps() {
        // The round-one counterexample, pinned: press on a zero-step
        // frame, a second distinct press on a frame that plans two
        // catch-up steps. A bit would fold them; the counter must not.
        let mut app = app();
        let mut control = LoopControl::default();
        press(&mut app);
        app.update_at(Timestamp::from_nanos(STEP / 4), &mut control);
        assert_eq!(app.pending_flaps, 1);
        release(&mut app);
        press(&mut app);
        app.update_at(Timestamp::from_nanos(2 * STEP), &mut control);
        assert_eq!(app.world.tick(), 2, "two catch-up steps ran");
        assert_eq!(
            app.pending_flaps, 0,
            "both presses were consumed, one per step"
        );
        // Two flaps in two ticks: differs from one flap in two ticks.
        let mut one_flap = World::new(7);
        one_flap.step(true);
        one_flap.step(false);
        assert_ne!(
            app.world.digest(),
            one_flap.digest(),
            "the second press must not have been folded away"
        );
    }

    #[test]
    fn two_presses_in_one_event_phase_fold_to_one_edge() {
        // The fold that remains is per event phase, not per step
        // interval: press-release-press before a single update is one
        // just_pressed transition report, so one edge — a fact about
        // edge reporting, pinned rather than hidden.
        let mut app = app();
        let mut control = LoopControl::default();
        press(&mut app);
        release(&mut app);
        press(&mut app);
        app.update_at(Timestamp::from_nanos(STEP), &mut control);
        assert_eq!(app.world.tick(), 1);
        assert_eq!(app.pending_flaps, 0, "one edge, consumed by the one step");
    }

    #[test]
    fn presses_on_two_zero_step_frames_carry_as_two_flaps() {
        // The counter deliberately UN-folds presses that arrive on
        // separate frames inside one step interval: each frame's edge
        // counts, and the two flaps fire one tick apart.
        let mut app = app();
        let mut control = LoopControl::default();
        press(&mut app);
        app.update_at(Timestamp::from_nanos(STEP / 8), &mut control);
        release(&mut app);
        press(&mut app);
        app.update_at(Timestamp::from_nanos(STEP / 4), &mut control);
        assert_eq!(app.world.tick(), 0, "still inside the first interval");
        assert_eq!(app.pending_flaps, 2, "both frames' edges are counted");
        app.update_at(Timestamp::from_nanos(2 * STEP), &mut control);
        assert_eq!(app.world.tick(), 2);
        assert_eq!(app.pending_flaps, 0, "both consumed, one per tick");
    }

    #[test]
    fn the_title_reflects_score_and_death_and_only_changes_when_they_do() {
        let mut title = Title::new();
        let alive = title.compose(0, true).expect("compose").to_string();
        assert_eq!(alive, "renew — glide — score 0");
        let scored = title.compose(12, true).expect("compose").to_string();
        assert_eq!(scored, "renew — glide — score 12");
        let over = title.compose(12, false).expect("compose").to_string();
        assert_eq!(over, "renew — glide — score 12 — over");
    }

    #[test]
    fn the_wedge_path_reports_after_the_stall_window() {
        let mut app = app();
        app.wedge_after = Nanos::from_nanos(1);
        let mut control = LoopControl::default();
        app.update_at(Timestamp::from_nanos(STEP), &mut control);
        assert!(
            matches!(&app.failure, Some(SampleError::Failed(message)) if message.starts_with("wedged")),
            "a run that never presents must wedge: {:?}",
            app.failure
        );
    }

    #[test]
    fn draw_outcomes_mean_what_the_run_records() {
        // Every record_draw arm, driven with constructed outcomes.
        let mut app = app();
        app.record_draw(Ok(PresentOutcome::Presented));
        assert_eq!(app.presented, 1);
        assert!(app.drawn_since_update);
        // NeedsResize is not an error and not progress; with no target
        // built, resize is a no-op and nothing fails.
        app.record_draw(Ok(PresentOutcome::NeedsResize));
        assert_eq!(app.presented, 1);
        assert!(app.failure.is_none());
        app.record_draw(Err(renew_rhi::TargetError::SurfaceCreation { code: -1 }));
        assert!(
            matches!(&app.failure, Some(SampleError::Failed(message)) if message.starts_with("rendering")),
            "a render error is the run's failure: {:?}",
            app.failure
        );
    }

    #[test]
    fn the_event_arms_latch_release_and_forward() {
        let mut app = app();
        let mut control = LoopControl::default();
        // Focus loss clears held input: a press followed by Focused(false)
        // must not flap.
        press(&mut app);
        app.event(WindowEvent::Focused(false));
        app.update_at(Timestamp::from_nanos(STEP), &mut control);
        assert_eq!(app.world.tick(), 1);
        // The edge fired before the release_all, so the press itself
        // still counts once — what must NOT happen is a stuck hold
        // producing more flaps later.
        let flaps_now = app.pending_flaps;
        app.update_at(Timestamp::from_nanos(2 * STEP), &mut control);
        assert_eq!(
            app.pending_flaps,
            flaps_now.saturating_sub(1).min(flaps_now),
            "no new edges appear after focus loss"
        );
        // The close latch ends the run.
        app.event(WindowEvent::CloseRequested);
        app.update_at(Timestamp::from_nanos(3 * STEP), &mut control);
        assert!(app.close_requested);
    }

    #[test]
    fn finish_maps_outcomes_to_the_report_or_the_named_refusal() {
        let options = Options {
            seed: 7,
            frames: 2_000,
            input_trace: "soar".to_string(),
            record_trace: None,
            replay_trace: None,
            window: true,
            window_ticks: None,
        };
        // Success: a report with the window's source.
        let app = GlideApp::new(&options);
        let report = app.finish(Ok(())).expect("a clean run reports");
        assert_eq!(report.source, "window");
        assert_eq!(report.seed, 7);
        // A recorded failure outranks a clean loop exit.
        let mut app = GlideApp::new(&options);
        app.failure = Some(SampleError::Failed("wedged: test".to_string()));
        assert!(matches!(
            app.finish(Ok(())),
            Err(SampleError::Failed(message)) if message.starts_with("wedged")
        ));
        // A missing display is the named refusal, not a generic failure.
        let app = GlideApp::new(&options);
        assert!(matches!(
            app.finish(Err(WindowError::LoopUnavailable {
                message: "no display".to_string()
            })),
            Err(SampleError::Unavailable(_))
        ));
    }

    #[test]
    fn the_unavailable_error_displays_its_reason() {
        let error = SampleError::Unavailable("no display server".to_string());
        assert_eq!(error.to_string(), "no display server");
    }
}
