//! Reproducibility, in the three forms a replay actually needs: the same
//! seed gives the same numbers, the numbers do not depend on when or in
//! what order streams were built, and the values are frozen against a
//! constant so a change to any of it fails here rather than in somebody's
//! recorded trace six months from now.
//!
//! The digest below is folded locally rather than borrowed from the frame
//! crate's `StateHash`. Not because a second implementation is welcome —
//! it is not — but because a test-only dependency from this crate to an
//! optional sibling would put a false edge in the removability graph, and
//! this fold is six lines. Where a shared digest should live once the
//! determinism harness needs one across several crates is an open question
//! in the design note, not something to settle from inside a test file.

use core::num::{NonZeroU32, NonZeroU64};

use renew_rng::{Rng, Seed, StreamId};

const SEED: Seed = Seed::from_u64(0x1234_5678_9abc_def0);
const PHYSICS: StreamId = StreamId::from_name("physics");

/// FNV-1a-64 over little-endian words, order-sensitive.
struct Fold(u64);

impl Fold {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn absorb(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// Draw a long, mixed run — raw words, wide words, coins, bounded draws
/// narrow and wide — and fold everything, including where the generator
/// ended up. One number that changes if anything at all changes.
// The tests-only expect allowance covers #[test] fns, not their
// helpers; this allow extends it, same spirit. Every bound here is a
// non-zero literal.
#[allow(clippy::expect_used)]
fn digest_of_a_mixed_run(mut rng: Rng) -> u64 {
    let narrow = NonZeroU32::new(6).expect("non-zero");
    let awkward = NonZeroU32::new(3 << 30).expect("non-zero");
    let wide = NonZeroU64::new((5 << 60) + 7).expect("non-zero");

    let mut fold = Fold::new();
    for _ in 0..2_000 {
        fold.absorb(u64::from(rng.next_u32()));
        fold.absorb(rng.next_u64());
        fold.absorb(u64::from(rng.next_bool()));
        fold.absorb(u64::from(rng.below_u32(narrow)));
        fold.absorb(u64::from(rng.below_u32(awkward)));
        fold.absorb(rng.below_u64(wide));
    }
    let (state, increment) = rng.parts();
    fold.absorb(state);
    fold.absorb(increment);
    fold.0
}

/// The frozen oracle. These constants were produced by this crate on
/// x86-64 Windows and are asserted on every platform CI runs: the point of
/// freezing them is that the day one platform disagrees, the disagreement
/// is a red build rather than a divergent replay.
#[test]
fn the_sequence_is_frozen_against_published_constants() {
    let mut rng = Rng::new(SEED, PHYSICS);
    let first: [u32; 8] = core::array::from_fn(|_| rng.next_u32());
    assert_eq!(
        first,
        [
            0x05f7_67bf,
            0xa6d4_1562,
            0xcfa5_8073,
            0x544e_bc35,
            0x93fa_5089,
            0x2c83_165d,
            0x584b_8dee,
            0x513c_045b,
        ]
    );
    assert_eq!(
        digest_of_a_mixed_run(Rng::new(SEED, PHYSICS)),
        0x682b_a758_cdb0_1ce9
    );
    assert_eq!(
        Rng::new(SEED, PHYSICS).parts(),
        (0xf60b_d34d_8990_bec7, 0xa544_b2cf_968b_f449)
    );
}

/// The same seed and stream, built five separate times, produce the same
/// run — the base case, and the one a determinism harness multiplies by a
/// seed matrix.
#[test]
fn the_same_seed_and_stream_replay_identically() {
    let expected = digest_of_a_mixed_run(Rng::new(SEED, PHYSICS));
    for _ in 0..5 {
        assert_eq!(digest_of_a_mixed_run(Rng::new(SEED, PHYSICS)), expected);
    }
}

/// Every seed in a small matrix gives a different run, and each of those
/// runs is itself repeatable. A generator that ignored its seed would pass
/// the test above and fail this one.
#[test]
fn different_seeds_give_different_runs_and_each_repeats() {
    let mut digests = std::collections::BTreeSet::new();
    for value in 0..32u64 {
        let seed = Seed::from_u64(value);
        let digest = digest_of_a_mixed_run(Rng::new(seed, PHYSICS));
        assert_eq!(digest_of_a_mixed_run(Rng::new(seed, PHYSICS)), digest);
        assert!(digests.insert(digest), "seed {value} repeated a run");
    }
}

/// Derivation is a pure function of `(seed, stream)`, so the order streams
/// are created in cannot matter. This is the property that lets a replay
/// reconstruct one entity's generator without reconstructing the world
/// that was around when it was first created.
#[test]
fn stream_derivation_does_not_depend_on_creation_order() {
    let forward: Vec<(u64, u64)> = (0..500u64)
        .map(|index| Rng::new(SEED, PHYSICS.child(index)).parts())
        .collect();

    // Backwards, then a stride that visits the same indices in a third
    // order, then again after unrelated generators have been built and
    // drawn from.
    let mut backward: Vec<(u64, u64)> = (0..500u64)
        .rev()
        .map(|index| Rng::new(SEED, PHYSICS.child(index)).parts())
        .collect();
    backward.reverse();
    assert_eq!(backward, forward);

    let mut noise = Rng::new(SEED, StreamId::from_name("noise"));
    for index in (0..500usize).map(|step| (step * 337) % 500) {
        let _ = noise.next_u64();
        assert_eq!(
            Rng::new(SEED, PHYSICS.child(index as u64)).parts(),
            forward[index]
        );
    }
}

/// Interleaving draws from other streams changes nothing about a stream's
/// own sequence. Obvious from the types — each generator owns its state —
/// and asserted anyway, because it is the assumption every system-per-
/// stream design rests on.
#[test]
fn streams_do_not_disturb_each_other() {
    let alone: [u32; 64] = {
        let mut rng = Rng::new(SEED, PHYSICS);
        core::array::from_fn(|_| rng.next_u32())
    };

    let mut rng = Rng::new(SEED, PHYSICS);
    let mut neighbours: Vec<Rng> = (0..8)
        .map(|index| Rng::new(SEED, PHYSICS.child(index)))
        .collect();
    let interleaved: [u32; 64] = core::array::from_fn(|step| {
        for neighbour in &mut neighbours {
            let _ = neighbour.next_u64();
        }
        let _ = step;
        rng.next_u32()
    });
    assert_eq!(interleaved, alone);
}

/// A snapshot taken anywhere resumes the same run. The determinism harness
/// and any mid-trace replay entry point depend on this.
#[test]
fn a_run_can_be_suspended_and_resumed_at_any_point() {
    let expected = digest_of_a_mixed_run(Rng::new(SEED, PHYSICS));
    for cut in [0u32, 1, 7, 64] {
        let mut rng = Rng::new(SEED, PHYSICS);
        for _ in 0..cut {
            let _ = rng.next_u32();
        }
        let (state, increment) = rng.parts();
        let resumed = Rng::from_parts(state, increment);

        let mut original = Rng::new(SEED, PHYSICS);
        for _ in 0..cut {
            let _ = original.next_u32();
        }
        assert_eq!(
            digest_of_a_mixed_run(resumed),
            digest_of_a_mixed_run(original)
        );
    }
    assert_eq!(digest_of_a_mixed_run(Rng::new(SEED, PHYSICS)), expected);
}
