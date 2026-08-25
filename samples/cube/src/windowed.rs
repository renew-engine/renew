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
use renew_render3d::{MeshRenderer, ShadowedCamera, ShadowedCameraRenderer, pass};
use renew_rhi::{
    Device, DeviceDesc, DeviceError, Extent, ItemList, Mesh, PresentOutcome, RenderDesc,
    Validation, WindowTarget, color_attachment,
};
use renew_sample_cube_world::{Cell, Cube, Intent, Tuning};

use crate::Script;

use crate::{Options, Report, arena};

/// How far one tick of held turn moves the view, in degrees.
///
/// A whole number of degrees per tick, so a quarter turn is an exact
/// number of ticks and a run that turns and returns ends where it began.
const TURN_DEGREES: i32 = 3;

/// How far the view turns per unit the mouse reports, in hundredths of
/// a degree.
///
/// A stated unit rather than a tuned number: the platform reports motion
/// in its own scale, and this converts it. It is the first thing that
/// should become configurable if anyone asks for a sensitivity slider.
const MOUSE_HUNDREDTHS: i32 = 12;

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

/// Width over height, or 1.0 where that means nothing.
///
/// The engine camera crate's helper, thinly wrapped for the rendering
/// crate's `Extent`. This used to be a local implementation that
/// guarded only a zero height; the engine's guards both axes, which
/// closes the width-zero corner where the local one answered an aspect
/// of exactly zero — and a zero aspect through a projection is the
/// same blank-frame failure the zero-height guard exists to prevent.
fn aspect_of(size: Extent) -> f32 {
    renew_camera::aspect_of(size.width, size.height)
}

/// The game, running against a window.
struct CubeApp {
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
    /// Mouse movement not yet spent, in hundredths of a degree.
    ///
    /// **This is where the mouse's float stops.** Deltas arrive as
    /// `f64`; the world is fixed point and its digest must depend on
    /// nothing but its inputs. A float reaching `look_at` would make a
    /// played run's digest a function of the platform's floating-point
    /// behaviour, which is exactly what turning with `Angle` was written
    /// to avoid.
    ///
    /// So the float is converted here, once, into hundredths of a degree,
    /// and only whole degrees are ever handed to `Angle::from_degrees`.
    /// The remainder stays for the next event, so slow movement
    /// accumulates rather than rounding to nothing.
    turn_owed: (i32, i32),
    /// Whether the cursor is held. `false` on a platform that would not,
    /// which is ordinary rather than exceptional — the arrows still turn.
    cursor_held: bool,
    /// The wall clock the frame loop reads. The only clock in the file,
    /// and it reaches the driver alone.
    clock: Clock,
    /// Absent until the window is up, so device bring-up is not banked as
    /// a burst of catch-up steps the moment play starts.
    frame: Option<FrameLoop>,
    /// The view at the previous completed tick, kept so a draw between
    /// ticks can blend toward the current one instead of repeating it —
    /// display-rate smoothness through the frame loop's interpolation
    /// factor. Presentation state in floats: it is written from the
    /// world and never read back into it, so no digest sees it.
    previous_view: Option<renew_camera::View>,
    /// The window's size in physical pixels, kept because recovering a
    /// swapchain needs the current size and the event that reports it is
    /// not the frame that discovers the loss.
    size: Extent,
    /// A script driving the player instead of the keyboard.
    ///
    /// **`Stand` is genuinely idle**, so "no script" and "the default
    /// script" are the same run and this needs no separate flag: with
    /// nothing named, every tick's intent comes from the keys. With a
    /// script named, the window watches it play — which is a way to show
    /// the game without touching it, and the only way anything without a
    /// keyboard can drive the parts of this file that need one.
    script: Option<Script>,
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
    /// The block-break dust. It watches the world and never touches it,
    /// so it lives beside the simulation rather than inside it — the
    /// digest never learns particles exist. It steps even when no GPU
    /// came up, because what it does is a function of the world's
    /// breaks, not of whether anyone is looking.
    dust: renew_particles::ParticleSystem,
}

/// Everything drawing needs, brought up once.
struct Gpu {
    device: Device,
    target: WindowTarget,
    renderer: ShadowedCameraRenderer,
    /// The sun's view-projection, packed for the caster's push and
    /// kept as columns for the lit block — a constant of the arena,
    /// computed once at bring-up because neither the sun nor the
    light_columns: [[f32; 4]; 4],
    /// Which edit count the caster mesh was built from. Its own
    /// counter rather than sharing the world mesh's: the caster is a
    /// function of the blocks ALONE (`casting_scene` never sees the
    /// aim), so rebuilding it when the player merely turns would remesh
    /// the whole arena into byte-identical geometry — and tracking it
    /// separately is also what lets a refused upload be re-asked on the
    /// next frame instead of being skipped for that world state.
    cast_at: (u32, u32),
    /// The caster's own mesh: the world minus the roof, which is what
    /// lets a sun light a closed box. Rebuilt when the blocks change.
    caster_mesh: Mesh,
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
    /// The plain mesh pipeline, for geometry that is already clip space.
    overlay: MeshRenderer,
    /// The crosshair, built at bring-up and rebuilt when the window's
    /// shape changes.
    ///
    /// **Built at bring-up rather than lazily**, because that is where a
    /// device failure is already reported: built on the first frame, a
    /// refused upload would have to be either swallowed — leaving the
    /// game to draw without one for ever, silently — or turned into a
    /// branch that draws a frame with no crosshair, which no working
    /// device ever takes and no test can therefore reach.
    crosshair: Mesh,
    /// The aspect the crosshair was built for, so a resized window gets
    /// square arms again rather than keeping the old window's stretch.
    crosshair_aspect: f32,
    /// The dust pipeline, blended as media because dust occludes
    /// rather than glows. Built at bring-up for the crosshair's
    /// reason: that is where a device failure is already reported.
    sprinkler: renew_particles::ParticleRenderer,
    /// Instance scratch, sized once for the pool's whole capacity, so
    /// packing a frame's particles allocates nothing.
    instances: Vec<u8>,
}

