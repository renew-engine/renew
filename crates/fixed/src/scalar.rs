//! The scalar type.

use core::ops::{Add, Div, Mul, Neg, Sub};

use crate::saturation;

/// Fractional bits. Q47.16.
const FRAC_BITS: u32 = 16;

/// One whole unit, as a raw pattern.
const ONE_RAW: i64 = 1 << FRAC_BITS;

/// A fixed-point number: Q47.16 in an `i64`.
///
/// # Contract
///
/// - **Resolution 2⁻¹⁶ ≈ 0.0000153; range ±2⁴⁷ ≈ ±1.4 × 10¹⁴.**
/// - **Every operation saturates on overflow**, in every build profile, and
///   increments the thread's [`crate::saturations`] counter when it does.
///   Never wraps. Never differs between debug and release.
/// - **Multiplication and division round to nearest, ties away from zero.**
///   Symmetric under negation, which the obvious implementation is not — see
///   [`Fixed::saturating_mul`].
/// - **Total ordering.** `Ord`, `Eq` and `Hash` are derived from the `i64`,
///   so this sorts, deduplicates and hashes the way an integer does and
///   floats cannot.
///
/// # Why Q47.16 and not Q32.32
///
/// Because physics squares things. A squared value has to fit the type it is
/// stored in, so the range that matters is not what is representable but what
/// is **squarable** — the square root of the representable range:
///
/// | | representable | squarable |
/// |---|---|---|
/// | Q47.16 | ±1.4 × 10¹⁴ | **±1.2 × 10⁷** |
/// | Q32.32 | ±2.1 × 10⁹ | **±4.6 × 10⁴** |
///
/// Two hundred and fifty-six times the working room, for a resolution that is
/// already finer than anything a game perceives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Fixed(i64);

impl Fixed {
    /// Zero.
    pub const ZERO: Self = Self(0);
    /// One whole unit.
    pub const ONE: Self = Self(ONE_RAW);
    /// The smallest representable value.
    pub const MIN: Self = Self(i64::MIN);
    /// The largest representable value.
    pub const MAX: Self = Self(i64::MAX);
    /// The smallest step between two values: 2⁻¹⁶.
    pub const EPSILON: Self = Self(1);

    /// The raw Q47.16 pattern, for serialisation and tests.
    #[must_use]
    pub const fn from_bits(raw: i64) -> Self {
        Self(raw)
    }

    /// The raw pattern back out.
    #[must_use]
    pub const fn to_bits(self) -> i64 {
        self.0
    }

    /// A whole number, exactly.
    ///
    /// `i32` rather than `i64` so the shift cannot overflow: every `i32`
    /// shifted left by 16 fits an `i64` with room to spare, which makes this
    /// total and lets it be `const`.
    #[must_use]
    pub const fn from_int(value: i32) -> Self {
        Self((value as i64) << FRAC_BITS)
    }

    /// A ratio of two integers — how a value like 9.81 is written without a
    /// float ever existing: `Fixed::from_ratio(981, 100)`.
    ///
    /// Rounds to nearest, ties away from zero, like [`Fixed::saturating_mul`].
    ///
    /// # Panics
    ///
    /// If `denominator` is zero. A contract violation rather than a runtime
    /// condition (D5): the arguments are almost always literals, so this
    /// fails at the call site that wrote it, and in a `const` context it
    /// fails at compile time.
    #[must_use]
    pub const fn from_ratio(numerator: i32, denominator: i32) -> Self {
        assert!(
            denominator != 0,
            "Fixed::from_ratio needs a nonzero denominator"
        );
        let scaled = (numerator as i64) << FRAC_BITS;
        let den = denominator as i64;
        Self(round_div(scaled, den))
    }

    /// The whole part, truncated toward zero.
    #[must_use]
    pub const fn trunc_int(self) -> i64 {
        self.0 / ONE_RAW
    }

    /// The fractional part, with the sign of the whole.
    #[must_use]
    pub const fn fract(self) -> Self {
        Self(self.0 % ONE_RAW)
    }

    /// Absolute value, saturating at [`Fixed::MAX`] for [`Fixed::MIN`].
    #[must_use]
    pub fn abs(self) -> Self {
        let Some(value) = self.0.checked_abs() else {
            saturation::record();
            return Self::MAX;
        };
        Self(value)
    }

    /// -1, 0 or 1, as whole units.
    #[must_use]
    pub const fn signum(self) -> Self {
        Self(ONE_RAW * self.0.signum())
    }

    /// The smaller of two values.
    #[must_use]
    pub const fn min(self, other: Self) -> Self {
        if self.0 < other.0 { self } else { other }
    }

