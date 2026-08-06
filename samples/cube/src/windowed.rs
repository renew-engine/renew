//! Playing it: a window, a keyboard, and the camera on the player's head.
//!
//! Behind the `window` feature, like the platformer's. The game a player
//! runs still carries no graphics crate unless asked for one.
//!
//! # Turning is fixed point, and that is not fussiness
//!
//! Yaw and pitch are [`Angle`]s and the look direction comes from
//! `Angle::sin_cos`, which is fixed point and identical on every
//! platform. A float here would reach the world through `look_at` — and
//! the world's whole claim is that its digest depends on nothing but its
//! inputs. Turning with a float would make a wall-clock run's digest a
//! function of the platform's maths library.
//!
//! # Aiming is the keyboard's, breaking is the mouse's
//!
//! Left breaks the block being aimed at and right places one against it,
//! beside the keys that do the same. Looking stays on the arrow keys:
//! turning the view with the pointer needs the cursor held inside the
//! window, and this engine's window layer has no way to ask for that
//! yet. A mouse-look that stops dead at the edge of the screen is worse
//! than one that is honestly absent: it reads as a bug in the game
//! rather than as a gap in the engine.
//!
//! # The world steps on its own clock, not the panel's
//!
//! The event loop spins as fast as the display allows, so a simulation
//! stepped once per spin would run two and a half times faster on a
//! 144 Hz panel than on a 60 Hz one — and unbounded where no window came
//! up at all. A [`FrameLoop`] absorbs the elapsed time and hands back how
//! many fixed steps are due, which is the shape the platformer already
//! uses for the same problem. The world's own step stays exactly what it
//! was; only how often it is called changes.
//!
//! The wall clock reaches the *driver* and never the world: what a step
//! consumes is an [`Intent`] of integers and angles. A scripted run and a
//! played one differ in when steps happen, not in what a step does.
//!
//! # Walking is camera-relative, in eight directions
//!
//! `Intent` takes whole steps on the world's own axes, clamped to −1, 0
//! or +1. So pressing forward while facing north-east walks north-east:
//! the driver rotates the key into a world direction and rounds it to the
//! nearest of the eight the world can express. It is steppy, and it is
//! honest — a smoother walk needs a fixed-point vector in the world's
//! vocabulary, which is a change to the simulation rather than to this
//! file.

use renew_fixed::{Angle, Fixed, Vec3};
use renew_frame::{FrameLoop, StepBudget, Timestamp, Timestep};
use renew_platform::Clock;
use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowError, WindowRef, run_window_app,
};
use renew_render3d::{Camera as RenderCamera, CameraRenderer, attachment, pass};
use renew_rhi::{
    Device, DeviceDesc, DeviceError, Extent, Mesh, PresentOutcome, RenderDesc, Validation,
    WindowTarget,
};
use renew_sample_cube_world::{Cell, Cube, Grid, Intent, Tuning};

use crate::{Options, Report, arena};

/// How far one tick of held turn moves the view, in degrees.
///
/// A whole number of degrees per tick, so a quarter turn is an exact
/// number of ticks and a run that turns and returns ends where it began.
const TURN_DEGREES: i32 = 3;

/// How far pitch may travel from level, in degrees. Short of a right
/// angle on purpose: at exactly vertical the look direction and world up
/// are parallel and the camera basis has no unique answer.
const PITCH_LIMIT: i32 = 85;

/// The colour beyond the world. Not black, so the edge of the geometry
/// reads as an edge rather than as an unlit surface.
const SKY: renew_rhi::Color = renew_rhi::Color::new(
    renew_rhi::builtin::HORIZON[0],
    renew_rhi::builtin::HORIZON[1],
    renew_rhi::builtin::HORIZON[2],
    1.0,
);

/// What the player is holding down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a keyboard is a set of independent keys, and naming each one is what makes the mapping below readable"
)]
struct Held {
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    turn_left: bool,
    turn_right: bool,
    look_up: bool,
    look_down: bool,
    jump: bool,
    dig: bool,
    place: bool,
}

