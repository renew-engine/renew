//! The world's cross-machine oracle.
//!
//! Every other determinism test this world has compares it to *itself*:
//! two runs in one process, same seed, same digest. That catches an
//! unseeded generator or an iteration-order dependency, and it cannot
//! catch a world whose state depends on the machine it ran on — because
//! both halves of the comparison ran on the same machine.
//!
//! A committed constant is what closes that. The three-platform test
//! matrix runs this file on Linux, macOS and Windows, and on two
//! instruction sets, so a digest that differs by target reddens on the
//! leg that disagrees. That is weaker than comparing the legs to each
//! other — all three could be wrong in the same way — but it is the
//! strongest thing a single test binary can assert, and this world is
//! the richest simulation in the tree: entity generations, store
//! iteration, a seeded generator, collision, and scoring, all folded
//! into one hash.
//!
//! The negative control at the bottom is what makes the rest mean
//! anything. If a different seed produced the same digest, the oracle
//! would be ignoring its input and every assertion here would be
//! theatre.

use renew_sample_glide_world::World;

/// The seed the frozen run uses. Arbitrary, and fixed forever: its only
/// job is to be the same number on every machine.
const SEED: u64 = 7;

/// How many ticks the frozen run covers. Long enough that pipes spawn,
/// travel, get scored and despawn — a run that ended before the first
/// pipe would freeze a digest of almost nothing.
const TICKS: u64 = 600;

/// The digest of `SEED` after `TICKS` ticks with no input.
///
/// Minted 2026-08-04 on `x86_64` Windows and confirmed by the test matrix
/// on the other two desktop platforms. If this changes, the world's
/// observable behaviour changed: update it in the same commit as the
/// change that moved it, and say in the commit why the new behaviour is
/// correct. A digest updated without that sentence is a determinism
/// oracle being silenced.
const FROZEN_WORLD_DIGEST: u64 = 0xcbbb_133e_9ae0_c988;

/// Run `SEED` for `TICKS` ticks, flapping never — gravity alone, which
/// is reproducible without an input trace.
fn frozen_run() -> World {
    let mut world = World::new(SEED);
    for _ in 0..TICKS {
        world.step(false);
    }
    world
}

#[test]
fn the_frozen_run_is_not_vacuous() {
    let world = frozen_run();
    assert_eq!(world.tick(), TICKS, "the run must actually have run");
    // Gravity with no flapping kills the bird, and the world keeps
    // ticking after it does. Both facts matter: a run that ended at
    // tick one would freeze a digest that proves nothing about pipes,
    // scoring, or despawn.
    assert!(!world.alive(), "no flapping, so the bird must have died");
}

#[test]
fn the_frozen_run_matches_its_committed_digest() {
    let world = frozen_run();
    assert_eq!(
        world.digest().finish(),
        FROZEN_WORLD_DIGEST,
        "the world's canonical digest changed; if that was deliberate, \
         update FROZEN_WORLD_DIGEST in the same commit and say why"
    );
}

/// The negative control. A different seed must move the digest — if it
/// does not, the hash is ignoring the state it claims to cover and the
/// test above is asserting a constant against itself.
#[test]
fn a_different_seed_moves_the_digest() {
    let mut other = World::new(SEED + 1);
    for _ in 0..TICKS {
        other.step(false);
    }
    assert_ne!(
        other.digest().finish(),
        FROZEN_WORLD_DIGEST,
        "two seeds produced one digest; the oracle is blind to its input"
    );
}

/// The other half of the discrimination check: the same seed run one
/// tick longer must differ too, so the digest is a function of the run
/// rather than of the seed alone.
#[test]
fn one_more_tick_moves_the_digest() {
    let mut longer = frozen_run();
    longer.step(false);
    assert_ne!(
        longer.digest().finish(),
        FROZEN_WORLD_DIGEST,
        "an extra tick left the digest unchanged; it is not covering the run"
    );
}
