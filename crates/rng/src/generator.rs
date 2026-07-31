//! The generator: PCG32 (the XSH-RR 64/32 variant), its derived seeding,
//! and the draws built on it.
//!
//! # Why this algorithm
//!
//! Integer operations only — a 64-bit multiply, an add, two shifts, a
//! xor and a rotate. No floating point appears anywhere in the generator
//! or in anything reachable from it, which is what lets the sequence be
//! stated as bit-identical rather than as approximately identical.
//!
//! The deciding property, though, was evidence. The algorithm's reference
//! implementation ships a demonstration program whose output is published,
//! and this crate reproduces that output exactly — six raw words, then a
//! run of coin flips, dice rolls and a shuffled deck drawn from the same
//! continuing stream (`tests/known_answer.rs`). A lookalike that got a
//! shift distance or the rotation direction wrong would still be a
//! perfectly deterministic generator, still pass every property test in
//! this crate, and still be the wrong algorithm. Only a known-answer test
//! against somebody else's numbers catches that.
//!
//! # Why the seeding is not the reference seeding
//!
//! The algorithm's own stream parameter is the increment of its
//! underlying linear congruential step. Two streams that differ in that
//! parameter are related: measured on the reference seeding, the
//! difference between two streams' internal states is a fixed constant
//! that does not depend on the master seed at all, and stays fixed as both
//! advance. That is a structure a caller can hit accidentally simply by
//! numbering entities 1, 2, 3. So callers never reach the stream parameter
//! directly: both words are derived through the mixer in [`crate::mix`],
//! which costs four multiplies once per generator and removes the
//! structure entirely.

use core::num::{NonZeroU32, NonZeroU64};

use crate::mix::{GAMMA, SplitMix64, mix};
use crate::seed::{Seed, StreamId};

/// The multiplier of the 64-bit linear congruential step, fixed by the
/// algorithm. Not a tunable: change it and every published vector, every
/// recorded trace and every golden state hash becomes wrong at once.
const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// A seeded generator: one reproducible sequence of numbers.
///
/// Sixteen bytes of state, no heap, no interior mutability, no shared
/// anything. Generators are values a caller owns and passes explicitly —
/// there is no ambient generator in this engine and no way to ask for one.
///
/// `Clone` but deliberately **not** `Copy`. A generator copied by accident
/// is the quietest bug this crate could ship: both halves continue from
/// the same state and produce the same "random" numbers forever, with
/// nothing failing. Requiring `.clone()` makes forking a sentence someone
/// wrote on purpose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
    /// Always odd — the algorithm's period argument depends on it, and
    /// every constructor enforces it.
    increment: u64,
}

impl Rng {
    /// The generator for one stream of one run.
    ///
    /// # Contract
    ///
    /// - **A pure function of its two arguments.** Not of call order, not
    ///   of how many generators were built before it, not of anything
    ///   ambient. Rebuilding a stream halfway through a replay gives the
    ///   same generator the recording had.
    /// - **Distinct streams under one seed cannot collide.** Both words
    ///   come from a bijective derivation, so two different `StreamId`s
    ///   under the same `Seed` always produce different starting states.
    ///   The property those streams do *not* have is proven statistical
    ///   independence — see the crate documentation, which states the
    ///   bound honestly.
    ///
    /// ```
    /// use renew_rng::{Rng, Seed, StreamId};
    ///
    /// const LOOT: StreamId = StreamId::from_name("loot");
    /// let seed = Seed::from_u64(20_260_731);
    ///
    /// let mut a = Rng::new(seed, LOOT);
    /// let mut b = Rng::new(seed, LOOT);
    /// assert_eq!(a.next_u32(), b.next_u32());
    ///
    /// let mut other = Rng::new(seed, StreamId::from_name("weather"));
    /// assert_ne!(a.next_u32(), other.next_u32());
    /// ```
    #[must_use]
    pub const fn new(seed: Seed, stream: StreamId) -> Self {
        // Bijective in the stream for a fixed seed (mix, an odd multiply
        // and an add are each invertible), so distinct streams start at
        // distinct roots; the walk below is bijective too, so distinct
        // roots stay distinct all the way to the generator.
        let root = mix(seed.get()).wrapping_add(mix(stream.get()).wrapping_mul(GAMMA));
        let mut walk = SplitMix64::new(root);
        let increment = walk.next();
        let state = walk.next();
        Self {
            state,
            increment: increment | 1,
        }
    }

