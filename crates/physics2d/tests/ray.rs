//! Casting rays at single shapes.

use proptest::prelude::*;
use renew_fixed::{Angle, Fixed, Vec2};
use renew_physics2d::{Shape, Transform, cast};

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

fn circle(units: i32) -> Shape {
    Shape::Circle {
        radius: Fixed::from_int(units),
    }
}

fn square(half: i32) -> Shape {
    Shape::Box {
        half_extents: v(half, half),
    }
}

const RIGHT: Vec2 = Vec2::new(Fixed::ONE, Fixed::ZERO);
const UP: Vec2 = Vec2::new(Fixed::ZERO, Fixed::ONE);
const FAR: Fixed = Fixed::from_bits(100 * 65536);
const SLACK: i64 = 16;

fn close(actual: Fixed, expected: Fixed, what: &str) {
    let gap = (actual - expected).to_bits().abs();
    assert!(
        gap <= SLACK,
        "{what}: got {} raw, expected {} raw",
        actual.to_bits(),
        expected.to_bits()
    );
}

#[test]
fn a_ray_meets_a_circle_at_its_near_surface() {
    let hit =
        cast(v(-5, 0), RIGHT, FAR, circle(1), Transform::at(Vec2::ZERO)).expect("straight at it");
    close(hit.distance, Fixed::from_int(4), "distance");
    close(hit.point.x, Fixed::from_int(-1), "point");
    close(hit.normal.x, Fixed::from_int(-1), "normal faces the ray");
}

#[test]
fn a_ray_that_misses_a_circle_reports_nothing() {
    // Passing two units above a unit circle.
    assert!(cast(v(-5, 2), RIGHT, FAR, circle(1), Transform::at(Vec2::ZERO)).is_none());
}

#[test]
fn a_circle_behind_the_origin_is_not_hit() {
    assert!(cast(v(5, 0), RIGHT, FAR, circle(1), Transform::at(Vec2::ZERO)).is_none());
}

#[test]
fn a_ray_stops_at_its_maximum_distance() {
    let short = Fixed::from_int(3);
    assert!(
        cast(v(-5, 0), RIGHT, short, circle(1), Transform::at(Vec2::ZERO)).is_none(),
        "the surface is four away and the ray reaches three"
    );
    let long = Fixed::from_int(5);
    assert!(cast(v(-5, 0), RIGHT, long, circle(1), Transform::at(Vec2::ZERO)).is_some());
}

/// **A ray beginning inside hits at distance zero**, which is what makes a
/// ground check usable: a character already intersecting the floor must be
/// told about the floor, not told there is nothing there.
#[test]
fn a_ray_starting_inside_a_circle_hits_at_zero() {
    let hit = cast(Vec2::ZERO, RIGHT, FAR, circle(2), Transform::at(Vec2::ZERO))
        .expect("inside is a hit");
    assert_eq!(hit.distance, Fixed::ZERO);
    assert_eq!(hit.point, Vec2::ZERO);
    close(
        hit.normal.x,
        Fixed::from_int(-1),
        "normal points back along the ray",
    );
}

#[test]
fn a_ray_meets_a_box_face() {
    let hit =
        cast(v(-5, 0), RIGHT, FAR, square(1), Transform::at(Vec2::ZERO)).expect("straight at it");
    close(hit.distance, Fixed::from_int(4), "distance");
    close(hit.point.x, Fixed::from_int(-1), "point");
    close(hit.normal.x, Fixed::from_int(-1), "normal faces the ray");
    close(hit.normal.y, Fixed::ZERO, "and is axis-aligned");
}

#[test]
fn a_ray_meets_a_box_from_below() {
    let hit =
        cast(v(0, -5), UP, FAR, square(1), Transform::at(Vec2::ZERO)).expect("straight at it");
    close(hit.distance, Fixed::from_int(4), "distance");
    close(hit.normal.y, Fixed::from_int(-1), "normal faces down");
}

#[test]
fn a_ray_that_passes_beside_a_box_reports_nothing() {
    assert!(cast(v(-5, 2), RIGHT, FAR, square(1), Transform::at(Vec2::ZERO)).is_none());
}

/// A ray exactly parallel to a slab divides by zero in the naive slab test.
/// Here it is a checked division and the case is decided by whether the
/// origin lies within that slab.
#[test]
fn a_ray_parallel_to_a_face_is_decided_by_its_offset() {
    // Travelling along +x at y = 2, outside a unit box's y slab: no hit.
    assert!(cast(v(-5, 2), RIGHT, FAR, square(1), Transform::at(Vec2::ZERO)).is_none());
    // Along +x at y = 0, inside the y slab: hits the −x face.
    assert!(cast(v(-5, 0), RIGHT, FAR, square(1), Transform::at(Vec2::ZERO)).is_some());
    // Exactly on the boundary, y = 1, is still inside.
    assert!(cast(v(-5, 1), RIGHT, FAR, square(1), Transform::at(Vec2::ZERO)).is_some());
}