impl Held {
    /// The walk this frame, as a direction in the world's axes.
    ///
    /// Rotated by `yaw` so forward means where the player is facing, then
    /// rounded to the eight steps `Intent` can carry.
    fn walk(self, yaw: Angle) -> (i32, i32) {
        let forward = i32::from(self.forward) - i32::from(self.back);
        let strafe = i32::from(self.right) - i32::from(self.left);
        if forward == 0 && strafe == 0 {
            return (0, 0);
        }

        // Yaw zero looks along +z, so forward is (sin, cos) and the
        // strafe axis is that turned a quarter: (cos, -sin).
        let (sin, cos) = yaw.sin_cos();
        let x = sin * Fixed::from_int(forward) + cos * Fixed::from_int(strafe);
        let z = cos * Fixed::from_int(forward) - sin * Fixed::from_int(strafe);
        (step_of(x), step_of(z))
    }
}

/// One axis of a walk, rounded to the step the world accepts.
///
/// The threshold is a little under half a unit, so a direction pointing
/// mostly along one axis contributes to that axis alone and a genuine
/// diagonal contributes to both.
fn step_of(value: Fixed) -> i32 {
    let threshold = Fixed::from_ratio(2, 5);
    if value > threshold {
        1
    } else if value < -threshold {
        -1
    } else {
        0
    }
}

/// The game, running against a window.
pub struct CubeApp {
    world: Cube,
    held: Held,
    yaw: Angle,
    /// Pitch in whole degrees, clamped.
    ///
    /// **Not an `Angle`, and the reason is a trap.** `Angle` orders by
    /// its wrapping bits, so a pitch below level compares as *enormous*
    /// rather than as negative, and a limit test against it silently
    /// never fires. Degrees are a plain integer with a plain order.
    pitch_degrees: i32,
    ticks: u32,
    /// `None` = play until the window closes; `Some(n)` = stop after `n`
    /// ticks.
    ///
    /// **A window with a default bound is a game that quits itself.** The
    /// headless run needs a bound because nothing else would ever end it;
    /// a window already has an ending, and reusing the headless default
    /// here stopped play after six hundred ticks — ten seconds at sixty
    /// hertz — with nothing on screen saying why.
    limit: Option<u32>,
    closing: bool,
    /// The wall clock the frame loop reads. The only clock in the file,
    /// and it reaches the driver alone.
    clock: Clock,
    /// Absent until the window is up, so device bring-up is not banked as
    /// a burst of catch-up steps the moment play starts.
    frame: Option<FrameLoop>,
    /// The window's size in physical pixels, kept because recovering a
    /// swapchain needs the current size and the event that reports it is
    /// not the frame that discovers the loss.
    size: Extent,
    /// Why there is nothing to draw, when the reason is not "this machine
    /// has no GPU".
    ///
    /// **The two used to be the same thing**, five `.ok()?` calls that
    /// turned a driver refusal, an out-of-memory and a genuinely absent
    /// adapter alike into an empty window and a run that exited zero.
    /// Only the last of those is an outcome the sample was designed for.
    failure: Option<String>,
    /// The drawing half, which exists only once there is a window.
    ///
    /// `None` is not a failure: a machine with no adapter still runs the
    /// simulation and still answers with a digest, which is the half a
    /// test can check.
    gpu: Option<Gpu>,
}

/// Everything drawing needs, brought up once.
struct Gpu {
    device: Device,
    target: WindowTarget,
    renderer: CameraRenderer,
    /// The world's geometry, uploaded once and redrawn from every angle.
    ///
    /// **This is what putting the matrix on the GPU bought.** The mesh is
    /// immutable, and with the camera in the shader it never needs
    /// rebuilding as the player turns -- only when the blocks change.
    mesh: Mesh,
    /// The edit count the mesh was built from, so a dig or a place is
    /// noticed and nothing else provokes a rebuild.
    built_at: (u32, u32),
    /// The block the mesh was built showing as aimed at.
    ///
    /// Colour lives in the vertices, so moving the aim is a rebuild --
    /// the honest cost of putting the highlight there rather than in a
    /// second draw. It happens when the aim crosses from one block to
    /// another, not every time the view turns.
    aimed_at: Option<Cell>,
}

