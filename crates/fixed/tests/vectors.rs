//! Vector properties.
//!
//! The interesting ones are the two the physics contract depends on and that
//! are easy to get subtly wrong: `slide_along` removing exactly the normal
//! component, and `normalize` producing something close enough to unit length
//! that a contact normal is usable.

use proptest::prelude::*;
use renew_fixed::{Fixed, Vec2, Vec3};

/// Components modest enough that squaring cannot saturate, so a property
/// about geometry is not silently a property about the clamp. Squarable range
/// is about 1.2e7 units; a few thousand is far inside it.
///
/// **Spanning magnitudes, and that is the whole point.** The first version of
/// this generator drew uniformly from the full range, which meant it produced
/// short vectors essentially never — and short vectors were where `normalize`
/// was catastrophically wrong: a direction of (181, 313) raw came back as a
/// "unit" vector forty-one per cent too long, and every property here passed.
/// A generator that cannot reach the broken region is a generator that
/// certifies the bug.
fn coordinate() -> impl Strategy<Value = Fixed> {
    prop_oneof![
        // Tiny: below the point where a squared component used to round to
        // zero. This is the band that was broken.
        3 => (-300i64..300).prop_map(Fixed::from_bits),
        // Sub-unit, where a normal from two nearly-coincident points lands.
        3 => (-65536i64..65536).prop_map(Fixed::from_bits),
        // Ordinary game-scale coordinates.
        2 => (-4096i64 * 65536..4096i64 * 65536).prop_map(Fixed::from_bits),
    ]
}

fn vec2() -> impl Strategy<Value = Vec2> {
    (coordinate(), coordinate()).prop_map(|(x, y)| Vec2::new(x, y))
}

