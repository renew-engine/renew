//! Whether two shapes touch in three dimensions, where, and which way.

use proptest::prelude::*;
use renew_fixed::{Fixed, Vec3};
use renew_physics3d::{Shape, Transform, collide, narrow::separation};

fn v(x: i32, y: i32, z: i32) -> Vec3 {
    Vec3::new(Fixed::from_int(x), Fixed::from_int(y), Fixed::from_int(z))
}

fn at(x: i32, y: i32, z: i32) -> Transform {
    Transform::at(v(x, y, z))
}

fn sphere(units: i32) -> Shape {
    Shape::Sphere {
        radius: Fixed::from_int(units),
    }
}

fn cube(half: i32) -> Shape {
    Shape::Box {
        half_extents: v(half, half, half),
    }
}

const SLACK: i64 = 8;

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
fn spheres_apart_do_not_touch() {
    assert!(collide(sphere(1), at(0, 0, 0), sphere(1), at(5, 0, 0)).is_none());
    assert!(collide(sphere(1), at(0, 0, 0), sphere(1), at(0, 0, 5)).is_none());
}

#[test]
fn overlapping_spheres_report_the_overlap_along_the_line_of_centres() {
    let contact =
        collide(sphere(2), at(0, 0, 0), sphere(2), at(0, 0, 3)).expect("they overlap by one");
    close(contact.depth, Fixed::ONE, "depth");
    close(
        contact.normal.z,
        Fixed::ONE,
        "the normal points at the second",
    );
    close(contact.normal.x, Fixed::ZERO, "and nowhere else");
    close(contact.normal.y, Fixed::ZERO, "and nowhere else");
}

#[test]
fn coincident_spheres_take_the_stated_fallback_direction() {
    let contact =
        collide(sphere(1), at(0, 0, 0), sphere(1), at(0, 0, 0)).expect("fully overlapped");
    assert_eq!(contact.normal, v(1, 0, 0), "the stated arbitrary direction");
    close(contact.depth, Fixed::from_int(2), "fully overlapped");
}

/// **Separation on any single axis is separation.** This is the test a
/// two-dimensional implementation lifted carelessly fails.
#[test]
fn boxes_separated_on_any_one_axis_do_not_touch() {
    for offset in [v(3, 0, 0), v(0, 3, 0), v(0, 0, 3)] {
        assert!(
            collide(cube(1), at(0, 0, 0), cube(1), Transform::at(offset)).is_none(),
            "separated along {offset:?} and still reported touching"
        );
    }
    // Diagonally apart, where a bounding-sphere test would wrongly report a hit.
    assert!(collide(cube(1), at(0, 0, 0), cube(1), at(3, 3, 3)).is_none());
}

#[test]
fn boxes_that_merely_touch_report_depth_zero() {
    let contact = collide(cube(1), at(0, 0, 0), cube(1), at(2, 0, 0)).expect("touching");
    close(contact.depth, Fixed::ZERO, "depth");
    close(
        contact.normal.x,
        Fixed::ONE,
        "the normal points at the second",
    );
}

/// The shallowest axis wins, which is what makes a body land on a floor rather
/// than being pushed sideways off it.
#[test]
fn the_shallowest_axis_decides_the_normal() {
    // Deeply overlapped on x and z, barely on y: the answer is y.
    let contact = collide(
        Shape::Box {
            half_extents: v(4, 1, 4),
        },
        at(0, 0, 0),
        Shape::Box {
            half_extents: v(4, 1, 4),
        },
        Transform::at(Vec3::new(
            Fixed::ZERO,
            Fixed::from_ratio(15, 8),
            Fixed::ZERO,
        )),
    )
    .expect("overlapping");
    close(contact.normal.y, Fixed::ONE, "pushed along y");
    close(contact.normal.x, Fixed::ZERO, "not x");
    close(contact.normal.z, Fixed::ZERO, "not z");
    close(
        contact.depth,
        Fixed::from_ratio(1, 8),
        "the shallow overlap",
    );
}

#[test]
fn a_sphere_beside_a_box_touches_its_face() {
    // A unit box at the origin, a unit sphere centred at z = 1.5.
    let contact = collide(
        sphere(1),
        Transform::at(Vec3::new(Fixed::ZERO, Fixed::ZERO, Fixed::from_ratio(3, 2))),
        cube(1),
        at(0, 0, 0),
    )
    .expect("overlapping");
    close(contact.depth, Fixed::from_ratio(1, 2), "depth");
    close(
        contact.normal.z,
        Fixed::from_int(-1),
        "from the sphere toward the box",
    );
}