impl CubeApp {
    fn new(options: &Options) -> Self {
        let start = Vec3::new(Fixed::ZERO, Fixed::from_int(4), Fixed::ZERO);
        Self {
            world: Cube::new(Tuning::default(), arena(), start),
            held: Held::default(),
            yaw: Angle::ZERO,
            pitch_degrees: 0,
            ticks: 0,
            limit: options.window_ticks,
            closing: false,
            clock: Clock::start(),
            frame: None,
            size: Extent {
                width: 1,
                height: 1,
            },
            failure: None,
            gpu: None,
        }
    }

    /// Point the world where the view is pointing.
    ///
    /// The world stores a direction rather than two angles, so this is
    /// where the two representations meet — and it is fixed point on both
    /// sides, so nothing about the world's answer depends on the machine.
    fn aim(&mut self) {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = Angle::from_degrees(self.pitch_degrees).sin_cos();
        self.world.look_at(Vec3::new(
            sin_yaw * cos_pitch,
            sin_pitch,
            cos_yaw * cos_pitch,
        ));
    }

    /// One simulation step from what is held down.
    fn advance(&mut self) {
        if self.held.turn_left {
            self.yaw = self.yaw - Angle::from_degrees(TURN_DEGREES);
        }
        if self.held.turn_right {
            self.yaw = self.yaw + Angle::from_degrees(TURN_DEGREES);
        }
        let pitch = i32::from(self.held.look_up) - i32::from(self.held.look_down);
        self.pitch_degrees =
            (self.pitch_degrees + pitch * TURN_DEGREES).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.aim();

        let (walk_x, walk_z) = self.held.walk(self.yaw);
        self.world.step(Intent {
            walk_x,
            walk_z,
            jump: self.held.jump,
            dig: self.held.dig,
            place: self.held.place,
        });
        // Digging and placing are edges, not states: holding the key
        // should break one block, not one per tick.
        self.held.dig = false;
        self.held.place = false;
        self.ticks += 1;
    }

    /// Stand up a device, a window target and the geometry.
    ///
    /// **Three outcomes, not two.** `Ok(None)` is "this machine cannot
    /// draw" — no Vulkan runtime, or no adapter that fits — and is not a
    /// failure: the simulation still runs and still answers with a
    /// digest, which is the half a test can check and the half a
    /// headless machine is owed. `Err` is everything else: a driver that
    /// refused, memory that ran out, a pipeline that would not build. A
    /// player on a working machine is owed a sentence about those, and
    /// used to get an empty window instead.
    ///
    /// # Errors
    ///
    /// The failing call, named, carrying what the layer below said.
    fn bring_up(&self, window: &WindowRef<'_>) -> Result<Option<Gpu>, String> {
        let (width, height) = window.physical_size();
        let size = Extent { width, height };
        let device = match Device::new(&DeviceDesc {
            app_name: "cube",
            validation: Validation::IfAvailable,
        }) {
            Ok(device) => device,
            // The two ways a machine can simply not have the hardware.
            Err(DeviceError::LoaderUnavailable { .. } | DeviceError::NoSuitableAdapter { .. }) => {
                return Ok(None);
            }
            Err(error) => return Err(format!("creating the device: {error}")),
        };
        let target = device
            .create_window_target(window.native(), size)
            .map_err(|error| format!("creating the window target: {error}"))?;
        let renderer = CameraRenderer::new(&device, target.format())
            .map_err(|error| format!("building the camera pipeline: {error}"))?;
        let mesh = renderer
            .upload(
                &device,
                &crate::render::build_world_space(self.world.grid(), self.aim_cell()),
            )
            .map_err(|error| format!("uploading the world's geometry: {error}"))?;
        Ok(Some(Gpu {
            device,
            target,
            renderer,
            mesh,
            built_at: self.world.edits(),
            aimed_at: self.aim_cell(),
        }))
    }