impl CubeApp {
    fn new(options: &Options) -> Self {
        let start = Vec3::new(Fixed::ZERO, Fixed::from_int(4), Fixed::ZERO);
        let script = (options.script != Script::Stand).then_some(options.script);
        // A script that digs needs something under the aim, so a
        // scripted run starts looking down and forward — the same
        // arrangement the headless run makes for the same reason,
        // though from whole-degree angles rather than its exact look
        // vector, so the two runs aim at nearby rather than identical
        // cells. A played run starts looking level, because a player
        // would.
        let (yaw, pitch_degrees) = if script.is_some() {
            (Angle::from_degrees(45), -40)
        } else {
            (Angle::ZERO, 0)
        };
        Self {
            world: Cube::new(Tuning::default(), arena(), start),
            held: Held::default(),
            yaw,
            pitch_degrees,
            ticks: 0,
            limit: options.window_ticks,
            script,
            closing: false,
            dust: crate::burst::pool(),
            turn_owed: (0, 0),
            cursor_held: false,
            clock: Clock::start(),
            frame: None,
            previous_view: None,
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

    /// Forget the movement not yet spent.
    ///
    /// **The cursor itself is not this file's to release.** The window
    /// layer frees it when focus is lost and takes it back when focus
    /// returns, because a caller cannot change window state outside
    /// bring-up — and because a grab that had to be re-asked for after
    /// every alt-tab would be a grab that quietly stopped working.
    ///
    /// What is this file's is the turn: kept across the gap, it would
    /// snap the view round the moment a player came back.
    fn forget_owed_turn(&mut self) {
        self.turn_owed = (0, 0);
    }

    /// Turn the view by a mouse movement.
    ///
    /// **The float ends here.** The delta is scaled into hundredths of a
    /// degree and added to an integer owed; `advance` spends the whole
    /// degrees and keeps the rest. Nothing downstream of this sees
    /// anything but an `i32`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is rounded to an integer and clamped inside i32's range on the two                   lines above the cast, so there is no fraction left to truncate and no                   magnitude left to lose"
    )]
    fn aim_by_mouse(&mut self, dx: f64, dy: f64) {
        if !self.cursor_held {
            // Without a grab the cursor wanders out of the window mid-turn
            // and the view stops for no reason a player can see. Better to
            // do nothing and leave the arrows in charge.
            return;
        }
        let scale = f64::from(MOUSE_HUNDREDTHS);
        // Rounded to an integer *before* the conversion, and clamped to a
        // range an `i32` holds, so the cast can neither truncate a
        // fraction nor overflow. A delta big enough to reach the clamp is
        // a device reporting nonsense, and a clamp is the right answer to
        // that as much as to a real spin.
        let hundredths = |delta: f64| -> i32 {
            let scaled = (delta * scale).round();
            if !scaled.is_finite() {
                return 0;
            }
            // `i32::MAX / 2` as an f64 is exact, and the clamp brings the
            // value inside it, so `try_from` on the rounded integer part
            // cannot fail — the fallback is unreachable and says so.
            let bounded = scaled.clamp(-f64::from(i32::MAX / 2), f64::from(i32::MAX / 2));
            bounded as i32
        };
        self.turn_owed.0 = self.turn_owed.0.saturating_add(hundredths(dx));
        self.turn_owed.1 = self.turn_owed.1.saturating_add(hundredths(dy));
    }