/// A ray beginning inside a box leaves through some face; the contract says it
/// hits at zero rather than reporting the exit.
#[test]
fn a_ray_starting_inside_a_box_hits_at_zero() {
    let hit = cast(Vec2::ZERO, RIGHT, FAR, square(1), Transform::at(Vec2::ZERO))
        .expect("inside is a hit");
    assert_eq!(hit.distance, Fixed::ZERO);
}

/// A turned box is a different target, and this is the cheapest check that the
/// rotation reaches the slab test rather than being ignored.
#[test]
fn rotating_a_box_moves_where_a_ray_meets_it() {
    let upright =
        cast(v(-5, 0), RIGHT, FAR, square(1), Transform::at(Vec2::ZERO)).expect("hits the face");
    let turned = cast(
        v(-5, 0),
        RIGHT,
        FAR,
        square(1),
        Transform::new(Vec2::ZERO, Angle::from_turn_ratio(1, 8)),
    )
    .expect("hits the corner");
    // A square turned an eighth presents its corner, which reaches further.
    assert!(
        turned.distance < upright.distance,
        "turned {} should be nearer than upright {}",
        turned.distance.to_bits(),
        upright.distance.to_bits()
    );
}

#[test]
fn a_capsule_is_not_castable_yet() {
    let capsule = Shape::Capsule {
        radius: Fixed::ONE,
        half_height: Fixed::ONE,
    };
    assert!(cast(v(-5, 0), RIGHT, FAR, capsule, Transform::at(Vec2::ZERO)).is_none());
}

proptest! {
    /// Whatever a ray hits, the point it reports lies on the ray at the
    /// distance it reports. A hit whose point and distance disagree would
    /// send a caller somewhere the geometry never said.
    #[test]
    fn a_hit_point_lies_on_the_ray(
        oy in -3i64..4, target in 0u8..2, turn in 0i32..8,
    ) {
        let shape = if target == 0 { circle(1) } else { square(1) };
        let at = Transform::new(Vec2::ZERO, Angle::from_turn_ratio(turn, 8));
        let origin = Vec2::new(Fixed::from_int(-6), Fixed::from_bits(oy * 32768));
        if let Some(hit) = cast(origin, RIGHT, FAR, shape, at) {
            prop_assert!(hit.distance >= Fixed::ZERO, "distance is never negative");
            prop_assert!(hit.distance <= FAR, "and never past the limit");
            let expected = origin + RIGHT * hit.distance;
            let gap_x = (hit.point.x - expected.x).to_bits().abs();
            let gap_y = (hit.point.y - expected.y).to_bits().abs();
            prop_assert!(gap_x <= SLACK && gap_y <= SLACK, "point is off the ray");
        }
    }

    /// A reported normal is usable as a direction, and faces the ray rather
    /// than away from it — a caller reflecting off a normal pointing the wrong
    /// way sends the body into the surface.
    #[test]
    fn a_hit_normal_is_unit_and_faces_the_ray(
        oy in -3i64..4, target in 0u8..2, turn in 0i32..8,
    ) {
        let shape = if target == 0 { circle(1) } else { square(1) };
        let at = Transform::new(Vec2::ZERO, Angle::from_turn_ratio(turn, 8));
        let origin = Vec2::new(Fixed::from_int(-6), Fixed::from_bits(oy * 32768));
        if let Some(hit) = cast(origin, RIGHT, FAR, shape, at) {
            let length_error = (hit.normal.length() - Fixed::ONE).to_bits().abs();
            prop_assert!(length_error <= 64, "normal is not unit");
            prop_assert!(
                hit.normal.dot(RIGHT) <= Fixed::ZERO,
                "normal must face the ray, not away from it"
            );
        }
    }
}

/// A box entirely behind the origin exits before the ray begins, which is a
/// different rejection from the one a miss to the side takes.
#[test]
fn a_box_behind_the_origin_is_not_hit() {
    assert!(cast(v(5, 0), RIGHT, FAR, square(1), Transform::at(Vec2::ZERO)).is_none());
    // And one behind on the other axis, so both slabs get to reject.
    assert!(cast(v(0, 5), UP, FAR, square(1), Transform::at(Vec2::ZERO)).is_none());
}

/// A zero-radius circle is a point, and the vocabulary requires queries to
/// answer for one rather than refuse. A ray through it has no surface to take
/// a normal from, so the normal comes from the stated fallback.
#[test]
fn a_zero_radius_circle_is_answerable() {
    let point_shape = Shape::Circle {
        radius: Fixed::ZERO,
    };
    // Straight through its centre: the near and far crossings coincide.
    let hit = cast(v(-5, 0), RIGHT, FAR, point_shape, Transform::at(Vec2::ZERO))
        .expect("a point is a target, not an error");
    close(hit.distance, Fixed::from_int(5), "distance to the point");
    let length_error = (hit.normal.length() - Fixed::ONE).to_bits().abs();
    assert!(length_error <= 64, "even here the normal is a direction");

    // A hair off to the side and it misses, which is what makes the hit above
    // a real answer rather than a degenerate always-true.
    assert!(
        cast(
            Vec2::new(Fixed::from_int(-5), Fixed::from_bits(64)),
            RIGHT,
            FAR,
            point_shape,
            Transform::at(Vec2::ZERO)
        )
        .is_none()
    );
}
