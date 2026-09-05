//! The world's picture, as data: what a renderer draws, with no
//! renderer in sight.
//!
//! Pure on purpose — this module is the game's share of the drawing
//! story that works without a GPU crate in the graph. Consumers (an
//! offscreen oracle, a windowed mode) map [`SceneSprite`] onto their
//! sprite type themselves; the mapping is a handful of lines each, and
//! that duplication is cheaper than a GPU edge on the game.

use renew_math::Alpha;
use renew_sample_glide_world::{
    BIRD_HALF_UNITS, BIRD_X_UNITS, PIPE_GAP_HALF_UNITS, PIPE_WIDTH_UNITS, TERMINAL_VELOCITY,
    UNITS_PER_PIXEL, VIEW_HEIGHT, World,
};
use renew_snapshot::{Blend, Key, Snapshots};

/// Which picture a sprite shows. Deliberately CLOSED, against the
/// grain of the input enums elsewhere: a new tile must break every
/// consumer's match, forcing an atlas region to exist for it before
/// anything compiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    /// The bird's square body.
    Bird,
    /// One pipe bar (either half; the gap is the absence between them).
    Pipe,
    /// One spark of the crash burst — a white texel the tint colours,
    /// drawn as light rather than ink.
    Spark,
}

/// One rectangle of the picture, in canvas units (the world's own
/// screen units; y down from the top-left).
///
/// `#[non_exhaustive]` without a constructor — a deliberate deviation
/// from the descriptor pattern: this is a read-side record produced only
/// by this module, by [`scene`] and by [`Presentation::fill`], never
/// built by callers, so a constructor would have no caller outside this
/// file.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct SceneSprite {
    /// Which picture.
    pub tile: Tile,
    /// Left edge, canvas units.
    pub x: f32,
    /// Top edge, canvas units.
    pub y: f32,
    /// Width, canvas units.
    pub width: f32,
    /// Height, canvas units.
    pub height: f32,
    /// How much of the sprite's own colour survives: `1.0` for all of
    /// it, `0.0` for grey at the same luminance. A dead bird is drawn
    /// grey; everything else keeps its colour.
    pub saturation: f32,
    /// Turn about the rectangle's centre, in turns, clockwise on
    /// screen. `0.0` for everything that does not tilt — every pipe.
    pub rotation: f32,
    /// How far the sprite is smeared, in canvas units, along the
    /// direction it moved: the displacement to average the sprite over,
    /// which reads as motion blur. `[0.0, 0.0]` for everything that does
    /// not move — every pipe, and a dead bird.
    pub smear: [f32; 2],
    /// Premultiplied tint, multiplied into the sprite after everything
    /// else. `[1.0; 4]` — no tint — for every sprite the world itself
    /// produces; a spark carries its colour here, with **alpha zero**,
    /// which is what makes it add light instead of covering what is
    /// under it.
    pub tint: [f32; 4],
}

/// The tint a sprite carries when it has none: premultiplied white,
/// which multiplies through unchanged.
///
/// Named rather than written out at four call sites, so "no tint" is one
/// decision and reads as one.
const UNTINTED: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// The most sprites a frame of this game can hold.
///
/// Five pipes as two bars each, the bird, and a full spark pool. Named
/// once so the windowed driver and the offscreen oracle size the same
/// batch — two hardcoded numbers used to say 32, which was headroom
/// before the sparks existed and would be a refusal now.
pub const SPRITE_BUDGET: u32 = 2 * PIPE_SLOTS + 1 + 32;

/// The steepest the bird tilts, in turns — an eighth, so a terminal
/// dive is forty-five degrees nose-down and a fresh flap is thirty-three
/// and three quarters degrees nose-up.
const MAX_TILT: f32 = 0.125;

/// The tilt a vertical velocity earns, in turns, clockwise on screen.
///
/// Linear in the velocity and clamped at the ends of the range the
/// world can actually produce, so the steepest dive and the freshest
/// flap are the extremes and nothing outside them is representable. The
/// velocity arrives in **world units per tick** rather than canvas
/// units: the tick-exact scene reads it straight from the world and the
/// blended one interpolates two such readings, and both must reach the
/// same function or the two pictures would tilt by different rules.
#[allow(
    clippy::cast_precision_loss,
    reason = "the velocity is bounded by the flap and terminal constants, far below f32's exact range"
)]
pub(crate) fn tilt(velocity: f32) -> f32 {
    (velocity / TERMINAL_VELOCITY as f32).clamp(-1.0, 1.0) * MAX_TILT
}

