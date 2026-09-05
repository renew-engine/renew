//! The block-break burst: what digging looks like.
//!
//! One module because two callers need the same effect: the window
//! bursts live as blocks break, and a still replays the headless
//! script with the pool riding along, so the picture shows the same
//! breaks its own report counts. Both callers detect breaks the same
//! way — the aim before a step, the edit count after — so the world
//! carries no presentation state and its digest never learns
//! particles exist. (The windowed scripted run aims from a slightly
//! different opening look than the headless one, so the two traces
//! share the mechanism, not the cells.)
//!
//! One effect, dust-coloured from the stone palette. A block-coloured
//! burst per material wants a colour that varies per burst, which the
//! effect description deliberately does not offer yet — it arrives with
//! the tile-range work, with a consumer that varies more than colour.

use renew_particles::{EffectDesc, ParticleSystem, Seed, StreamId, VelocityCone};
use renew_sample_cube_world::{Cell, Cube};

/// One simulation step, in seconds — the cadence the pool advances at,
/// matching the fixed timestep the world itself runs.
pub const DT: f32 = 1.0 / 60.0;

/// How many particles one break throws.
pub const BURST: u32 = 24;

/// The break effect: a short puff of stone dust, rising a little and
/// falling under gravity, fading as it goes.
#[must_use]
pub fn effect() -> EffectDesc {
    // The stone palette's top-face colour, premultiplied at four
    // tenths opacity, fading to transparent black. Premultiplied means
    // the fade lives in the colour channels as much as in alpha — a
    // fade that moved only alpha would leave the channels adding ink
    // they no longer have the coverage for. Dust occludes rather than
    // glows, so the pipeline blends it as media (`Alpha`), not light
    // (`Additive`): a fresh burst is two dozen overlapping quads, and
    // summing them saturates to a white orb where layering them
    // converges to the stone's own colour.
    const DIM: f32 = 0.4;
    let ink = crate::mesh::colour(
        renew_sample_cube_world::STONE,
        renew_sample_cube_world::ray::Face::Top,
    );
    EffectDesc {
        capacity: 256,
        lifetime: (0.45, 0.8),
        velocity: VelocityCone {
            axis: [0.0, 3.2, 0.0],
            spread: 0.9,
            speed: (1.5, 4.0),
        },
        // A gentle pull, deliberately: the plume must clear the
        // one-unit hole its block leaves before the lifetime ends. An
        // earth-weighted first tuning (-9.8 against a 2.2 rise) peaked
        // a quarter unit up and every burst died inside its hole,
        // where nobody ever sees it.
        gravity: [0.0, -5.5, 0.0],
        drag_per_step: 0.98,
        size: (0.10, 0.03),
        color: (
            [ink[0] * DIM, ink[1] * DIM, ink[2] * DIM, DIM],
            [0.0, 0.0, 0.0, 0.0],
        ),
        tile: [0.0, 0.0, 1.0, 1.0],
        // Dust does not turn, and a billboard could not show it if it did.
        angle: (0.0, 0.0),
        spin: (0.0, 0.0),
    }
}

/// A pool for one run.
///
/// The seed is fixed: reproducibility comes from the whole pipeline
/// being deterministic — the same input trace breaks the same blocks on
/// the same ticks, so the same draws leave the pool in the same state.
#[must_use]
pub fn pool() -> ParticleSystem {
    ParticleSystem::new(
        &effect(),
        Seed::from_u64(20_260_811),
        StreamId::from_name("block-break"),
    )
}

/// The centre of `cell`, as world-space floats — where a break's dust
/// appears.
#[must_use]
pub fn centre_of(cell: Cell) -> [f32; 3] {
    [
        crate::mesh::world_units(cell.x),
        crate::mesh::world_units(cell.y),
        crate::mesh::world_units(cell.z),
    ]
}

/// What to watch before a step to know what broke after it: the aimed
/// cell and the broken count.
#[must_use]
pub fn watch(world: &Cube) -> (Option<Cell>, u32) {
    (world.looking_at().map(|pick| pick.cell), world.edits().0)
}

