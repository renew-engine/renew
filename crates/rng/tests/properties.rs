//! Property coverage over inputs no hand-written case would reach: the
//! bounded draws across the whole range of bounds and seeds, the wide
//! draw's construction from the narrow one, snapshot round-trips, and the
//! two structural guarantees the stream design rests on.
//!
//! What these properties cannot do is tell a correct generator from a
//! wrong one — every one of them passes on any deterministic sequence of
//! bits. That job belongs to `tests/known_answer.rs`. These hold the
//! *reductions* built on top of the sequence, which is where the bugs a
//! known-answer test cannot see live: an off-by-one bound, a rejection
//! loop that rejects the wrong side, a snapshot that drops the low bit of
//! an increment.

use core::num::{NonZeroU32, NonZeroU64};

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use renew_rng::{Rng, Seed, StreamId};

// Test helpers (called only from #[test] fns): the tests-only expect
// allowance covers #[test] fns, not their helpers; this allow extends it,
// same spirit. Both generators produce non-zero values by construction.
#[allow(clippy::expect_used)]
fn narrow(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("non-zero")
}

#[allow(clippy::expect_used)]
fn wide(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).expect("non-zero")
}

proptest! {
    // Fixed RNG seed: the suite explores the same inputs on every run and
    // every machine, so a property failure anywhere reproduces everywhere.
    // Fresh exploration is a deliberate act (change the seed), never an
    // ambient one.
    #![proptest_config(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x0000_2607),
        cases: 64,
        ..ProptestConfig::default()
    })]

    /// The whole point of a bounded draw. Checked across the range of
    /// bounds where the rejection threshold matters most — everything
    /// above 2^31 refuses a large share of words.
    #[test]
    fn a_narrow_bounded_draw_is_always_below_its_bound(
        seed in any::<u64>(),
        stream in any::<u64>(),
        bounds in prop::collection::vec(1u32..=u32::MAX, 1..12),
    ) {
        let mut rng = Rng::new(Seed::from_u64(seed), StreamId::from_u64(stream));
        for value in bounds {
            let bound = narrow(value);
            for _ in 0..8 {
                prop_assert!(rng.below_u32(bound) < value);
            }
        }
    }

    #[test]
    fn a_wide_bounded_draw_is_always_below_its_bound(
        seed in any::<u64>(),
        bounds in prop::collection::vec(1u64..=u64::MAX, 1..12),
    ) {
        let mut rng = Rng::new(Seed::from_u64(seed), StreamId::from_name("wide"));
        for value in bounds {
            let bound = wide(value);
            for _ in 0..8 {
                prop_assert!(rng.below_u64(bound) < value);
            }
        }
    }

    /// A bound of one has one answer and must not cost more than one word:
    /// its rejection threshold is zero, so the loop can never turn.
    #[test]
    fn a_bound_of_one_never_rejects(seed in any::<u64>()) {
        let mut bounded = Rng::new(Seed::from_u64(seed), StreamId::from_name("one"));
        let mut raw = bounded.clone();
        for _ in 0..32 {
            prop_assert_eq!(bounded.below_u32(narrow(1)), 0);
            prop_assert_eq!(bounded.below_u64(wide(1)), 0);
            let _ = raw.next_u32();
            let _ = raw.next_u64();
        }
        prop_assert_eq!(bounded.parts(), raw.parts());
    }

    /// A bound that is an exact power of two divides the word range, so
    /// its threshold is zero as well — the common case in game code, and
    /// the one where the rejection loop must stay out of the way.
    #[test]
    fn a_power_of_two_bound_never_rejects(seed in any::<u64>(), exponent in 0u32..32) {
        let bound = narrow(1u32 << exponent);
        let mut bounded = Rng::new(Seed::from_u64(seed), StreamId::from_name("pow2"));
        let mut raw = bounded.clone();
        for _ in 0..32 {
            let value = bounded.below_u32(bound);
            let word = raw.next_u32();
            prop_assert_eq!(value, word % bound.get());
        }
        prop_assert_eq!(bounded.parts(), raw.parts());
    }

    /// The wide draw is exactly two narrow ones, low half first. Stated in
    /// the documentation, so it is asserted rather than trusted.
    #[test]
    fn a_wide_draw_is_two_narrow_draws_low_half_first(seed in any::<u64>()) {
        let mut widely = Rng::new(Seed::from_u64(seed), StreamId::from_name("halves"));
        let mut narrowly = widely.clone();
        for _ in 0..16 {
            let value = widely.next_u64();
            let low = u64::from(narrowly.next_u32());
            let high = u64::from(narrowly.next_u32());
            prop_assert_eq!(value, (high << 32) | low);
        }
        prop_assert_eq!(widely.parts(), narrowly.parts());
    }

    /// A coin is the bounded draw for two, everywhere, not just for the
    /// seed the known-answer test uses.
    #[test]
    fn a_coin_is_the_bounded_draw_for_two(seed in any::<u64>()) {
        let mut coins = Rng::new(Seed::from_u64(seed), StreamId::from_name("coins"));
        let mut bounded = coins.clone();
        for _ in 0..64 {
            prop_assert_eq!(coins.next_bool(), bounded.below_u32(narrow(2)) != 0);
        }
    }

    /// Snapshots round-trip exactly for every generator this crate
    /// produces, including after an arbitrary number of draws of every
    /// shape.
    #[test]
    fn a_snapshot_round_trips(
        seed in any::<u64>(),
        stream in any::<u64>(),
        draws in 0u32..64,
    ) {
        let mut rng = Rng::new(Seed::from_u64(seed), StreamId::from_u64(stream));
        for step in 0..draws {
            match step % 3 {
                0 => { let _ = rng.next_u32(); }
                1 => { let _ = rng.next_u64(); }
                _ => { let _ = rng.below_u32(narrow(7)); }
            }
        }
        let (state, increment) = rng.parts();
        let mut resumed = Rng::from_parts(state, increment);
        prop_assert_eq!(resumed.clone(), rng.clone());
        for _ in 0..8 {
            prop_assert_eq!(resumed.next_u64(), rng.next_u64());
        }
    }

    /// Restoring from an arbitrary pair — including an even increment,
    /// which no generator here produces — is total and stays odd.
    #[test]
    fn an_arbitrary_pair_restores_to_a_usable_generator(
        state in any::<u64>(),
        increment in any::<u64>(),
    ) {
        let mut rng = Rng::from_parts(state, increment);
        let (restored_state, restored_increment) = rng.parts();
        prop_assert_eq!(restored_state, state);
        prop_assert_eq!(restored_increment, increment | 1);
        prop_assert_eq!(restored_increment & 1, 1);
        let _ = rng.next_u64();
    }

    /// The guarantee the stream design is stated on: under one seed, two
    /// different stream identifiers never start from the same place.
    #[test]
    fn distinct_streams_under_one_seed_never_collide(
        seed in any::<u64>(),
        streams in prop::collection::btree_set(any::<u64>(), 2..48),
    ) {
        let seed = Seed::from_u64(seed);
        let starts: std::collections::BTreeSet<(u64, u64)> = streams
            .iter()
            .map(|&stream| Rng::new(seed, StreamId::from_u64(stream)).parts())
            .collect();
        prop_assert_eq!(starts.len(), streams.len());
    }

    /// And the same for one stream across seeds: changing the seed
    /// changes every stream of the run, which is what makes a seed worth
    /// recording.
    #[test]
    fn one_stream_moves_when_the_seed_moves(
        seeds in prop::collection::btree_set(any::<u64>(), 2..48),
        stream in any::<u64>(),
    ) {
        let stream = StreamId::from_u64(stream);
        let starts: std::collections::BTreeSet<(u64, u64)> = seeds
            .iter()
            .map(|&seed| Rng::new(Seed::from_u64(seed), stream).parts())
            .collect();
        prop_assert_eq!(starts.len(), seeds.len());
    }

    /// Children of one parent are distinct, and the same index under two
    /// different parents is two different streams.
    #[test]
    fn child_streams_are_distinct(
        parent in any::<u64>(),
        other in any::<u64>(),
        indices in prop::collection::btree_set(any::<u64>(), 2..48),
    ) {
        prop_assume!(parent != other);
        let parent = StreamId::from_u64(parent);
        let other = StreamId::from_u64(other);
        let children: std::collections::BTreeSet<u64> = indices
            .iter()
            .map(|&index| parent.child(index).get())
            .collect();
        prop_assert_eq!(children.len(), indices.len());
        for &index in &indices {
            prop_assert_ne!(parent.child(index), other.child(index));
        }
    }
}
