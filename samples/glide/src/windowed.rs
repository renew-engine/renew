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
use renew_math::Alpha;
use renew_platform::Clock;
use renew_platform::window::{
    LoopControl, NativeWindow, WindowApp, WindowConfig, WindowError, WindowRef, run_window_app,
};
use renew_render2d::{AtlasDesc, Canvas, Region, Sprite, SpriteRenderer};
use renew_rhi::{
    Color, Device, DeviceDesc, Extent, Pass, PresentOutcome, RenderDesc, WindowTarget,
};
use renew_sample_glide_world::{Action, VIEW_HEIGHT, VIEW_WIDTH, World};

#[cfg(feature = "audio")]
use crate::audio::Audio;
use crate::cli::{Options, Report};
use crate::scene::{Presentation, SceneSprite, Tile};
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
/// touch the heap. The capacity is a type parameter rather than a
/// constant — the sibling's trick — so the overflow refusal is
/// reachable from a test holding a deliberately tiny buffer; a branch
/// nothing can execute is a branch nobody has checked.
struct Title<const N: usize = 64> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Title<N> {
    fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }

    /// The title for a score, with the game-over suffix once dead.
    /// Returns `None` when the buffer would overflow.
    fn compose(&mut self, score: u64, alive: bool) -> Option<&str> {
        use core::fmt::Write as _;
        struct Sink<'a> {
            bytes: &'a mut [u8],
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

/// The relabel decision as a free function, generic over the title's
/// capacity so the compose-refusal arm is reachable from a test holding
/// a deliberately tiny buffer — the same reason the capacity is a type
/// parameter at all.
fn relabel_into<const N: usize>(
    title: &mut Title<N>,
    titled: &mut Option<(u64, bool)>,
    window: Option<&NativeWindow>,
    state: (u64, bool),
) -> bool {
    if *titled == Some(state) {
        return false;
    }
    let Some(text) = title.compose(state.0, state.1) else {
        return false;
    };
    if let Some(window) = window {
        window.set_title(text);
    }
    *titled = Some(state);
    true
}

// Sprites reserved for HUD and label text beyond the menu's own
// quads: enough for the score line and both button labels.
const UI_TEXT_SPRITES: u32 = 64;
// Opaque white ink for the HUD score.
const HUD_INK: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
// Slightly warm ink for button labels.
const LABEL_INK: [f32; 4] = [0.92, 0.94, 1.0, 1.0];

/// Canvas pixels from the solver's fixed-point, for label placement.
#[allow(
    clippy::cast_precision_loss,
    reason = "label coordinates are tens of pixels, exact in an f32"
)]
fn fixed_px(value: renew_ui::Fixed) -> f32 {
    value.to_bits() as f32 / 65536.0
}

