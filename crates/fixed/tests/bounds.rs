//! The bounds the geometry vocabulary quotes, asserted rather than argued.
//!
//! Every number in here was, at some point, written down after being measured
//! at a single point and then stated as if it were a property. **Eleven such
//! figures have turned out to be wrong**, each derived correctly for an
//! operation that was not the one being described. The cure is not more care
//! while writing prose: it is that a figure quoted anywhere has an assertion
//! here that fails when it stops being true.
//!
//! Each test is named for the claim it decides. A claim with no test here is
//! a claim nobody has checked.
//!
//! **And one of the eleven was wrong in this file**, which is the honest limit
//! of the technique: the world-bound assertion re-derived the algebra behind
//! the bound and reproduced the algebra's own mistake, because the same hand
//! wrote the claim and the check. An assertion about a bound should *exercise*
//! the bound — require the arithmetic to fail one side of it and hold at it —
//! rather than recompute the reasoning that produced it.

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

/// The world bound differs by dimension, and the operand is the coordinate
/// *difference* rather than the coordinate.
///
/// **The first version of this test squared E where the operation squares 2E,
/// while its own failure message said "three squared maximum differences".**
/// It therefore asserted a quantity four times too small and could not fail
/// when the prose it was written to guard was wrong — which it was. That is
/// the same operand confusion the whole file exists to catch, reproduced
/// inside the catcher, because the same hand wrote the claim and the check.
///
/// The lesson is in the shape of what follows: an assertion about a bound
/// must exercise the bound, not recompute the algebra that produced it.
/// Below, the figure is decided by making the arithmetic saturate one unit
/// past it and not saturate at it.
#[test]
fn the_world_bound_differs_by_dimension_and_is_measured_at_the_bound() {
    // Largest coordinate E whose *difference* 2E survives `dimension`
    // squared terms in the wide type.
    let largest_safe = |dimension: i128| {
        let (mut lo, mut hi) = (0i128, i128::from(i64::MAX) / 2);
        while lo < hi {
            let mid = lo + (hi - lo).div_euclid(2) + 1;
            let fits = (2 * mid)
                .checked_mul(2 * mid)
                .and_then(|square| square.checked_mul(dimension))
                .is_some();
            if fits {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    };

    let two_d = largest_safe(2);
    let three_d = largest_safe(3);
    let one = i128::from(ONE);

    // In 2D the wide sum and the subtraction coincide exactly: the
    // subtraction caps a coordinate at half the range, and that is also the
    // largest coordinate whose doubled difference squares twice inside an
    // i128. The margin is 4.3 × 10⁻¹⁹ of the range — it fits by a hair.
    assert_eq!(
        two_d,
        i128::from(Fixed::MAX.to_bits() / 2),
        "in 2D the subtraction bound and the wide bound are the same figure"
    );
    assert_eq!(two_d / one, 70_368_744_177_663, "2D world bound, in units");

    // In 3D the wide sum binds well below the subtraction — 18% lower — so
    // a world built to the 2D figure saturates on an ordinary distance test.
    assert_eq!(
        three_d / one,
        57_455_839_025_240,
        "3D world bound, in units"
    );
    assert!(
        three_d < two_d,
        "the 3D bound must be the tighter one, or there is nothing to state"
    );

    // Decided by exercising it, not by re-deriving it. At the 3D bound the
    // mandated wide path is quiet; at the 2D figure — which the vocabulary
    // quoted as the world bound for both dimensions — it saturates.
    let at = Fixed::from_bits(i64::try_from(three_d).unwrap_or(i64::MAX));
    let past = Fixed::from_bits(i64::try_from(two_d).unwrap_or(i64::MAX));

    let difference = |e: Fixed| e - Fixed::from_bits(-e.to_bits());

    let before = renew_fixed::saturations();
    let quiet = difference(at);
    let _ = Vec3::new(quiet, quiet, quiet).length_squared_wide();
    assert_eq!(
        renew_fixed::saturations(),
        before,
        "at the 3D bound the mandated wide path must not saturate"
    );

    let loud = difference(past);
    let _ = Vec3::new(loud, loud, loud).length_squared_wide();
    assert!(
        renew_fixed::saturations().0 > before.0,
        "at the 2D figure a 3D distance must saturate — that is why the \
         bound is stated per dimension"
    );

    // And the 2D figure is genuinely safe in 2D, which is what makes it a
    // bound rather than a mistake.
    let steady = renew_fixed::saturations();
    let _ = Vec2::new(loud, loud).length_squared_wide();
    assert_eq!(
        renew_fixed::saturations(),
        steady,
        "the 2D bound must be quiet in 2D"
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

        // **And in three dimensions**, which the vocabulary says inherits this
        // property unchanged while the assertion covered only `Vec2`. It is a
        // different code path — three products summed rather than two — and
        // the constant term is a per-component rounding allowance, so a third
        // component is exactly where a slope that was fitted rather than
        // derived would start to fail.
        let mut worst_3d = 0i64;
        for (dx, dy, dz) in [
            (1i32, 2i32, 2i32),
            (2, 3, 6),
            (1, 1, 1),
            (7, 4, 4),
            (1, 1, 100),
            (99, 1, 1),
        ] {
            let normal = Vec3::new(
                Fixed::from_int(dx),
                Fixed::from_int(dy),
                Fixed::from_int(dz),
            )
            .normalize()
            .expect("non-zero direction");
            let push = Vec3::new(
                Fixed::from_bits(normal.x.to_bits() * magnitude),
                Fixed::from_bits(normal.y.to_bits() * magnitude),
                Fixed::from_bits(normal.z.to_bits() * magnitude),
            );
            let residual = push.slide_along(normal);
            worst_3d = worst_3d.max(residual.x.to_bits().abs());
            worst_3d = worst_3d.max(residual.y.to_bits().abs());
            worst_3d = worst_3d.max(residual.z.to_bits().abs());
        }
        assert!(
            worst_3d <= permitted,
            "at magnitude {magnitude} the 3D residual was {worst_3d}, past {permitted}"
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

/// Which tick rates are exact, which is a stricter question than it looks and
/// has a different answer in each representation.
///
/// A rate exact in whole nanoseconds need not be exact in Q47.16 seconds:
/// 125 Hz divides 10⁹ evenly and does *not* divide 65536, landing on 524.288
/// raw. Quoting it as an exact alternative — which is easy to do, because the
/// loop's own timestep documentation quotes it correctly for the nanosecond
/// question — offers a caller an escape from rounding that is not there.
///
/// End-to-end exactness needs both, so the rates are the powers of two.
#[test]
fn a_rate_exact_in_nanoseconds_need_not_be_exact_in_the_number_type() {
    let billion = 1_000_000_000i128;
    let one = i128::from(ONE);

    let exact_in_nanos = |hz: i128| billion % hz == 0;
    let exact_in_fixed = |hz: i128| one % hz == 0;

    // The trap, stated as an assertion so it cannot be quoted away again.
    assert!(exact_in_nanos(125), "125 Hz divides a second evenly");
    assert!(
        !exact_in_fixed(125),
        "and does not divide 65536, so it is not exact end to end"
    );
    assert_eq!(one * 1000 / 125, 524_288, "125 Hz is 524.288 raw");

    // The rates that are exact in both are the powers of two, and the list
    // has a top: a second is 2⁹·5⁹ nanoseconds, so nine factors of two is
    // all there are, even though the number type would carry sixteen.
    for hz in [1i128, 2, 4, 8, 16, 32, 64, 128, 256, 512] {
        assert!(
            exact_in_nanos(hz) && exact_in_fixed(hz),
            "{hz} Hz should be exact in both representations"
        );
    }
    assert_eq!(one / 64, 1024, "64 Hz in raw units");
    assert_eq!(one / 512, 128, "512 Hz in raw units");

    assert!(exact_in_fixed(1024), "1024 divides 65536");
    assert!(
        !exact_in_nanos(1024),
        "but not a second, so the exact rates stop at 512 Hz"
    );
}

/// A timestep is exact in the number type exactly when its nanosecond count
/// is a multiple of 1 953 125, which is the condition in the units a caller
/// actually sets — a timestep is a nanosecond count, not a rate.
///
/// The derivation, checked below rather than trusted: raw = ns · 2¹⁶ / 10⁹,
/// and 10⁹ = 2⁹ · 5⁹, so raw = ns · 2⁷ / 5⁹. Since 2⁷ and 5⁹ are coprime, that
/// is an integer exactly when 5⁹ = 1 953 125 divides ns.
#[test]
fn a_timestep_is_exact_exactly_when_its_nanoseconds_divide_by_the_fifth_power() {
    let billion = 1_000_000_000i128;
    let one = i128::from(ONE);
    let condition = 1_953_125i128;

    // 5⁹ · 2⁹ = 10⁹, which is the factorisation the condition comes from.
    assert_eq!(condition * 512, billion, "5^9 times 2^9 is a second");

    let exact = |nanos: i128| nanos * one % billion == 0;

    // The condition decides it, in both directions, across the range a game
    // would ever use — including rates that are exact in nanoseconds and not
    // in the number type, which is the trap this replaces.
    for hz in [1i128, 24, 30, 50, 60, 64, 100, 120, 125, 128, 240, 256, 512] {
        if billion % hz != 0 {
            continue; // not representable as whole nanoseconds at all
        }
        let nanos = billion / hz;
        assert_eq!(
            exact(nanos),
            nanos % condition == 0,
            "{hz} Hz ({nanos} ns): the multiple-of-1953125 condition must decide exactness"
        );
    }

    // And the two the vocabulary names explicitly.
    assert_eq!(billion / 512, condition, "512 Hz is exactly one multiple");
    assert!(
        (billion / 125) % condition != 0,
        "125 Hz divides a second and is not exact in the number type"
    );
}

/// **Cutting a displacement into segments does not bound slide creep**, and
/// this test exists to stop the idea being re-invented.
///
/// The reasoning that produced it looked sound. The residual per slide
/// iteration is `2 + L/8192` raw — affine — so a long displacement creeps
/// more than a short one; cap the length of a piece and the creep per piece
/// is capped too. Both halves are true and the conclusion does not follow.
///
/// **The proportional term is scale-invariant under cutting.** `k` segments of
/// `L/k` contribute `k · (L/k)/8192 = L/8192` — exactly what one segment of
/// `L` contributes. Nothing is saved. And **the constant term gets strictly
/// worse**: each segment pays its own `2` raw per iteration, so more segments
/// means more creep, not less.
///
/// So creep is a function of the distance travelled, full stop. Bounding the
/// final clearance needs the clearance re-established *between* segments —
/// a depenetration step, which costs real work — or it needs the guarantee
/// stated as proportional to distance rather than as a constant. That is a
/// decision for an implementation that can measure it, not for prose.
#[test]
fn segmentation_does_not_bound_slide_creep() {
    // Total creep over a displacement cut into `segments` pieces, with `n`
    // slide iterations available in each.
    let creep = |displacement: i64, segments: i64, n: i64| {
        let piece = displacement / segments;
        segments * n * (2 + piece / 8192)
    };

    let displacement = 100 * ONE; // a hundred units

    // The proportional part is unchanged by cutting, and the total only rises.
    let whole = creep(displacement, 1, 4);
    for segments in [2i64, 4, 10, 50] {
        let cut = creep(displacement, segments, 4);
        assert!(
            cut >= whole,
            "cutting into {segments} pieces gave {cut} raw against {whole} for one —              segmentation must never reduce creep, or this test has the arithmetic wrong"
        );
    }
    assert!(
        creep(displacement, 50, 4) > creep(displacement, 1, 4),
        "more segments must cost more, because each pays the constant term again"
    );

    // And the cap that was written on the strength of the false conclusion
    // fails the property it was introduced to deliver, in every configuration
    // the vocabulary suggests. A body stops at a clearance of `skin`; the
    // property demands it end no closer than `skin - tolerance`; so the creep
    // budget is `tolerance`, and the cap budgeted `skin - tolerance` instead.
    let cap = |skin: i64, tolerance: i64, n: i64| 8192 * ((skin - tolerance) / n - 2);
    for &(skin, tolerance, n) in &[
        (65536i64, 1024i64, 4i64),
        (4096, 64, 8),
        (1024, 16, 4),
        (600, 8, 2),
    ] {
        let l_max = cap(skin, tolerance, n);
        let accumulated = n * (2 + l_max / 8192);
        let final_clearance = skin - accumulated;
        assert!(
            final_clearance < skin - tolerance,
            "the superseded cap must fail the property, or nothing was learned"
        );
        // What it actually delivers: the contact tolerance, not the stop-short.
        assert_eq!(
            final_clearance, tolerance,
            "skin {skin}, tolerance {tolerance}, {n} iterations"
        );
    }
}

/// **A length is never negative, and beyond the range it saturates.**
///
/// `Wide::checked_sqrt` cast a `u128` integer root straight to `i64`. A root
/// below 2^64 does not fit one, so squared lengths above about 2^126 produced
/// a *negative* distance — silently, from a function named `checked_`, and
/// reachable through `Vec2::length` at coordinates beyond the documented world
/// bounds. The crate's front page promises that overflow saturates in every
/// profile and is counted, never wraps; this is that promise applied to the
/// one path that was not keeping it.
///
/// **The assertion exercises the boundary rather than restating it**: it
/// requires the arithmetic to be finite and exact on the low side, to saturate
/// on the high side, and to record the saturation — so a change that silently
/// widened or narrowed the range fails here instead of passing.
#[test]
fn a_length_saturates_instead_of_going_negative() {
    // Below the threshold: exact, positive, and not saturated.
    let below = Fixed::from_bits(6_000_000_000_000_000_000);
    let before = renew_fixed::saturations();
    let length = Vec2::new(below, below).length();
    assert!(
        length.to_bits() > 0,
        "a length below the threshold came back {} raw",
        length.to_bits()
    );
    assert_eq!(
        renew_fixed::saturations().0,
        before.0,
        "a length well inside the range recorded a saturation"
    );

    // Above it: saturated, counted, and still not negative.
    for raw in [
        6_600_000_000_000_000_000i64,
        7_000_000_000_000_000_000,
        8_000_000_000_000_000_000,
        i64::MAX,
    ] {
        let component = Fixed::from_bits(raw);
        let before = renew_fixed::saturations();
        let length = Vec2::new(component, component).length();
        assert!(
            length.to_bits() > 0,
            "a component of {raw} raw gave a length of {} raw — a distance less than nothing",
            length.to_bits()
        );
        assert_eq!(
            length.to_bits(),
            i64::MAX,
            "a component of {raw} raw did not saturate"
        );
        assert!(
            renew_fixed::saturations().0 > before.0,
            "a component of {raw} raw saturated without recording it"
        );
    }
}
