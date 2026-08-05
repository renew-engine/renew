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
use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowError, WindowRef, run_window_app,
};
use renew_render3d::{Camera as RenderCamera, CameraRenderer, attachment, pass};
use renew_rhi::{Device, DeviceDesc, Extent, Mesh, RenderDesc, Validation, WindowTarget};
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
const SKY: renew_rhi::Color = renew_rhi::Color::new(0.09, 0.10, 0.13, 1.0);

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
    limit: u32,
    closing: bool,
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
            limit: options.ticks,
            closing: false,
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
    /// Returns `None` rather than an error when there is no adapter: a
    /// machine without one should still be able to run the game's
    /// simulation and print its digest.
    fn bring_up(&self, window: &WindowRef<'_>) -> Option<Gpu> {
        let (width, height) = window.physical_size();
        let size = Extent { width, height };
        let device = Device::new(&DeviceDesc {
            app_name: "cube",
            validation: Validation::IfAvailable,
        })
        .ok()?;
        let target = device.create_window_target(window.native(), size).ok()?;
        let renderer = CameraRenderer::new(&device, target.format()).ok()?;
        let mesh = renderer
            .upload(
                &device,
                &crate::render::build_world_space(self.world.grid(), self.aim_cell()),
            )
            .ok()?;
        Some(Gpu {
            device,
            target,
            renderer,
            mesh,
            built_at: self.world.edits(),
            aimed_at: self.aim_cell(),
        })
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
        // A refused frame is not fatal: a resize can invalidate a
        // swapchain between one frame and the next, and the next frame
        // rebuilds it.
        let _ = gpu.target.render(&RenderDesc::new(&passes));
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
        self.gpu = self.bring_up(window);
    }

    fn event(&mut self, event: renew_event::WindowEvent) {
        use renew_event::{KeyCode, WindowEvent};
        match event {
            WindowEvent::CloseRequested => self.closing = true,
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
        self.advance();
        self.draw();
        if self.closing || (self.limit > 0 && self.ticks >= self.limit) {
            control.exit();
        }
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
    Ok(crate::report_from(options.script, &app.world, "window"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
