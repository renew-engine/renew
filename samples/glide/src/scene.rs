//! The world's picture, as data: what a renderer draws, with no
//! renderer in sight.
//!
//! Pure on purpose — this module is the game's share of the drawing
//! story that works without a GPU crate in the graph. Consumers (an
//! offscreen oracle, a windowed mode) map [`SceneSprite`] onto their
//! sprite type themselves; the mapping is a handful of lines each, and
//! that duplication is cheaper than a GPU edge on the game.

use renew_sample_glide_world::{
    BIRD_HALF_UNITS, BIRD_X_UNITS, PIPE_GAP_HALF_UNITS, PIPE_WIDTH_UNITS, VIEW_HEIGHT, World,
};

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
    let height = VIEW_HEIGHT as f32;
    world.for_each_pipe_units(|x, gap_y| {
        let gap_top = (gap_y - PIPE_GAP_HALF_UNITS) as f32;
        let gap_bottom = (gap_y + PIPE_GAP_HALF_UNITS) as f32;
        out.push(SceneSprite {
            tile: Tile::Pipe,
            x: x as f32,
            y: 0.0,
            width: PIPE_WIDTH_UNITS as f32,
            height: gap_top,
        });
        out.push(SceneSprite {
            tile: Tile::Pipe,
            x: x as f32,
            y: gap_bottom,
            width: PIPE_WIDTH_UNITS as f32,
            height: height - gap_bottom,
        });
    });
    out.push(SceneSprite {
        tile: Tile::Bird,
        x: (BIRD_X_UNITS - BIRD_HALF_UNITS) as f32,
        y: (world.bird_y_units() - BIRD_HALF_UNITS) as f32,
        width: (2 * BIRD_HALF_UNITS) as f32,
        height: (2 * BIRD_HALF_UNITS) as f32,
    });
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
}