/// How many ticks of motion the bird is drawn averaged over.
///
/// An exaggerated exposure, chosen so the ghost is visible at this
/// resolution rather than because a real camera works this way. A fall
/// accelerates from nothing and tops out at [`TERMINAL_VELOCITY`],
/// which is one and two tenths of a canvas unit per tick, so the widest
/// smear this can ever ask for is `8 × 1.2 = 9.6` units on a twelve-unit
/// body. Four ticks would top out at 4.8 — a ramp of two pixels at the
/// scale the pictures are drawn, which nobody would call a ghost.
const SMEAR_TICKS: f32 = 8.0;

/// How far a bird is smeared, in canvas units, from its velocity.
///
/// Vertical only, because the bird's horizontal position never changes —
/// the world scrolls past it. A dead bird does not smear: it is a corpse
/// falling out of the frame, and a step at the moment of death is what
/// the greying already says.
///
/// The velocity arrives in **world units per tick**, the same units
/// [`tilt`] takes and for the same reason; this one converts to canvas
/// units because a smear is a distance on screen rather than a fraction
/// of a range.
#[allow(
    clippy::cast_precision_loss,
    reason = "the velocity is bounded by the flap and terminal constants, far below f32's exact range"
)]
fn smear(velocity: f32, alive: bool) -> [f32; 2] {
    if alive {
        [0.0, velocity / UNITS_PER_PIXEL as f32 * SMEAR_TICKS]
    } else {
        [0.0, 0.0]
    }
}
/// Fill `out` with the world's picture, in draw order: every pipe as
/// two bars (top bar from the ceiling to the gap, bottom bar from the
/// gap to the floor), then the bird over them. Pipe order is the
/// store's ascending-slot walk, so the picture is a property of the
/// rules; a dead world still draws — the corpse is a legal scene.
///
/// Clears `out` first; with a caller-preallocated vector the fill
/// allocates nothing once capacity has been reached.
#[allow(
    clippy::cast_precision_loss,
    reason = "canvas units are bounded by the view constants, far below f32's exact range"
)]
pub fn scene(world: &World, out: &mut Vec<SceneSprite>) {
    out.clear();
    world.for_each_pipe_units(|x, gap_y| push_pipe(out, x as f32, gap_y as f32));
    let v = velocity(world.bird_velocity());
    push_bird(
        out,
        world.bird_y_units() as f32,
        tilt(v),
        saturation(world.alive()),
        smear(v, world.alive()),
    );
}

/// One pipe's two bars, from the pipe's left edge and gap centre: the
/// top bar from the ceiling down to the gap, the bottom bar from the gap
/// down to the floor.
///
/// The one derivation, shared by the tick-exact fill above and the
/// blended one below. Two copies of this arithmetic would be two places
/// for the gap's half-height to drift.
#[allow(
    clippy::cast_precision_loss,
    reason = "canvas units are bounded by the view constants, far below f32's exact range"
)]
fn push_pipe(out: &mut Vec<SceneSprite>, x: f32, gap_y: f32) {
    let half = PIPE_GAP_HALF_UNITS as f32;
    let gap_top = gap_y - half;
    let gap_bottom = gap_y + half;
    out.push(SceneSprite {
        tile: Tile::Pipe,
        x,
        y: 0.0,
        width: PIPE_WIDTH_UNITS as f32,
        height: gap_top,
        // A pipe never tilts: it is the fixed frame the bird moves in.
        rotation: 0.0,
        // A pipe neither dies nor moves under its own power.
        saturation: 1.0,
        smear: [0.0, 0.0],
        tint: UNTINTED,
    });
    out.push(SceneSprite {
        tile: Tile::Pipe,
        x,
        y: gap_bottom,
        width: PIPE_WIDTH_UNITS as f32,
        height: VIEW_HEIGHT as f32 - gap_bottom,
        rotation: 0.0,
        // A pipe neither dies nor moves under its own power.
        saturation: 1.0,
        smear: [0.0, 0.0],
        tint: UNTINTED,
    });
}

/// How much colour a bird keeps: all of it alive, none of it dead.
///
/// A bool rather than a float on the presentation side, and blended
/// nowhere: death is a step, not a slide, and interpolating it would
/// draw a half-grey bird for one frame at every death.
fn saturation(alive: bool) -> f32 {
    if alive { 1.0 } else { 0.0 }
}

/// The bird's square body, from its centre's y and its tilt.
#[allow(
    clippy::cast_precision_loss,
    reason = "canvas units are bounded by the view constants, far below f32's exact range"
)]
fn push_bird(
    out: &mut Vec<SceneSprite>,
    centre_y: f32,
    rotation: f32,
    saturation: f32,
    smear: [f32; 2],
) {
    let half = BIRD_HALF_UNITS as f32;
    out.push(SceneSprite {
        tile: Tile::Bird,
        x: (BIRD_X_UNITS - BIRD_HALF_UNITS) as f32,
        y: centre_y - half,
        width: 2.0 * half,
        height: 2.0 * half,
        rotation,
        saturation,
        smear,
        tint: UNTINTED,
    });
}