#[test]
fn a_sphere_past_a_box_corner_does_not_touch() {
    let far = Transform::at(Vec3::new(
        Fixed::from_ratio(5, 2),
        Fixed::from_ratio(5, 2),
        Fixed::from_ratio(5, 2),
    ));
    assert!(collide(sphere(1), far, cube(1), at(0, 0, 0)).is_none());

    let near = Transform::at(Vec3::new(
        Fixed::from_ratio(3, 2),
        Fixed::from_ratio(3, 2),
        Fixed::from_ratio(3, 2),
    ));
    let contact = collide(sphere(1), near, cube(1), at(0, 0, 0)).expect("corner contact");
    assert!(contact.depth > Fixed::ZERO);
}

/// A sphere whose centre is inside the box has no closest surface point, so
/// the direction comes from the nearest of the six faces.
#[test]
fn a_sphere_inside_a_box_leaves_through_the_nearest_face() {
    let small = Shape::Sphere {
        radius: Fixed::from_ratio(1, 4),
    };
    // Nearest face, and the outward normal from the sphere toward the box.
    let cases = [
        (
            v(0, 0, 0) + Vec3::new(Fixed::from_ratio(4, 5), Fixed::ZERO, Fixed::ZERO),
            (-1, 0, 0),
        ),
        (
            Vec3::new(-Fixed::from_ratio(4, 5), Fixed::ZERO, Fixed::ZERO),
            (1, 0, 0),
        ),
        (
            Vec3::new(Fixed::ZERO, Fixed::from_ratio(4, 5), Fixed::ZERO),
            (0, -1, 0),
        ),
        (
            Vec3::new(Fixed::ZERO, -Fixed::from_ratio(4, 5), Fixed::ZERO),
            (0, 1, 0),
        ),
        (
            Vec3::new(Fixed::ZERO, Fixed::ZERO, Fixed::from_ratio(4, 5)),
            (0, 0, -1),
        ),
        (
            Vec3::new(Fixed::ZERO, Fixed::ZERO, -Fixed::from_ratio(4, 5)),
            (0, 0, 1),
        ),
    ];
    for (centre, (nx, ny, nz)) in cases {
        let contact =
            collide(small, Transform::at(centre), cube(1), at(0, 0, 0)).expect("inside the box");
        close(contact.normal.x, Fixed::from_int(nx), "normal x");
        close(contact.normal.y, Fixed::from_int(ny), "normal y");
        close(contact.normal.z, Fixed::from_int(nz), "normal z");
        assert!(contact.depth > Fixed::ZERO, "inside means penetrating");
    }
}

/// A box first and a sphere second is the same geometry reversed, and a
/// separate arm — so without this the box-first path never runs.
#[test]
fn a_box_against_a_sphere_is_the_reverse_of_the_other_order() {
    let sphere_at = Transform::at(Vec3::new(Fixed::from_ratio(3, 2), Fixed::ZERO, Fixed::ZERO));
    let forward = collide(sphere(1), sphere_at, cube(1), at(0, 0, 0)).expect("overlapping");
    let reversed = collide(cube(1), at(0, 0, 0), sphere(1), sphere_at).expect("overlapping");

    close(
        forward.normal.x + reversed.normal.x,
        Fixed::ZERO,
        "normals oppose",
    );
    close(forward.depth, reversed.depth, "same depth either way");
    close(
        reversed.normal.x,
        Fixed::ONE,
        "the box points at the sphere",
    );
}

#[test]
fn separation_measures_the_gap_and_the_direction() {
    // Two unit spheres five apart: three units of clear air.
    let (gap, direction) = separation(sphere(1), at(0, 0, 0), sphere(1), at(5, 0, 0));
    close(gap, Fixed::from_int(3), "gap");
    close(direction.x, Fixed::ONE, "pointing at the second");

    // Overlapping is a negative gap.
    let (overlapped, _) = separation(sphere(2), at(0, 0, 0), sphere(2), at(3, 0, 0));
    assert!(overlapped < Fixed::ZERO, "overlap is a negative gap");

    // Boxes, on each axis in turn.
    for (offset, axis) in [(v(5, 0, 0), 0), (v(0, 5, 0), 1), (v(0, 0, 5), 2)] {
        let (gap, direction) = separation(cube(1), at(0, 0, 0), cube(1), Transform::at(offset));
        close(gap, Fixed::from_int(3), "box gap");
        let component = match axis {
            0 => direction.x,
            1 => direction.y,
            _ => direction.z,
        };
        close(
            component,
            Fixed::ONE,
            "the widest axis is the separating one",
        );
    }

    // And the box-first-sphere-second arm, which is its own path.
    let (gap, direction) = separation(cube(1), at(0, 0, 0), sphere(1), at(5, 0, 0));
    close(gap, Fixed::from_int(3), "gap");
    close(direction.x, Fixed::ONE, "pointing at the sphere");
}