    /// The whole degrees owed, taken out of the accumulator.
    ///
    /// Truncating toward zero, so a remainder keeps its sign and a slow
    /// drag in one direction accumulates instead of oscillating.
    fn spend_owed_turn(&mut self) -> (i32, i32) {
        let yaw = self.turn_owed.0 / 100;
        let pitch = self.turn_owed.1 / 100;
        self.turn_owed.0 -= yaw * 100;
        self.turn_owed.1 -= pitch * 100;
        (yaw, pitch)
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
        // Whole degrees the mouse has earned since the last step. Added
        // to the keys rather than replacing them: both are ways of
        // turning, and a player using one should not disable the other.
        let (mouse_yaw, mouse_pitch) = self.spend_owed_turn();
        self.yaw = self.yaw + Angle::from_degrees(mouse_yaw);
        // Down on the screen is down in pitch, which is the sign a player
        // expects: pushing the mouse forward looks up.
        self.pitch_degrees = (self.pitch_degrees + pitch * TURN_DEGREES - mouse_pitch)
            .clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.aim();

        let intent = if let Some(script) = self.script {
            script.intent(self.ticks)
        } else {
            let (walk_x, walk_z) = self.held.walk(self.yaw);
            Intent {
                walk_x,
                walk_z,
                jump: self.held.jump,
                dig: self.held.dig,
                place: self.held.place,
            }
        };
        // The aim before the step and the edit count after are enough to
        // know a block broke and where — so the dust needs nothing from
        // the world that the world does not already say.
        let watched = crate::burst::watch(&self.world);
        self.world.step(intent);
        crate::burst::settle(&mut self.dust, &self.world, watched);
        self.dust.step(crate::burst::DT);
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
        // The atlas is generated, so building the renderer is where it
        // is uploaded. One renderer, one atlas: every block in this world
        // samples the same sheet.
        let renderer = ShadowedCameraRenderer::new(
            &device,
            target.format(),
            Extent {
                width: crate::atlas::WIDTH,
                height: crate::atlas::HEIGHT,
            },
            &crate::atlas::pixels(),
            crate::render::SHADOW_MAP_SIZE,
        )
        .map_err(|error| format!("building the camera pipeline: {error}"))?;
        let grid = self.world.grid();
        let light_columns = crate::camera::sun_light(
            crate::render::low_corner(grid),
            crate::render::high_corner(grid),
        )
        .columns();
        let mesh = renderer
            .upload(
                &device,
                &crate::render::build_world_space(self.world.grid(), self.aim_cell()),
            )
            .map_err(|error| format!("uploading the world's geometry: {error}"))?;
        let caster_mesh = renderer
            .upload(&device, &crate::render::casting_scene(self.world.grid()))
            .map_err(|error| format!("uploading the caster's geometry: {error}"))?;
        let overlay = MeshRenderer::new(&device, target.format())
            .map_err(|error| format!("building the overlay pipeline: {error}"))?;
        let crosshair_aspect = aspect_of(size);
        let crosshair = overlay
            .upload(&device, &crate::crosshair::scene(crosshair_aspect))
            .map_err(|error| format!("uploading the crosshair: {error}"))?;
        let (tile_side, tile_pixels) = crate::atlas::particle_pixels();
        let sprinkler = renew_particles::ParticleRenderer::new(
            &device,
            target.format(),
            Extent {
                width: tile_side,
                height: tile_side,
            },
            &tile_pixels,
            renew_particles::ParticleBlend::Alpha,
            self.dust.capacity(),
        )
        .map_err(|error| format!("building the dust pipeline: {error}"))?;
        let instances = vec![0u8; self.dust.capacity() as usize * renew_particles::INSTANCE_STRIDE];
        Ok(Some(Gpu {
            device,
            target,
            renderer,
            light_columns,
            mesh,
            caster_mesh,
            cast_at: self.world.edits(),
            built_at: self.world.edits(),
            aimed_at: self.aim_cell(),
            overlay,
            crosshair,
            crosshair_aspect,
            sprinkler,
            instances,
        }))
    }