    /// Rebuild a generator from a snapshot taken with [`Rng::parts`].
    ///
    /// This is the save-and-resume path: a determinism harness, a replay
    /// that starts mid-trace, or a save file records the two words and
    /// hands them back later. The sequence continues exactly where the
    /// snapshot was taken.
    ///
    /// The low bit of `increment` is forced set. The algorithm requires an
    /// odd increment — an even one walks half the state space and can
    /// reach zero — and forcing it keeps this constructor total, with no
    /// error to report and no failure a caller has to handle. Snapshots
    /// taken from this crate always round-trip exactly, because every
    /// generator it produces already has an odd increment.
    #[must_use]
    pub const fn from_parts(state: u64, increment: u64) -> Self {
        Self {
            state,
            increment: increment | 1,
        }
    }

    /// The generator's whole state, as `(state, increment)` — everything
    /// [`Rng::from_parts`] needs to resume it, and everything a state hash
    /// needs to fingerprint it.
    #[must_use]
    pub const fn parts(&self) -> (u64, u64) {
        (self.state, self.increment)
    }

    /// The next 32 bits.
    ///
    /// One linear congruential step, then the algorithm's output
    /// function: xor the state's high bits down over itself, take 32 bits
    /// of the result, and rotate them by an amount taken from the state's
    /// top five bits. The data-dependent rotation is the part that makes
    /// this more than a linear congruential generator, and it is why every
    /// output bit here is as good as every other — unlike a bare linear
    /// congruential generator, whose low bit alternates.
    pub fn next_u32(&mut self) -> u32 {
        let previous = self.state;
        self.state = previous
            .wrapping_mul(MULTIPLIER)
            .wrapping_add(self.increment);
        // Both casts drop the high bits on purpose: the output function is
        // defined as 32 bits taken from a 64-bit intermediate, and a
        // rotation amount taken from five bits.
        #[allow(clippy::cast_possible_truncation)]
        let xorshifted = (((previous >> 18) ^ previous) >> 27) as u32;
        #[allow(clippy::cast_possible_truncation)]
        let rotation = (previous >> 59) as u32;
        xorshifted.rotate_right(rotation)
    }

    /// The next 64 bits, as two 32-bit draws.
    ///
    /// The first draw is the low half. Stated because it is observable:
    /// the choice fixes what every trace and every state hash contains,
    /// and it matches the little-endian convention the rest of the tree
    /// already writes integers with.
    pub fn next_u64(&mut self) -> u64 {
        let low = u64::from(self.next_u32());
        let high = u64::from(self.next_u32());
        (high << 32) | low
    }

    /// A coin flip.
    ///
    /// Exactly `below_u32(2) != 0`, and the crate's tests assert that
    /// equality rather than trusting it: for a bound of two the rejection
    /// threshold is zero, so a bounded draw is one word and its low bit.
    pub fn next_bool(&mut self) -> bool {
        self.next_u32() & 1 == 1
    }

