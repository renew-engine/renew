//! Property-based tests. Two strictly separated tiers:
//! - **bit-exact** properties, compared via `to_bits`, holding to the
//!   last bit **on the sampled domain** stated below;
//! - **tolerance** properties, where floating-point rounding is inherent,
//!   compared against an epsilon and labeled as such.
//!
//! Sampled domain, stated once and binding on every property here:
//! finite components in `(-1e6, 1e6)`. The strategy never generates NaN,
//! infinities, signed zero, or subnormals — outside that domain some
//! bit-exact laws genuinely fail (IEEE signed-zero cancellation: exact
//! cancellations produce `+0.0`, so `-(b × a)` and `0 · y` terms can flip
//! a zero's sign). Edge-domain behavior is covered by targeted unit tests
//! in the crate, not by these laws.

use core::num::NonZeroU64;
use proptest::prelude::*;
use proptest::test_runner::RngSeed;

use renew_math::{Aabb3, Alpha, Mat4, Quat, Vec3};

/// Finite, moderately sized components: large enough to explore, small
/// enough that tolerance properties are well-conditioned. See the module
/// docs for what this strategy deliberately excludes.
fn component() -> impl Strategy<Value = f32> {
    -1.0e6_f32..1.0e6_f32
}

fn vec3() -> impl Strategy<Value = Vec3> {
    (component(), component(), component()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

fn unit_vec3() -> impl Strategy<Value = Vec3> {
    vec3().prop_filter_map("needs a direction", Vec3::try_normalize)
}

fn bits(v: Vec3) -> [u32; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

proptest! {
    // Fixed RNG seed: the suite explores the same inputs on every run
    // and every machine, so a property failure anywhere reproduces
    // everywhere. Fresh exploration is a deliberate act (change the
    // seed), never an ambient one.
    #![proptest_config(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x00D1_A600),
        ..ProptestConfig::default()
    })]

    // ---- bit-exact tier ----------------------------------------------

    #[test]
    fn addition_commutes_bitwise(a in vec3(), b in vec3()) {
        prop_assert_eq!(bits(a + b), bits(b + a));
    }

    #[test]
    fn dot_products_commute_bitwise(a in vec3(), b in vec3()) {
        prop_assert_eq!(a.dot(b).to_bits(), b.dot(a).to_bits());
    }

    #[test]
    fn double_negation_is_the_identity_bitwise(a in vec3()) {
        prop_assert_eq!(bits(-(-a)), bits(a));
    }

    #[test]
    fn cross_products_anticommute_bitwise(a in vec3(), b in vec3()) {
        // Domain-dependent: for parallel inputs the components cancel
        // exactly and IEEE gives +0.0 on both sides of the negation,
        // where negating flips the sign bit — the sampled domain makes
        // exact cancellation unreachable (see module docs).
        prop_assert_eq!(bits(a.cross(b)), bits(-(b.cross(a))));
    }

    #[test]
    fn identity_matrix_transforms_bitwise(a in vec3()) {
        // Domain-dependent: a -0.0 component would come back +0.0 through
        // the 0·y terms; the sampled domain excludes signed zero.
        prop_assert_eq!(bits(Mat4::IDENTITY.transform_point(a)), bits(a));
        prop_assert_eq!(bits(Mat4::IDENTITY.transform_vector(a)), bits(a));
    }

    #[test]
    fn identity_quaternion_rotates_bitwise(a in vec3()) {
        prop_assert_eq!(bits(Quat::IDENTITY.rotate(a)), bits(a));
    }

    #[test]
    fn min_and_max_bound_their_inputs_bitwise(a in vec3(), b in vec3()) {
        let low = a.min(b);
        let high = a.max(b);
        // min/max pick one of the two inputs per component, exactly.
        for (component_low, (component_a, component_b)) in
            [(low.x, (a.x, b.x)), (low.y, (a.y, b.y)), (low.z, (a.z, b.z))]
        {
            prop_assert!(
                component_low.to_bits() == component_a.to_bits()
                    || component_low.to_bits() == component_b.to_bits()
            );
        }
        prop_assert!(low.x <= high.x && low.y <= high.y && low.z <= high.z);
    }

    #[test]
    fn boxes_contain_what_built_them(points in prop::collection::vec(vec3(), 1..16)) {
        let bounds = Aabb3::from_points(&points).expect("non-empty input");
        for point in &points {
            prop_assert!(bounds.contains(*point));
        }
    }

    #[test]
    fn union_contains_both_operands(a in vec3(), b in vec3(), c in vec3(), d in vec3()) {
        let first = Aabb3::new(a.min(b), a.max(b));
        let second = Aabb3::new(c.min(d), c.max(d));
        let union = first.union(second);
        prop_assert!(union.contains(first.min()) && union.contains(first.max()));
        prop_assert!(union.contains(second.min()) && union.contains(second.max()));
        prop_assert!(union.intersects(first) && union.intersects(second));
    }

    #[test]
    fn lerp_endpoints_are_exact(a in vec3(), b in vec3()) {
        // t = 0 must return `self` exactly: a + (b - a) * 0 == a + 0.
        // (Exact because x * 0 = ±0 for finite x and x + ±0 = x, except
        // x = -0; the strategy's range keeps signed zero unreachable.)
        prop_assert_eq!(bits(a.lerp(b, 0.0)), bits(a));
    }

    // ---- tolerance tier ----------------------------------------------

    #[test]
    fn normalized_vectors_have_unit_length(v in unit_vec3()) {
        prop_assert!((v.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn rotation_preserves_length(axis in unit_vec3(), angle in -10.0f32..10.0, v in vec3()) {
        let q = Quat::from_axis_angle(axis, angle);
        let rotated = q.rotate(v);
        let scale = v.length().max(1.0);
        prop_assert!((rotated.length() - v.length()).abs() / scale < 1e-4);
    }

    #[test]
    fn conjugate_undoes_rotation(axis in unit_vec3(), angle in -10.0f32..10.0, v in vec3()) {
        let q = Quat::from_axis_angle(axis, angle);
        let round_trip = q.conjugate().rotate(q.rotate(v));
        let scale = v.length().max(1.0);
        prop_assert!((round_trip - v).length() / scale < 1e-4);
    }

    #[test]
    fn matrix_and_quaternion_rotation_agree(
        axis in unit_vec3(),
        angle in -10.0f32..10.0,
        v in vec3(),
    ) {
        let q = Quat::from_axis_angle(axis, angle);
        let m = Mat4::from_quat(q);
        let scale = v.length().max(1.0);
        prop_assert!((m.transform_vector(v) - q.rotate(v)).length() / scale < 1e-4);
    }

    #[test]
    fn matrix_multiplication_is_associative_within_tolerance(
        a_axis in unit_vec3(),
        a_angle in -10.0f32..10.0,
        b_axis in unit_vec3(),
        b_angle in -10.0f32..10.0,
        translation in vec3(),
        point in vec3(),
    ) {
        // Well-conditioned inputs: rotations and a translation, applied
        // to a moderate point — float multiplication is not associative
        // in general, so this is a tolerance law by nature.
        let a = Mat4::from_quat(Quat::from_axis_angle(a_axis, a_angle));
        let b = Mat4::from_quat(Quat::from_axis_angle(b_axis, b_angle));
        let c = Mat4::from_translation(translation * 1e-3);
        let left = (a * b) * c;
        let right = a * (b * c);
        let scale = point.length().max(1.0);
        prop_assert!(
            (left.transform_point(point) - right.transform_point(point)).length() / scale
                < 1e-3
        );
    }
}

// ---------------------------------------------------------------------
// Alpha — the render interpolation factor.
//
// These two properties moved here with the type. They used to live in
// the frame crate and be driven through a `FrameLoop`, which could only
// reach the (timestep, remainder) pairs a loop actually produces. Stated
// directly against the constructor they cover the whole domain,
// including pairs no loop can build — which is the domain the type's
// contract is written over.
// ---------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 4096,
        rng_algorithm: proptest::test_runner::RngAlgorithm::ChaCha,
        ..ProptestConfig::default()
    })]

    /// The contract, over every representable pair: never below zero,
    /// never at or above one. The interesting region is one nanosecond
    /// short of a whole step, where a naive `f32` division returns
    /// exactly `1.0` — a renderer drawing a full tick ahead of the state
    /// it interpolates from.
    #[test]
    fn alpha_stays_in_the_unit_interval_for_every_timestep_and_remainder(
        step in 1u64..=u64::MAX,
        fraction in 0u64..=u64::MAX,
    ) {
        let remainder = fraction % step;
        let alpha = Alpha::new(remainder, NonZeroU64::new(step).expect("nonzero")).get();
        prop_assert!(alpha >= 0.0, "step {} rem {} gave {}", step, remainder, alpha);
        prop_assert!(alpha < 1.0, "step {} rem {} gave {}", step, remainder, alpha);
    }

    /// The clamp holds where the caller does not: a remainder at or past
    /// the step still cannot escape the range. The frame loop never
    /// produces this, and a type whose whole contract is a range must not
    /// depend on its caller to keep it.
    #[test]
    fn alpha_cannot_escape_the_range_even_on_an_out_of_contract_remainder(
        step in 1u64..=u64::MAX,
        remainder in 0u64..=u64::MAX,
    ) {
        let alpha = Alpha::new(remainder, NonZeroU64::new(step).expect("nonzero")).get();
        prop_assert!((0.0..1.0).contains(&alpha), "step {} rem {} gave {}", step, remainder, alpha);
    }

    /// Monotone in the remainder: interpolating further between two
    /// steps never moves the renderer backwards. Stated at the type
    /// rather than through a loop, so it covers pairs a loop cannot
    /// reach.
    #[test]
    fn alpha_never_decreases_as_the_remainder_grows(
        step in 2u64..=1_000_000_000,
        fraction in 0u64..=u64::MAX,
    ) {
        let step_nz = NonZeroU64::new(step).expect("nonzero");
        let low = fraction % step;
        let high = low.max(step / 2);
        prop_assert!(Alpha::new(high, step_nz) >= Alpha::new(low, step_nz));
    }

    /// A pure function of its two arguments — the property the frame
    /// digest's exclusion of alpha rests on. Same pair, same bits, every
    /// time, with no state anywhere to carry a difference.
    #[test]
    fn alpha_is_a_pure_function_of_its_two_arguments(
        step in 1u64..=u64::MAX,
        fraction in 0u64..=u64::MAX,
    ) {
        let step_nz = NonZeroU64::new(step).expect("nonzero");
        let remainder = fraction % step;
        prop_assert_eq!(
            Alpha::new(remainder, step_nz).get().to_bits(),
            Alpha::new(remainder, step_nz).get().to_bits()
        );
    }
}