    /// The larger of two values.
    #[must_use]
    pub const fn max(self, other: Self) -> Self {
        if self.0 > other.0 { self } else { other }
    }

    /// Constrained to `[low, high]`.
    ///
    /// # Panics
    ///
    /// If `low > high`, which is a contract violation rather than a value to
    /// interpret — the caller has said something they cannot mean.
    #[must_use]
    pub const fn clamp(self, low: Self, high: Self) -> Self {
        assert!(low.0 <= high.0, "Fixed::clamp needs low <= high");
        self.max(low).min(high)
    }

    /// Multiply, rounding to nearest with ties away from zero.
    ///
    /// **The rounding rule is load-bearing.** The obvious implementation —
    /// `(a as i128 * b as i128) >> 16` — is an arithmetic shift, which rounds
    /// toward negative infinity and is therefore *asymmetric under negation*:
    /// `(-a) * b` and `-(a * b)` differ for some inputs. That is deterministic
    /// and still wrong for physics, because a body moving left and the same
    /// body moving right would accumulate different error. Rounding to nearest
    /// with ties away from zero is symmetric, and halves the worst-case error
    /// besides.
    ///
    /// Saturates rather than wrapping, and counts when it does.
    #[must_use]
    pub fn saturating_mul(self, other: Self) -> Self {
        let product = i128::from(self.0) * i128::from(other.0);
        Self(narrow(round_shift(product)))
    }

    /// Divide, rounding to nearest with ties away from zero.
    ///
    /// # Panics
    ///
    /// If `other` is zero. Division by zero is a contract violation (D5), and
    /// returning a sentinel would put a NaN-shaped value into a type whose
    /// whole contract is that it has none.
    #[must_use]
    pub fn saturating_div(self, other: Self) -> Self {
        assert!(other.0 != 0, "Fixed division by zero");
        let numerator = i128::from(self.0) << FRAC_BITS;
        Self(narrow(round_div_i128(numerator, i128::from(other.0))))
    }

    /// The square root, floored to the representable value below the exact
    /// result.
    ///
    /// Uses `u128::isqrt`, which is exact by its own contract, on a `u128`
    /// intermediate — the shifted value needs 79 bits, so a 64-bit one would
    /// be wrong rather than merely slower. Not a hand-rolled iteration: the
    /// standard library's is boring and already correct, and this is the one
    /// kernel here with a non-trivial correctness argument.
    ///
    /// # Panics
    ///
    /// If `self` is negative. See [`Fixed::checked_sqrt`] for the form that
    /// answers instead of refusing.
    #[must_use]
    pub fn sqrt(self) -> Self {
        assert!(self.0 >= 0, "Fixed::sqrt of a negative value");
        // The assertion above is the whole precondition, so the only `None`
        // this can produce is one the assertion already refused.
        self.checked_sqrt().unwrap_or(Self::ZERO)
    }

    /// The square root, or `None` for a negative value.
    #[must_use]
    pub fn checked_sqrt(self) -> Option<Self> {
        if self.0 < 0 {
            return None;
        }
        // Both casts are guarded by the sign check above: the widening is
        // value-preserving on a non-negative input, and the root of a value
        // below 2^63 shifted left by 16 is below 2^40, so narrowing it back
        // cannot reach the sign bit.
        #[expect(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "guarded by the sign check above and by the root's own magnitude"
        )]
        let root = ((self.0 as u128) << FRAC_BITS).isqrt() as i64;
        Some(Self(root))
    }

    /// Add, or `None` if the result would not fit.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(sum) => Some(Self(sum)),
            None => None,
        }
    }

    /// Subtract, or `None` if the result would not fit.
    #[must_use]
    pub const fn checked_sub(self, other: Self) -> Option<Self> {
        match self.0.checked_sub(other.0) {
            Some(difference) => Some(Self(difference)),
            None => None,
        }
    }

    /// Multiply, or `None` if the result would not fit.
    ///
    /// `const`, and therefore not counted: a compile-time context has no
    /// thread to count on, and a caller asking this question wants the answer
    /// rather than a diagnostic.
    #[must_use]
    pub const fn checked_mul(self, other: Self) -> Option<Self> {
        let rounded = round_shift(self.0 as i128 * other.0 as i128);
        if rounded > i64::MAX as i128 || rounded < i64::MIN as i128 {
            None
        } else {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the branches above establish the value is in range"
            )]
            let narrowed = rounded as i64;
            Some(Self(narrowed))
        }
    }
}