/// The most pipe slots presentation is sized for.
///
/// The rules bound live pipes well under this: they spawn every ninety
/// ticks and are culled once fully past the left edge, and the world's
/// own test pins the peak. Sixteen is that with room to spare, and
/// `Capture::put` refuses by name rather than silently dropping a pipe if
/// the rules ever outgrow it — a refusal being the outcome that gets
/// noticed.
const PIPE_SLOTS: u32 = 16;

/// One world unit as a screen unit.
#[expect(
    clippy::cast_precision_loss,
    reason = "world coordinates are bounded by the view constants, far below f32's exact range"
)]
fn units(world_units: i64) -> f32 {
    world_units as f32 / UNITS_PER_PIXEL as f32
}

/// A world velocity as a float, in world units per tick.
///
/// Not divided by [`UNITS_PER_PIXEL`] like [`units`] above: [`tilt`]
/// maps from the world's own range, so converting here would mean
/// converting back there.
#[allow(
    clippy::cast_precision_loss,
    reason = "the velocity is bounded by the flap and terminal constants, far below f32's exact range"
)]
fn velocity(world_units: i64) -> f32 {
    world_units as f32
}

/// One pipe's blendable locals.
///
/// The locals, deliberately, and not the two bars derived from them: two
/// derived rectangles blended directly interpolate along a chord the
/// derivation would never have drawn, and the gap would breathe as the
/// pipe moved. Blend what the pipe *is*, then derive what it looks like.
///
/// A named struct rather than `[f32; 2]`, because a transposed column in
/// an anonymous array is invisible at the call site.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PipeAt {
    /// Left edge, screen units.
    pub x: f32,
    /// Gap centre, screen units.
    pub gap_y: f32,
}

impl Blend for PipeAt {
    fn blend(from: Self, to: Self, alpha: Alpha) -> Self {
        Self {
            x: f32::blend(from.x, to.x, alpha),
            gap_y: f32::blend(from.gap_y, to.gap_y, alpha),
        }
    }
}

/// The game's picture between two ticks.
///
/// The world steps sixty times a second and frames arrive faster, so
/// drawing current state means every pipe holds position for a frame and
/// then jumps. This keeps the last two ticks and draws between them.
#[derive(Debug)]
pub struct Presentation {
    pipes: Snapshots<PipeAt>,
    /// The bird is not an entity — three scalars on the world — so it is
    /// one previous value and one blend, which is the whole of what a
    /// singleton needs. A key is wanted exactly when a slot can be
    /// recycled, and this one cannot be.
    bird_y: f32,
    /// The bird's velocity in **world** units per tick, not canvas
    /// units: `tilt` maps from the world's own range, and blending
    /// before tilting is what keeps this picture and the tick-exact one
    /// on the same function.
    bird_velocity: f32,
    previous_bird_velocity: Option<f32>,
    /// Whether the bird was alive at the newest capture. A bool, and
    /// deliberately not blended: death is a step, and interpolating it
    /// would grey the bird halfway for one frame.
    bird_alive: bool,
    previous_bird_y: Option<f32>,
}

impl Presentation {
    /// A presentation seeded from the world as it stands, so the first
    /// frame blends out of a real tick rather than out of a zero.
    ///
    /// Only the bird is seeded, and only the bird needs to be: a new
    /// world holds no pipes at all — the first appears inside a step —
    /// so there is nothing for the pipe captures to start from.
    #[must_use]
    pub fn new(world: &World) -> Self {
        Self {
            pipes: Snapshots::new(PIPE_SLOTS),
            bird_y: units(world.bird_y()),
            bird_velocity: velocity(world.bird_velocity()),
            previous_bird_velocity: None,
            bird_alive: world.alive(),
            previous_bird_y: None,
        }
    }

    /// Capture this tick's locals. Call once per **executed** step, after
    /// the step — a frame that runs three catch-up steps captures three
    /// times, or the earlier capture is stale and the blend spans the
    /// wrong interval.
    pub fn capture(&mut self, world: &World) {
        let mut capture = self.pipes.capture();
        world.for_each_pipe(|slot, generation, x, gap_y| {
            capture.put(
                Key::new(slot, u64::from(generation)),
                PipeAt {
                    x: units(x),
                    gap_y: units(gap_y),
                },
            );
        });
        self.previous_bird_y = Some(self.bird_y);
        self.previous_bird_velocity = Some(self.bird_velocity);
        self.bird_y = units(world.bird_y());
        self.bird_velocity = velocity(world.bird_velocity());
        self.bird_alive = world.alive();
    }