    /// Draw one frame, blending the view `alpha` of the way from the
    /// previous tick's toward the current one.
    ///
    /// The blend happens entirely on the float side — a lerp of eye and
    /// target — and nothing flows back into the world, so every digest
    /// and every replay comparison is untouched by how smoothly the
    /// picture moves.
    fn draw(&mut self, alpha: renew_math::Alpha) {
        let current = crate::camera::player_view(&self.world, self.aspect());
        let view = match self.previous_view {
            Some(previous) => renew_camera::View::blend(previous, current.view, alpha),
            None => current.view,
        };
        let camera = crate::camera::Camera {
            view,
            projection: current.projection,
        };
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

        // The caster is rebuilt when the BLOCKS change, which is not
        // the same event as the world mesh's: that one also rebuilds
        // when the aim moves, and the aim is invisible to the shadow.
        // A refused upload leaves `cast_at` behind, so the next frame
        // asks again rather than shadowing a world that no longer
        // exists for as long as the player stands still.
        if gpu.cast_at != edits
            && let Ok(caster) = gpu.renderer.upload(
                &gpu.device,
                &crate::render::casting_scene(self.world.grid()),
            )
        {
            gpu.caster_mesh = caster;
            gpu.cast_at = edits;
        }

        // A resize changes what "square" means, so the arms are rebuilt
        // rather than left with the old window's stretch. Two quads: not
        // a cost worth caching around. A refused rebuild keeps the arms
        // it has, which are stretched rather than absent.
        let wanted = aspect_of(gpu.target.extent());
        if (wanted - gpu.crosshair_aspect).abs() > f32::EPSILON
            && let Ok(rebuilt) = gpu
                .overlay
                .upload(&gpu.device, &crate::crosshair::scene(wanted))
        {
            gpu.crosshair = rebuilt;
            gpu.crosshair_aspect = wanted;
        }

        let packed = ShadowedCamera::from_columns(camera.columns(), gpu.light_columns);
        let color = [color_attachment(SKY)];
        // The frame's particles, packed into the scratch sized at
        // bring-up — a burst costs the steady-state loop no allocation.
        // The billboard basis comes from the same blended view the world
        // is drawn through, so the dust never lags the camera.
        let live = self.dust.write_instances(&mut gpu.instances);
        let (right, up, _) = camera.view.axes();
        let push = renew_particles::CameraPush::from_parts(
            camera.columns(),
            [right.x, right.y, right.z],
            [up.x, up.y, up.z],
        );
        let world = gpu.renderer.item(&gpu.mesh, &packed);
        // The caster pass leads every frame: the same world mesh as the
        // light sees it, depth only, into the map the lit item samples.
        let casting = [gpu.renderer.caster_item(&gpu.caster_mesh, &packed)];
        let shadow = gpu.renderer.shadow_pass(&casting);

        // **The outcome is the recovery signal, not noise.** `render`
        // never rebuilds a swapchain on its own; a target whose surface
        // has changed reports `NeedsResize` and stays dormant until
        // someone calls `resize`. Discarding this is how a window comes
        // back from the first resize showing the last frame it managed,
        // for ever.
        //
        // The world first, dust over it, the crosshair over everything:
        // the dust tests the world's depth without writing its own, and
        // a sight that hides behind smoke is not a sight. The overlay
        // sits at the near plane, so the depth test cannot put a block
        // in front of it; the order settles it anyway.
        //
        // A stack list rather than a `Vec`, because this runs once a
        // frame and the steady-state loop is meant to reach the heap
        // never — and rather than the two branches this used to carry,
        // which duplicated the render call to hold two array sizes. A
        // frame with no dust — most of them — pays for no empty draw.
        let mut items = ItemList::<3>::new(world);
        if live > 0 {
            items.push(gpu.sprinkler.item(&gpu.instances, live, &push));
        }
        items.push(gpu.overlay.item(&gpu.crosshair));
        let passes = [shadow, pass(&color, items.as_slice())];
        let outcome = gpu.target.render(&RenderDesc::new(&passes));
        self.record_present(&outcome);
    }

    /// What a present outcome means — split off so a test can drive the
    /// recovery with a constructed value, no window and no driver.
    ///
    /// A refused frame is not fatal and not silence either: a target
    /// whose surface has changed reports [`PresentOutcome::NeedsResize`]
    /// and stays dormant until someone calls `resize`, so this is where
    /// the picture comes back.
    fn record_present(&mut self, outcome: &Result<PresentOutcome, renew_rhi::TargetError>) {
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
        // same call; only how many is in question — plus how far into
        // the next step this frame lands, which is what the draw blends
        // by.
        let (due, alpha) = match self.frame.as_mut() {
            Some(frame) => {
                let plan = frame.begin_frame(now);
                (
                    plan.steps().count(),
                    renew_math::Alpha::new(plan.remainder().get(), Timestep::HZ_60.nanos()),
                )
            }
            // No window came up, so there is no clock anchor and nothing
            // to draw. One step a spin keeps the simulation moving for
            // whoever is reading the digest.
            None => (1, renew_math::Alpha::ZERO),
        };
        for _ in 0..due {
            // Snapshot before each step: after the loop this holds the
            // view at the tick before the last, which is what a blend
            // interpolates from. A frame with no step due keeps the
            // snapshot it has, and only alpha moves.
            self.previous_view = Some(crate::camera::player_eye_view(&self.world));
            self.advance();
        }
        self.draw(alpha);
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
        self.gpu
            .as_ref()
            .map_or(1.0, |gpu| aspect_of(gpu.target.extent()))
    }
}

