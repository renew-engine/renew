//! Whether two shapes touch, where, and which way the normal points.

use proptest::prelude::*;
use renew_fixed::{Angle, Fixed, Vec2};
use renew_physics2d::{Shape, Transform, collide};

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

fn raw(x: i64, y: i64) -> Vec2 {
    Vec2::new(Fixed::from_bits(x), Fixed::from_bits(y))
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

fn at(x: i32, y: i32) -> Transform {
    Transform::at(v(x, y))
}

/// Raw units of slack for a value that went through a square root or a sine.
const SLACK: i64 = 8;

fn close(actual: Fixed, expected: Fixed, what: &str) {
    let difference = (actual - expected).to_bits().abs();
    assert!(
        difference <= SLACK,
        "{what}: got {} raw, expected {} raw",
        actual.to_bits(),
        expected.to_bits()
    );
}

#[test]
fn circles_apart_do_not_touch() {
    assert!(collide(circle(1), at(0, 0), circle(1), at(5, 0)).is_none());
}

#[test]
fn circles_exactly_touching_report_a_contact_at_depth_zero() {
    // Radii sum to 2, centres 2 apart: they touch and nothing more.
    let manifold =
        collide(circle(1), at(0, 0), circle(1), at(2, 0)).expect("touching is a contact");
    close(manifold.points()[0].depth, Fixed::ZERO, "depth");
    close(manifold.normal.x, Fixed::ONE, "normal x");
    close(manifold.normal.y, Fixed::ZERO, "normal y");
}

#[test]
fn overlapping_circles_report_the_overlap_and_point_from_the_first() {
    let manifold = collide(circle(2), at(0, 0), circle(2), at(3, 0)).expect("overlapping");
    close(manifold.deepest(), Fixed::ONE, "depth");
    close(
        manifold.normal.x,
        Fixed::ONE,
        "the normal points toward the second",
    );
    // The point sits in the middle of the overlapped span: 2 - 0.5 = 1.5.
    close(
        manifold.points()[0].position.x,
        Fixed::from_ratio(3, 2),
        "contact point",
    );
}

/// Coincident circles have no direction to separate along, and the contract
/// picks one rather than leaving it to whatever `normalize` does with zero.
#[test]
fn coincident_circles_take_the_stated_fallback_direction() {
    let manifold = collide(circle(1), at(0, 0), circle(1), at(0, 0)).expect("overlapping");
    assert_eq!(manifold.normal, v(1, 0), "the stated arbitrary direction");
    close(manifold.deepest(), Fixed::from_int(2), "fully overlapped");
}

#[test]
fn a_circle_beside_a_box_touches_its_face() {
    // Unit square at the origin, unit circle centred at x = 1.5: they overlap
    // by half a unit through the +x face.
    let manifold = collide(
        circle(1),
        Transform::at(Vec2::new(Fixed::from_ratio(3, 2), Fixed::ZERO)),
        square(1),
        at(0, 0),
    )
    .expect("overlapping");
    close(manifold.deepest(), Fixed::from_ratio(1, 2), "depth");
    // From the circle toward the box is −x.
    close(manifold.normal.x, Fixed::from_int(-1), "normal x");
    close(manifold.normal.y, Fixed::ZERO, "normal y");
}

#[test]
fn a_circle_past_a_box_corner_does_not_touch() {
    // The corner is at (1,1); a unit circle centred at (2.5, 2.5) is about
    // 2.12 away from it, well past its radius.
    let far = Transform::at(Vec2::new(Fixed::from_ratio(5, 2), Fixed::from_ratio(5, 2)));
    assert!(collide(circle(1), far, square(1), at(0, 0)).is_none());

    // Move it in until the corner is inside the radius and it touches.
    let near = Transform::at(Vec2::new(Fixed::from_ratio(3, 2), Fixed::from_ratio(3, 2)));
    let manifold = collide(circle(1), near, square(1), at(0, 0)).expect("corner contact");
    assert!(manifold.deepest() > Fixed::ZERO);
}

/// A circle whose centre is inside the box has no closest surface point to
/// measure from, so the direction comes from the nearest face instead.
#[test]
fn a_circle_inside_a_box_leaves_through_the_nearest_face() {
    // Centre at (0.8, 0) inside a unit square: nearest face is +x, 0.2 away.
    let inside = Transform::at(Vec2::new(Fixed::from_ratio(4, 5), Fixed::ZERO));
    let manifold = collide(circle(1), inside, square(1), at(0, 0)).expect("inside");
    close(manifold.normal.x, Fixed::from_int(-1), "out through +x");
    close(manifold.normal.y, Fixed::ZERO, "normal y");
    // Depth is the distance to the face plus the radius.
    close(
        manifold.deepest(),
        Fixed::from_ratio(1, 5) + Fixed::ONE,
        "depth",
    );
}

/// **The manifold that stops a box rocking on a floor.** Two axis-aligned
/// boxes meeting face to face touch along a segment, and reporting one
/// representative point of it loses half the support.
#[test]
fn boxes_meeting_face_to_face_report_two_points() {
    // A unit square resting on a wide floor, overlapping by a quarter.
    let floor = Shape::Box {
        half_extents: v(10, 1),
    };
    let resting = Transform::at(Vec2::new(Fixed::ZERO, Fixed::from_ratio(7, 4)));
    let manifold = collide(square(1), resting, floor, at(0, 0)).expect("resting");

    assert_eq!(manifold.count, 2, "a face contact has two points");
    close(manifold.normal.x, Fixed::ZERO, "normal x");
    close(
        manifold.normal.y,
        Fixed::from_int(-1),
        "the box is pushed up",
    );
    for point in manifold.points() {
        close(point.depth, Fixed::from_ratio(1, 4), "each point's depth");
    }
    // The two points are the ends of the overlap, one unit either side.
    let xs: Vec<i64> = manifold
        .points()
        .iter()
        .map(|point| point.position.x.to_bits())
        .collect();
    assert!(xs[0] != xs[1], "two distinct points, not one twice");
}

#[test]
fn boxes_apart_do_not_touch() {
    assert!(collide(square(1), at(0, 0), square(1), at(3, 0)).is_none());
    assert!(collide(square(1), at(0, 0), square(1), at(0, 3)).is_none());
    // Diagonally apart, where a bounding-circle test would wrongly report a hit.
    assert!(collide(square(1), at(0, 0), square(1), at(3, 3)).is_none());
}

#[test]
fn boxes_that_merely_touch_report_depth_zero() {
    let manifold = collide(square(1), at(0, 0), square(1), at(2, 0)).expect("touching");
    close(manifold.deepest(), Fixed::ZERO, "depth");
    close(manifold.normal.x, Fixed::ONE, "normal points at the second");
}

/// A box turned by an eighth turn reaches further along the world axes, so a
/// pair that misses when upright touches when turned. This is the cheapest
/// check that the rotation actually reaches the separating-axis test.
#[test]
fn rotation_changes_whether_boxes_touch() {
    let apart = Transform::at(Vec2::new(Fixed::from_ratio(11, 5), Fixed::ZERO));
    assert!(
        collide(square(1), at(0, 0), square(1), apart).is_none(),
        "upright, 2.2 apart, they miss"
    );

    let turned = Transform::new(apart.translation, Angle::from_turn_ratio(1, 8));
    let manifold =
        collide(square(1), at(0, 0), square(1), turned).expect("turned, the corner reaches in");
    assert!(manifold.deepest() > Fixed::ZERO);
}

proptest! {
    /// **Swapping the arguments must flip the normal and keep the depth.** A
    /// narrowphase that is not symmetric makes a contact depend on which
    /// collider the broadphase happened to name first, and the pair ordering
    /// exists precisely so it cannot.
    #[test]
    fn colliding_is_symmetric_for_circles(
        ax in -4i64..5, ay in -4i64..5, ra in 1i64..4,
        bx in -4i64..5, by in -4i64..5, rb in 1i64..4,
    ) {
        let a = Shape::Circle { radius: Fixed::from_ratio(i32::try_from(ra).unwrap_or(1), 2) };
        let b = Shape::Circle { radius: Fixed::from_ratio(i32::try_from(rb).unwrap_or(1), 2) };
        let a_at = Transform::at(raw(ax * 65536, ay * 65536));
        let b_at = Transform::at(raw(bx * 65536, by * 65536));

        // Exactly coincident centres carry no direction, so both orders take
        // the stated fallback and agree rather than opposing. That case is
        // pinned by `coincident_circles_take_the_stated_fallback_direction`
        // instead; excluding it here keeps this property about the thing it
        // is actually testing.
        prop_assume!((ax, ay) != (bx, by));

        match (collide(a, a_at, b, b_at), collide(b, b_at, a, a_at)) {
            (Some(forward), Some(backward)) => {
                let sum_x = (forward.normal.x + backward.normal.x).to_bits().abs();
                let sum_y = (forward.normal.y + backward.normal.y).to_bits().abs();
                prop_assert!(sum_x <= SLACK, "normals must oppose on x");
                prop_assert!(sum_y <= SLACK, "normals must oppose on y");
                let depth_gap = (forward.deepest() - backward.deepest()).to_bits().abs();
                prop_assert!(depth_gap <= SLACK, "depth must not depend on argument order");
            }
            (None, None) => {}
            _ => prop_assert!(false, "one order found a contact and the other did not"),
        }
    }

    /// The same, for boxes — where the separating-axis enumeration visits the
    /// two shapes' axes in a fixed order, so symmetry is a real risk rather
    /// than an obvious property.
    #[test]
    fn colliding_is_symmetric_for_boxes(
        bx in -5i64..6, by in -5i64..6, half in 1i64..4,
    ) {
        let a = square(1);
        let b = Shape::Box {
            half_extents: Vec2::new(
                Fixed::from_ratio(i32::try_from(half).unwrap_or(1), 2),
                Fixed::from_ratio(i32::try_from(half).unwrap_or(1), 2),
            ),
        };
        let a_at = at(0, 0);
        let b_at = Transform::at(raw(bx * 32768, by * 32768));
        // Same exclusion, same reason: coincident centres have no direction.
        prop_assume!((bx, by) != (0, 0));

        match (collide(a, a_at, b, b_at), collide(b, b_at, a, a_at)) {
            (Some(forward), Some(backward)) => {
                let sum_x = (forward.normal.x + backward.normal.x).to_bits().abs();
                let sum_y = (forward.normal.y + backward.normal.y).to_bits().abs();
                prop_assert!(sum_x <= SLACK, "normals must oppose on x");
                prop_assert!(sum_y <= SLACK, "normals must oppose on y");
            }
            (None, None) => {}
            _ => prop_assert!(false, "one order found a contact and the other did not"),
        }
    }

    /// Depth is never negative, and every reported point carries one. A
    /// negative depth would mean "they touch, by less than nothing", which a
    /// caller cannot act on.
    #[test]
    fn every_reported_depth_is_non_negative(
        bx in -4i64..5, by in -4i64..5, turn in 0i32..8,
    ) {
        let b_at = Transform::new(
            raw(bx * 32768, by * 32768),
            Angle::from_turn_ratio(turn, 8),
        );
        if let Some(manifold) = collide(square(1), at(0, 0), square(1), b_at) {
            prop_assert!(manifold.count >= 1, "a contact has at least one point");
            for point in manifold.points() {
                prop_assert!(
                    point.depth >= Fixed::ZERO,
                    "depth {} is negative",
                    point.depth.to_bits()
                );
            }
        }
    }
}

/// **A box first and a circle second**, which is the same geometry reversed —
/// and a separate code path, because solving it in one order and flipping is
/// what stops two implementations of the same test from drifting apart.
#[test]
fn a_box_against_a_circle_is_the_reverse_of_a_circle_against_a_box() {
    let circle_at = Transform::at(Vec2::new(Fixed::from_ratio(3, 2), Fixed::ZERO));
    let forward = collide(circle(1), circle_at, square(1), at(0, 0)).expect("overlapping");
    let reversed = collide(square(1), at(0, 0), circle(1), circle_at).expect("overlapping");

    close(
        forward.normal.x + reversed.normal.x,
        Fixed::ZERO,
        "normals oppose",
    );
    close(
        forward.normal.y + reversed.normal.y,
        Fixed::ZERO,
        "normals oppose",
    );
    close(
        forward.deepest(),
        reversed.deepest(),
        "same depth either way",
    );
    // The box is first, so its normal points toward the circle: +x.
    close(reversed.normal.x, Fixed::ONE, "box toward circle");
}

/// Capsules are declared and not yet implemented. Reporting "no contact" for
/// them would be a lie a caller acts on, so the gap is visible here rather
/// than silent — and this test is what will fail when the arm is filled in,
/// which is the reminder to delete it.
#[test]
fn a_capsule_reports_nothing_because_it_is_not_implemented_yet() {
    let capsule = Shape::Capsule {
        radius: Fixed::ONE,
        half_height: Fixed::ONE,
    };
    assert!(collide(capsule, at(0, 0), square(1), at(0, 0)).is_none());
    assert!(collide(square(1), at(0, 0), capsule, at(0, 0)).is_none());
    assert!(collide(capsule, at(0, 0), capsule, at(0, 0)).is_none());
}

/// A circle inside a box leaves through whichever face is nearest, and all
/// four are candidates. Testing only one of them leaves three sign branches
/// that no test has ever run.
#[test]
fn a_circle_inside_a_box_can_leave_through_any_of_the_four_faces() {
    let small = Shape::Circle {
        radius: Fixed::from_ratio(1, 4),
    };
    // Nearest face, expected outward normal from the circle toward the box.
    let cases = [
        (Fixed::from_ratio(4, 5), Fixed::ZERO, -1, 0),
        (-Fixed::from_ratio(4, 5), Fixed::ZERO, 1, 0),
        (Fixed::ZERO, Fixed::from_ratio(4, 5), 0, -1),
        (Fixed::ZERO, -Fixed::from_ratio(4, 5), 0, 1),
    ];
    for (x, y, nx, ny) in cases {
        let inside = Transform::at(Vec2::new(x, y));
        let manifold = collide(small, inside, square(1), at(0, 0)).expect("inside the box");
        close(manifold.normal.x, Fixed::from_int(nx), "normal x");
        close(manifold.normal.y, Fixed::from_int(ny), "normal y");
        assert!(manifold.deepest() > Fixed::ZERO, "inside means penetrating");
    }
}

/// **A sweep over rotations and offsets**, which is where the shapes the
/// hand-written cases never think of live: deep corner overlaps whose clipped
/// face leaves no point in front of the reference surface, and shallow ones
/// that leave both.
///
/// It asserts the invariants rather than specific numbers, and then asserts
/// that it actually exercised both manifold sizes — a sweep that only ever
/// produced one shape would be checking half of what it claims to.
#[test]
fn a_sweep_of_rotated_overlaps_holds_every_invariant() {
    let sq = square(1);
    let mut singles = 0u32;
    let mut pairs = 0u32;

    for turn in 0..32u32 {
        for xi in -24i64..25 {
            for yi in -24i64..25 {
                let other = Transform::new(
                    Vec2::new(Fixed::from_bits(xi * 8192), Fixed::from_bits(yi * 8192)),
                    Angle::from_bits(turn.wrapping_mul(u32::MAX / 32)),
                );
                let Some(manifold) = collide(sq, at(0, 0), sq, other) else {
                    continue;
                };

                assert!(
                    manifold.count == 1 || manifold.count == 2,
                    "a manifold carries one or two points, not {}",
                    manifold.count
                );
                for point in manifold.points() {
                    assert!(
                        point.depth >= Fixed::ZERO,
                        "negative depth {} at turn {turn}, offset ({xi}, {yi})",
                        point.depth.to_bits()
                    );
                }
                // The normal has to be usable as a direction, or a caller
                // sliding along it moves the wrong distance.
                let length = manifold.normal.length();
                let error = (length - Fixed::ONE).to_bits().abs();
                assert!(
                    error <= 64,
                    "normal length {} raw at turn {turn}, offset ({xi}, {yi})",
                    length.to_bits()
                );

                if manifold.count == 1 {
                    singles += 1;
                } else {
                    pairs += 1;
                }
            }
        }
    }

    assert!(singles > 0, "the sweep never produced a one-point manifold");
    assert!(pairs > 0, "the sweep never produced a two-point manifold");
}
