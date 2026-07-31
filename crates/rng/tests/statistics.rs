//! The measured evidence: the things that are true of this crate's output
//! and false of the plausible wrong versions of it.
//!
//! Every test here is a fixed-seed measurement, so none of them can flake:
//! the sample is the same sequence on every machine and every run. Where a
//! test names a tolerance, the tolerance is many standard deviations wide
//! for the correct implementation and hopelessly narrow for the wrong one,
//! and the comment says which.
//!
//! Two of them compute the wrong answer alongside the right one, from the
//! same words. A test that only checks the correct path leaves the reader
//! to take on trust that the wrong path would have been caught; these show
//! it, in the same output.

// The crate's no-floating-point rule extends to its tests: statistics
// about a bit-exact generator are counted in integers or not at all.
#![deny(clippy::float_arithmetic)]

use core::num::NonZeroU32;

use renew_rng::{Rng, Seed, StreamId};

#[allow(clippy::expect_used)]
fn narrow(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("non-zero")
}

/// The bound where modulo bias stops being academic. `3 * 2^30` leaves a
/// tail of `2^30` words, so a bare remainder hands the bottom third of the
/// range a second chance: it comes up half the time instead of a third.
const AWKWARD: u32 = 3 << 30;

/// Bounded draws are uniform, and the same words reduced with a bare
/// remainder are not. Both counts come out of one measurement so the
/// comparison is exact rather than rhetorical.
#[test]
fn the_bounded_draw_is_uniform_where_a_bare_remainder_is_not() {
    const DRAWS: u32 = 90_000;
    let third = AWKWARD / 3;

    for seed in 0..3u64 {
        let mut correct = Rng::new(Seed::from_u64(seed), StreamId::from_name("bias"));
        let mut naive = correct.clone();

        let mut correct_low = 0u32;
        let mut naive_low = 0u32;
        for _ in 0..DRAWS {
            if correct.below_u32(narrow(AWKWARD)) < third {
                correct_low += 1;
            }
            // The wrong reduction, on this crate's own words.
            if naive.next_u32() % AWKWARD < third {
                naive_low += 1;
            }
        }

        // Uniform: 30_000 of 90_000 in the bottom third. One standard
        // deviation is about 141, so 1_500 is more than ten of them.
        let expected = DRAWS / 3;
        assert!(
            correct_low.abs_diff(expected) < 1_500,
            "seed {seed}: {correct_low} of {DRAWS} in the bottom third, expected about {expected}"
        );
        // Biased: about 45_000 — half. Nowhere near the window above, and
        // that gap is the whole reason the rejection loop exists.
        assert!(
            naive_low > DRAWS / 2 - 1_500,
            "seed {seed}: a bare remainder produced {naive_low}, which is not the bias this test exists to demonstrate"
        );
    }
}

/// A small bound, the case where bias is real but far too small to
/// measure: six does not divide the word range, and the excess is four
/// words out of four billion. This test cannot tell a biased
/// implementation from an unbiased one — it is here to say so, and to
/// catch a reduction that is grossly wrong rather than subtly biased.
#[test]
fn a_small_bound_is_flat_across_its_buckets() {
    const DRAWS: u32 = 600_000;
    let mut rng = Rng::new(Seed::from_u64(7), StreamId::from_name("dice"));
    let mut buckets = [0u32; 6];
    for _ in 0..DRAWS {
        buckets[rng.below_u32(narrow(6)) as usize] += 1;
    }
    let expected = DRAWS / 6;
    for (face, count) in buckets.iter().enumerate() {
        // One standard deviation is about 289; the window is over ten.
        assert!(
            count.abs_diff(expected) < 3_000,
            "face {face}: {count} of {DRAWS}, expected about {expected} — buckets {buckets:?}"
        );
    }
}

/// The rejection loop turns about a third of the time at this bound (one
/// word in four is refused, so four words buy three draws). Measured,
/// because a rejection loop that never rejects and a rejection loop that
/// rejects everything both produce plausible-looking values.
#[test]
fn the_rejection_rate_matches_the_threshold() {
    const DRAWS: u32 = 60_000;
    let mut rng = Rng::new(Seed::from_u64(11), StreamId::from_name("rate"));
    let mut words = rng.clone();
    for _ in 0..DRAWS {
        let _ = rng.below_u32(narrow(AWKWARD));
    }
    let mut consumed = 0u32;
    while words.parts() != rng.parts() {
        let _ = words.next_u32();
        consumed += 1;
    }
    // Expected 4/3 words per draw: 80_000 for 60_000 draws, within 1%.
    assert!(
        consumed.abs_diff(80_000) < 800,
        "{consumed} words for {DRAWS} draws"
    );
}

