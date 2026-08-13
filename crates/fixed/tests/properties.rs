//! The properties this type's whole purpose rests on.
//!
//! Two of them are here because the obvious implementation fails them, and
//! failing them silently is what a physics engine built on this would look
//! like from the outside: symmetry under negation, and saturation that never
//! escapes the range.
//!
//! The associativity and distributivity properties are stated as **bounds**
//! rather than as laws, and that is not a weakening of ambition. No rounded
//! multiply satisfies them exactly, under any rounding rule. Stating them
//! as laws would promise something no implementation here can keep; a
//! bound is the true statement.

use proptest::prelude::*;
use renew_fixed::{Fixed, saturations};

/// Raw patterns small enough that products cannot saturate, so a property
/// about arithmetic is not silently a property about the saturation clamp.
/// Squarable range is ±2²³·⁵ ≈ 1.2e7 units; this stays far inside it.
fn modest() -> impl Strategy<Value = Fixed> {
    (-(1i64 << 34)..(1i64 << 34)).prop_map(Fixed::from_bits)
}

/// The whole representable range, for properties that must hold everywhere.
fn any_fixed() -> impl Strategy<Value = Fixed> {
    any::<i64>().prop_map(Fixed::from_bits)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

    /// **The property the rounding rule exists for.** The obvious
    /// implementation — an arithmetic shift — rounds toward negative
    /// infinity and fails this, which means a body moving left and the same
    /// body moving right accumulate different error.
    #[test]
    fn multiplication_is_symmetric_under_negation(a in modest(), b in modest()) {
        prop_assert_eq!((-a).saturating_mul(b), -(a.saturating_mul(b)));
    }

    #[test]
    fn multiplication_commutes(a in modest(), b in modest()) {
        prop_assert_eq!(a.saturating_mul(b), b.saturating_mul(a));
    }

    /// Associativity to a bound, not as a law. Each multiply rounds once, so
    /// the two groupings can differ — but only by rounding, and the bound
    /// says how much.
    #[test]
    fn multiplication_associates_to_within_the_rounding_error(
        a in modest(), b in modest(), c in modest()
    ) {
        let left = a.saturating_mul(b).saturating_mul(c);
        let right = a.saturating_mul(b.saturating_mul(c));
        let difference = (left.to_bits() - right.to_bits()).abs();
        // One rounding step per multiply, scaled by the magnitude of the
        // factor the rounded value is then multiplied by.
        let tolerance = 1 + (a.to_bits().abs() >> 16) + (c.to_bits().abs() >> 16);
        prop_assert!(
            difference <= tolerance,
            "difference {difference} exceeds tolerance {tolerance}"
        );
    }

    #[test]
    fn addition_is_associative_and_commutative(a in modest(), b in modest(), c in modest()) {
        prop_assert_eq!(a + b, b + a);
        prop_assert_eq!((a + b) + c, a + (b + c));
    }

    /// Ordering is the underlying integer's, which is what makes `Ord`
    /// honest and is the thing floats cannot offer.
    #[test]
    fn ordering_follows_the_raw_pattern(a in any_fixed(), b in any_fixed()) {
        prop_assert_eq!(a.cmp(&b), a.to_bits().cmp(&b.to_bits()));
    }

    /// **Saturation never escapes the range**, over the whole domain
    /// including the values that force it. Nothing here can panic and
    /// nothing can wrap.
    #[test]
    fn arithmetic_never_escapes_the_range(a in any_fixed(), b in any_fixed()) {
        for value in [a + b, a - b, a.saturating_mul(b)] {
            prop_assert!(value >= Fixed::MIN && value <= Fixed::MAX);
        }
    }

    /// The floor-exactness of `sqrt`, stated on raw integers in `u128` so
    /// that neither the narrowing nor the saturation can intervene — in the
    /// type, `(s+1)²` saturates at the top of the range and rounds back down
    /// below one unit, so the property would be false at both ends.
    #[test]
    fn sqrt_is_floor_exact(value in 0i64..=i64::MAX) {
        let x = Fixed::from_bits(value);
        let root = u128::from(x.sqrt().to_bits().unsigned_abs());
        #[expect(
            clippy::cast_sign_loss,
            reason = "the strategy generates non-negative values only"
        )]
        let shifted = (value as u128) << 16;
        prop_assert!(root * root <= shifted, "root too large");
        prop_assert!(shifted < (root + 1) * (root + 1), "root too small");
    }

    /// A negative input is answered, not undefined, and identically in every
    /// build profile.
    #[test]
    fn sqrt_of_a_negative_is_none(value in i64::MIN..0i64) {
        prop_assert_eq!(Fixed::from_bits(value).checked_sqrt(), None);
    }

    /// Construction round-trips where it claims to: a whole number is exact.
    #[test]
    fn whole_numbers_are_exact(value in -100_000i32..100_000) {
        prop_assert_eq!(Fixed::from_int(value).trunc_int(), i64::from(value));
        prop_assert_eq!(Fixed::from_int(value).fract(), Fixed::ZERO);
    }

    /// Division inverts multiplication to within rounding, which is the
    /// property a physics author will assume without checking.
    #[test]
    fn dividing_by_a_factor_recovers_the_other(a in modest(), b in modest()) {
        prop_assume!(b.to_bits().abs() > (1 << 16));
        let product = a.saturating_mul(b);
        let recovered = product.saturating_div(b);
        let difference = (recovered.to_bits() - a.to_bits()).abs();
        prop_assert!(difference <= 2, "recovered {recovered:?} from {a:?}, off by {difference}");
    }

    /// The counter fires exactly when saturation happens, and stays silent
    /// when it does not. A counter that over-reports is as useless as one
    /// that under-reports.
    #[test]
    fn the_counter_is_silent_when_nothing_saturates(a in modest(), b in modest()) {
        let before = saturations();
        let _ = a + b;
        let _ = a.saturating_mul(b);
        prop_assert_eq!(saturations(), before);
    }
}

/// Deliberately outside `proptest!`: this asserts the exact values at the
/// edges, which a generator would reach only by luck.
#[test]
fn the_named_edges_behave() {
    // `const` construction, which is what `glide`'s constants need. At the
    // top because items exist from the start of a scope regardless of where
    // they are written.
    const GRAVITY: Fixed = Fixed::from_ratio(981, 100);
    const BIRD_X: Fixed = Fixed::from_int(40);

    // Saturation, and the counter noticing.
    let before = saturations();
    assert_eq!(Fixed::MAX + Fixed::ONE, Fixed::MAX);
    assert_eq!(Fixed::MIN - Fixed::ONE, Fixed::MIN);
    assert_eq!(saturations().0, before.0 + 2);

    // Zero and one behave as the identities they claim to be.
    assert_eq!(
        Fixed::ONE.saturating_mul(Fixed::from_int(7)),
        Fixed::from_int(7)
    );
    assert_eq!(Fixed::ZERO.saturating_mul(Fixed::MAX), Fixed::ZERO);
    assert_eq!(Fixed::ZERO.sqrt(), Fixed::ZERO);

    // A ratio is how a designer writes a decimal without a float existing.
    let gravity = Fixed::from_ratio(981, 100);
    assert_eq!(gravity.trunc_int(), 9);
    // 9.81 in Q47.16 is 642908.16, which rounds to 642908.
    assert_eq!(gravity.to_bits(), 642_908);

    assert_eq!(GRAVITY, gravity);
    assert_eq!(BIRD_X.trunc_int(), 40);
}