    /// Draw one frame.
    fn draw(&mut self) {
        let camera = crate::camera::player_view(&self.world, self.aspect());
        let edits = self.world.edits();
        let aimed = self.aim_cell();
        let stale = self
            .gpu
            .as_ref()
            .is_some_and(|gpu| gpu.built_at != edits || gpu.aimed_at != aimed);
        let grid_scene = stale.then(|| crate::render::build_world_space(self.world.grid(), aimed));

        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        // Blocks changed, so the geometry has to be rebuilt. Turning does
        // not reach here -- that is the camera's job now.
        if let Some(scene) = grid_scene
            && let Ok(mesh) = gpu.renderer.upload(&gpu.device, &scene)
        {
            gpu.mesh = mesh;
            gpu.built_at = edits;
            gpu.aimed_at = aimed;
        }

        let packed = RenderCamera::from_columns(camera.view_projection());
        let color = [attachment(SKY)];
        let items = [gpu.renderer.item(&gpu.mesh, &packed)];
        let passes = [pass(&color, &items)];
        // **The outcome is the recovery signal, not noise.** `render`
        // never rebuilds a swapchain on its own; a target whose surface
        // has changed reports `NeedsResize` and stays dormant until
        // someone calls `resize`. Discarding this is how a window comes
        // back from the first resize showing the last frame it managed,
        // for ever.
        let outcome = gpu.target.render(&RenderDesc::new(&passes));
        if matches!(outcome, Ok(PresentOutcome::NeedsResize)) {
            let size = self.size;
            self.resize(size);
        }
    }

    /// Follow the window's size.
    ///
    /// **A refused resize is not the end of the picture.** The swapchain
    /// stays dormant, every later frame reports [`PresentOutcome::NeedsResize`],
    /// and each of those asks again — so a transient refusal costs a
    /// frame rather than the session. What ends the picture is never
    /// asking, which is what this file did before.
    fn resize(&mut self, size: Extent) {
        self.size = size;
        if let Some(gpu) = &mut self.gpu {
            drop(gpu.target.resize(size));
        }
    }

    /// Whether any reason to keep looping remains.
    ///
    /// The bound is checked at frame boundaries rather than mid-plan, so
    /// a lagging frame that owes several steps executes all of them and
    /// the reported tick count never disagrees with the world.
    fn done(&self) -> bool {
        self.failure.is_some()
            || self.closing
            || self.limit.is_some_and(|bound| self.ticks >= bound)
    }

    /// What a bring-up outcome means — split off so a test can drive the
    /// failing arm with a constructed value, no window and no driver.
    fn record_bring_up(&mut self, outcome: Result<Option<Gpu>, String>) {
        match outcome {
            Ok(gpu) => self.gpu = gpu,
            Err(message) => self.failure = Some(message),
        }
    }

    /// Why the run stopped, when it stopped for a reason worth saying.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// The update as a pure function of the frame's timestamp — the
    /// testable core, with the clock read left to the seam.
    fn update_at(&mut self, now: Timestamp, control: &mut LoopControl) {
        // Counted rather than iterated so the borrow of `frame` ends
        // before `advance` takes the whole of `self`. Every step is the
        // same call; only how many is in question.
        let due = match self.frame.as_mut() {
            Some(frame) => frame.begin_frame(now).steps().count(),
            // No window came up, so there is no clock anchor and nothing
            // to draw. One step a spin keeps the simulation moving for
            // whoever is reading the digest.
            None => 1,
        };
        for _ in 0..due {
            self.advance();
        }
        self.draw();
        if self.done() {
            control.exit();
        }
    }

    /// The block the player is aiming at, if any.
    fn aim_cell(&self) -> Option<Cell> {
        self.world.looking_at().map(|pick| pick.cell)
    }