    /// Fill `out` with the picture standing `alpha` of the way from the
    /// second-newest capture to the newest, in [`scene`]'s draw order:
    /// pipes as two bars each, then the bird over them.
    ///
    /// **So the picture lags the world by up to one tick.** At a zero
    /// factor this draws the tick before last, and it reaches the last
    /// tick only as the factor approaches one. That is inherent to
    /// interpolating between two known states rather than extrapolating
    /// past the newest one, and it is the usual trade: extrapolation has
    /// no lag and overshoots instead, which shows as a rubber-band
    /// correction on every direction change.
    ///
    /// A pipe that left between the two captures draws once more at its
    /// last known place, underneath the living. It is still on screen
    /// there — the cull fires once a pipe is fully past the left edge,
    /// which happens *after* the move that took it there — so dropping it
    /// would pop a visible sliver out at a tick boundary.
    pub fn fill(&self, alpha: Alpha, out: &mut Vec<SceneSprite>) {
        out.clear();
        for drawn in self.pipes.frame(alpha) {
            push_pipe(out, drawn.value.x, drawn.value.gap_y);
        }
        let y = match self.previous_bird_y {
            Some(previous) => f32::blend(previous, self.bird_y, alpha),
            None => self.bird_y,
        };
        let velocity = match self.previous_bird_velocity {
            Some(previous) => f32::blend(previous, self.bird_velocity, alpha),
            None => self.bird_velocity,
        };
        push_bird(
            out,
            y,
            tilt(velocity),
            saturation(self.bird_alive),
            smear(velocity, self.bird_alive),
        );
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    reason = "test coordinates are small integers, exact in f32"
)]
mod tests {
    use super::*;
    use renew_sample_glide_world::FLAP_VELOCITY;

    /// Exact float claims compare bits, the math crate's own pattern:
    /// every expected value is an integer-valued f32.
    fn b(value: f32) -> u32 {
        value.to_bits()
    }

    /// Seed 7 piloted to a tick, the world crate's committed-fixture
    /// method: expectations below are observed values, and the world's
    /// own tests pin the same run's accessors.
    fn piloted(ticks: u64) -> World {
        let mut world = World::new(7);
        for _ in 0..ticks {
            let flap = world.autopilot();
            world.step(flap);
        }
        world
    }

    #[test]
    fn the_scene_is_two_bars_per_pipe_then_the_bird() {
        let world = piloted(361);
        let mut out = Vec::new();
        scene(&world, &mut out);

        // Accessor-consistency: every visited pipe appears as exactly
        // two bars at the accessor-reported coordinates, in visit
        // order; the bird derives from bird_y_units. The accessors are
        // the independent witness for the geometry arithmetic.
        let mut expected = Vec::new();
        world.for_each_pipe_units(|x, gap| expected.push((x, gap)));
        assert_eq!(out.len(), expected.len() * 2 + 1, "two bars + bird");
        for (index, (x, gap)) in expected.iter().enumerate() {
            let top = out[index * 2];
            let bottom = out[index * 2 + 1];
            assert_eq!(top.tile, Tile::Pipe);
            assert_eq!(bottom.tile, Tile::Pipe);
            assert_eq!((b(top.x), b(top.y)), (b(*x as f32), b(0.0)));
            assert_eq!(b(top.height), b((*gap - PIPE_GAP_HALF_UNITS) as f32));
            assert_eq!(b(bottom.x), b(*x as f32));
            assert_eq!(b(bottom.y), b((*gap + PIPE_GAP_HALF_UNITS) as f32));
            assert_eq!(
                b(bottom.y + bottom.height),
                b(VIEW_HEIGHT as f32),
                "the bottom bar reaches the floor"
            );
        }
        let bird = out[out.len() - 1];
        assert_eq!(bird.tile, Tile::Bird);
        assert_eq!(
            (b(bird.x), b(bird.y), b(bird.width), b(bird.height)),
            (
                b(34.0),
                b((world.bird_y_units() - BIRD_HALF_UNITS) as f32),
                b(12.0),
                b(12.0)
            )
        );
        assert_eq!(
            b(bird.rotation),
            b(tilt(world.bird_velocity() as f32)),
            "the bird's tilt is its velocity's, bit for bit"
        );
        assert_eq!(b(bird.saturation), b(1.0), "a living bird keeps its colour");
        assert_eq!(
            bird.tint.map(f32::to_bits),
            [1.0f32.to_bits(); 4],
            "the bird carries no tint either — only a spark does"
        );
        assert_eq!(
            (b(bird.smear[0]), b(bird.smear[1])),
            (
                b(0.0),
                b(world.bird_velocity() as f32 / UNITS_PER_PIXEL as f32 * SMEAR_TICKS)
            ),
            "the bird smears along its fall by eight ticks of it, and never sideways"
        );
        for pipe in &out[..out.len() - 1] {
            assert_eq!(b(pipe.rotation), b(0.0), "a pipe never tilts");
            assert_eq!(
                (b(pipe.smear[0]), b(pipe.smear[1])),
                (b(0.0), b(0.0)),
                "a pipe never smears"
            );
            assert_eq!(
                pipe.tint.map(f32::to_bits),
                [1.0f32.to_bits(); 4],
                "a sprite the world produced carries no tint"
            );
        }
    }

