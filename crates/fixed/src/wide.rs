//! Products that have not been narrowed yet.

use crate::Fixed;

/// The fractional bits of a [`Fixed`].
const FRAC_BITS: u32 = 16;

/// A product of two [`Fixed`] values, kept at full width: Q95.32 in an `i128`.
///
/// # Why this exists
///
/// [`Fixed::saturating_mul`] narrows its `i128` product back to an `i64`,
/// which is right for arithmetic that stays in the world and wrong for
/// geometry that squares things twice. Ray-versus-sphere forms `b² − 4ac`
/// where `a` and `c` are themselves squared lengths; at ordinary game scales
/// `a·c` overflows a `Fixed` long before the ray does anything unusual, and
/// the result saturates — deterministically, and to the wrong number.
///
/// So the narrowing is deferred. A product of two full-range `Fixed` values
/// needs 126 bits and an `i128` holds 127, which means **a single `wide_mul`
/// can never overflow**, for any inputs at all. Sums of a few of them cannot
/// either at any scale a world reaches.
///
/// # Contract
///
/// - **32 fractional bits**, being the sum of its operands' sixteen. That is
///   not an implementation detail: it is why [`Wide::sqrt`] needs no shift.
/// - **Ordering is exact**, so comparing two products — which is most of what
///   geometry does with them — never rounds at all.
/// - **Narrowing is explicit**, and says whether it lost anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Wide(i128);

impl Wide {
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// The raw Q95.32 pattern.
    #[must_use]
    pub const fn from_bits(raw: i128) -> Self {
        Self(raw)
    }

    /// The raw pattern back out.
    #[must_use]
    pub const fn to_bits(self) -> i128 {
        self.0
    }

    /// −1, 0 or 1.
    #[must_use]
    pub const fn signum(self) -> i32 {
        self.0.signum() as i32
    }

    /// The square root, as a [`Fixed`].
    ///
    /// **No shift, and that is the point of 32 fractional bits.** A `Wide`
    /// holding the real value `v` has raw pattern `v · 2³²`, whose integer
    /// square root is `√v · 2¹⁶` — exactly a `Fixed`'s raw pattern. Squaring
    /// and then rooting therefore loses nothing to scaling, where the
    /// narrow path had to shift left by sixteen first and could overflow
    /// doing it.
    ///
    /// Floor-exact, by `u128::isqrt`'s own contract.
    ///
    /// # Panics
    ///
    /// If negative. See [`Wide::checked_sqrt`].
    #[must_use]
    pub fn sqrt(self) -> Fixed {
        assert!(self.0 >= 0, "Wide::sqrt of a negative value");
        self.checked_sqrt().unwrap_or(Fixed::ZERO)
    }

    /// The square root, or `None` if negative — which a discriminant is,
    /// routinely, and which is a miss rather than a mistake.
    #[must_use]
    pub fn checked_sqrt(self) -> Option<Fixed> {
        if self.0 < 0 {
            return None;
        }
        #[expect(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "non-negative by the check above; the root of a value below 2^127 is \
                      below 2^64 and the callers that matter stay far inside that"
        )]
        let root = (self.0 as u128).isqrt() as i64;
        Some(Fixed::from_bits(root))
    }

    /// Back to a [`Fixed`], or `None` if it will not fit.
    #[must_use]
    pub const fn checked_narrow(self) -> Option<Fixed> {
        let rounded = round_shift(self.0);
        if rounded > i64::MAX as i128 || rounded < i64::MIN as i128 {
            None
        } else {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the branches above establish the value is in range"
            )]
            let narrowed = rounded as i64;
            Some(Fixed::from_bits(narrowed))
        }
    }
}

/// Shift a Q95.32 value down to Q47.16, rounding to nearest, ties away from
/// zero — the same rule the scalar multiply uses, so a value that travels the
/// wide path and one that does not agree.
const fn round_shift(value: i128) -> i128 {
    let half = 1i128 << (FRAC_BITS - 1);
    if value >= 0 {
        (value + half) >> FRAC_BITS
    } else {
        -((-value + half) >> FRAC_BITS)
    }
}

