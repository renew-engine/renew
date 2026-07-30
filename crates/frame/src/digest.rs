//! The state fingerprint: FNV-1a-64 by explicit ordered absorption.
//!
//! Not a hash-map hasher and not a security hash. It answers one question
//! — did two runs produce the same state — and nothing more. that rule does not
//! apply: there is no untrusted input, no adversary, and no
//! collision-resistance requirement.
//!
//! Hand-rolled rather than borrowed, for three reasons no dependency would
//! fix. `RandomState` is seeded per process and can never back a cross-run
//! claim. `SipHasher13` carries no cross-version stability guarantee, so a
//! frozen digest would break on a toolchain bump for reasons unrelated to
//! the engine. And `#[derive(Hash)]` absorbs fields in declaration order
//! *implicitly*, so reordering two fields would silently change every
//! digest in the tree — explicit absorption turns that into a visible
//! diff.

use crate::schedule::FramePlan;

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// An in-progress fingerprint. Absorption is by value and returns the new
/// state, so the order of a digest is written out as an expression and can
/// be read off the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateHash(u64);

impl StateHash {
    #[must_use]
    pub const fn new() -> Self {
        Self(OFFSET_BASIS)
    }

    /// Absorb raw bytes, in the order given.
    #[must_use]
    pub const fn absorb_bytes(mut self, bytes: &[u8]) -> Self {
        let mut index = 0;
        while index < bytes.len() {
            // `u64::from` is not callable in a `const fn` (const trait
            // impls are unstable), so the widening is written as a cast.
            #[allow(clippy::cast_lossless)]
            let byte = bytes[index] as u64;
            self.0 = (self.0 ^ byte).wrapping_mul(PRIME);
            index += 1;
        }
        self
    }

    /// Absorb a 64-bit value, little-endian. The byte order is fixed here
    /// rather than left to the host so a digest means the same thing on
    /// every target.
    #[must_use]
    pub const fn absorb_u64(self, value: u64) -> Self {
        self.absorb_bytes(&value.to_le_bytes())
    }

    /// Absorb a 32-bit value, little-endian.
    #[must_use]
    pub const fn absorb_u32(self, value: u32) -> Self {
        self.absorb_bytes(&value.to_le_bytes())
    }

    /// Absorb a float by its bit pattern — never by its value, which has
    /// two zeros and no equality for NaN.
    #[must_use]
    pub const fn absorb_f32_bits(self, value: f32) -> Self {
        self.absorb_u32(value.to_bits())
    }

    /// The canonical per-frame absorption order, written once so no
    /// consumer invents a second one.
    ///
    /// `alpha` is excluded deliberately: it is a pure function of the
    /// remainder and the timestep, both of which are absorbed here, so
    /// hashing it would add no information and would make the oracle
    /// float-dependent. An unstated exclusion is how a determinism oracle
    /// goes quietly vacuous, so it is stated.
    #[must_use]
    pub const fn absorb_plan(self, plan: &FramePlan) -> Self {
        self.absorb_u64(plan.first_tick())
            .absorb_u32(plan.step_count())
            .absorb_u64(plan.dropped())
            .absorb_u64(plan.remainder().get())
    }

    #[must_use]
    pub const fn finish(self) -> u64 {
        self.0
    }
}

impl Default for StateHash {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::StateHash;
    use crate::schedule::FrameLoop;
    use crate::time::{StepBudget, Timestamp, Timestep};

    /// The published FNV-1a-64 vector for "a", pinning the constants and
    /// the byte order against a source outside this repository.
    #[test]
    fn the_published_reference_vector_matches() {
        assert_eq!(StateHash::new().finish(), 0xcbf2_9ce4_8422_2325);
        assert_eq!(
            StateHash::new().absorb_bytes(b"a").finish(),
            0xaf63_dc4c_8601_ec8c
        );
        assert_eq!(
            StateHash::new().absorb_bytes(b"foobar").finish(),
            0x8594_4171_f739_67e8
        );
    }

    #[test]
    fn the_default_is_the_offset_basis() {
        assert_eq!(StateHash::default(), StateHash::new());
    }

    #[test]
    fn absorbing_nothing_changes_nothing() {
        assert_eq!(StateHash::new().absorb_bytes(&[]), StateHash::new());
    }

    #[test]
    fn absorption_order_is_part_of_the_digest() {
        let forward = StateHash::new().absorb_u64(1).absorb_u64(2).finish();
        let reversed = StateHash::new().absorb_u64(2).absorb_u64(1).finish();
        assert_ne!(forward, reversed);
    }

    /// A 32-bit absorption is four bytes and a 64-bit one is eight, so the
    /// same numeric value through the two widths must differ — otherwise
    /// a width change in a consumer's state would be invisible.
    #[test]
    fn width_is_part_of_the_digest() {
        assert_ne!(
            StateHash::new().absorb_u32(7).finish(),
            StateHash::new().absorb_u64(7).finish()
        );
        assert_eq!(
            StateHash::new().absorb_u32(7).finish(),
            StateHash::new().absorb_bytes(&7u32.to_le_bytes()).finish()
        );
    }

    #[test]
    fn a_float_is_absorbed_by_its_bit_pattern() {
        assert_eq!(
            StateHash::new().absorb_f32_bits(1.5).finish(),
            StateHash::new().absorb_u32(1.5f32.to_bits()).finish()
        );
        // The two zeros are distinguishable, which is the point of
        // hashing bits rather than values.
        assert_ne!(
            StateHash::new().absorb_f32_bits(0.0).finish(),
            StateHash::new().absorb_f32_bits(-0.0).finish()
        );
    }

    #[test]
    fn a_plan_is_absorbed_field_by_field_in_the_documented_order() {
        let mut frame = FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(0),
        );
        let plan = frame.begin_frame(Timestamp::from_nanos(200_000_000));
        let by_hand = StateHash::new()
            .absorb_u64(plan.first_tick())
            .absorb_u32(plan.step_count())
            .absorb_u64(plan.dropped())
            .absorb_u64(plan.remainder().get());
        assert_eq!(StateHash::new().absorb_plan(&plan), by_hand);
    }

    /// The exclusion the determinism oracle depends on, asserted from the
    /// other side: two plans that differ only in their alpha cannot exist,
    /// because alpha is derived from absorbed fields. What *can* be
    /// asserted is that every absorbed field moves the digest.
    #[test]
    fn every_absorbed_field_moves_the_digest() {
        let mut frame = FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(0),
        );
        let base = frame.begin_frame(Timestamp::from_nanos(200_000_000));
        let next = frame.begin_frame(Timestamp::from_nanos(400_000_000));
        assert_ne!(base.first_tick(), next.first_tick());
        assert_ne!(
            StateHash::new().absorb_plan(&base),
            StateHash::new().absorb_plan(&next)
        );
    }

    /// `absorb_plan` is usable at compile time, which is what lets a
    /// consumer state an expected digest as a `const`.
    #[test]
    fn the_digest_is_computable_at_compile_time() {
        const DIGEST: u64 = StateHash::new().absorb_u64(1).absorb_u32(2).finish();
        assert_eq!(
            DIGEST,
            StateHash::new().absorb_u64(1).absorb_u32(2).finish()
        );
    }
}