/// Shift a 128-bit product right by the fractional bits, rounding to nearest
/// with ties away from zero.
const fn round_shift(product: i128) -> i128 {
    let half = 1i128 << (FRAC_BITS - 1);
    if product >= 0 {
        (product + half) >> FRAC_BITS
    } else {
        // Symmetric: round the magnitude, then restore the sign. Using the
        // shift directly here is what makes the operation asymmetric.
        -((-product + half) >> FRAC_BITS)
    }
}

/// Divide, rounding to nearest with ties away from zero.
const fn round_div(numerator: i64, denominator: i64) -> i64 {
    let (magnitude, negative) = match (numerator < 0, denominator < 0) {
        (false, false) => (numerator / denominator, false),
        (true, true) => ((-numerator) / (-denominator), false),
        (true, false) => ((-numerator) / denominator, true),
        (false, true) => (numerator / (-denominator), true),
    };
    let remainder = (numerator % denominator).abs();
    let half = denominator.abs() / 2;
    let rounded = if remainder * 2 >= denominator.abs() && half >= 0 {
        magnitude + 1
    } else {
        magnitude
    };
    if negative { -rounded } else { rounded }
}

/// The 128-bit form of [`round_div`].
const fn round_div_i128(numerator: i128, denominator: i128) -> i128 {
    let negative = (numerator < 0) != (denominator < 0);
    let num = if numerator < 0 { -numerator } else { numerator };
    let den = if denominator < 0 {
        -denominator
    } else {
        denominator
    };
    let quotient = num / den;
    let rounded = if (num % den) * 2 >= den {
        quotient + 1
    } else {
        quotient
    };
    if negative { -rounded } else { rounded }
}

/// Bring a 128-bit result back to an `i64`, saturating and counting.
fn narrow(value: i128) -> i64 {
    if value > i128::from(i64::MAX) {
        saturation::record();
        i64::MAX
    } else if value < i128::from(i64::MIN) {
        saturation::record();
        i64::MIN
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the branches above establish the value is in range"
        )]
        let narrowed = value as i64;
        narrowed
    }
}

impl Add for Fixed {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        let Some(sum) = self.0.checked_add(other.0) else {
            saturation::record();
            return if self.0 > 0 { Self::MAX } else { Self::MIN };
        };
        Self(sum)
    }
}

impl Sub for Fixed {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        let Some(difference) = self.0.checked_sub(other.0) else {
            saturation::record();
            return if self.0 > 0 { Self::MAX } else { Self::MIN };
        };
        Self(difference)
    }
}

impl Neg for Fixed {
    type Output = Self;
    fn neg(self) -> Self {
        let Some(negated) = self.0.checked_neg() else {
            saturation::record();
            return Self::MAX;
        };
        Self(negated)
    }
}

impl Mul for Fixed {
    type Output = Self;
    fn mul(self, other: Self) -> Self {
        self.saturating_mul(other)
    }
}

impl Div for Fixed {
    type Output = Self;
    fn div(self, other: Self) -> Self {
        self.saturating_div(other)
    }
}

#[cfg(test)]
mod tests {
    use super::{Fixed, round_div};
    use crate::saturations;

    #[test]
    fn absolute_value_saturates_at_the_bottom_of_the_range() {
        assert_eq!(Fixed::from_int(-3).abs(), Fixed::from_int(3));
        assert_eq!(Fixed::from_int(3).abs(), Fixed::from_int(3));
        assert_eq!(Fixed::ZERO.abs(), Fixed::ZERO);
        // MIN has no positive counterpart, which is the one input where
        // this cannot answer exactly.
        let before = saturations();
        assert_eq!(Fixed::MIN.abs(), Fixed::MAX);
        assert_eq!(saturations().0, before.0 + 1, "the clamp must be counted");
    }

    #[test]
    fn signum_reports_whole_units() {
        assert_eq!(Fixed::from_int(-9).signum(), Fixed::from_int(-1));
        assert_eq!(Fixed::ZERO.signum(), Fixed::ZERO);
        assert_eq!(Fixed::from_ratio(1, 1000).signum(), Fixed::ONE);
    }

    #[test]
    fn min_max_and_clamp_agree_with_the_ordering() {
        let low = Fixed::from_int(-2);
        let high = Fixed::from_int(5);
        assert_eq!(low.min(high), low);
        assert_eq!(low.max(high), high);
        assert_eq!(Fixed::from_int(9).clamp(low, high), high);
        assert_eq!(Fixed::from_int(-9).clamp(low, high), low);
        assert_eq!(Fixed::from_int(1).clamp(low, high), Fixed::from_int(1));
    }