impl WindowApp for CubeApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        self.aim();
        let (width, height) = window.physical_size();
        self.size = Extent { width, height };
        // **`false` is ordinary, not a failure.** Cursor confinement is
        // one of the places the three desktops differ; where it is
        // refused the arrows still turn and nothing is lost, which is
        // why nothing here reports it as a problem.
        self.cursor_held = window.grab_cursor(true);
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
            WindowEvent::Focused(false) => {
                self.held = Held::default();
                // The window layer frees the cursor itself and takes it
                // back when focus returns, so nothing here touches it.
                // What would survive the gap and should not is the turn
                // not yet spent.
                self.forget_owed_turn();
            }
            WindowEvent::PointerMotion { dx, dy } => self.aim_by_mouse(dx, dy),
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
                // **Escape ends the game, and that releases the cursor
                // with it.** A two-stage escape — free the cursor, then
                // quit — is the convention, and it needs the app to
                // change window state from an event, which this seam
                // cannot do: only `ready` is handed a window. Routing a
                // request back through the loop is a larger change than
                // the difference is worth while alt-tab already frees the
                // cursor by losing focus. Recorded rather than half-done.
                KeyCode::Escape => self.closing |= pressed,
                // Keys this game has nothing to do with. While the
                // vocabulary was seventeen curated keys this arm named
                // every one it ignored, so a new variant reddened the
                // match and got a decision; the vocabulary is the whole
                // keyboard now, and a sample enumerating every key it
                // ignores is stenography, not decision-making. The keys
                // this game answers to are the named arms above.
                _ => {}
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

    /// **A dormant swapchain is asked to come back.** `render` never
    /// rebuilds one on its own: it reports `NeedsResize` and stays
    /// dormant until `resize` is called, so a driver that discards this
    /// outcome shows the last frame it managed for ever.
    #[test]
    fn a_refused_frame_asks_for_the_swapchain_back() {
        let mut app = app();
        app.size = Extent {
            width: 640,
            height: 480,
        };

        // With nothing to draw the recovery is a no-op, which is the
        // point: it must not panic, and it must not lose the size the
        // recovery would use.
        app.record_present(&Ok(PresentOutcome::NeedsResize));
        assert_eq!(
            app.size,
            Extent {
                width: 640,
                height: 480
            },
            "the size a recovery resizes to must survive the recovery"
        );

        // A presented frame and a failed one both leave it alone.
        app.record_present(&Ok(PresentOutcome::Presented));
        app.record_present(&Err(renew_rhi::TargetError::DeviceLost));
        assert_eq!(
            app.size,
            Extent {
                width: 640,
                height: 480
            }
        );
        assert_eq!(
            app.failure(),
            None,
            "a refused frame is not a bring-up failure"
        );
    }

    /// **A named script drives the player; nothing named leaves the keys
    /// in charge.** `Stand` is idle, so the default and "no script" are
    /// the same run and no separate flag is needed to tell them apart.
    #[test]
    fn a_named_script_takes_over_from_the_keyboard() {
        let played = CubeApp::new(&Options::default());
        assert_eq!(played.script, None, "the default script is no script");

        let watched = CubeApp::new(&Options {
            script: Script::Build,
            ..Options::default()
        });
        assert_eq!(watched.script, Some(Script::Build));
        assert!(
            watched.pitch_degrees < 0,
            "a script that digs must start looking down at something"
        );
    }

    /// **A held key does not steer a scripted run.** The script is
    /// driving, and a run that answered to both would be neither
    /// watchable nor reproducible.
    #[test]
    fn keys_do_not_steer_a_scripted_run() {
        // The same script, once with a key held and once without. A
        // keyboard that reached the world would separate them.
        let mut quiet = CubeApp::new(&Options {
            script: Script::Patrol,
            ..Options::default()
        });
        let mut pressed = CubeApp::new(&Options {
            script: Script::Patrol,
            ..Options::default()
        });
        pressed.held.forward = true;
        pressed.held.jump = true;
        for _ in 0..40 {
            quiet.advance();
            pressed.advance();
        }
        assert_eq!(
            quiet.world.digest(),
            pressed.world.digest(),
            "a key reached a scripted world, so watching it would not be reproducible"
        );

        // And with nothing named, the keys do reach it.
        let mut playing = CubeApp::new(&Options::default());
        let mut still = CubeApp::new(&Options::default());
        playing.held.forward = true;
        for _ in 0..40 {
            playing.advance();
            still.advance();
        }
        assert_ne!(
            playing.world.digest(),
            still.world.digest(),
            "with no script the keys are what drives the world"
        );
    }

    /// **Every key the game binds, in one place.** The mapping is the
    /// game's whole interface; a binding that quietly stopped working
    /// would read as "the controls feel wrong" rather than as a failure,
    /// and no other test drives these arms.
    #[test]
    fn every_bound_key_moves_what_it_says_it_moves() {
        use renew_event::{KeyCode, WindowEvent};

        /// A key, and how to read what holding it set.
        type Binding = (KeyCode, fn(&Held) -> bool);

        let cases: [Binding; 9] = [
            (KeyCode::KeyW, |h| h.forward),
            (KeyCode::KeyS, |h| h.back),
            (KeyCode::KeyA, |h| h.left),
            (KeyCode::KeyD, |h| h.right),
            (KeyCode::ArrowLeft, |h| h.turn_left),
            (KeyCode::ArrowRight, |h| h.turn_right),
            (KeyCode::ArrowUp, |h| h.look_up),
            (KeyCode::ArrowDown, |h| h.look_down),
            (KeyCode::Space, |h| h.jump),
        ];
        for (code, reads) in cases {
            let mut app = app();
            app.event(WindowEvent::Key {
                code,
                pressed: true,
                repeat: false,
            });
            assert!(reads(&app.held), "{code:?} pressed did not register");
            app.event(WindowEvent::Key {
                code,
                pressed: false,
                repeat: false,
            });
            assert!(!reads(&app.held), "{code:?} released did not clear");
        }
    }

    /// The two edge keys, and the one that stops the game.
    #[test]
    fn the_edge_keys_and_escape() {
        use renew_event::{KeyCode, WindowEvent};

        /// A key, and how to read what it did to the app.
        type Effect = (KeyCode, fn(&CubeApp) -> bool);

        let cases: [Effect; 3] = [
            (KeyCode::Enter, |a| a.held.dig),
            (KeyCode::Tab, |a| a.held.place),
            (KeyCode::Escape, |a| a.closing),
        ];
        for (code, reads) in cases {
            let mut app = app();
            app.event(WindowEvent::Key {
                code,
                pressed: true,
                repeat: false,
            });
            assert!(reads(&app), "{code:?} did not take effect");
        }
    }

    /// The unidentified key changes nothing.
    ///
    /// It is the only unbound variant the event crate has — every other
    /// one this game binds — so this is the whole of the "does nothing"
    /// surface rather than a sample of it.
    #[test]
    fn an_unidentified_key_changes_nothing() {
        use renew_event::{KeyCode, WindowEvent};

        let mut app = app();
        app.event(WindowEvent::Key {
            code: KeyCode::Unidentified,
            pressed: true,
            repeat: false,
        });
        assert_eq!(app.held, Held::default());
        assert!(!app.closing);
    }

    /// An event this game ignores is ignored, rather than reaching some
    /// arm by accident.
    #[test]
    fn unhandled_events_are_ignored() {
        use renew_event::WindowEvent;

        let mut app = app();
        app.event(WindowEvent::RedrawRequested);
        app.event(WindowEvent::ScaleFactorChanged { scale: 2.0 });
        app.event(WindowEvent::Wheel { dx: 1.0, dy: -1.0 });
        app.event(WindowEvent::PointerMoved { x: 10.0, y: 20.0 });
        assert_eq!(app.held, Held::default());
        assert!(!app.closing);
        assert_eq!(app.failure(), None);
    }

    /// **A minimised window reports a height of zero**, and dividing by
    /// it would put an infinity into the projection — a blank frame that
    /// looks exactly like a driver failure and is not one.
    #[test]
    fn a_window_with_no_height_still_gives_a_usable_aspect() {
        let ordinary = aspect_of(Extent {
            width: 800,
            height: 600,
        });
        assert!((ordinary - 4.0 / 3.0).abs() < 1e-6, "got {ordinary}");
        let degenerate = aspect_of(Extent {
            width: 800,
            height: 0,
        });
        assert!(
            degenerate.is_finite() && degenerate > 0.0,
            "a zero height must not reach the projection as an infinity: {degenerate}"
        );
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

    /// Press a key, or let go of it.
    fn key(app: &mut CubeApp, code: renew_event::KeyCode, pressed: bool) {
        app.event(renew_event::WindowEvent::Key {
            code,
            pressed,
            repeat: false,
        });
    }

    /// An app with its clock anchored, ready to be played.
    fn session() -> (CubeApp, LoopControl, u64) {
        let mut app = app();
        app.frame = Some(FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(0),
        ));
        (app, LoopControl::default(), 0)
    }

    /// Let `ticks` of wall clock pass, a sixtieth at a time.
    fn play(app: &mut CubeApp, control: &mut LoopControl, ticks: u64, now: &mut u64) {
        let step = Timestep::HZ_60.nanos().get();
        for _ in 0..ticks {
            *now += step;
            app.update_at(Timestamp::from_nanos(*now), control);
        }
    }

    /// Stand on the floor rather than in the air, which is where a run
    /// starts and is not what any of this is about.
    fn land(app: &mut CubeApp, control: &mut LoopControl, now: &mut u64) {
        play(app, control, 60, now);
    }

    /// **Motion reaches the view through the event, not only the
    /// helper.** Every test above calls `aim_by_mouse` directly, which
    /// says nothing about whether the event is wired to it — and a
    /// mapping that compiled but was never matched would look exactly
    /// like a mouse nobody had plugged in.
    #[test]
    fn a_motion_event_turns_the_view() {
        use renew_event::WindowEvent;

        let mut app = app();
        app.cursor_held = true;
        app.event(WindowEvent::PointerMotion {
            dx: 500.0 / f64::from(MOUSE_HUNDREDTHS),
            dy: 0.0,
        });
        assert_eq!(
            app.turn_owed.0, 500,
            "the event should reach the accumulator the helper fills"
        );
    }

    /// **Slow movement accumulates rather than rounding to nothing.**
    /// Half a degree twice is one degree; a mouse moved gently would
    /// otherwise turn the view not at all, however long it was moved.
    #[test]
    fn a_turn_smaller_than_a_degree_is_owed_rather_than_lost() {
        let mut app = app();
        app.cursor_held = true;

        // Half a degree: fifty hundredths, which is under the hundred a
        // whole degree costs.
        let half_a_degree = 50.0 / f64::from(MOUSE_HUNDREDTHS);
        app.aim_by_mouse(half_a_degree, 0.0);
        assert_eq!(app.spend_owed_turn(), (0, 0), "half a degree is not one");
        assert_eq!(app.turn_owed.0, 50, "and it is still owed");

        app.aim_by_mouse(half_a_degree, 0.0);
        assert_eq!(
            app.spend_owed_turn(),
            (1, 0),
            "two halves make the degree the first one did not"
        );
        assert_eq!(app.turn_owed.0, 0, "and nothing is owed after it is spent");
    }

    /// The remainder keeps its sign, so a slow drag one way accumulates
    /// instead of oscillating about zero.
    #[test]
    fn a_remainder_keeps_its_direction() {
        let mut app = app();
        app.cursor_held = true;
        let one_and_a_half = 150.0 / f64::from(MOUSE_HUNDREDTHS);

        app.aim_by_mouse(-one_and_a_half, 0.0);
        assert_eq!(app.spend_owed_turn(), (-1, 0));
        assert_eq!(app.turn_owed.0, -50, "the leftover is still leftward");
    }

    /// **Nothing but whole degrees reaches the world.** The mouse's `f64`
    /// stops at the accumulator: a float reaching `look_at` would make a
    /// played run's digest a function of the platform's floating-point
    /// behaviour, which is what turning with `Angle` exists to avoid.
    #[test]
    fn the_world_only_ever_sees_whole_degrees() {
        let mut app = app();
        app.cursor_held = true;
        let before = app.world.look();

        // A movement worth well under a degree changes nothing at all,
        // rather than nudging the look direction by a fraction.
        app.aim_by_mouse(0.3, 0.2);
        app.advance();
        let after = app.world.look();

        // The yaw is unmoved because nothing whole was owed; the world's
        // own look is fixed point either way, and comparing it is the
        // check that no float slipped through.
        assert_eq!(
            app.yaw,
            Angle::ZERO,
            "a fraction of a degree turned the view"
        );
        assert_eq!(before.x, after.x, "the world's look moved by a fraction");
        assert_eq!(before.z, after.z, "the world's look moved by a fraction");
    }

    /// Pushing the mouse forward looks up, which is the sign a player
    /// expects, and pitch still stops short of vertical.
    #[test]
    fn the_mouse_pitches_the_way_a_player_expects_and_stops_short() {
        let mut app = app();
        app.cursor_held = true;

        // Forward is negative dy on every platform's screen coordinates.
        app.aim_by_mouse(0.0, -1000.0);
        app.advance();
        assert!(app.pitch_degrees > 0, "forward should look up");
        assert!(
            app.pitch_degrees <= PITCH_LIMIT,
            "pitch ran past the limit: {}",
            app.pitch_degrees
        );

        app.aim_by_mouse(0.0, 1_000_000.0);
        app.advance();
        assert!(
            app.pitch_degrees >= -PITCH_LIMIT,
            "pitch ran past the limit the other way: {}",
            app.pitch_degrees
        );
    }

    /// **Without a grab the mouse does nothing**, which is the whole of
    /// how this degrades on a platform that will not hold a cursor: the
    /// arrows still turn and nothing is lost.
    #[test]
    fn without_a_held_cursor_the_mouse_is_ignored() {
        let mut app = app();
        assert!(!app.cursor_held, "nothing has granted a grab");

        app.aim_by_mouse(500.0, 500.0);
        assert_eq!(
            app.turn_owed,
            (0, 0),
            "an ungrabbed cursor wanders out of the window mid-turn, so it drives nothing"
        );
    }

    /// Losing focus lets the cursor go and forgets what it owed.
    ///
    /// Both halves matter: a held cursor would trap a player who has
    /// tabbed away, and a turn left owed across the gap would snap the view
    /// round when they came back.
    #[test]
    fn losing_focus_forgets_the_turn_but_not_the_grab() {
        use renew_event::WindowEvent;

        let mut app = app();
        app.cursor_held = true;
        app.aim_by_mouse(50.0, 50.0);
        assert_ne!(app.turn_owed, (0, 0));

        app.event(WindowEvent::Focused(false));
        assert!(
            app.cursor_held,
            "the window layer owns the grab across a focus change; this file must not fight it"
        );
        assert_eq!(
            app.turn_owed,
            (0, 0),
            "a kept owed turn snaps the view on return"
        );
    }

    /// A device reporting nonsense turns the view a lot, and does not
    /// overflow the accumulator doing it.
    #[test]
    fn an_absurd_delta_is_clamped_rather_than_wrapping() {
        let mut app = app();
        app.cursor_held = true;

        for delta in [f64::MAX, f64::INFINITY, f64::NAN, -f64::MAX] {
            app.turn_owed = (0, 0);
            app.aim_by_mouse(delta, delta);
            let (yaw, pitch) = app.spend_owed_turn();
            assert!(
                yaw.abs() <= i32::MAX / 100 && pitch.abs() <= i32::MAX / 100,
                "delta {delta} produced {yaw}, {pitch}"
            );
        }
    }

    /// **Played, not simulated: the keys move the player where they are
    /// looking.** Every other test here checks a part — the mapping, the
    /// rotation, the pacing. This presses keys and asks the world what
    /// happened, which is the only thing that catches two correct parts
    /// wired together wrongly.
    #[test]
    fn a_played_session_walks_where_it_is_facing() {
        use renew_event::KeyCode;

        let (mut app, mut control, mut now) = session();
        land(&mut app, &mut control, &mut now);
        let landed = app.world.eye();

        // Yaw zero looks along +z, so forward is north.
        key(&mut app, KeyCode::KeyW, true);
        play(&mut app, &mut control, 40, &mut now);
        key(&mut app, KeyCode::KeyW, false);
        let north = app.world.eye();
        assert!(
            north.z > landed.z,
            "holding forward at yaw zero should walk north: {landed:?} to {north:?}"
        );
        assert!(
            (north.x - landed.x).abs() <= Fixed::from_ratio(1, 4),
            "and should not drift sideways: {landed:?} to {north:?}"
        );

        // Back is not another word for forward.
        key(&mut app, KeyCode::KeyS, true);
        play(&mut app, &mut control, 40, &mut now);
        key(&mut app, KeyCode::KeyS, false);
        let returned = app.world.eye();
        assert!(
            returned.z < north.z,
            "back should undo forward: {north:?} to {returned:?}"
        );

        // A quarter turn to the right, then forward: now east. Three
        // degrees a tick, so thirty ticks is ninety degrees.
        key(&mut app, KeyCode::ArrowRight, true);
        play(&mut app, &mut control, 30, &mut now);
        key(&mut app, KeyCode::ArrowRight, false);
        key(&mut app, KeyCode::KeyW, true);
        play(&mut app, &mut control, 40, &mut now);
        key(&mut app, KeyCode::KeyW, false);
        let east = app.world.eye();
        assert!(
            east.x > returned.x,
            "after a quarter turn, forward should walk east: {returned:?} to {east:?}"
        );

        assert!(!app.done(), "a played session should still be playable");
        assert_eq!(app.failure(), None);
    }

    /// The other half of playing it: the world changes when you tell it
    /// to, and refuses when it should.
    #[test]
    fn a_played_session_digs_and_places_and_jumps() {
        use renew_event::KeyCode;

        let (mut app, mut control, mut now) = session();
        land(&mut app, &mut control, &mut now);

        // Look down. Seven ticks of three degrees is about twenty, which
        // from standing height puts the aim a few blocks ahead rather
        // than at the player's own feet.
        key(&mut app, KeyCode::ArrowDown, true);
        play(&mut app, &mut control, 7, &mut now);
        key(&mut app, KeyCode::ArrowDown, false);
        let floor = app
            .aim_cell()
            .expect("looking down from standing height should reach the floor");
        assert_eq!(floor.y, 0, "the thing ahead and below is the arena's floor");

        // **The shell refuses to be dug**, which is why this world has a
        // mound in it at all: a player with only the box in reach could
        // break nothing. A test that dug the floor would assert the
        // opposite of what this world does.
        let (shell_broken, _) = app.world.edits();
        key(&mut app, KeyCode::Enter, true);
        key(&mut app, KeyCode::Enter, false);
        play(&mut app, &mut control, 2, &mut now);
        assert_eq!(
            app.world.edits().0,
            shell_broken,
            "the arena's shell must not be breakable, or the box can be walked out of"
        );

        // The mound sits east of where the player starts. Turn to it and
        // walk, then look down at it.
        key(&mut app, KeyCode::ArrowUp, true);
        play(&mut app, &mut control, 7, &mut now);
        key(&mut app, KeyCode::ArrowUp, false);
        key(&mut app, KeyCode::ArrowRight, true);
        play(&mut app, &mut control, 30, &mut now);
        key(&mut app, KeyCode::ArrowRight, false);
        key(&mut app, KeyCode::KeyW, true);
        play(&mut app, &mut control, 40, &mut now);
        key(&mut app, KeyCode::KeyW, false);
        key(&mut app, KeyCode::ArrowDown, true);
        play(&mut app, &mut control, 7, &mut now);
        key(&mut app, KeyCode::ArrowDown, false);

        let mound = app
            .aim_cell()
            .expect("the mound should be in reach after walking to it");
        assert!(
            mound.y >= 1,
            "the mound stands above the floor, and the floor cannot be dug: {mound:?}"
        );

        // Break it, and put one back. The refused shell dig above must
        // not have burst — dust marks breaks, not attempts.
        assert_eq!(app.dust.live(), 0, "no break yet, so no dust yet");
        let (broken_before, placed_before) = app.world.edits();
        key(&mut app, KeyCode::Enter, true);
        key(&mut app, KeyCode::Enter, false);
        play(&mut app, &mut control, 2, &mut now);
        assert_eq!(
            app.world.edits(),
            (broken_before + 1, placed_before),
            "enter should break exactly the block aimed at, and place nothing"
        );
        assert_eq!(
            app.dust.live(),
            crate::burst::BURST,
            "a windowed dig must burst the same way the headless one does"
        );

        key(&mut app, KeyCode::Tab, true);
        key(&mut app, KeyCode::Tab, false);
        play(&mut app, &mut control, 2, &mut now);
        assert_eq!(
            app.world.edits().1,
            placed_before + 1,
            "tab should place one block against what is aimed at"
        );

        // Jumping leaves the ground, which is the whole of what it does.
        let before_jump = app.world.eye();
        key(&mut app, KeyCode::Space, true);
        play(&mut app, &mut control, 4, &mut now);
        key(&mut app, KeyCode::Space, false);
        let airborne = app.world.eye();
        assert!(
            airborne.y > before_jump.y,
            "space should leave the ground: {before_jump:?} to {airborne:?}"
        );
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

        // With no cursor held, one press ends it.
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
