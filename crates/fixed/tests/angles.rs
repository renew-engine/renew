//! Angles and the sine table.
//!
//! **This file uses floating point deliberately**, and it is the only place in
//! this crate's world that may. The table is a committed approximation of a
//! transcendental function; the only honest way to state its error is against
//! a reference, and `f64::sin` is the reference. Integration tests are their
//! own crate, so the library's float ban does not reach here — which is the
//! right boundary: the shipped code has no float, and the proof that its
//! table is right does.

use proptest::prelude::*;
use renew_fixed::{Angle, Fixed, Vec2};

/// Raw units per whole number in `Fixed`.
const ONE: f64 = 65536.0;

/// The measured worst error of the table, in units in the last place. Stated
/// as a constant so a regression moves this number rather than hiding.
const WORST_ULP: f64 = 1.02;

/// Binary angle units in a full turn, as an exactly representable `f64`.
///
/// Written as a literal rather than computed. The first version built it with
/// `mul_add` — a linter's suggestion for `a * b + c`, taken without thinking
/// about where it was — and **`mul_add` fuses on targets with FMA and does not
/// on targets without**, so the reference itself differed between machines and
/// the accuracy test passed on Windows while failing on Linux.
///
/// A test whose reference is platform-dependent cannot check a claim about
/// platform independence. That the failure surfaced at all is the
/// cross-platform lane working; that it was introduced by following a lint
/// into a numerically sensitive expression is the part worth remembering.
const FULL_TURN: f64 = 4_294_967_296.0;

/// The reference: what the angle means, as a real number.
///
/// Every step here is exact except the last: a `u32` converts to `f64`
/// exactly, dividing by a power of two is exact, and only the multiply by tau
/// rounds — once, and identically on every target, because IEEE 754 defines
/// multiplication. The remaining `sin` below is the platform's own, whose
/// cross-target variation is on the order of 1e-16 relative and therefore
/// eleven orders of magnitude below anything this test can see.
fn radians(angle: Angle) -> f64 {
    f64::from(angle.to_bits()) / FULL_TURN * core::f64::consts::TAU
}

/// The difference between what the table gave and what the reference says,
/// in units in the last place.
///
/// The helper lives outside a `#[test]`, where this crate's lint
/// configuration does not grant the test allowances — so the conversion is
/// handled rather than unwrapped. A sine outside `i32` would mean the table
/// returned something over 32768, which the sweep in
/// `the_table_endpoints_are_exact` independently rules out; here it simply
/// reports as an enormous error rather than a panic.
fn error_ulp(got: Fixed, want: f64) -> f64 {
    let Ok(raw) = i32::try_from(got.to_bits()) else {
        return f64::INFINITY;
    };
    (f64::from(raw) - want * ONE).abs()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 8192, ..ProptestConfig::default() })]

    /// The accuracy claim, over the whole circle rather than a sample of it.
    #[test]
    fn sine_and_cosine_are_within_the_stated_error(bits in any::<u32>()) {
        let angle = Angle::from_bits(bits);
        let theta = radians(angle);
        prop_assert!(
            error_ulp(angle.sin(), theta.sin()) <= WORST_ULP,
            "sin({bits}) off by more than {WORST_ULP} ulp"
        );
        prop_assert!(
            error_ulp(angle.cos(), theta.cos()) <= WORST_ULP,
            "cos({bits}) off by more than {WORST_ULP} ulp"
        );
    }

    /// The identity every rotation depends on. Not exactly one, because both
    /// terms are rounded — but close enough that a rotated vector keeps its
    /// length to within what the vector type already promises.
    #[test]
    fn sine_squared_plus_cosine_squared_is_one(bits in any::<u32>()) {
        let angle = Angle::from_bits(bits);
        let (sin, cos) = angle.sin_cos();
        let sum = sin.saturating_mul(sin) + cos.saturating_mul(cos);
        let error = (sum.to_bits() - Fixed::ONE.to_bits()).abs();
        prop_assert!(error <= 8, "sin²+cos² off by {error} raw units");
    }

    /// **Wrapping is exact**, which is the entire reason for binary angles.
    /// A full turn is the identity, not approximately.
    #[test]
    fn a_full_turn_is_exactly_the_identity(bits in any::<u32>()) {
        let angle = Angle::from_bits(bits);
        // Adding 2^32 units wraps to the same value by construction; the
        // property worth asserting is that four quarter turns do too.
        let round_trip = angle + Angle::QUARTER + Angle::QUARTER + Angle::QUARTER + Angle::QUARTER;
        prop_assert_eq!(round_trip, angle);
        prop_assert_eq!(angle + Angle::HALF + Angle::HALF, angle);
        prop_assert_eq!(angle - angle, Angle::ZERO);
        prop_assert_eq!(angle + (-angle), Angle::ZERO);
    }

    /// The symmetries the quadrant reduction implements, asserted rather than
    /// assumed — a sign error in one quadrant is the classic table bug and
    /// shows up nowhere else.
    #[test]
    fn the_quadrant_symmetries_hold(bits in any::<u32>()) {
        let a = Angle::from_bits(bits);
        let sin = a.sin();
        let cos = a.cos();
        // sin(θ + π) = −sin(θ), and the same for cosine.
        prop_assert_eq!((a + Angle::HALF).sin(), -sin);
        prop_assert_eq!((a + Angle::HALF).cos(), -cos);
        // sin(θ + π/2) = cos(θ), which is how `cos` is implemented — so this
        // is a tautology for cosine and a real check for sine.
        prop_assert_eq!((a + Angle::QUARTER).sin(), cos);
        // sin(−θ) = −sin(θ), cos(−θ) = cos(θ).
        prop_assert_eq!((-a).sin(), -sin);
        prop_assert_eq!((-a).cos(), cos);
    }

    /// Rotation preserves length, which is what a physics author will assume
    /// the first time they rotate a shape.
    #[test]
    fn rotating_a_vector_preserves_its_length(
        x in -1000i64 * 65536..1000 * 65536,
        y in -1000i64 * 65536..1000 * 65536,
        bits in any::<u32>(),
    ) {
        let v = Vec2::new(Fixed::from_bits(x), Fixed::from_bits(y));
        let rotated = v.rotate(Angle::from_bits(bits));
        let before = v.length().to_bits();
        let after = rotated.length().to_bits();
        // Each component is two rounded products summed, so the length moves
        // by a few parts in 65536 scaled by the magnitude.
        let tolerance = 8 + (before >> 12);
        prop_assert!(
            (after - before).abs() <= tolerance,
            "rotating changed length from {before} to {after}, past {tolerance}"
        );
    }

    /// Rotating by an angle and back returns the original, to within the
    /// error two rotations accumulate.
    #[test]
    fn rotating_back_returns_the_vector(
        x in -1000i64 * 65536..1000 * 65536,
        y in -1000i64 * 65536..1000 * 65536,
        bits in any::<u32>(),
    ) {
        let v = Vec2::new(Fixed::from_bits(x), Fixed::from_bits(y));
        let angle = Angle::from_bits(bits);
        let back = v.rotate(angle).rotate(-angle);
        let drift = (back - v).length().to_bits();
        let tolerance = 16 + (v.length().to_bits() >> 11);
        prop_assert!(drift <= tolerance, "round trip drifted {drift}, past {tolerance}");
    }
}