/// Adjacent master seeds — 1, 2, 3, the ones people actually type —
/// produce unrelated first draws. About half the bits differ, which is
/// what "unrelated" means for 32 bits.
#[test]
fn adjacent_seeds_produce_unrelated_streams() {
    const PAIRS: u32 = 4_000;
    let stream = StreamId::from_name("physics");
    let mut total = 0u32;
    let mut closest = 32u32;
    for value in 0..u64::from(PAIRS) {
        let mut left = Rng::new(Seed::from_u64(value), stream);
        let mut right = Rng::new(Seed::from_u64(value + 1), stream);
        let differing = (left.next_u32() ^ right.next_u32()).count_ones();
        total += differing;
        closest = closest.min(differing);
    }
    // Mean of 16 out of 32 bits; the window is 15 to 17, and the closest
    // of four thousand pairs still differs in at least four bits.
    assert!((15..=17).contains(&(total / PAIRS)), "mean {total}/{PAIRS}");
    assert!(closest >= 4, "one pair differed in only {closest} bits");
}

/// The same for adjacent stream identifiers under one seed — the
/// per-entity case, where identifiers are literally 0, 1, 2, 3.
#[test]
fn adjacent_streams_produce_unrelated_sequences() {
    const PAIRS: u32 = 4_000;
    let seed = Seed::from_u64(0xfeed_face);
    let parent = StreamId::from_name("enemies");
    let mut total = 0u32;
    let mut closest = 32u32;
    for index in 0..u64::from(PAIRS) {
        let mut left = Rng::new(seed, parent.child(index));
        let mut right = Rng::new(seed, parent.child(index + 1));
        let differing = (left.next_u32() ^ right.next_u32()).count_ones();
        total += differing;
        closest = closest.min(differing);
    }
    assert!((15..=17).contains(&(total / PAIRS)), "mean {total}/{PAIRS}");
    assert!(closest >= 4, "one pair differed in only {closest} bits");
}

/// Why the derivation exists, demonstrated on the failure it prevents.
///
/// `from_parts` restores a snapshot: it takes the generator's internal
/// state verbatim and mixes nothing, which is exactly what a snapshot
/// needs and exactly what a seed must never get. Feed a small number in as
/// state and the first draw is not merely predictable — for every value
/// below 2^27 it is *the same value*, zero, because the output function
/// shifts those bits away before it ever reaches the rotation.
///
/// This is a real regression guard in both directions: it fails if someone
/// starts mixing inside `from_parts` (which would break every snapshot),
/// and it stands as the reason nobody should reach for `from_parts` when
/// they mean `new`.
#[test]
fn seeding_a_generator_with_a_raw_small_number_is_the_trap_the_mixer_avoids() {
    for state in 0..3_000u64 {
        assert_eq!(
            Rng::from_parts(state, 1).next_u32(),
            0,
            "state {state} should expose the unmixed-seed trap"
        );
    }
    // The trap has an exact edge, which is worth pinning: the first
    // non-zero first-draw appears once the state reaches 2^27.
    assert_eq!(Rng::from_parts((1 << 27) - 1, 1).next_u32(), 0);
    assert_eq!(Rng::from_parts(1 << 27, 1).next_u32(), 1);

    // The supported path, on the same numbers.
    let distinct: std::collections::BTreeSet<u32> = (0..3_000u64)
        .map(|value| Rng::new(Seed::from_u64(value), StreamId::from_u64(0)).next_u32())
        .collect();
    assert!(
        distinct.len() > 2_990,
        "only {} distinct first draws from 3000 seeds",
        distinct.len()
    );
}

/// Coin flips are balanced. A generator whose low bit was weak — a bare
/// linear congruential generator's low bit alternates — would fail here
/// while passing every other test in this file.
#[test]
fn coin_flips_are_balanced_and_not_alternating() {
    const FLIPS: u32 = 200_000;
    let mut rng = Rng::new(Seed::from_u64(3), StreamId::from_name("coins"));
    let mut heads = 0u32;
    let mut alternations = 0u32;
    let mut previous = rng.next_bool();
    for _ in 0..FLIPS {
        let flip = rng.next_bool();
        if flip {
            heads += 1;
        }
        if flip != previous {
            alternations += 1;
        }
        previous = flip;
    }
    // Balanced to within ten standard deviations (one is about 224).
    assert!(
        heads.abs_diff(FLIPS / 2) < 2_500,
        "{heads} heads of {FLIPS}"
    );
    // A strictly alternating low bit would give 200_000 alternations; a
    // stuck one would give zero. Both are far outside this window.
    assert!(
        alternations.abs_diff(FLIPS / 2) < 2_500,
        "{alternations} alternations of {FLIPS}"
    );
}