    /// A uniform value in `0..bound`.
    ///
    /// # Why the bound is a non-zero type
    ///
    /// There is no uniform value below zero, so an empty range is not a
    /// runtime condition to report — it is a case the caller has to have
    /// handled before asking. Spelling that in the type keeps this method
    /// total, and it puts the check where the emptiness is actually known:
    ///
    /// ```
    /// use core::num::NonZeroU32;
    /// use renew_rng::{Rng, Seed, StreamId};
    ///
    /// // A literal bound, fixed at compile time. The `match` is how this
    /// // tree writes a non-zero constant: engine code has no `unwrap`.
    /// const SIDES: NonZeroU32 = match NonZeroU32::new(6) {
    ///     Some(sides) => sides,
    ///     None => NonZeroU32::MIN,
    /// };
    ///
    /// let mut rng = Rng::new(Seed::from_u64(1), StreamId::from_name("dice"));
    /// let roll = rng.below_u32(SIDES) + 1;
    /// assert!((1..=6).contains(&roll));
    ///
    /// // A bound that comes from data: the empty case is handled here,
    /// // where "there is nothing to choose from" means something.
    /// let candidates = ["a", "b", "c"];
    /// # #[allow(clippy::cast_possible_truncation)]
    /// let picked = NonZeroU32::new(candidates.len() as u32)
    ///     .map(|count| candidates[rng.below_u32(count) as usize]);
    /// assert!(picked.is_some());
    /// ```
    ///
    /// # No modulo bias
    ///
    /// `next_u32() % bound` is not uniform unless `bound` divides 2^32:
    /// the first `2^32 % bound` outputs get one extra chance each. For
    /// small bounds the effect is too small to measure, which is exactly
    /// why it survives in shipped code; for large ones it is gross —
    /// at `bound = 3 * 2^30` a bare remainder makes the low third of the
    /// range come up half the time instead of a third, measured in
    /// `tests/statistics.rs`.
    ///
    /// The fix here is rejection sampling with a threshold: refuse the
    /// first `2^32 % bound` words, leaving an accepted range whose length
    /// is an exact multiple of `bound`, over which the remainder is
    /// uniform. This is the technique the algorithm's reference
    /// implementation uses and the one behind `arc4random_uniform`. It
    /// costs one remainder per draw and, for the bounds a game actually
    /// uses, essentially never redraws.
    ///
    /// # Consumption is data-dependent
    ///
    /// A bounded draw consumes one word *or more*. That is deterministic —
    /// same seed, same rejections, same everything — but it means a trace
    /// cannot assume one draw advances the stream by one step. Anything
    /// that needs to know exactly where the stream is reads
    /// [`Rng::parts`].
    pub fn below_u32(&mut self, bound: NonZeroU32) -> u32 {
        let bound = bound.get();
        // `-bound % bound` in unsigned arithmetic is `2^32 % bound`: the
        // size of the short tail that has to be refused.
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let word = self.next_u32();
            if word >= threshold {
                return word % bound;
            }
        }
    }

    /// A uniform value in `0..bound`, over the full 64-bit range.
    ///
    /// The same technique as [`Rng::below_u32`], one word wider. It is
    /// written out again rather than sharing an implementation with the
    /// 32-bit case: the 32-bit path is the one pinned by the published
    /// known-answer vectors, and expressing it through this one would
    /// consume two words per draw and change every value in them.
    pub fn below_u64(&mut self, bound: NonZeroU64) -> u64 {
        let bound = bound.get();
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let word = self.next_u64();
            if word >= threshold {
                return word % bound;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MULTIPLIER, Rng};
    use crate::seed::{Seed, StreamId};
    use core::num::{NonZeroU32, NonZeroU64};

    const SEED: Seed = Seed::from_u64(0x0bad_f00d_dead_beef);
    const STREAM: StreamId = StreamId::from_name("tests");

    #[test]
    fn the_step_is_the_documented_linear_congruential_step() {
        let mut rng = Rng::from_parts(12_345, 7);
        let (before, increment) = rng.parts();
        let _ = rng.next_u32();
        let (after, _) = rng.parts();
        assert_eq!(
            after,
            before.wrapping_mul(MULTIPLIER).wrapping_add(increment)
        );
    }

    #[test]
    fn an_even_increment_is_made_odd() {
        assert_eq!(Rng::from_parts(0, 108).parts(), (0, 109));
        assert_eq!(Rng::from_parts(0, 109).parts(), (0, 109));
        assert_eq!(Rng::from_parts(0, 0).parts(), (0, 1));
    }

    #[test]
    fn a_snapshot_resumes_the_same_sequence() {
        let mut rng = Rng::new(SEED, STREAM);
        for _ in 0..17 {
            let _ = rng.next_u32();
        }
        let (state, increment) = rng.parts();
        let mut resumed = Rng::from_parts(state, increment);
        assert_eq!(resumed, rng);
        let expected: [u32; 8] = core::array::from_fn(|_| rng.next_u32());
        let got: [u32; 8] = core::array::from_fn(|_| resumed.next_u32());
        assert_eq!(got, expected);
    }

    #[test]
    fn every_derived_increment_is_odd() {
        for stream in 0..2_000u64 {
            let (_, increment) = Rng::new(SEED, StreamId::from_u64(stream)).parts();
            assert_eq!(increment & 1, 1);
        }
    }

    #[test]
    fn sixty_four_bits_are_two_draws_low_half_first() {
        let mut wide = Rng::new(SEED, STREAM);
        let mut narrow = Rng::new(SEED, STREAM);
        let value = wide.next_u64();
        let low = u64::from(narrow.next_u32());
        let high = u64::from(narrow.next_u32());
        assert_eq!(value, (high << 32) | low);
        assert_eq!(wide.parts(), narrow.parts());
    }

    #[test]
    fn a_coin_is_the_bounded_draw_for_two() {
        let two = NonZeroU32::new(2).expect("non-zero");
        let mut coins = Rng::new(SEED, STREAM);
        let mut bounded = Rng::new(SEED, STREAM);
        for _ in 0..1_000 {
            assert_eq!(coins.next_bool(), bounded.below_u32(two) != 0);
        }
        assert_eq!(coins.parts(), bounded.parts());
    }

    #[test]
    fn a_bound_of_one_is_always_zero_and_costs_one_word() {
        let one = NonZeroU32::new(1).expect("non-zero");
        let mut bounded = Rng::new(SEED, STREAM);
        let mut raw = Rng::new(SEED, STREAM);
        for _ in 0..64 {
            assert_eq!(bounded.below_u32(one), 0);
            let _ = raw.next_u32();
        }
        assert_eq!(bounded.parts(), raw.parts());
    }

    #[test]
    fn bounded_draws_stay_below_their_bound() {
        let mut rng = Rng::new(SEED, STREAM);
        for bound in [2u32, 3, 6, 7, 52, 1_000, 1 << 31, (3 << 30) + 1, u32::MAX] {
            let bound = NonZeroU32::new(bound).expect("non-zero");
            for _ in 0..200 {
                assert!(rng.below_u32(bound) < bound.get());
            }
        }
        for bound in [2u64, 6, 1 << 63, (3 << 62) + 1, u64::MAX] {
            let bound = NonZeroU64::new(bound).expect("non-zero");
            for _ in 0..200 {
                assert!(rng.below_u64(bound) < bound.get());
            }
        }
    }

    /// The rejection branch, exercised on purpose rather than by luck. At
    /// this bound one word in four is refused, so a clone of the same
    /// generator can be used to count how many refusals the draws below
    /// actually went through — a test that "covers" the loop without ever
    /// looping would be worth nothing.
    #[test]
    fn the_rejection_path_runs_for_a_bound_that_provokes_it() {
        let bound = NonZeroU32::new(3 << 30).expect("non-zero");
        let threshold = bound.get().wrapping_neg() % bound.get();
        assert_eq!(threshold, 1 << 30);

        let mut rng = Rng::new(SEED, STREAM);
        let mut words = rng.clone();
        let mut drawn = 0u32;
        for _ in 0..256 {
            assert!(rng.below_u32(bound) < bound.get());
        }
        let mut refusals = 0u32;
        while words.parts() != rng.parts() {
            if words.next_u32() < threshold {
                refusals += 1;
            }
            drawn += 1;
        }
        assert!(refusals > 0, "no word was ever refused");
        assert_eq!(drawn, 256 + refusals);
    }

    #[test]
    fn the_wide_rejection_path_runs_too() {
        let bound = NonZeroU64::new(3 << 62).expect("non-zero");
        let threshold = bound.get().wrapping_neg() % bound.get();
        assert_eq!(threshold, 1 << 62);

        let mut rng = Rng::new(SEED, STREAM);
        let mut words = rng.clone();
        let mut drawn = 0u32;
        for _ in 0..256 {
            assert!(rng.below_u64(bound) < bound.get());
        }
        let mut refusals = 0u32;
        while words.parts() != rng.parts() {
            if words.next_u64() < threshold {
                refusals += 1;
            }
            drawn += 1;
        }
        assert!(refusals > 0, "no word pair was ever refused");
        assert_eq!(drawn, 256 + refusals);
    }

    /// The threshold identity the whole no-bias argument rests on: after
    /// refusing the tail, the accepted range is an exact multiple of the
    /// bound, so the remainder over it is uniform. Checked over every
    /// bound up to a hundred thousand and at the awkward extremes, by
    /// arithmetic rather than by sampling.
    #[test]
    fn the_accepted_range_is_always_a_multiple_of_the_bound() {
        const RANGE: u64 = 1 << 32;
        for bound in (1..=100_000u32).chain([1 << 31, (1 << 31) + 1, 3 << 30, u32::MAX]) {
            let threshold = u64::from(bound.wrapping_neg() % bound);
            assert_eq!((RANGE - threshold) % u64::from(bound), 0, "bound {bound}");
            assert!(threshold < u64::from(bound));
        }
        for bound in (1..=100_000u64).chain([1 << 63, (1 << 63) + 1, 3 << 62, u64::MAX]) {
            let threshold = bound.wrapping_neg() % bound;
            // 2^64 is not representable, so the identity is checked in the
            // equivalent form: the accepted count is a multiple of bound.
            assert_eq!(threshold, (u64::MAX % bound + 1) % bound, "bound {bound}");
        }
    }

    #[test]
    fn a_generator_is_comparable_and_printable() {
        let rng = Rng::new(SEED, STREAM);
        assert_eq!(rng, rng.clone());
        assert_ne!(rng, Rng::new(SEED, StreamId::from_name("other")));
        assert!(format!("{rng:?}").contains("Rng"));
    }
}