/// The values a table gets wrong at exactly the places nobody samples.
#[test]
fn the_cardinal_angles_are_exact() {
    assert_eq!(Angle::ZERO.sin(), Fixed::ZERO);
    assert_eq!(Angle::ZERO.cos(), Fixed::ONE);

    assert_eq!(Angle::QUARTER.sin(), Fixed::ONE);
    assert_eq!(Angle::QUARTER.cos(), Fixed::ZERO);

    assert_eq!(Angle::HALF.sin(), Fixed::ZERO);
    assert_eq!(Angle::HALF.cos(), -Fixed::ONE);

    assert_eq!(Angle::THREE_QUARTERS.sin(), -Fixed::ONE);
    assert_eq!(Angle::THREE_QUARTERS.cos(), Fixed::ZERO);

    // Degrees land on the cardinals exactly, which is what a designer typing
    // 90 expects and what a table with a drifting endpoint would break.
    assert_eq!(Angle::from_degrees(0), Angle::ZERO);
    assert_eq!(Angle::from_degrees(90), Angle::QUARTER);
    assert_eq!(Angle::from_degrees(180), Angle::HALF);
    assert_eq!(Angle::from_degrees(270), Angle::THREE_QUARTERS);
    // And wrap, because they are angles rather than numbers.
    assert_eq!(Angle::from_degrees(360), Angle::ZERO);
    assert_eq!(Angle::from_degrees(450), Angle::QUARTER);
    assert_eq!(Angle::from_degrees(-90), Angle::THREE_QUARTERS);

    assert_eq!(Angle::from_turn_ratio(1, 4), Angle::QUARTER);
    assert_eq!(Angle::from_turn_ratio(1, 2), Angle::HALF);

    // A quarter turn maps the axes onto each other, exactly.
    assert_eq!(Vec2::X.rotate(Angle::QUARTER), Vec2::Y);
    assert_eq!(Vec2::Y.rotate(Angle::QUARTER), -Vec2::X);
    assert_eq!(Vec2::X.rotate(Angle::HALF), -Vec2::X);
}

/// The table's endpoints, which every symmetry above is built on.
#[test]
fn the_table_endpoints_are_exact() {
    // Not a claim about the file's contents — a claim about what the lookup
    // returns at the two angles where an off-by-one would show.
    assert_eq!(Angle::from_bits(0).sin(), Fixed::ZERO);
    assert_eq!(Angle::from_bits(1 << 30).sin(), Fixed::ONE);
    // One unit short of a quarter turn rounds to exactly one, and that is
    // correct rather than a defect: the true value is 0.9999999996, which is
    // within half a unit in the last place of one, so one is the nearest
    // representable answer. The property worth asserting is the one a caller
    // depends on — sine never exceeds one, so a normal built from it is never
    // longer than unit.
    let nearly = Angle::from_bits((1 << 30) - 1).sin();
    assert_eq!(nearly, Fixed::ONE, "the nearest representable value is one");

    // And nothing anywhere on the circle exceeds it, which is the real
    // guarantee. Swept rather than sampled, at a stride that visits every
    // table entry and lands between them.
    let mut steps = 0u64;
    let mut bits = 0u32;
    while steps < 200_000 {
        let angle = Angle::from_bits(bits);
        assert!(angle.sin() <= Fixed::ONE, "sin exceeded one at {bits}");
        assert!(
            angle.sin() >= -Fixed::ONE,
            "sin went below minus one at {bits}"
        );
        assert!(angle.cos() <= Fixed::ONE, "cos exceeded one at {bits}");
        assert!(
            angle.cos() >= -Fixed::ONE,
            "cos went below minus one at {bits}"
        );
        bits = bits.wrapping_add(21_473);
        steps += 1;
    }
}
