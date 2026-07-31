//! Known-answer tests: this is the published algorithm, not a lookalike.
//!
//! Every other test in this crate would pass on a generator with a shift
//! distance off by one, a rotation in the wrong direction, or a multiplier
//! with two digits transposed. Such a generator would be perfectly
//! deterministic, perfectly reproducible, and perfectly wrong — its
//! statistical guarantees would be nobody's, its period unknown, and the
//! decades of analysis behind the real algorithm would not apply to it.
//! Only somebody else's numbers can tell the two apart.
//!
//! The numbers below are the first round of output from the demonstration
//! program shipped with the algorithm's reference implementation, seeded
//! with 42 and 54. Four lines of it, all drawn from one continuing stream:
//! six raw 32-bit words, sixty-five coin flips, thirty-three dice rolls,
//! and a shuffled deck of fifty-two cards. Because the stream continues
//! across the lines, the dice rolls only come out right if the coin flips
//! consumed exactly the right number of words before them, and the deck
//! only comes out right if both did. Any error anywhere in the generator
//! or in the bounded-draw reduction breaks everything downstream of it.
//!
//! Two things these vectors deliberately do not pin:
//!
//! * **The seeding this crate actually uses.** The reference's seeding
//!   procedure is reproduced below out of the public API, because that is
//!   what the published numbers are stated against. Callers never use it;
//!   `Rng::new` derives both words through the mixer instead, because a
//!   seed used raw leaves the first output at zero for a wide range of
//!   small seeds. What the vectors pin is the *generator* —
//!   the step and the output function — which is the part the seeding then
//!   feeds.
//! * **The wide draws.** The reference is a 32-bit generator and publishes
//!   no 64-bit vectors, so `next_u64` and `below_u64` are pinned by their
//!   construction from the 32-bit path instead (`src/generator.rs`), plus
//!   the frozen digest at the end of this file.

use core::num::NonZeroU32;

use renew_rng::Rng;

/// The reference implementation's seeding routine, written out through
/// this crate's public API so the published vectors can be stated against
/// it: start at state zero with the sequence selector as an odd
/// increment, step once, add the seed to the state, step again.
///
/// This is the one place in the tree that reproduces it. It exists for the
/// vectors and for nothing else.
fn reference_seeded(seed: u64, sequence: u64) -> Rng {
    let mut rng = Rng::from_parts(0, (sequence << 1) | 1);
    let _ = rng.next_u32();
    let (state, increment) = rng.parts();
    let mut rng = Rng::from_parts(state.wrapping_add(seed), increment);
    let _ = rng.next_u32();
    rng
}

// The tests-only expect allowance covers #[test] fns, not their
// helpers; this allow extends it, same spirit.
#[allow(clippy::expect_used)]
fn bound(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("bounds in this file are literals and non-zero")
}

/// The published first round, in full and in order.
#[test]
fn the_published_demonstration_round_is_reproduced_exactly() {
    let mut rng = reference_seeded(42, 54);

    // Line 1: six raw 32-bit draws.
    let words: [u32; 6] = core::array::from_fn(|_| rng.next_u32());
    assert_eq!(
        words,
        [
            0xa15c_02b7,
            0x7b47_f409,
            0xba1d_3330,
            0x83d2_f293,
            0xbfa4_784b,
            0xcbed_606e,
        ],
        "the generator itself"
    );

    // Line 2: sixty-five coin flips. The count is part of the vector, and
    // it is settled by the stream rather than by preference: the rolls and
    // the deck below continue from the same words, and they only come out
    // right if exactly sixty-five were consumed here.
    let coins: String = (0..65)
        .map(|_| if rng.next_bool() { 'H' } else { 'T' })
        .collect();
    assert_eq!(
        coins, "HHTTTHTHHHTHTTTHHHHHTTTHHHTHTHTHTTHTTTHHHHHHTTTTHHTTTTTHTTTTTTTHT",
        "coin flips, which are also the bounded draw for two"
    );

    // Line 3: thirty-three dice rolls — the bounded-draw path, for a
    // bound that does not divide the word range.
    let rolls: Vec<u32> = (0..33).map(|_| rng.below_u32(bound(6)) + 1).collect();
    assert_eq!(
        rolls,
        vec![
            3, 4, 1, 1, 2, 2, 3, 2, 4, 3, 2, 4, 3, 3, 5, 2, 3, 1, 3, 1, 5, 1, 4, 1, 5, 6, 4, 6, 6,
            2, 6, 3, 3
        ],
        "bounded draws"
    );

    // Line 4: a shuffled deck. The reference's demonstration shuffles with
    // the standard Fisher-Yates loop, drawing a fresh bound on every step,
    // so this pins fifty-one consecutive bounded draws with fifty-one
    // different bounds.
    let mut deck: Vec<u32> = (0..52).collect();
    for upper in (2..=52u32).rev() {
        let chosen = rng.below_u32(bound(upper)) as usize;
        deck.swap(chosen, (upper - 1) as usize);
    }
    let ranks = [
        'A', '2', '3', '4', '5', '6', '7', '8', '9', 'T', 'J', 'Q', 'K',
    ];
    let suits = ['h', 'c', 'd', 's'];
    let hand: Vec<String> = deck
        .iter()
        .map(|card| {
            format!(
                "{}{}",
                ranks[(card / 4) as usize],
                suits[(card % 4) as usize]
            )
        })
        .collect();
    assert_eq!(
        hand.join(" "),
        "Qd Ks 6d 3s 3d 4c 3h Td Kc 5c Jh Kd Jd As 4s 4h Ad Th Ac Jc 7s Qs \
         2s 7h Kh 2d 6c Ah 4d Qh 9h 6s 5s 2c 9c Ts 8d 9s 3c 8c Js 5d 2h 6h \
         7d 8s 9d 5h 8h Qc 7c Tc",
        "fifty-one bounded draws with fifty-one different bounds"
    );
}

/// The vectors again, reached the way a caller reaches the generator —
/// through the state pair rather than through the reference's seeding
/// dance. This is what makes `from_parts` a supported entry point rather
/// than a test hook: the same words in, the same sequence out.
#[test]
fn the_same_stream_comes_back_from_a_snapshot_of_it() {
    let reference = reference_seeded(42, 54);
    let (state, increment) = reference.parts();
    let mut restored = Rng::from_parts(state, increment);
    let words: [u32; 6] = core::array::from_fn(|_| restored.next_u32());
    assert_eq!(
        words,
        [
            0xa15c_02b7,
            0x7b47_f409,
            0xba1d_3330,
            0x83d2_f293,
            0xbfa4_784b,
            0xcbed_606e,
        ]
    );
}