    /// Width over height of the window, for the projection.
    fn aspect(&self) -> f32 {
        self.gpu.as_ref().map_or(1.0, |gpu| {
            let size = gpu.target.extent();
            if size.height == 0 {
                return 1.0;
            }
            f32::from(u16::try_from(size.width).unwrap_or(u16::MAX))
                / f32::from(u16::try_from(size.height).unwrap_or(u16::MAX))
        })
    }

    /// The world as the run left it.
    #[must_use]
    pub fn world(&self) -> &Cube {
        &self.world
    }

    /// The grid as the run left it.
    #[must_use]
    pub fn grid(&self) -> &Grid {
        self.world.grid()
    }
}

impl WindowApp for CubeApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        self.aim();
        let (width, height) = window.physical_size();
        self.size = Extent { width, height };
        let outcome = self.bring_up(window);
        self.record_bring_up(outcome);
        // Anchored after bring-up: device creation can take a noticeable
        // fraction of a second, and anchoring before it would bank that
        // as catch-up steps the player never asked for.
        self.frame = Some(FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(self.clock.elapsed_nanos()),
        ));
    }

    fn event(&mut self, event: renew_event::WindowEvent) {
        use renew_event::{KeyCode, PointerButton, WindowEvent};
        match event {
            WindowEvent::CloseRequested => self.closing = true,
            WindowEvent::Resized { width, height } => self.resize(Extent { width, height }),
            // **No key-up arrives for a key held when focus leaves.** Tab
            // away mid-stride and the player would walk into a wall until
            // the window came back and the key was pressed and released
            // again.
            WindowEvent::Focused(false) => self.held = Held::default(),
            // **The mouse does what the mouse does in this genre.** Left
            // breaks the block being aimed at, right places one against
            // it — the same two edges the keys carry, because a player
            // who has aimed at a block reaches for the button rather
            // than for a key named after the thing it is not.
            //
            // Edges, like the keys: holding a button breaks one block,
            // not one every tick.
            WindowEvent::PointerButton { button, pressed } => match button {
                PointerButton::Left => self.held.dig |= pressed,
                PointerButton::Right => self.held.place |= pressed,
                _ => {}
            },
            WindowEvent::Key { code, pressed, .. } => match code {
                KeyCode::KeyW => self.held.forward = pressed,
                KeyCode::KeyS => self.held.back = pressed,
                KeyCode::KeyA => self.held.left = pressed,
                KeyCode::KeyD => self.held.right = pressed,
                KeyCode::ArrowLeft => self.held.turn_left = pressed,
                KeyCode::ArrowRight => self.held.turn_right = pressed,
                KeyCode::ArrowUp => self.held.look_up = pressed,
                KeyCode::ArrowDown => self.held.look_down = pressed,
                KeyCode::Space => self.held.jump = pressed,
                // Edges rather than states: a held key should break one
                // block, not one every tick.
                KeyCode::Enter => self.held.dig |= pressed,
                KeyCode::Tab => self.held.place |= pressed,
                KeyCode::Escape => self.closing |= pressed,
                KeyCode::Unidentified => {}
            },
            _ => {}
        }
    }

    fn update(&mut self, control: &mut LoopControl) {
        let now = Timestamp::from_nanos(self.clock.elapsed_nanos());
        self.update_at(now, control);
    }
}