    /// The tilt's two ends and its clamp: a flap points the nose up, a
    /// terminal dive points it down by the full eighth turn, and nothing
    /// faster tilts further.
    #[test]
    fn a_flap_tilts_the_bird_up_and_a_dive_tilts_it_down_within_the_clamp() {
        assert!(
            tilt(FLAP_VELOCITY as f32) < 0.0,
            "a flap must tilt the nose up"
        );
        assert_eq!(
            b(tilt(TERMINAL_VELOCITY as f32)),
            b(0.125),
            "a terminal dive is the full eighth turn"
        );
        assert_eq!(
            b(tilt(5_000.0)),
            b(tilt(TERMINAL_VELOCITY as f32)),
            "nothing faster than terminal tilts further"
        );
        assert_eq!(
            b(tilt(-5_000.0)),
            b(-0.125),
            "and the clamp is symmetric, though the world never gets there"
        );
        assert_eq!(b(tilt(0.0)), b(0.0), "a still bird is level");
    }

    /// A corpse keeps the tilt death left it with, because a dead world
    /// stops integrating: stepping it further moves neither the velocity
    /// nor the tilt drawn from it.
    ///
    /// Observed, the crate's committed-fixture method: falling from the
    /// start without a flap, seed 7 hits the floor on tick 108 at the
    /// terminal velocity, so the corpse lies at the full eighth turn.
    /// That is the picture `sink-240` shows, and the number its
    /// structural check reads.
    #[test]
    fn a_corpse_keeps_the_tilt_death_left_it_with() {
        let mut world = World::new(7);
        let mut ticks = 0;
        while world.alive() {
            world.step(false);
            ticks += 1;
        }
        assert_eq!(ticks, 108, "observed: the fall reaches the floor here");
        assert_eq!(
            world.bird_velocity(),
            TERMINAL_VELOCITY,
            "observed: the fall is at terminal by the time it lands"
        );
        let mut out = Vec::new();
        scene(&world, &mut out);
        let at_death = out[out.len() - 1].rotation;
        assert_eq!(b(at_death), b(0.125), "the corpse lies nose-down, fully");
        for _ in 0..30 {
            world.step(false);
        }
        assert_eq!(
            world.bird_velocity(),
            TERMINAL_VELOCITY,
            "a dead world stopped integrating"
        );
        scene(&world, &mut out);
        assert_eq!(
            b(out[out.len() - 1].rotation),
            b(at_death),
            "the corpse's tilt moved after death"
        );
    }

    #[test]
    fn observed_pin_and_reuse_across_fills() {
        // The hardcoded half of the method: tick 361's first bar comes
        // from the pipe truncated to -4 with its gap at 148 — numbers
        // pinned in the world's own tests, re-pinned here through the
        // mapping. And a second fill into the same vector replaces, not
        // appends.
        let world = piloted(361);
        let mut out = Vec::new();
        scene(&world, &mut out);
        assert_eq!(out.len(), 11, "five pipes and the bird, observed");
        assert_eq!((b(out[0].x), b(out[0].height)), (b(-4.0), b(118.0)));
        let len = out.len();
        scene(&world, &mut out);
        assert_eq!(out.len(), len, "a fill replaces the previous fill");
    }