/// After a step: burst if the watched aim was dug.
pub fn settle(pool: &mut ParticleSystem, world: &Cube, watched: (Option<Cell>, u32)) {
    let (aimed, broken_before) = watched;
    if world.edits().0 > broken_before
        && let Some(cell) = aimed
    {
        pool.burst(centre_of(cell), BURST);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    /// The whole run, twice: the pool is a pure function of the world's
    /// breaks, so identical runs leave identical particles — which is
    /// what lets a committed render of a digging script stand as
    /// evidence.
    #[test]
    fn the_same_run_leaves_the_same_dust() {
        let options = crate::parse(arguments("--script build --ticks 12")).expect("well formed");
        let (world, pool) = crate::run_world_with_breaks(&options);
        // The build script's first dig lands on tick 10, so at tick 12
        // one block is broken and its burst is two steps old — young
        // enough that every particle of it is still alive.
        assert_eq!(
            world.edits().0,
            1,
            "the script should have dug once by tick 12"
        );
        assert_eq!(pool.live(), BURST, "one break throws one full burst");

        let (_, again) = crate::run_world_with_breaks(&options);
        let mut first = vec![0u8; effect().capacity as usize * renew_particles::INSTANCE_STRIDE];
        let mut second = first.clone();
        pool.write_instances(&mut first);
        again.write_instances(&mut second);
        assert_eq!(
            first, second,
            "two identical runs must leave identical dust"
        );
    }

    /// The world the report describes and the world the dust watched
    /// are bit-identical: the pool observes and never touches, so the
    /// picture and the report describe one run.
    #[test]
    fn the_dust_never_touches_the_world() {
        let options = crate::parse(arguments("--script build --ticks 30")).expect("well formed");
        assert_eq!(
            crate::run_world(&options).digest(),
            crate::run_world_with_breaks(&options).0.digest(),
            "a watched world must equal an unwatched one, bit for bit"
        );
    }

    /// A run that breaks nothing leaves an empty pool.
    #[test]
    fn a_quiet_run_leaves_no_dust() {
        let options = crate::parse(arguments("--script stand --ticks 60")).expect("well formed");
        let (world, pool) = crate::run_world_with_breaks(&options);
        assert_eq!(world.edits().0, 0, "standing still digs nothing");
        assert_eq!(pool.live(), 0, "no break, no dust");
    }

    /// A burst needs both halves of the watch: an edit that happened,
    /// and an aim it happened at.
    ///
    /// On a world that has really dug once — a fresh world's zero count
    /// can never read as risen (no `u32` is below zero), which would
    /// leave the aim half of the guard untested behind a count half
    /// that always refuses first.
    #[test]
    fn settle_needs_both_a_break_and_an_aim() {
        let options = crate::parse(arguments("--script build --ticks 12")).expect("well formed");
        let (world, _) = crate::run_world_with_breaks(&options);
        assert_eq!(
            world.edits().0,
            1,
            "the fixture needs a real break to lean on"
        );
        let mut dust = pool();
        // Nothing broke since the watch: no burst, aimed or not.
        settle(&mut dust, &world, (None, world.edits().0));
        settle(
            &mut dust,
            &world,
            (Some(Cell { x: 0, y: 0, z: 0 }), world.edits().0),
        );
        assert_eq!(dust.live(), 0, "a quiet tick must not burst");
        // The count HAS risen since this watch — the count half of the
        // guard passes, and it is the missing aim that refuses.
        settle(&mut dust, &world, (None, world.edits().0 - 1));
        assert_eq!(
            dust.live(),
            0,
            "a break with no watched aim has nowhere to burst"
        );
    }

    /// The effect holds a full burst and fades all the way out — in
    /// the colour channels as much as in alpha, because premultiplied
    /// colour carries its own coverage.
    #[test]
    fn the_effect_holds_a_burst_and_fades_to_black() {
        let desc = effect();
        assert!(desc.capacity >= BURST, "a burst must fit its pool");
        assert!(desc.lifetime.0 <= desc.lifetime.1, "lifetimes are a range");
        let (birth, death) = desc.color;
        assert!(
            birth[..3].iter().any(|&channel| channel > 0.0),
            "dust born black would never be seen"
        );
        assert_eq!(
            death.map(f32::to_bits),
            [0.0f32; 4].map(f32::to_bits),
            "a premultiplied fade must end at transparent black, or the dust dies visible"
        );
    }
}