/// Where a label starts inside its button: centred both ways by the
/// same integer advances the tree measured the button with.
fn label_origin(rect: &renew_ui::Rect, label: &str) -> (f32, f32) {
    let text_width = renew_ui::text::measure(label);
    let line = renew_ui::Fixed::from_int(i32::try_from(renew_ui::text::LINE_HEIGHT).unwrap_or(16));
    let two = renew_ui::Fixed::from_int(2);
    let x = rect.x + (rect.width - text_width) / two;
    let y = rect.y + (rect.height - line) / two;
    (fixed_px(x), fixed_px(y))
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
    /// The pause menu: a real widget tree, folded into the session
    /// digest. Pausing gates the world's steps; the menu decides.
    menu: crate::menu::Menu,
    /// The menu's snapshot pair, advanced per redraw while the menu
    /// is open.
    presenter: renew_ui_render::UiPresenter,
    /// The UI's own sprite renderer over the UI atlas — the second
    /// texture in the frame, drawn as its own item in the same pass.
    ui_sprites: Option<renew_render2d::SpriteRenderer>,
    scene_scratch: Vec<SceneSprite>,
    /// The last two ticks of the world's picture, so a frame between
    /// them draws between them rather than repeating whichever tick
    /// happened last.
    presentation: Presentation,
    /// How far past the last executed step this frame stands. Stored
    /// rather than passed, because drawing is an event the platform
    /// raises and not a tail of the update that computed it.
    alpha: Alpha,
    /// The HUD's score line, formatted in place each frame.
    hud_score: String,
    title: Title,
    /// The score last written into the title, so relabeling happens on
    /// change only. Death changes the suffix, tracked beside it.
    titled: Option<(u64, bool)>,
    frame: Option<FrameLoop>,
    window: Option<NativeWindow>,
    device: Option<Device>,
    target: Option<WindowTarget>,
    renderer: Option<SpriteRenderer>,
    /// The sound half, when this build has one and a device opened.
    /// A run whose machine has no audio keeps playing in silence.
    #[cfg(feature = "audio")]
    audio: Option<Audio>,
    /// Why this run has no sound, when it has none.
    #[cfg(feature = "audio")]
    muted: Option<String>,
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
        // Hoisted so the presentation starts from a real tick: seeded
        // from a default world, the first frame would blend the bird up
        // from y = 0.
        let world = World::new(options.seed);
        Self {
            clock: Clock::start(),
            seed: options.seed,
            ticks_wanted: options.window_ticks,
            presentation: Presentation::new(&world),
            alpha: Alpha::ZERO,
            world,
            input: scripted::input_map(),
            pending_flaps: 0,
            #[cfg(feature = "audio")]
            audio: None,
            #[cfg(feature = "audio")]
            muted: None,
            stats: FrameStats::new(),
            menu: crate::menu::Menu::new(),
            presenter: renew_ui_render::UiPresenter::new(8),
            ui_sprites: None,
            scene_scratch: Vec::new(),
            hud_score: String::with_capacity(24),
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
        // Validation follows the diagnostics switch: off for an
        // ordinary run, where the layer costs frame time and says
        // nothing, and on for a run that is being debugged, where it is
        // the only thing that can name a fault inside the driver.
        let device = Device::new(&DeviceDesc {
            app_name: "renew-glide",
            validation: crate::validation_policy(),
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
        // The UI's own renderer: a second texture in the frame,
        // carried as its own whole pipeline and its own item — the
        // tolerated shape until the frame model revisits descriptors.
        let ui_capacity =
            core::num::NonZeroU32::new(self.presenter.max_quads().saturating_add(UI_TEXT_SPRITES))
                .ok_or_else(|| SampleError::Failed("zero UI sprite capacity".to_string()))?;
        let ui_sprites = SpriteRenderer::new(
            &device,
            &AtlasDesc::new(
                renew_rhi::Extent {
                    width: renew_ui_render::atlas::WIDTH,
                    height: renew_ui_render::atlas::HEIGHT,
                },
                &renew_ui_render::atlas::pixels(),
            ),
            canvas,
            target.format(),
            ui_capacity,
        )
        .map_err(|error| SampleError::failed("building the UI sprite renderer", &error))?;
        self.device = Some(device);
        self.target = Some(target);
        self.renderer = Some(renderer);
        self.ui_sprites = Some(ui_sprites);
        #[cfg(feature = "audio")]
        {
            // A machine with no sound card is a machine that plays in
            // silence, not a failed run: bring-up says why once and
            // moves on.
            match Audio::open() {
                Ok(audio) => self.audio = Some(audio),
                Err(reason) => self.muted = Some(reason),
            }
        }
        Ok(())
    }

    /// Draw the world as it stands.
    fn draw(&mut self) {
        let (Some(target), Some(renderer), Some(ui_sprites)) =
            (&mut self.target, &mut self.renderer, &mut self.ui_sprites)
        else {
            return;
        };
        self.presentation.fill(self.alpha, &mut self.scene_scratch);
        renderer.begin();
        for sprite in &self.scene_scratch {
            let region = match sprite.tile {
                Tile::Bird => BIRD_REGION,
                Tile::Pipe => PIPE_REGION,
            };
            renderer
                .push(&Sprite::new(region, sprite.x, sprite.y).size(sprite.width, sprite.height));
        }
        // The UI over the world: the score always, the menu when
        // open — panels from the presenter's snapshots, labels
        // centred in the buttons' solved rectangles by the same
        // integer advances the tree measured them with.
        ui_sprites.begin();
        self.hud_score.clear();
        let _ = core::fmt::Write::write_fmt(
            &mut self.hud_score,
            format_args!("Score {}", self.world.score()),
        );
        renew_ui_render::emit_text(ui_sprites, 6.0, 4.0, &self.hud_score, HUD_INK);
        if self.menu.is_open() {
            self.presenter.advance(self.menu.ui());
            self.presenter.emit(renew_math::Alpha::ZERO, ui_sprites);
            for (node, label) in self.menu.labels() {
                if let Some(rect) = self.menu.ui().rect(node) {
                    let (x, y) = label_origin(&rect, label);
                    renew_ui_render::emit_text(ui_sprites, x, y, label, LABEL_INK);
                }
            }
        }
        // The frame, composed on this stack; the borrows end at the
        // render call. Two items: the world's atlas, then the UI's.
        let color = [renew_rhi::color_attachment(SKY)];
        let items = [renderer.item(), ui_sprites.item()];
        let passes = [Pass::new(&color, &items)];
        let outcome = target.render(&RenderDesc::new(&passes));
        self.record_draw(outcome);
    }

    /// A window event with any pointer position rescaled from
    /// physical surface pixels into the 320x240 canvas the menu
    /// solved in. Everything else passes through untouched.
    fn to_canvas(&self, event: WindowEvent) -> WindowEvent {
        if let WindowEvent::PointerMoved { x, y } = event {
            let sx = f64::from(VIEW_WIDTH) / f64::from(self.size.width.max(1));
            let sy = f64::from(VIEW_HEIGHT) / f64::from(self.size.height.max(1));
            WindowEvent::PointerMoved {
                x: x * sx,
                y: y * sy,
            }
        } else {
            event
        }
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
        if let Some(target) = &mut self.target {
            let outcome = target.resize(size);
            self.record_resize(outcome);
        }
    }

    /// What a resize outcome means — split so tests drive the error
    /// arm with a constructed value, no window.
    fn record_resize(&mut self, outcome: Result<(), renew_rhi::TargetError>) {
        if let Err(error) = outcome {
            self.failure = Some(SampleError::failed("resizing", &error));
        }
    }

    /// What a failed bring-up means — the same driven-seam split.
    fn record_bring_up(&mut self, outcome: Result<(), SampleError>) {
        if let Err(error) = outcome {
            self.failure = Some(error);
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
    ///
    /// The tick bound is checked at frame boundaries, never mid-plan:
    /// every absorbed plan is fully executed, so the digest line's tick
    /// count and the world can never disagree. The cost is a bounded
    /// overshoot of at most the step budget minus one on a lagging
    /// frame, documented where the flag is.
    fn update_at(&mut self, now: Timestamp, control: &mut LoopControl) {
        if !self.done()
            && let Some(frame) = &mut self.frame
        {
            let plan = frame.begin_frame(now);
            // Capture the edge before it retires, count it, spend one
            // per step: a press on a zero-step frame survives to the
            // next frame's first step, and two presses with two due
            // steps deliver two flaps. An edge banks only while the
            // menu is closed: a press racing the pause is abandoned,
            // exactly as the scripted loop abandons it when the paused
            // step never runs and the frame's advance retires it.
            if !self.menu.is_open() {
                self.pending_flaps = self
                    .pending_flaps
                    .saturating_add(u8::from(self.input.state(Action::Flap).just_pressed));
            }
            for action in self.menu.drain() {
                if action == crate::menu::MenuAction::Restart {
                    self.world = World::new(self.seed);
                    // The pair belongs to the world it captured. Keeping
                    // it across a restart would blend the new bird out of
                    // the old one's last position and drag every pipe
                    // across the screen for one interval.
                    self.presentation = Presentation::new(&self.world);
                    self.alpha = Alpha::ZERO;
                    self.pending_flaps = 0;
                }
            }
            let paused = self.menu.is_open();
            for _step in plan.steps() {
                // Readings around the tick: the world exposes no
                // events, so the difference across a step is what
                // happened in it.
                // An open menu pauses the world: frames keep coming,
                // steps do not, and the pause bit is digested.
                if paused {
                    continue;
                }
                let flap_passed = self.pending_flaps > 0;
                let before_alive = self.world.alive();
                let before_score = self.world.score();
                self.world.step(flap_passed);
                self.pending_flaps = self.pending_flaps.saturating_sub(1);
                #[cfg(feature = "audio")]
                {
                    let sounds = crate::sound::tick_sounds(
                        before_alive,
                        flap_passed,
                        before_score,
                        self.world.alive(),
                        self.world.score(),
                    );
                    if let Some(audio) = &self.audio {
                        audio.play(sounds);
                    }
                }
                #[cfg(not(feature = "audio"))]
                {
                    // Read in both builds so a silent build cannot
                    // drift away from the sounding one.
                    let _ = (before_alive, before_score, flap_passed);
                }
                // Once per EXECUTED step, never once per frame: a frame
                // that runs three catch-up steps must leave the earlier
                // capture one tick back, not three.
                self.presentation.capture(&self.world);
            }
            self.input.advance();
            // The factor moves only while the world does. The plan's
            // remainder keeps cycling whether or not the caller executes
            // the steps, so updating this while paused would sweep it
            // from zero to one against a frozen pair of captures and make
            // a paused game oscillate by a whole tick of motion — worse
            // than the stutter this removes. Frozen factor over a frozen
            // pair is the last frame drawn before the pause, exactly.
            if !paused {
                self.alpha = Alpha::new(plan.remainder().get(), Timestep::HZ_60.nanos());
            }
            self.stats.absorb(&plan);
            let _ = self.relabel();
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
    /// otherwise the OS hears nothing. Returns whether a compose
    /// happened, so the change-only gate is a tested fact rather than a
    /// lane-only one; composing is tracked even windowless, and only
    /// the delivery needs the OS handle.
    fn relabel(&mut self) -> bool {
        let state = (self.world.score(), self.world.alive());
        relabel_into(
            &mut self.title,
            &mut self.titled,
            self.window.as_ref(),
            state,
        )
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
        // Sound is not a gate: a run that played silently is a
        // complete run, and the only thing owed is saying so. Said
        // here, once, rather than every frame the stream is missing.
        #[cfg(feature = "audio")]
        {
            if let Some(reason) = &self.muted {
                eprintln!("sound: silent this run ({reason})");
            } else if self.audio.as_ref().is_some_and(|audio| !audio.healthy()) {
                eprintln!("sound: the device stopped mid-run; the rest was silent");
            }
        }
        let session_hash = self.menu.absorb(self.world.digest()).finish();
        Ok(Report {
            seed: self.seed,
            source: "window".to_string(),
            stats: self.stats,
            world: self.world,
            session_hash,
        })
    }
}

impl WindowApp for GlideApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let (width, height) = window.physical_size();
        self.size = Extent { width, height };
        let native = window.native();
        self.window = Some(native.clone());
        let outcome = self.bring_up(native, self.size);
        self.record_bring_up(outcome);
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
            other => {
                // Pointer coordinates arrive in physical window
                // pixels; the menu solved in canvas pixels, and the
                // sprite renderer stretches that canvas over the whole
                // surface — so the driver maps positions into canvas
                // space BEFORE the tree hears them, or the menu would
                // draw in one place and hit-test in another. The
                // mapping is part of the input seam: scripted traces
                // speak canvas coordinates directly, and this is the
                // one place a window's pixels become those.
                let other = self.to_canvas(other);
                // The menu hears everything; gameplay hears an event
                // only when the menu was closed as it arrived, so the
                // click that presses Resume never also flaps. Opening
                // releases gameplay input: a key held into the pause
                // must not wedge the map with a press whose release
                // the menu will swallow.
                let was_open = self.menu.is_open();
                self.menu.handle(&other);
                if !was_open && self.menu.is_open() {
                    self.input.release_all();
                }
                if !was_open {
                    self.input.handle(other);
                }
            }
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
            json: false,
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

    /// Through the seam, not straight into the map: the fall-through
    /// event arm is part of what these tests witness.
    fn press(app: &mut GlideApp) {
        app.event(WindowEvent::Key {
            code: renew_event::KeyCode::Space,
            pressed: true,
            repeat: false,
        });
    }

    fn release(app: &mut GlideApp) {
        app.event(WindowEvent::Key {
            code: renew_event::KeyCode::Space,
            pressed: false,
            repeat: false,
        });
    }

    fn escape(app: &mut GlideApp) {
        app.event(WindowEvent::Key {
            code: renew_event::KeyCode::Escape,
            pressed: true,
            repeat: false,
        });
    }

    /// A click in physical window pixels, routed through the seam:
    /// the driver rescales it into canvas space before the menu
    /// hears it.
    fn click_physical(app: &mut GlideApp, x: f64, y: f64) {
        app.event(WindowEvent::PointerMoved { x, y });
        app.event(WindowEvent::PointerButton {
            button: renew_event::PointerButton::Left,
            pressed: true,
        });
        app.event(WindowEvent::PointerButton {
            button: renew_event::PointerButton::Left,
            pressed: false,
        });
    }

    /// A button's centre in canvas pixels, from the same solved
    /// rectangle the menu hit-tests with.
    fn centre_of(app: &GlideApp, index: usize) -> (f64, f64) {
        let (node, _) = app.menu.labels()[index];
        let rect = app.menu.ui().rect(node).expect("solved");
        let two = renew_ui::Fixed::from_int(2);
        let x = rect.x + rect.width / two;
        let y = rect.y + rect.height / two;
        let px = |value: renew_ui::Fixed| f64::from(i32::try_from(value.trunc_int()).unwrap_or(0));
        (px(x), px(y))
    }

    #[test]
    fn opening_the_menu_pauses_the_world_and_releases_input() {
        let mut app = app();
        let mut control = LoopControl::default();
        // A key held into the pause: opening releases it, and the
        // release the menu later swallows can no longer wedge the map.
        press(&mut app);
        escape(&mut app);
        assert!(app.menu.is_open());
        app.update_at(Timestamp::from_nanos(3 * STEP), &mut control);
        assert_eq!(app.world.tick(), 0, "an open menu holds the world still");
        release(&mut app);
        escape(&mut app);
        assert!(!app.menu.is_open());
        press(&mut app);
        app.update_at(Timestamp::from_nanos(4 * STEP), &mut control);
        assert_eq!(app.world.tick(), 1, "resuming lets the world step again");
        assert_eq!(app.pending_flaps, 0, "the fresh press was a clean edge");
    }

    #[test]
    fn a_restart_click_lands_through_the_physical_to_canvas_seam() {
        let mut app = app();
        let mut control = LoopControl::default();
        app.update_at(Timestamp::from_nanos(5 * STEP), &mut control);
        assert_eq!(app.world.tick(), 5, "the run has an age to lose");
        escape(&mut app);
        // The window is twice the canvas in each direction: the click
        // arrives in physical pixels and must still land, because the
        // driver rescales positions before the tree hears them.
        app.size = Extent {
            width: 2 * VIEW_WIDTH,
            height: 2 * VIEW_HEIGHT,
        };
        let (x, y) = centre_of(&app, 1);
        click_physical(&mut app, 2.0 * x, 2.0 * y);
        app.update_at(Timestamp::from_nanos(6 * STEP), &mut control);
        assert!(!app.menu.is_open(), "an activated button closes the menu");
        assert_eq!(
            app.world.tick(),
            1,
            "the restart made the world new; one step has run since"
        );
        assert_eq!(app.pending_flaps, 0, "a restart abandons pending flaps");
    }

    #[test]
    fn a_resume_click_plays_on_from_where_it_paused() {
        let mut app = app();
        let mut control = LoopControl::default();
        app.update_at(Timestamp::from_nanos(5 * STEP), &mut control);
        escape(&mut app);
        // A window exactly the canvas size: the rescale is identity.
        app.size = Extent {
            width: VIEW_WIDTH,
            height: VIEW_HEIGHT,
        };
        let (x, y) = centre_of(&app, 0);
        click_physical(&mut app, x, y);
        app.update_at(Timestamp::from_nanos(6 * STEP), &mut control);
        assert!(!app.menu.is_open());
        assert_eq!(
            app.world.tick(),
            6,
            "resume keeps the world; a restart would have reset it"
        );
    }

    /// The placement arithmetic behind the exempt draw block: every
    /// label starts strictly inside its own button.
    #[test]
    fn labels_centre_inside_their_buttons() {
        let app = app();
        for (node, label) in app.menu.labels() {
            let rect = app.menu.ui().rect(node).expect("solved");
            let (x, y) = label_origin(&rect, label);
            assert!(
                fixed_px(rect.x) < x,
                "{label} starts right of its left edge"
            );
            assert!(
                x < fixed_px(rect.x + rect.width),
                "{label} starts left of its right edge"
            );
            assert!(fixed_px(rect.y) < y, "{label} starts below its top edge");
            assert!(
                y < fixed_px(rect.y + rect.height),
                "{label} starts above its bottom edge"
            );
        }
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
    fn the_title_refuses_a_buffer_it_cannot_fit() {
        // The overflow branch, reachable through a deliberately tiny
        // capacity — the reason the capacity is a type parameter — and
        // the relabel decision that rides the refusal: no compose, no
        // recorded state, try again next time.
        let mut tiny = Title::<8>::new();
        assert!(tiny.compose(0, true).is_none(), "eight bytes cannot fit");
        let mut titled = None;
        assert!(
            !relabel_into(&mut tiny, &mut titled, None, (0, true)),
            "a refused compose is not a relabel"
        );
        assert_eq!(titled, None, "and records nothing as delivered");
    }

    #[test]
    fn draw_without_a_target_is_a_quiet_no_op() {
        let mut app = app();
        app.event(WindowEvent::RedrawRequested);
        app.event(WindowEvent::Resized {
            width: 320,
            height: 240,
        });
        assert!(app.failure.is_none());
        assert_eq!(app.presented, 0);
    }

    #[test]
    fn the_outcome_seams_record_their_failures() {
        let mut first = app();
        first.record_resize(Err(renew_rhi::TargetError::SurfaceCreation { code: -3 }));
        assert!(
            matches!(&first.failure, Some(SampleError::Failed(message)) if message.starts_with("resizing"))
        );
        let mut second = app();
        second.record_bring_up(Err(SampleError::Failed("no device".to_string())));
        assert!(matches!(&second.failure, Some(SampleError::Failed(_))));
        second.record_bring_up(Ok(()));
        second.record_resize(Ok(()));
    }

    #[test]
    fn an_update_before_ready_is_a_quiet_no_op() {
        // No frame is anchored until ready runs; an update arriving
        // first must step nothing and fail nothing.
        let options = Options {
            seed: 7,
            frames: 2_000,
            input_trace: "soar".to_string(),
            record_trace: None,
            replay_trace: None,
            window: true,
            window_ticks: None,
            json: false,
        };
        let mut app = GlideApp::new(&options);
        let mut control = LoopControl::default();
        app.update_at(Timestamp::from_nanos(STEP), &mut control);
        assert_eq!(app.world.tick(), 0);
        assert!(app.failure.is_none());
    }

    #[test]
    fn the_tick_bound_stops_at_the_frame_boundary_and_the_line_stays_true() {
        // The bound never cuts a plan: the lagging frame's three steps
        // all execute (a bounded overshoot the flag documents), and the
        // stats agree with the world — the digest line cannot contradict
        // itself.
        let mut app = app();
        app.ticks_wanted = Some(1);
        let mut control = LoopControl::default();
        app.update_at(Timestamp::from_nanos(3 * STEP), &mut control);
        assert_eq!(app.world.tick(), 3, "the lagging plan finished whole");
        assert_eq!(
            app.stats.ticks(),
            app.world.tick(),
            "absorbed and executed are the same number"
        );
        assert!(app.done(), "the bound is reached at the boundary");
        // A further update steps nothing: done at entry skips the plan.
        app.update_at(Timestamp::from_nanos(6 * STEP), &mut control);
        assert_eq!(app.world.tick(), 3, "no ticks after the bound");
    }

    #[test]
    fn a_latched_close_steps_no_further_ticks() {
        // The close click is honored before any catch-up burst: done at
        // entry means the plan is never begun.
        let mut app = app();
        let mut control = LoopControl::default();
        app.event(WindowEvent::CloseRequested);
        app.update_at(Timestamp::from_nanos(3 * STEP), &mut control);
        assert_eq!(app.world.tick(), 0, "no ticks after the close");
    }

    #[test]
    fn the_title_composes_on_change_and_only_on_change() {
        // The change-only gate, as a tested fact: first sight composes,
        // repetition does not, a score or death change does.
        let mut app = app();
        assert!(app.relabel(), "first sight composes");
        assert!(!app.relabel(), "unchanged state is silent");
        for _ in 0..240 {
            app.world.step(false);
        }
        assert!(!app.world.alive(), "gravity won");
        assert!(app.relabel(), "death recomposes");
        assert!(!app.relabel(), "and then silence again");
    }

    #[test]
    fn progress_marks_ride_a_drawn_frame() {
        let mut app = app();
        let mut control = LoopControl::default();
        app.record_draw(Ok(PresentOutcome::Presented));
        assert!(app.drawn_since_update);
        app.update_at(Timestamp::from_nanos(STEP), &mut control);
        assert_eq!(
            app.last_progress,
            Timestamp::from_nanos(STEP),
            "a drawn frame moves the progress mark"
        );
        assert!(!app.drawn_since_update, "consumed by the update");
    }

    #[test]
    fn finish_names_a_loop_that_ran_and_failed() {
        let options = Options {
            seed: 7,
            frames: 2_000,
            input_trace: "soar".to_string(),
            record_trace: None,
            replay_trace: None,
            window: true,
            window_ticks: None,
            json: false,
        };
        let app = GlideApp::new(&options);
        assert!(matches!(
            app.finish(Err(WindowError::Loop {
                message: "test".to_string()
            })),
            Err(SampleError::Failed(message)) if message.starts_with("running the window loop")
        ));
    }

    #[test]
    fn the_title_reflects_score_and_death_and_only_changes_when_they_do() {
        let mut title = Title::<64>::new();
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
    fn focus_loss_forgets_the_held_key_so_a_new_press_registers() {
        // The observable consequence of release_all, which a deleted
        // arm would fail: without it the key is still "held" and the
        // second press is no transition — no new edge, a dropped input.
        let mut app = app();
        let mut control = LoopControl::default();
        press(&mut app);
        app.update_at(Timestamp::from_nanos(STEP / 8), &mut control);
        assert_eq!(app.pending_flaps, 1, "the first press banked");
        app.event(WindowEvent::Focused(false));
        press(&mut app);
        app.update_at(Timestamp::from_nanos(STEP / 4), &mut control);
        assert_eq!(
            app.pending_flaps, 2,
            "focus loss forgot the held key, so the re-press is a real new              edge — with the release_all arm deleted the key stays held and              this press is no transition at all"
        );
        // The close latch ends the run.
        app.event(WindowEvent::CloseRequested);
        app.update_at(Timestamp::from_nanos(STEP), &mut control);
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
            json: false,
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
