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
    BIRD_HALF_UNITS, BIRD_X_UNITS, PIPE_GAP_HALF_UNITS, PIPE_WIDTH_UNITS, UNITS_PER_PIXEL,
    VIEW_HEIGHT, World,
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
}

/// One rectangle of the picture, in canvas units (the world's own
/// screen units; y down from the top-left).
///
/// `#[non_exhaustive]` without a constructor — a deliberate deviation
/// from the descriptor pattern: this is a read-side record produced
/// only by [`scene`], never built by callers, so the constructor would
/// have exactly one caller and it lives in this file.
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
    push_bird(out, world.bird_y_units() as f32);
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
    });
    out.push(SceneSprite {
        tile: Tile::Pipe,
        x,
        y: gap_bottom,
        width: PIPE_WIDTH_UNITS as f32,
        height: VIEW_HEIGHT as f32 - gap_bottom,
    });
}

/// The bird's square body, from its centre's y.
#[allow(
    clippy::cast_precision_loss,
    reason = "canvas units are bounded by the view constants, far below f32's exact range"
)]
fn push_bird(out: &mut Vec<SceneSprite>, centre_y: f32) {
    let half = BIRD_HALF_UNITS as f32;
    out.push(SceneSprite {
        tile: Tile::Bird,
        x: (BIRD_X_UNITS - BIRD_HALF_UNITS) as f32,
        y: centre_y - half,
        width: 2.0 * half,
        height: 2.0 * half,
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
    previous_bird_y: Option<f32>,
}

impl Presentation {
    /// A presentation seeded from the world as it stands, so the first
    /// frame blends out of a real tick rather than out of a zero.
    #[must_use]
    pub fn new(world: &World) -> Self {
        Self {
            pipes: Snapshots::new(PIPE_SLOTS),
            bird_y: units(world.bird_y()),
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
        self.bird_y = units(world.bird_y());
    }

    /// Fill `out` with the picture standing `alpha` past the last
    /// capture, in [`scene`]'s draw order: pipes as two bars each, then
    /// the bird over them.
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
        push_bird(out, y);
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    reason = "test coordinates are small integers, exact in f32"
)]
mod tests {
    use super::*;

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
            let before = {
                presentation.fill(Alpha::ZERO, &mut scratch);
                scratch.iter().filter(|s| s.tile == Tile::Pipe).count()
            };
            let live_before = pipe_slots(&world);
            advance(&mut presentation, &mut world);
            let live_after = pipe_slots(&world);
            if live_after.len() < live_before.len() {
                presentation.fill(Alpha::ZERO, &mut scratch);
                let drawn = scratch.iter().filter(|s| s.tile == Tile::Pipe).count();
                departure = Some((before, drawn, live_after.len()));
                break;
            }
        }
        let (_, drawn_on_the_tick_it_left, live) =
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
        assert!(
            drawn <= live * 2,
            "and then it stops being drawn rather than lingering"
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
            if !world.alive() {
                world = World::new(7);
            }
        }
        assert!(
            highest < PIPE_SLOTS,
            "the rules allocated slot {highest}, past the presentation budget of {PIPE_SLOTS}"
        );
    }

    /// A frozen pair with a frozen factor is the same picture every
    /// frame. This is what a paused game rests on — the driver holds the
    /// factor still, and this is the half of that contract living here.
    #[test]
    fn a_frozen_pair_at_a_frozen_factor_repeats_exactly() {
        let world = piloted(300);
        let mut presentation = Presentation::new(&world);
        presentation.capture(&world);
        let mut first = Vec::new();
        presentation.fill(half(), &mut first);
        for _ in 0..4 {
            let mut again = Vec::new();
            presentation.fill(half(), &mut again);
            assert_eq!(first, again, "nothing moves while nothing is captured");
        }
    }
}