    #[test]
    #[should_panic(expected = "Fixed::clamp needs low <= high")]
    fn clamp_refuses_an_inverted_range() {
        let _ = Fixed::ZERO.clamp(Fixed::ONE, Fixed::ZERO);
    }

    #[test]
    #[should_panic(expected = "Fixed::from_ratio needs a nonzero denominator")]
    fn a_ratio_over_zero_is_refused() {
        let _ = Fixed::from_ratio(1, 0);
    }

    #[test]
    #[should_panic(expected = "Fixed division by zero")]
    fn division_by_zero_is_refused() {
        let _ = Fixed::ONE.saturating_div(Fixed::ZERO);
    }

    #[test]
    #[should_panic(expected = "Fixed::sqrt of a negative value")]
    fn the_square_root_of_a_negative_is_refused() {
        let _ = Fixed::from_int(-1).sqrt();
    }

    /// The checked forms answer instead of clamping, which is what a caller
    /// wanting to handle overflow rather than be told about it asks for.
    #[test]
    fn the_checked_forms_report_rather_than_saturate() {
        assert_eq!(Fixed::ONE.checked_add(Fixed::ONE), Some(Fixed::from_int(2)));
        assert_eq!(Fixed::MAX.checked_add(Fixed::ONE), None);
        assert_eq!(Fixed::ONE.checked_sub(Fixed::ONE), Some(Fixed::ZERO));
        assert_eq!(Fixed::MIN.checked_sub(Fixed::ONE), None);
        assert_eq!(
            Fixed::from_int(3).checked_mul(Fixed::from_int(4)),
            Some(Fixed::from_int(12))
        );
        assert_eq!(Fixed::MAX.checked_mul(Fixed::MAX), None);

        // And they do not touch the counter: a caller asking the question
        // wants the answer, not a diagnostic about having asked.
        let before = saturations();
        let _ = Fixed::MAX.checked_add(Fixed::ONE);
        let _ = Fixed::MAX.checked_mul(Fixed::MAX);
        assert_eq!(saturations(), before);
    }

    #[test]
    fn subtraction_and_negation_saturate_at_both_ends() {
        assert_eq!(Fixed::from_int(5) - Fixed::from_int(3), Fixed::from_int(2));
        assert_eq!(-Fixed::from_int(3), Fixed::from_int(-3));
        let before = saturations();
        assert_eq!(Fixed::MAX - Fixed::MIN, Fixed::MAX);
        assert_eq!(-Fixed::MIN, Fixed::MAX);
        assert_eq!(saturations().0, before.0 + 2);
    }

    /// The whole and fractional parts of a negative value both carry the
    /// sign, which is the convention `i64` division already has and the one
    /// a reader will assume.
    #[test]
    fn the_parts_of_a_negative_value_carry_its_sign() {
        let value = Fixed::from_ratio(-7, 2);
        assert_eq!(value.trunc_int(), -3);
        assert_eq!(value.fract(), Fixed::from_ratio(-1, 2));
    }

    /// Ratios round to nearest with ties away from zero, symmetrically, so
    /// a negative constant is the negation of its positive twin.
    #[test]
    fn ratios_round_symmetrically() {
        assert_eq!(Fixed::from_ratio(-981, 100), -Fixed::from_ratio(981, 100));
        assert_eq!(Fixed::from_ratio(981, -100), -Fixed::from_ratio(981, 100));
        assert_eq!(Fixed::from_ratio(1, 2), Fixed::from_bits(1 << 15));
    }

    /// The rounding helper on its own, over the sign quadrants and the tie,
    /// because every constructor and both of the rounded operators go
    /// through one of these.
    #[test]
    fn the_rounding_helper_is_symmetric_and_rounds_ties_away_from_zero() {
        assert_eq!(round_div(7, 2), 4);
        assert_eq!(round_div(-7, 2), -4);
        assert_eq!(round_div(7, -2), -4);
        assert_eq!(round_div(-7, -2), 4);
        assert_eq!(round_div(5, 2), 3, "a tie rounds away from zero");
        assert_eq!(round_div(-5, 2), -3, "and symmetrically");
        assert_eq!(round_div(4, 2), 2, "an exact quotient is untouched");
    }

    /// Division saturates like everything else rather than wrapping.
    #[test]
    fn division_saturates_when_the_quotient_does_not_fit() {
        let before = saturations();
        assert_eq!(Fixed::MAX.saturating_div(Fixed::EPSILON), Fixed::MAX);
        assert_eq!(saturations().0, before.0 + 1);
    }
}