/// Play it.
///
/// # Errors
///
/// [`WindowError`] when no display server is reachable, which a caller
/// may reasonably treat as "run headless instead" rather than as a
/// failure.
pub fn run(options: &Options) -> Result<Report, WindowError> {
    let mut app = CubeApp::new(options);
    let config = WindowConfig {
        title: "cube".to_string(),
        logical_width: 960.0,
        logical_height: 720.0,
        resizable: true,
    };
    run_window_app(&config, &mut app)?;
    // Said out loud rather than folded into the return: the simulation
    // ran and its digest is real, so the report is still the answer —
    // but a player who asked to play and got an empty window is owed the
    // reason, on the stream reserved for it.
    if let Some(message) = app.failure() {
        eprintln!("cube: {message}");
    }
    Ok(crate::report_from(options.script, &app.world, "window"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app with no window, for the seam tests below.
    fn app() -> CubeApp {
        CubeApp::new(&Options::default())
    }

    /// The mouse carries the same two edges the keys do.
    #[test]
    fn the_mouse_breaks_and_places() {
        use renew_event::{PointerButton, WindowEvent};

        let mut breaking = app();
        breaking.event(WindowEvent::PointerButton {
            button: PointerButton::Left,
            pressed: true,
        });
        assert!(breaking.held.dig, "left breaks");
        assert!(!breaking.held.place, "and does not place");

        let mut other = app();
        other.event(WindowEvent::PointerButton {
            button: PointerButton::Right,
            pressed: true,
        });
        assert!(other.held.place, "right places");
        assert!(!other.held.dig, "and does not break");
    }

    /// A held button breaks one block, not one every tick — the same
    /// edge-not-state rule the keys follow, and the reason a `|=` and a
    /// clear in `advance` sit either side of it.
    #[test]
    fn a_held_mouse_button_breaks_one_block() {
        use renew_event::{PointerButton, WindowEvent};

        let mut app = app();
        app.event(WindowEvent::PointerButton {
            button: PointerButton::Left,
            pressed: true,
        });
        app.advance();
        assert!(!app.held.dig, "the edge is spent by the step that used it");

        // The button is still down; no new press arrived.
        app.advance();
        assert!(!app.held.dig, "a held button is not a second break");
    }

    /// Buttons this game has no use for change nothing.
    #[test]
    fn other_mouse_buttons_do_nothing() {
        use renew_event::{PointerButton, WindowEvent};

        let mut app = app();
        for button in [
            PointerButton::Middle,
            PointerButton::Back,
            PointerButton::Forward,
            PointerButton::Other(9),
        ] {
            app.event(WindowEvent::PointerButton {
                button,
                pressed: true,
            });
        }
        assert_eq!(app.held, Held::default(), "no button here is bound");
    }

    /// **A driver that refuses is not a machine without a GPU.** The two
    /// used to be the same `None`: an empty window, no explanation, and a
    /// run that exited zero as though nothing had gone wrong.
    #[test]
    fn a_failed_bring_up_is_reported_and_ends_the_run() {
        let mut app = app();
        assert_eq!(app.failure(), None, "nothing has failed yet");
        assert!(!app.done(), "and nothing has ended it");

        app.record_bring_up(Err("creating the device: out of host memory".to_string()));
        assert_eq!(
            app.failure(),
            Some("creating the device: out of host memory"),
            "the reason must survive to whoever reads it"
        );
        assert!(
            app.done(),
            "a run that cannot draw should stop, not spin drawing nothing"
        );
    }

    /// The other outcome: no adapter at all. Quiet on purpose — the
    /// simulation still runs and still answers with a digest, which is
    /// what a machine without a GPU is owed.
    #[test]
    fn a_machine_without_a_gpu_is_not_a_failure() {
        let mut app = app();
        app.record_bring_up(Ok(None));
        assert_eq!(app.failure(), None, "an absent adapter is not an error");
        assert!(!app.done(), "and the simulation should carry on without it");
    }

    /// **The window must not quit itself.** The headless run needs a tick
    /// bound because nothing else would end it; a window has an ending
    /// already, and borrowing the headless default stopped play ten
    /// seconds in.
    #[test]
    fn a_window_plays_until_it_is_closed() {
        let mut app = app();
        assert_eq!(app.limit, None, "no bound unless one was asked for");
        app.ticks = 100_000;
        assert!(!app.done(), "an unbounded run has no tick that ends it");
        app.closing = true;
        assert!(app.done(), "closing the window ends it");
    }

    /// A bound, when asked for, is honoured — the headless lanes and the
    /// windowed smoke test both depend on this.
    #[test]
    fn a_tick_bound_is_honoured_when_given() {
        let options = Options {
            window_ticks: Some(3),
            ..Options::default()
        };
        let mut app = CubeApp::new(&options);
        app.ticks = 2;
        assert!(!app.done(), "two of three ticks is not done");
        app.ticks = 3;
        assert!(app.done(), "the third tick ends it");
    }

    /// **The clock decides how many steps happen, not the panel.**
    /// Stepping once per redraw ran the world at the refresh rate: two
    /// and a half times faster on a 144 Hz screen than on a 60 Hz one.
    #[test]
    fn the_clock_decides_how_many_steps_happen() {
        let mut app = app();
        let start = Timestamp::from_nanos(0);
        app.frame = Some(FrameLoop::new(Timestep::HZ_60, StepBudget::DEFAULT, start));
        let mut control = LoopControl::default();

        app.update_at(start, &mut control);
        assert_eq!(app.ticks, 0, "no time has passed, so nothing is due");

        // Against the step length itself rather than a round number of
        // milliseconds: a sixtieth is 16_666_667 ns, so fifty
        // milliseconds is three steps *less one nanosecond* — the kind of
        // arithmetic that makes a test wrong rather than the code.
        let step = Timestep::HZ_60.nanos().get();
        app.update_at(Timestamp::from_nanos(3 * step), &mut control);
        assert_eq!(app.ticks, 3, "three step lengths are three steps");

        // Not quite a fourth: the shortfall stays in the accumulator.
        app.update_at(Timestamp::from_nanos(4 * step - 1), &mut control);
        assert_eq!(app.ticks, 3, "a step short of due is not due");

        // And the nanosecond that completes it delivers it, which is what
        // "the remainder is carried" means.
        app.update_at(Timestamp::from_nanos(4 * step), &mut control);
        assert_eq!(app.ticks, 4, "the carried remainder completes the step");
    }

    /// With no frame loop — no window came up — the simulation still
    /// moves, so a machine without an adapter still answers with a
    /// digest rather than a stall.
    #[test]
    fn without_a_window_the_simulation_still_advances() {
        let mut app = app();
        let mut control = LoopControl::default();
        app.update_at(Timestamp::from_nanos(0), &mut control);
        assert_eq!(app.ticks, 1, "one step a spin keeps the digest moving");
    }

    /// **Losing focus releases everything.** No key-up arrives for a key
    /// held when the window loses focus, so without this the player walks
    /// into a wall until the key is pressed and released again.
    #[test]
    fn losing_focus_releases_every_key() {
        use renew_event::WindowEvent;

        let mut app = app();
        app.held = Held {
            forward: true,
            jump: true,
            turn_left: true,
            ..Held::default()
        };
        app.event(WindowEvent::Focused(false));
        assert_eq!(
            app.held,
            Held::default(),
            "a key held across a focus change is a key held for ever"
        );
    }

    /// Regaining focus is not a reason to change anything: the keys are
    /// already released, and the player has not pressed one yet.
    #[test]
    fn regaining_focus_changes_nothing() {
        use renew_event::WindowEvent;

        let mut app = app();
        app.event(WindowEvent::Focused(true));
        assert_eq!(app.held, Held::default());
        assert!(!app.closing, "focus is not a close");
    }

    /// **A resize is followed, not dropped.** The size is kept because
    /// recovering a swapchain needs the current one, and the frame that
    /// discovers the loss is not the event that reported the size.
    #[test]
    fn a_resize_is_recorded_even_with_nothing_to_draw() {
        use renew_event::WindowEvent;

        let mut app = app();
        app.event(WindowEvent::Resized {
            width: 800,
            height: 600,
        });
        assert_eq!(
            app.size,
            Extent {
                width: 800,
                height: 600
            },
            "the size a recovery would resize to must be the current one"
        );
    }

    /// Closing is closing, however it arrives.
    #[test]
    fn either_way_of_closing_ends_the_run() {
        use renew_event::{KeyCode, WindowEvent};

        let mut closed = app();
        closed.event(WindowEvent::CloseRequested);
        assert!(closed.done());

        let mut escaped = app();
        escaped.event(WindowEvent::Key {
            code: KeyCode::Escape,
            pressed: true,
            repeat: false,
        });
        assert!(escaped.done(), "escape stops it too");
    }

    /// Forward means where the player is facing.
    ///
    /// The whole reason the driver rotates a key rather than handing it
    /// to the world: `Intent` walks the world's axes, so pressing forward
    /// while facing east has to become a step east.
    #[test]
    fn forward_follows_the_facing() {
        let held = Held {
            forward: true,
            ..Held::default()
        };
        assert_eq!(held.walk(Angle::ZERO), (0, 1), "facing north walks north");
        assert_eq!(
            held.walk(Angle::QUARTER),
            (1, 0),
            "a quarter turn walks east"
        );
        assert_eq!(held.walk(Angle::HALF), (0, -1), "half a turn walks south");
        assert_eq!(
            held.walk(Angle::THREE_QUARTERS),
            (-1, 0),
            "three quarters walks west"
        );
    }

    /// Strafing is a quarter turn from forward, and the two combine.
    #[test]
    fn strafing_is_square_to_walking_and_the_two_combine() {
        let right = Held {
            right: true,
            ..Held::default()
        };
        assert_eq!(right.walk(Angle::ZERO), (1, 0), "right of north is east");

        let both = Held {
            forward: true,
            right: true,
            ..Held::default()
        };
        assert_eq!(
            both.walk(Angle::ZERO),
            (1, 1),
            "forward and right is the diagonal between them"
        );
    }

    /// Opposite keys cancel rather than fighting.
    #[test]
    fn opposite_keys_cancel() {
        let held = Held {
            forward: true,
            back: true,
            left: true,
            right: true,
            ..Held::default()
        };
        assert_eq!(held.walk(Angle::ZERO), (0, 0));
        assert_eq!(Held::default().walk(Angle::QUARTER), (0, 0));
    }

    /// Pitch stops short of vertical, in both directions.
    ///
    /// At exactly vertical the look direction is parallel to world up and
    /// the camera basis has no unique answer — the picture would spin on
    /// its own axis for no input.
    #[test]
    fn pitch_stops_short_of_straight_up_and_down() {
        let mut app = CubeApp::new(&Options {
            ticks: 0,
            ..Options::default()
        });
        app.held.look_up = true;
        for _ in 0..200 {
            app.advance();
        }
        assert_eq!(
            app.pitch_degrees, PITCH_LIMIT,
            "pitch ran past its limit going up"
        );

        app.held.look_up = false;
        app.held.look_down = true;
        for _ in 0..400 {
            app.advance();
        }
        assert_eq!(
            app.pitch_degrees, -PITCH_LIMIT,
            "pitch ran past its limit going down"
        );
    }

    /// Digging is an edge: holding the key breaks one block, not one per
    /// tick.
    #[test]
    fn digging_is_an_edge_rather_than_a_state() {
        let mut app = CubeApp::new(&Options {
            ticks: 0,
            ..Options::default()
        });
        app.held.dig = true;
        app.advance();
        assert!(!app.held.dig, "the edge should be consumed by the step");
    }

    /// A turn and its opposite return the view exactly where it began.
    ///
    /// Fixed-point angles make this exact rather than approximate, which
    /// is the property a float would quietly lose.
    #[test]
    fn turning_back_returns_to_where_it_started() {
        let mut app = CubeApp::new(&Options {
            ticks: 0,
            ..Options::default()
        });
        let start = app.yaw;
        app.held.turn_right = true;
        for _ in 0..40 {
            app.advance();
        }
        app.held.turn_right = false;
        app.held.turn_left = true;
        for _ in 0..40 {
            app.advance();
        }
        assert_eq!(app.yaw, start, "turning back should be exact");
    }
}