proptest! {
    /// Swapping the arguments negates the normal and keeps the depth, except
    /// where the centres coincide exactly — which carries no direction and is
    /// pinned by its own test.
    #[test]
    fn colliding_is_symmetric(
        bx in -4i64..5, by in -4i64..5, bz in -4i64..5, use_box in prop::bool::ANY,
    ) {
        prop_assume!((bx, by, bz) != (0, 0, 0));
        let second = if use_box { cube(1) } else { sphere(1) };
        let b_at = Transform::at(Vec3::new(
            Fixed::from_bits(bx * 32768),
            Fixed::from_bits(by * 32768),
            Fixed::from_bits(bz * 32768),
        ));

        match (
            collide(cube(1), at(0, 0, 0), second, b_at),
            collide(second, b_at, cube(1), at(0, 0, 0)),
        ) {
            (Some(forward), Some(backward)) => {
                for (a, b) in [
                    (forward.normal.x, backward.normal.x),
                    (forward.normal.y, backward.normal.y),
                    (forward.normal.z, backward.normal.z),
                ] {
                    prop_assert!((a + b).to_bits().abs() <= SLACK, "normals must oppose");
                }
                let gap = (forward.depth - backward.depth).to_bits().abs();
                prop_assert!(gap <= SLACK, "depth must not depend on argument order");
            }
            (None, None) => {}
            _ => prop_assert!(false, "one order found a contact and the other did not"),
        }
    }

    /// A reported depth is never negative, and a reported normal is a
    /// direction: a caller pushing along a zero vector moves nowhere and one
    /// pushing along a long vector overshoots.
    #[test]
    fn every_contact_is_usable(
        bx in -3i64..4, by in -3i64..4, bz in -3i64..4,
    ) {
        let b_at = Transform::at(Vec3::new(
            Fixed::from_bits(bx * 32768),
            Fixed::from_bits(by * 32768),
            Fixed::from_bits(bz * 32768),
        ));
        if let Some(contact) = collide(cube(1), at(0, 0, 0), cube(1), b_at) {
            prop_assert!(contact.depth >= Fixed::ZERO, "depth is never negative");
            let error = (contact.normal.length() - Fixed::ONE).to_bits().abs();
            prop_assert!(error <= 64, "the normal is not unit");
        }
    }
}

/// Separation in the negative direction, and with the sphere named first.
///
/// The other separation test only ever puts the second shape on the positive
/// side and only ever names the box first, which leaves the sign branch and
/// one whole arm unexercised — both of which would report a direction pointing
/// the wrong way, and a sweep built on that walks into what it meant to avoid.
#[test]
fn separation_points_the_right_way_from_either_side() {
    // Boxes, with the second one behind the first on each axis in turn.
    for (offset, axis) in [(v(-5, 0, 0), 0), (v(0, -5, 0), 1), (v(0, 0, -5), 2)] {
        let (gap, direction) = separation(cube(1), at(0, 0, 0), cube(1), Transform::at(offset));
        close(gap, Fixed::from_int(3), "gap");
        let component = match axis {
            0 => direction.x,
            1 => direction.y,
            _ => direction.z,
        };
        close(
            component,
            Fixed::from_int(-1),
            "the direction follows the second shape",
        );
    }

    // A sphere named first against a box, which is its own arm.
    let (gap, direction) = separation(sphere(1), at(0, 0, 0), cube(1), at(5, 0, 0));
    close(gap, Fixed::from_int(3), "gap");
    close(direction.x, Fixed::ONE, "pointing at the box");

    // And behind it, so that arm's sign is exercised too.
    let (gap, direction) = separation(sphere(1), at(0, 0, 0), cube(1), at(-5, 0, 0));
    close(gap, Fixed::from_int(3), "gap");
    close(direction.x, Fixed::from_int(-1), "pointing behind");
}
