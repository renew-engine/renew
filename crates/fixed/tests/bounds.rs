//! The bounds the geometry vocabulary quotes, asserted rather than argued.
//!
//! Every number in here was, at some point, written down after being measured
//! at a single point and then stated as if it were a property. Six of six such
//! figures turned out to be wrong — each derived correctly for an operation
//! that was not the one being described. The cure is not more care while
//! writing prose: it is that a figure quoted anywhere has an assertion here
//! that fails when it stops being true.
//!
//! Each test is named for the claim it decides. A claim with no test here is
//! a claim nobody has checked.

use renew_fixed::{Fixed, Vec2, Vec3};

/// Raw units per whole unit.
const ONE: i64 = 65536;

/// The largest raw component for which the *narrow* squared length is exact,
/// found by bisection so the answer comes from the arithmetic rather than
/// from an algebraic step that could be wrong the same way the prose was.
fn narrow_component_bound(dimension: i128) -> i128 {
    let (mut lo, mut hi) = (0i128, i128::from(i64::MAX));
    while lo < hi {
        let mid = lo + (hi - lo).div_euclid(2) + 1;
        let square = (mid * mid) >> 16;
        if square * dimension <= i128::from(i64::MAX) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// There are two of these figures and they differ by a factor of two: the
/// bound on a vector handed straight in, and the bound on coordinates that
/// will be subtracted to make one. Quoting the second as the first is the
/// easiest mistake to make here, and the one most often made.
#[test]
fn the_narrow_squared_length_bound_is_stated_for_the_right_operand() {
    let two_d = narrow_component_bound(2);
    let three_d = narrow_component_bound(3);
    let one = i128::from(ONE);

    // On a vector passed straight in, in whole units.
    assert_eq!(two_d / one, 8_388_607, "2D component bound, in units");
    assert_eq!(three_d / one, 6_849_269, "3D component bound, in units");

    // On coordinates that will be subtracted first: half, because the
    // difference of two coordinates each bounded by E is bounded by 2E.
    // These two are the figures previously quoted as if they were the line
    // above, which is the whole reason this test exists.
    assert_eq!(two_d / one / 2, 4_194_303, "2D coordinate bound, in units");
    assert_eq!(
        three_d / one / 2,
        3_424_634,
        "3D coordinate bound, in units"
    );

    // And the claim underneath all four, measured rather than derived: at the
    // bound the narrow path agrees with the wide one, and one raw unit past
    // it, it does not.
    let at = Fixed::from_bits(i64::try_from(two_d).unwrap_or(i64::MAX));
    let past = Fixed::from_bits(i64::try_from(two_d + 1).unwrap_or(i64::MAX));
    assert_eq!(
        Vec2::new(at, at).length_squared_wide().checked_narrow(),
        Some(Vec2::new(at, at).length_squared()),
        "at the bound the two paths agree"
    );
    assert_ne!(
        Vec2::new(past, past).length_squared_wide().checked_narrow(),
        Some(Vec2::new(past, past).length_squared()),
        "one raw unit past the bound the narrow path loses"
    );

    // The 3D bound binds where the 2D one does not, which is why they are
    // quoted separately.
    let between = Fixed::from_bits(i64::try_from(three_d + 1).unwrap_or(i64::MAX));
    assert_ne!(
        Vec3::new(between, between, between)
            .length_squared_wide()
            .checked_narrow(),
        Some(Vec3::new(between, between, between).length_squared()),
        "past the 3D bound the narrow path loses"
    );
}

/// A single wide product cannot overflow; a *sum* of them can. Stated as "no
/// world a coordinate can express can overflow a distance test", the claim is
/// false — a distance test forms a sum.
#[test]
fn a_wide_product_cannot_overflow_but_a_sum_of_them_can() {
    let extreme = Fixed::MAX.wide_mul(Fixed::MAX);
    assert!(extreme.to_bits() > 0, "a single wide product survives");

    let before = renew_fixed::saturations();
    let _ = extreme + extreme + extreme;
    assert!(
        renew_fixed::saturations().0 > before.0,
        "three extreme products summed must report saturating"
    );
}

/// The world bound is set by the *subtraction*, not by the wide sum — so
/// quoting a wide-path figure as the world bound describes a constraint that
/// is not the binding one.
#[test]
fn the_world_bound_is_set_by_the_coordinate_subtraction() {
    // Largest raw difference d with 3·d² inside an i128.
    let (mut lo, mut hi) = (0i128, i128::MAX >> 1);
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        let fits = mid
            .checked_mul(mid)
            .and_then(|square| square.checked_mul(3))
            .is_some();
        if fits {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    // The subtraction bound: two coordinates of E must differ without
    // saturating, so E is half the range.
    let subtraction_bound = i128::from(Fixed::MAX.to_bits() / 2);
    let half = Fixed::from_bits(Fixed::MAX.to_bits() / 2);
    assert!(
        half.checked_add(half).is_some(),
        "half the range doubles without saturating"
    );
    assert_eq!(
        subtraction_bound / i128::from(ONE),
        70_368_744_177_663,
        "coordinate bound in whole units, set by the subtraction"
    );

    // And the point: the subtraction is the *binding* one. The wide sum
    // permits a larger difference than the subtraction can produce, so no
    // further tightening is needed to keep three squared differences inside
    // the wide type — the note claiming a tightened figure was describing a
    // constraint that does not bind.
    assert!(
        lo > subtraction_bound,
        "wide-sum bound {lo} should exceed the subtraction bound {subtraction_bound}"
    );

    // Stated concretely, since the margin is what makes the tightening
    // unnecessary rather than merely unproven.
    let worst = subtraction_bound * subtraction_bound * 3;
    assert!(
        worst > 0 && worst < i128::MAX,
        "three squared maximum differences stay inside the wide type"
    );
}

/// The slide residual scales with the displacement, and the slope a
/// *vocabulary* may quote is the one derived from the worst normal it permits
/// — not the one this number type happens to achieve.
///
/// Conflating those is how a quoted slope came to be four times tighter than
/// any conforming implementation could meet.
#[test]
fn the_slide_residual_scales_with_the_displacement() {
    // Derived, not fitted: a normal whose length is off by e parts in 65536
    // has a squared length off by about 2e, so a push of length L entirely
    // into it leaves about L·2e/65536. The permitted e is 4, giving L/8192.
    let permitted_divisor = ONE / (2 * 4);
    assert_eq!(permitted_divisor, 8192, "permitted slope divisor");

    for magnitude in [1i64, 10, 100, 1_000, 10_000] {
        let permitted = 2 + (magnitude * ONE) / permitted_divisor;
        let mut worst = 0i64;
        for (dx, dy) in [(3i32, 4i32), (1, 1), (7, 2), (1, 100), (99, 1), (5, 12)] {
            let normal = Vec2::new(Fixed::from_int(dx), Fixed::from_int(dy))
                .normalize()
                .expect("non-zero direction");
            let push = Vec2::new(
                Fixed::from_bits(normal.x.to_bits() * magnitude),
                Fixed::from_bits(normal.y.to_bits() * magnitude),
            );
            let residual = push.slide_along(normal);
            worst = worst.max(residual.x.to_bits().abs());
            worst = worst.max(residual.y.to_bits().abs());
        }
        assert!(
            worst <= permitted,
            "at magnitude {magnitude} the residual was {worst}, past the permitted {permitted}"
        );
    }
}

/// The timestep conversion is lossy, and the loss is part of the vocabulary
/// because two implementations rounding it differently diverge on step one.
#[test]
fn the_sixty_hertz_timestep_converts_with_a_stated_error() {
    let nanos: i128 = 16_666_667;
    let billion = 1_000_000_000i128;

    // Round to nearest, ties away from zero — the number type's own rule.
    let exact = nanos * i128::from(ONE);
    let raw = (exact + billion / 2) / billion;
    assert_eq!(raw, 1092, "60 Hz as a Q47.16 count of seconds");
    let dt = Fixed::from_bits(i64::try_from(raw).unwrap_or(i64::MAX));
    assert_eq!(dt.to_bits(), 1092);

    let shortfall = exact - raw * billion;
    assert_eq!(
        shortfall * 1_000_000 / exact,
        244,
        "conversion error in parts per million"
    );

    // What that costs a body moving at ten units per second over a minute of
    // wall time — 3600 ticks. Worth quoting because it is the figure a reader
    // can feel, and because it is six times smaller than the estimate that
    // prompted this test.
    let drift_raw = shortfall * 3600 * 10 / billion;
    assert_eq!(
        drift_raw * 1000 / i128::from(ONE),
        146,
        "drift in thousandths of a unit per minute at ten units per second"
    );
}