    #[test]
    fn a_dead_world_still_draws() {
        let mut world = World::new(7);
        for _ in 0..240 {
            world.step(false);
        }
        assert!(!world.alive(), "gravity won by tick 240");
        let mut out = Vec::new();
        scene(&world, &mut out);
        assert!(
            out.len() > 1,
            "the corpse and the frozen pipes are a legal scene"
        );
        assert_eq!(out[out.len() - 1].tile, Tile::Bird);
        assert_eq!(
            b(out[out.len() - 1].y),
            b(228.0),
            "observed: frozen at death"
        );
        assert_eq!(
            b(out[out.len() - 1].saturation),
            b(0.0),
            "a dead bird is drawn grey"
        );
        assert_eq!(
            (
                b(out[out.len() - 1].smear[0]),
                b(out[out.len() - 1].smear[1])
            ),
            (b(0.0), b(0.0)),
            "a corpse does not smear, whatever velocity it froze at"
        );
        for pipe in &out[..out.len() - 1] {
            assert_eq!(b(pipe.saturation), b(1.0), "a pipe keeps its colour");
        }
    }

    /// Half a step past the boundary — exact in binary, so expectations
    /// are literals rather than tolerances.
    fn half() -> Alpha {
        Alpha::new(1, core::num::NonZeroU64::new(2).expect("two"))
    }

    /// Step the world once and capture it, the pairing the driver makes.
    fn advance(presentation: &mut Presentation, world: &mut World) {
        let flap = world.autopilot();
        world.step(flap);
        presentation.capture(world);
    }

    /// **The defect this fill exists to remove.** The whole-unit reading
    /// truncates a pipe's 0.9-units-per-tick motion, so its reported
    /// position holds still for one tick in every ten. Captured locals
    /// keep the sub-unit part, so the same run moves every tick.
    #[test]
    fn captured_motion_has_no_stalls_the_truncated_reading_has() {
        let mut world = piloted(200);
        let mut presentation = Presentation::new(&world);
        // Warm the pair. Reading at the boundary hands back the EARLIER
        // capture, so an unwarmed pair reports its first tick twice and
        // the duplicate would be this harness rather than the code.
        presentation.capture(&world);

        let mut truncated: Vec<i32> = Vec::new();
        let mut captured: Vec<f32> = Vec::new();
        let mut scratch = Vec::new();
        for _ in 0..24 {
            let mut first_truncated = None;
            world.for_each_pipe_units(|x, _| {
                if first_truncated.is_none() {
                    first_truncated = Some(x);
                }
            });
            if let Some(x) = first_truncated {
                truncated.push(x);
            }
            // Step first, then capture: the boundary reading hands back
            // the earlier capture, so this records the very state the
            // truncated reading above was taken from.
            let flap = world.autopilot();
            world.step(flap);
            presentation.capture(&world);
            presentation.fill(Alpha::ZERO, &mut scratch);
            captured.push(scratch[0].x);
        }

        // Bit equality, the module's own pattern: a stall IS two identical
        // readings, so an epsilon would be answering a different question.
        let stalls = |series: &[f32]| series.windows(2).filter(|w| b(w[0]) == b(w[1])).count();
        let truncated_stalls = truncated.windows(2).filter(|w| w[0] == w[1]).count();
        assert!(
            truncated_stalls > 0,
            "premise: the whole-unit reading must really stall, or this test proves nothing \
             (series {truncated:?})"
        );
        assert_eq!(
            stalls(&captured),
            0,
            "captured locals must move every tick (series {captured:?})"
        );
        for pair in captured.windows(2) {
            assert!(
                pair[1] < pair[0],
                "and always leftward, never jittering back: {pair:?}"
            );
        }
    }

    /// A pipe that left the screen draws once more where it last stood,
    /// then stops.
    ///
    /// **Why the recycling case is not tested here.** The rule that a
    /// newcomer must not be blended out of its slot's previous tenant is
    /// held — and constructed directly — in the crate that owns the pair.
    /// This game cannot reach it: measured over three thousand ticks, a
    /// vacated slot is never reused on the tick it is freed, and instead
    /// sits empty for seventy-seven ticks before a new pipe takes it. The
    /// previous capture therefore never holds the corpse when the
    /// newcomer appears, so every new pipe here is a newborn and no
    /// streak is expressible. A test asserting otherwise would pass
    /// against a missing guard, which is worse than no test.
    #[test]
    fn a_departed_pipe_draws_once_more_and_then_stops() {
        let mut world = piloted(120);
        let mut presentation = Presentation::new(&world);
        presentation.capture(&world);

        let mut scratch = Vec::new();
        let mut departure = None;
        for _ in 0..900 {
            let live_before = pipe_slots(&world);
            advance(&mut presentation, &mut world);
            let live_after = pipe_slots(&world);
            if live_after.len() < live_before.len() {
                presentation.fill(Alpha::ZERO, &mut scratch);
                let drawn = scratch.iter().filter(|s| s.tile == Tile::Pipe).count();
                departure = Some((drawn, live_after.len()));
                break;
            }
        }
        let (drawn_on_the_tick_it_left, live) =
            departure.expect("a pipe must leave the screen within nine hundred ticks");
        assert_eq!(
            drawn_on_the_tick_it_left,
            (live + 1) * 2,
            "the departing pipe is still drawn, as two bars, beside the {live} that remain"
        );

        // One more capture with nothing else changing, and it is gone.
        let live = pipe_slots(&world).len();
        advance(&mut presentation, &mut world);
        presentation.fill(Alpha::ZERO, &mut scratch);
        let drawn = scratch.iter().filter(|s| s.tile == Tile::Pipe).count();
        assert_eq!(
            drawn,
            live * 2,
            "and then it stops being drawn rather than lingering — exactly the {live} live \
             pipes, as two bars each, and no more"
        );
    }