impl Fixed {
    /// Multiply without narrowing, so nothing can overflow.
    ///
    /// The form to reach for when the product is itself going to be squared,
    /// summed with other products, or only compared — which covers most of
    /// what collision detection does with a multiply.
    #[must_use]
    pub const fn wide_mul(self, other: Self) -> Wide {
        Wide(self.to_bits() as i128 * other.to_bits() as i128)
    }
}

impl core::ops::Add for Wide {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl core::ops::Sub for Wide {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

impl core::ops::Neg for Wide {
    type Output = Self;
    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

#[cfg(test)]
mod tests {
    use super::Wide;
    use crate::Fixed;

    #[test]
    fn a_product_of_two_extremes_does_not_overflow() {
        // The case the narrow multiply saturates on, and this one cannot.
        let wide = Fixed::MAX.wide_mul(Fixed::MAX);
        assert!(wide.to_bits() > 0, "the product stayed positive");
        assert_eq!(wide.checked_narrow(), None, "and it does not fit a Fixed");
        // Which is the whole point: it is representable here and reportable
        // there, rather than clamped in silence.
        assert_eq!(Fixed::MIN.wide_mul(Fixed::MAX).signum(), -1);
    }

    #[test]
    fn squaring_then_rooting_returns_the_value() {
        for units in [1i32, 2, 7, 100, 4096, 1_000_000] {
            let value = Fixed::from_int(units);
            let root = value.wide_mul(value).sqrt();
            assert_eq!(root, value, "sqrt of {units} squared should be {units}");
        }
    }

    /// The scaling property the type exists for: no shift between squaring
    /// and rooting, so nothing overflows on the way.
    #[test]
    fn rooting_works_at_magnitudes_the_narrow_path_cannot_reach() {
        // A value whose square does not fit a Fixed at all.
        let big = Fixed::from_int(100_000_000);
        assert_eq!(
            big.wide_mul(big).checked_narrow(),
            None,
            "square does not fit"
        );
        // And yet its root comes back exactly.
        assert_eq!(big.wide_mul(big).sqrt(), big);
    }

    #[test]
    fn narrowing_rounds_the_way_the_scalar_multiply_does() {
        // A value that travels the wide path and one that does not must
        // agree, or the two multiplies are different arithmetic.
        for (a, b) in [(3, 7), (-3, 7), (3, -7), (-3, -7), (1, 3), (-1, 3)] {
            let x = Fixed::from_ratio(a, 4);
            let y = Fixed::from_ratio(b, 8);
            assert_eq!(
                x.wide_mul(y).checked_narrow(),
                Some(x.saturating_mul(y)),
                "wide and narrow multiply disagreed on {a}/4 * {b}/8"
            );
        }
    }

    #[test]
    fn a_negative_value_has_no_root_and_says_so() {
        let negative = Fixed::from_int(-1).wide_mul(Fixed::from_int(1));
        assert_eq!(negative.checked_sqrt(), None);
        assert_eq!(negative.signum(), -1);
        assert_eq!(Wide::ZERO.signum(), 0);
        assert_eq!(Wide::ZERO.sqrt(), Fixed::ZERO);
    }

    #[test]
    #[should_panic(expected = "Wide::sqrt of a negative value")]
    fn the_asserting_root_refuses_a_negative() {
        let _ = Fixed::from_int(-1).wide_mul(Fixed::ONE).sqrt();
    }

    /// Sums and differences, which is what a discriminant is.
    #[test]
    fn wide_values_add_subtract_and_order() {
        let two = Fixed::from_int(2);
        let three = Fixed::from_int(3);
        let six = two.wide_mul(three);
        let four = two.wide_mul(two);
        assert!(six > four);
        assert_eq!((six - four).checked_narrow(), Some(Fixed::from_int(2)));
        assert_eq!((four + four).checked_narrow(), Some(Fixed::from_int(8)));
        assert_eq!((-six).signum(), -1);
        // Ordering is exact, which is what makes comparing products safe.
        assert_eq!(six.max(four), six);
    }
}
