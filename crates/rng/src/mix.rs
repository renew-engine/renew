//! The bit mixer every derivation in this crate runs through: `SplitMix64`.
//!
//! Two jobs, both of them structural rather than statistical.
//!
//! The first is decorrelation. Callers hand this crate small, adjacent,
//! human-chosen numbers — seed 1, entity 2, stream 3 — and a generator
//! seeded straight from such a number starts in a corner of its state
//! space. `SplitMix64`'s finalizer is the standard answer: an
//! xor-shift-multiply avalanche that turns a one-bit input change into a
//! half-of-the-output change.
//!
//! The second is provability. The finalizer is a bijection, so the map
//! from a stream identifier to a generator's starting point is injective
//! for a fixed master seed. That turns "different streams are probably
//! different" into "different streams cannot collide", which is a claim a
//! test can hold.
//!
//! `SplitMix64` rather than something invented here because it is published,
//! frozen, and has known output for a known seed — the same standard the
//! generator itself is held to. Its constants are not adjustable and its
//! output for seed zero is pinned by the unit test below.

/// The golden-ratio odd increment `SplitMix64` walks its state by. Odd, so
/// stepping it visits all 2^64 values before repeating.
pub(crate) const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

/// `SplitMix64`'s finalizer, applied to one value. A bijection with
/// avalanche: flipping one input bit flips about half the output bits.
pub(crate) const fn mix(value: u64) -> u64 {
    let mixed = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    mixed ^ (mixed >> 31)
}

/// The `SplitMix64` generator: step the state by [`GAMMA`], finalize, emit.
///
/// Used only to expand one 64-bit derivation root into the two words a
/// generator needs. It is never handed to a caller — one published
/// algorithm is enough to audit, and this one exists to seed the other.
pub(crate) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(crate) const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub(crate) const fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        mix(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::{GAMMA, SplitMix64, mix};

    /// The published `SplitMix64` output for seed zero, pinning the two
    /// multipliers, the three shift distances and the increment against a
    /// source outside this repository. Every derived generator in the
    /// crate walks through this function, so these eight values stand
    /// behind every stream the engine will ever draw from.
    #[test]
    fn the_published_reference_vector_matches() {
        let mut walk = SplitMix64::new(0);
        let got: [u64; 8] = core::array::from_fn(|_| walk.next());
        assert_eq!(
            got,
            [
                0xE220_A839_7B1D_CDAF,
                0x6E78_9E6A_A1B9_65F4,
                0x06C4_5D18_8009_454F,
                0xF88B_B8A8_724C_81EC,
                0x1B39_896A_51A8_749B,
                0x53CB_9F0C_747E_A2EA,
                0x2C82_9ABE_1F45_32E1,
                0xC584_133A_C916_AB3C,
            ]
        );
    }

    /// The walk is exactly "add gamma, then finalize", so its first output
    /// is reproducible from the finalizer alone. This is what makes the
    /// vector above a check on `mix` and not only on the pair.
    #[test]
    fn the_walk_is_the_finalizer_over_a_gamma_step() {
        let mut walk = SplitMix64::new(0x1234_5678_9abc_def0);
        assert_eq!(
            walk.next(),
            mix(0x1234_5678_9abc_def0u64.wrapping_add(GAMMA))
        );
        assert_eq!(
            walk.next(),
            mix(0x1234_5678_9abc_def0u64
                .wrapping_add(GAMMA)
                .wrapping_add(GAMMA))
        );
    }

    /// Injectivity is the property the stream-derivation guarantee rests
    /// on, so it is asserted rather than assumed. Exhaustive proof is out
    /// of reach for a 64-bit domain; a dense sweep plus the structural
    /// argument (three invertible steps: xor-shift, odd multiply,
    /// xor-shift) is what is available.
    #[test]
    fn the_finalizer_does_not_collide_over_a_dense_sweep() {
        // Two disjoint input sets: the small adjacent integers callers
        // actually pass, and a stride across the whole 64-bit range.
        let inputs: std::collections::BTreeSet<u64> = (0..40_000u64)
            .chain((0..40_000u64).map(|step| step.wrapping_mul(GAMMA)))
            .collect();
        let outputs: std::collections::BTreeSet<u64> =
            inputs.iter().map(|&value| mix(value)).collect();
        assert_eq!(inputs.len(), outputs.len());
    }

    /// Avalanche, measured rather than asserted from the literature: one
    /// input bit flipped moves close to half the output bits. A mixer that
    /// merely permuted bytes would pass injectivity above and fail here.
    #[test]
    fn one_input_bit_moves_about_half_the_output_bits() {
        let mut total = 0u32;
        let mut samples = 0u32;
        let mut worst = 64u32;
        for value in 0..1_000u64 {
            for bit in 0..64 {
                let moved = (mix(value) ^ mix(value ^ (1 << bit))).count_ones();
                total += moved;
                samples += 1;
                worst = worst.min(moved);
            }
        }
        // Mean in [30, 34] out of 64, and no single flip moving fewer than
        // 8 bits. Integer arithmetic on purpose: this crate has no floats.
        assert!(
            (30..=34).contains(&(total / samples)),
            "mean {total}/{samples}"
        );
        assert!(worst >= 8, "a one-bit change moved only {worst} bits");
    }
}