    /// The slots the world currently holds pipes in.
    fn pipe_slots(world: &World) -> Vec<u32> {
        let mut slots = Vec::new();
        world.for_each_pipe(|slot, _, _, _| slots.push(slot));
        slots
    }

    #[test]
    fn the_blend_stands_at_its_captured_ticks() {
        let mut world = piloted(200);
        let mut presentation = Presentation::new(&world);
        presentation.capture(&world);
        let mut earlier = Vec::new();
        presentation.fill(Alpha::ZERO, &mut earlier);
        let earlier_bird = earlier
            .iter()
            .find(|s| s.tile == Tile::Bird)
            .copied()
            .expect("a bird is always drawn");

        advance(&mut presentation, &mut world);
        let mut at_zero = Vec::new();
        presentation.fill(Alpha::ZERO, &mut at_zero);
        let bird_at_zero = at_zero
            .iter()
            .find(|s| s.tile == Tile::Bird)
            .copied()
            .expect("a bird is always drawn");
        assert_eq!(
            b(bird_at_zero.y),
            b(earlier_bird.y),
            "at the boundary the picture is the earlier capture, bit for bit"
        );

        let mut at_half = Vec::new();
        presentation.fill(half(), &mut at_half);
        let bird_at_half = at_half
            .iter()
            .find(|s| s.tile == Tile::Bird)
            .copied()
            .expect("a bird is always drawn");
        assert_ne!(
            b(bird_at_half.y),
            b(earlier_bird.y),
            "and past it the picture has actually moved"
        );
    }

    /// Two identical captures leave nothing to interpolate, so the
    /// blended fill must agree with the tick-exact oracle at every
    /// factor — within the whole unit the oracle truncates away.
    #[test]
    fn the_blended_fill_agrees_with_the_oracle_at_a_standstill() {
        let world = piloted(240);
        let mut presentation = Presentation::new(&world);
        presentation.capture(&world);
        presentation.capture(&world);

        let mut oracle = Vec::new();
        scene(&world, &mut oracle);
        let mut blended = Vec::new();
        for step in 0..8u64 {
            let alpha = Alpha::new(
                step * 125,
                core::num::NonZeroU64::new(1000).expect("nonzero"),
            );
            presentation.fill(alpha, &mut blended);
            assert_eq!(
                blended.len(),
                oracle.len(),
                "the same picture, sprite for sprite"
            );
            for (drawn, expected) in blended.iter().zip(&oracle) {
                assert_eq!(drawn.tile, expected.tile, "and in the same order");
                assert!(
                    (drawn.x - expected.x).abs() < 1.0 && (drawn.y - expected.y).abs() < 1.0,
                    "within the unit the oracle truncates: {drawn:?} against {expected:?}"
                );
            }
        }
    }

    #[test]
    fn pipe_slots_stay_inside_the_budget() {
        let mut world = World::new(7);
        let mut highest = 0;
        for _ in 0..3_000 {
            let flap = world.autopilot();
            world.step(flap);
            world.for_each_pipe(|slot, _, _, _| highest = highest.max(slot));
        }
        // Asserted rather than handled: the pilot survives the whole run,
        // so a restart arm here would be a branch nothing reaches, and a
        // dead world would silently stop spawning and make the budget
        // claim vacuous.
        assert!(
            world.alive(),
            "the pilot must survive, or nothing was spawning"
        );
        assert!(
            highest < PIPE_SLOTS,
            "the rules allocated slot {highest}, past the presentation budget of {PIPE_SLOTS}"
        );
    }