fn vec3() -> impl Strategy<Value = Vec3> {
    (coordinate(), coordinate(), coordinate()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    #[test]
    fn addition_commutes_and_subtraction_inverts_it(a in vec2(), b in vec2()) {
        prop_assert_eq!(a + b, b + a);
        prop_assert_eq!((a + b) - b, a);
        prop_assert_eq!(a - a, Vec2::ZERO);
        prop_assert_eq!(a + Vec2::ZERO, a);
    }

    #[test]
    fn negation_is_its_own_inverse(a in vec2()) {
        prop_assert_eq!(-(-a), a);
        prop_assert_eq!(a + (-a), Vec2::ZERO);
    }

    #[test]
    fn the_dot_product_commutes(a in vec2(), b in vec2()) {
        prop_assert_eq!(a.dot(b), b.dot(a));
    }

    /// The 2D cross product is antisymmetric, and zero for parallel vectors —
    /// which is the property a winding or collinearity test reads.
    #[test]
    fn the_cross_product_is_antisymmetric(a in vec2(), b in vec2()) {
        prop_assert_eq!(a.cross(b), -(b.cross(a)));
        prop_assert_eq!(a.cross(a), Fixed::ZERO);
    }

    /// A 3D cross product is perpendicular to both its inputs. Exactly zero
    /// is too strong under rounding, so the assertion is that the dot is
    /// small relative to the magnitudes involved.
    #[test]
    fn the_3d_cross_product_is_perpendicular_to_its_inputs(a in vec3(), b in vec3()) {
        let c = a.cross(b);
        // Each component of the cross rounds once, so the dot accumulates a
        // few units in the last place scaled by the input magnitudes.
        let scale = a.length().to_bits().max(b.length().to_bits()) >> 16;
        let tolerance = 8 * (scale + 1);
        prop_assert!(c.dot(a).to_bits().abs() <= tolerance, "not perpendicular to a");
        prop_assert!(c.dot(b).to_bits().abs() <= tolerance, "not perpendicular to b");
    }

    #[test]
    fn length_squared_agrees_with_the_dot_product(a in vec2()) {
        prop_assert_eq!(a.length_squared(), a.dot(a));
        // And the length is the floored root of it, which is the scalar's
        // property lifted.
        prop_assert_eq!(a.length(), a.length_squared().sqrt());
    }

    #[test]
    fn distance_is_symmetric_and_zero_only_for_the_same_point(a in vec2(), b in vec2()) {
        prop_assert_eq!(a.distance(b), b.distance(a));
        prop_assert_eq!(a.distance(a), Fixed::ZERO);
    }

    /// **The property a contact normal depends on.** Normalising cannot give
    /// exactly unit length in a rounded type, so the contract promises "unit
    /// to the type's resolution" — this is what that means numerically.
    #[test]
    fn normalize_produces_something_close_to_unit_length(a in vec2()) {
        let Some(unit) = a.normalize() else {
            prop_assert_eq!(a, Vec2::ZERO, "only the zero vector has no direction");
            return Ok(());
        };
        let length = unit.length();
        let error = (length.to_bits() - Fixed::ONE.to_bits()).abs();
        // Each component is divided once and the length takes a floored
        // root, so a few hundred units in the last place — out of 65536 —
        // is the honest bound. That is about half a percent, which is why
        // the contract tells callers to compare squared lengths against a
        // tolerance rather than expecting exactly one.
        // Four raw units out of 65536 — six thousandths of a per cent.
        // This was 512 while `normalize` divided a short vector by its own
        // rounded-to-zero length; rescaling first earned the tighter bound,
        // and the bound is what would catch a regression to the old way.
        prop_assert!(
            error <= 4,
            "normalized {a:?} to length {length:?}, off by {error} raw units"
        );
    }

    /// **The regression this file exists to prevent from returning.** Every
    /// non-zero direction has a direction, however short — and pushing
    /// straight into the normal derived from it must move nothing at all.
    /// Before `normalize` rescaled, a direction shorter than 256 raw units
    /// had no length at all, and one of 41 raw units came back as a normal
    /// that let a body slide 1.4 units per step through the wall it was
    /// pressed against.
    #[test]
    fn even_the_shortest_directions_normalise_and_stop_a_slide(
        x in -400i64..400,
        y in -400i64..400,
    ) {
        prop_assume!(x != 0 || y != 0);
        let direction = Vec2::new(Fixed::from_bits(x), Fixed::from_bits(y));
        let normal = direction.normalize();
        prop_assert!(normal.is_some(), "{direction:?} has a direction and got none");
        let normal = normal.expect("checked above");

        let error = (normal.length().to_bits() - Fixed::ONE.to_bits()).abs();
        prop_assert!(error <= 4, "short direction gave a normal off by {error}");

        // Pressed straight into it, a body must not move.
        let push = -normal;
        prop_assert_eq!(push.slide_along(normal).length(), Fixed::ZERO);
    }

    #[test]
    fn the_zero_vector_has_no_direction(_ in 0u8..1) {
        prop_assert_eq!(Vec2::ZERO.normalize(), None);
        prop_assert_eq!(Vec3::ZERO.normalize(), None);
    }

    /// **The property move-and-slide depends on.** After sliding, nothing of
    /// the displacement remains along the normal — which is what stops a
    /// character creeping into a wall it is pressed against.
    #[test]
    fn sliding_removes_the_component_along_the_normal(
        displacement in vec2(),
        normal_source in vec2(),
    ) {
        let Some(normal) = normal_source.normalize() else {
            return Ok(());
        };
        let slid = displacement.slide_along(normal);
        let residual = slid.dot(normal);
        // The normal is itself only unit to the type's resolution, so the
        // residual is bounded rather than zero. Scaled by the displacement,
        // because a larger slide carries proportionally more rounding.
        let magnitude = displacement.length().to_bits() >> 16;
        let tolerance = 64 * (magnitude + 1);
        prop_assert!(
            residual.to_bits().abs() <= tolerance,
            "residual {residual:?} along the normal exceeds {tolerance}"
        );
    }

    /// Interpolation is exact at both ends, which is the reason for the
    /// `a + (b - a) * t` form.
    #[test]
    fn interpolation_is_exact_at_the_endpoints(a in vec2(), b in vec2()) {
        prop_assert_eq!(a.lerp(b, Fixed::ZERO), a);
        prop_assert_eq!(a.lerp(b, Fixed::ONE), b);
        // And `t` outside the range is clamped rather than extrapolated.
        prop_assert_eq!(a.lerp(b, Fixed::from_int(5)), b);
        prop_assert_eq!(a.lerp(b, Fixed::from_int(-5)), a);
    }

    /// A quarter turn is exact — no trigonometry, no rounding — which is why
    /// it is available when general rotation is not.
    #[test]
    fn the_perpendicular_is_exact_and_four_turns_return(a in vec2()) {
        prop_assert_eq!(a.perpendicular().dot(a), Fixed::ZERO, "exactly perpendicular");
        prop_assert_eq!(
            a.perpendicular().perpendicular().perpendicular().perpendicular(),
            a
        );
        prop_assert_eq!(a.perpendicular().length_squared(), a.length_squared());
    }

    #[test]
    fn scaling_by_one_and_zero_behaves(a in vec2()) {
        prop_assert_eq!(a * Fixed::ONE, a);
        prop_assert_eq!(a * Fixed::ZERO, Vec2::ZERO);
        prop_assert_eq!(a * Fixed::from_int(-1), -a);
    }

    #[test]
    fn the_3d_operations_mirror_the_2d_ones(a in vec3(), b in vec3()) {
        prop_assert_eq!(a + b, b + a);
        prop_assert_eq!((a + b) - b, a);
        prop_assert_eq!(-(-a), a);
        prop_assert_eq!(a.dot(b), b.dot(a));
        prop_assert_eq!(a.length_squared(), a.dot(a));
        prop_assert_eq!(a.distance(b), b.distance(a));
        prop_assert_eq!(a.lerp(b, Fixed::ZERO), a);
        prop_assert_eq!(a.lerp(b, Fixed::ONE), b);
        // Interior interpolation, not only the endpoints: a lerp that
        // returned an endpoint everywhere would satisfy the two above.
        let midpoint = a.lerp(b, Fixed::from_ratio(1, 2));
        prop_assert!(
            midpoint.distance(a).to_bits() <= a.distance(b).to_bits() + 2
                && midpoint.distance(b).to_bits() <= a.distance(b).to_bits() + 2,
            "the midpoint left the segment"
        );

        // Normalising and sliding, the two operations the 2D properties
        // cover and this one reached only through its endpoints.
        if let Some(unit) = a.normalize() {
            let error = (unit.length().to_bits() - Fixed::ONE.to_bits()).abs();
            prop_assert!(error <= 4, "3D normalize off by {error} raw units");

            let slid = b.slide_along(unit);
            let magnitude = b.length().to_bits() >> 16;
            let tolerance = 64 * (magnitude + 1);
            prop_assert!(
                slid.dot(unit).to_bits().abs() <= tolerance,
                "3D slide left a residual along the normal"
            );
        } else {
            prop_assert_eq!(a, Vec3::ZERO);
        }
    }
}

/// The exact values a generator reaches only by luck.
#[test]
fn the_named_cases_are_exact() {
    let three_four = Vec2::from_ints(3, 4);
    assert_eq!(
        three_four.length(),
        Fixed::from_int(5),
        "the 3-4-5 triangle"
    );
    assert_eq!(three_four.length_squared(), Fixed::from_int(25));

    assert_eq!(Vec2::X.dot(Vec2::Y), Fixed::ZERO, "the axes are orthogonal");
    assert_eq!(Vec2::X.cross(Vec2::Y), Fixed::ONE, "and counter-clockwise");
    assert_eq!(Vec2::X.perpendicular(), Vec2::Y);

    assert_eq!(
        Vec2::X.normalize(),
        Some(Vec2::X),
        "an axis is already unit"
    );

    // Sliding a displacement straight into a wall moves nothing: the
    // property that stops a character drifting through it over a minute of
    // leaning. Exact here because the normal is an axis.
    let into_the_wall = Vec2::from_ints(-5, 0);
    assert_eq!(into_the_wall.slide_along(Vec2::X), Vec2::ZERO);

    // And a diagonal against the same wall keeps exactly its tangent.
    let diagonal = Vec2::from_ints(-5, 3);
    assert_eq!(diagonal.slide_along(Vec2::X), Vec2::from_ints(0, 3));

    // 3D: the standard basis cross product, exactly.
    let x = Vec3::from_ints(1, 0, 0);
    let y = Vec3::from_ints(0, 1, 0);
    assert_eq!(x.cross(y), Vec3::from_ints(0, 0, 1));
    assert_eq!(y.cross(x), Vec3::from_ints(0, 0, -1));
}