    /// **The claim the blended fill exists to make.** Between two ticks a
    /// pipe stands strictly between where it was and where it is, and it
    /// gets there monotonically as the factor rises.
    ///
    /// Without this the whole fill could ignore its factor for pipes and
    /// every other test here would still pass: the rest read at the
    /// boundary, or through the bird, or across two identical captures.
    #[test]
    fn a_pipe_between_ticks_stands_between_its_captured_positions() {
        let mut world = piloted(200);
        let mut presentation = Presentation::new(&world);
        presentation.capture(&world);
        let earlier = pipe_lefts(&presentation, Alpha::ZERO);
        advance(&mut presentation, &mut world);
        let later = pipe_lefts_at_one(&presentation);
        assert!(!earlier.is_empty(), "premise: there must be pipes to move");
        assert_eq!(
            earlier.len(),
            later.len(),
            "premise: the same pipes both ticks"
        );
        assert!(
            earlier.iter().zip(&later).all(|(was, is)| is < was),
            "premise: the pipes must actually have moved left ({earlier:?} then {later:?})"
        );

        let mut previous = earlier.clone();
        for step in 1..=4u64 {
            let alpha = Alpha::new(
                step * 200,
                core::num::NonZeroU64::new(1000).expect("nonzero"),
            );
            let between = pipe_lefts(&presentation, alpha);
            for (index, drawn) in between.iter().enumerate() {
                let (was, is) = (earlier[index], later[index]);
                let held = previous[index];
                assert!(
                    *drawn < was && *drawn > is,
                    "at factor {step}/5 pipe {index} stands at {drawn}, outside ({is} .. {was})"
                );
                assert!(
                    *drawn < held,
                    "and each step of the factor must move pipe {index} further than {held}, \
                     never hold it"
                );
            }
            previous = between;
        }
    }

    /// The bird interpolates too, and by the same rule.
    #[test]
    fn the_bird_between_ticks_stands_between_its_captured_heights() {
        let mut world = piloted(205);
        let mut presentation = Presentation::new(&world);
        presentation.capture(&world);
        let earlier = bird_top(&presentation, Alpha::ZERO);
        advance(&mut presentation, &mut world);
        let later = bird_top_at_one(&presentation);
        assert!(
            b(earlier) != b(later),
            "premise: the bird must actually have moved between these ticks"
        );
        let middle = bird_top(&presentation, half());
        let low = earlier.min(later);
        let high = earlier.max(later);
        assert!(
            middle > low && middle < high,
            "the half-way bird stands at {middle}, outside ({low} .. {high})"
        );
    }

    /// A frozen pair with a frozen factor repeats the picture it was
    /// frozen at — which is what a paused game rests on, the driver
    /// holding the factor still being the other half of that contract.
    ///
    /// Asserting only that two calls agree would be a tautology: `fill`
    /// is a pure function, so it would pass with the body deleted. The
    /// picture is therefore compared against the one taken before the
    /// freeze, and asserted non-empty.
    #[test]
    fn a_frozen_pair_at_a_frozen_factor_repeats_the_picture_it_froze_at() {
        let mut world = piloted(300);
        let mut presentation = Presentation::new(&world);
        presentation.capture(&world);
        advance(&mut presentation, &mut world);
        let mut before_the_pause = Vec::new();
        presentation.fill(half(), &mut before_the_pause);
        assert!(
            before_the_pause.iter().any(|s| s.tile == Tile::Pipe),
            "premise: the frozen picture must contain something to freeze"
        );
        // The world keeps stepping; nothing is captured, exactly as a
        // paused driver behaves.
        for _ in 0..4 {
            let flap = world.autopilot();
            world.step(flap);
            let mut again = Vec::new();
            presentation.fill(half(), &mut again);
            assert_eq!(
                before_the_pause, again,
                "an uncaptured world must not reach the picture"
            );
        }
    }

    /// Every pipe's left edge at `alpha`, in draw order.
    fn pipe_lefts(presentation: &Presentation, alpha: Alpha) -> Vec<f32> {
        let mut out = Vec::new();
        presentation.fill(alpha, &mut out);
        out.iter()
            .filter(|s| s.tile == Tile::Pipe)
            .map(|s| s.x)
            .step_by(2)
            .collect()
    }

    /// The same, as close to the newer capture as the factor can get.
    fn pipe_lefts_at_one(presentation: &Presentation) -> Vec<f32> {
        pipe_lefts(
            presentation,
            Alpha::new(u64::MAX, core::num::NonZeroU64::new(1).expect("one")),
        )
    }

    fn bird_top(presentation: &Presentation, alpha: Alpha) -> f32 {
        let mut out = Vec::new();
        presentation.fill(alpha, &mut out);
        out.iter()
            .find(|s| s.tile == Tile::Bird)
            .map(|s| s.y)
            .expect("a bird is always drawn")
    }

    fn bird_top_at_one(presentation: &Presentation) -> f32 {
        bird_top(
            presentation,
            Alpha::new(u64::MAX, core::num::NonZeroU64::new(1).expect("one")),
        )
    }
}
