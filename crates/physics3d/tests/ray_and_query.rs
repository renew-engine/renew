//! Casting rays at shapes, and asking the world what is where.

use proptest::prelude::*;
use renew_ecs::{Entities, Entity};
use renew_fixed::{Fixed, Vec3};
use renew_physics3d::{
    BodyKind, Collider, Counts, Exclude, Filter, Shape, ShapeIndex, Transform, World, cast,
};

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

const RIGHT: Vec3 = Vec3::new(Fixed::ONE, Fixed::ZERO, Fixed::ZERO);
const UP: Vec3 = Vec3::new(Fixed::ZERO, Fixed::ONE, Fixed::ZERO);
const FORWARD: Vec3 = Vec3::new(Fixed::ZERO, Fixed::ZERO, Fixed::ONE);
const FAR: Fixed = Fixed::from_bits(100 * 65536);
const ANY: u32 = u32::MAX;
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
fn a_ray_meets_a_sphere_at_its_near_surface() {
    let hit =
        cast(v(-5, 0, 0), RIGHT, FAR, sphere(1), Transform::IDENTITY).expect("straight at it");
    close(hit.distance, Fixed::from_int(4), "distance");
    close(hit.point.x, Fixed::from_int(-1), "point");
    close(hit.normal.x, Fixed::from_int(-1), "normal faces the ray");
}

#[test]
fn a_ray_that_misses_a_sphere_reports_nothing() {
    assert!(cast(v(-5, 2, 0), RIGHT, FAR, sphere(1), Transform::IDENTITY).is_none());
    // And missing on the third axis is a miss too.
    assert!(cast(v(-5, 0, 2), RIGHT, FAR, sphere(1), Transform::IDENTITY).is_none());
}

#[test]
fn a_sphere_behind_the_origin_is_not_hit() {
    assert!(cast(v(5, 0, 0), RIGHT, FAR, sphere(1), Transform::IDENTITY).is_none());
}

#[test]
fn a_ray_stops_at_its_maximum_distance() {
    let short = Fixed::from_int(3);
    assert!(cast(v(-5, 0, 0), RIGHT, short, sphere(1), Transform::IDENTITY).is_none());
    assert!(
        cast(
            v(-5, 0, 0),
            RIGHT,
            Fixed::from_int(5),
            sphere(1),
            Transform::IDENTITY
        )
        .is_some()
    );
}

#[test]
fn a_ray_starting_inside_a_sphere_hits_at_zero() {
    let hit = cast(Vec3::ZERO, RIGHT, FAR, sphere(2), Transform::IDENTITY).expect("inside");
    assert_eq!(hit.distance, Fixed::ZERO);
    close(hit.normal.x, Fixed::from_int(-1), "normal points back");
}

/// A box is met on whichever axis the ray arrives along, and all six faces
/// have to work — the other five are exactly what a careless lift leaves out.
#[test]
fn a_ray_meets_a_box_on_every_face() {
    let cases = [
        (v(-5, 0, 0), RIGHT, (-1, 0, 0)),
        (v(0, -5, 0), UP, (0, -1, 0)),
        (v(0, 0, -5), FORWARD, (0, 0, -1)),
        (v(5, 0, 0), -RIGHT, (1, 0, 0)),
        (v(0, 5, 0), -UP, (0, 1, 0)),
        (v(0, 0, 5), -FORWARD, (0, 0, 1)),
    ];
    for (origin, direction, (nx, ny, nz)) in cases {
        let hit = cast(origin, direction, FAR, cube(1), Transform::IDENTITY)
            .expect("straight at the box");
        close(hit.distance, Fixed::from_int(4), "distance");
        close(hit.normal.x, Fixed::from_int(nx), "normal x");
        close(hit.normal.y, Fixed::from_int(ny), "normal y");
        close(hit.normal.z, Fixed::from_int(nz), "normal z");
    }
}

#[test]
fn a_ray_that_passes_beside_a_box_reports_nothing() {
    assert!(cast(v(-5, 2, 0), RIGHT, FAR, cube(1), Transform::IDENTITY).is_none());
    assert!(cast(v(-5, 0, 2), RIGHT, FAR, cube(1), Transform::IDENTITY).is_none());
}

/// A ray exactly parallel to a slab divides by zero in the naive test. Here it
/// is checked and the case is decided by whether the origin lies within it.
#[test]
fn a_ray_parallel_to_a_face_is_decided_by_its_offset() {
    assert!(cast(v(-5, 2, 0), RIGHT, FAR, cube(1), Transform::IDENTITY).is_none());
    assert!(cast(v(-5, 0, 0), RIGHT, FAR, cube(1), Transform::IDENTITY).is_some());
    assert!(
        cast(v(-5, 1, 1), RIGHT, FAR, cube(1), Transform::IDENTITY).is_some(),
        "exactly on two boundaries is still inside both"
    );
}

#[test]
fn a_ray_starting_inside_a_box_hits_at_zero() {
    let hit = cast(Vec3::ZERO, RIGHT, FAR, cube(1), Transform::IDENTITY).expect("inside");
    assert_eq!(hit.distance, Fixed::ZERO);
}

#[test]
fn a_box_behind_the_origin_is_not_hit() {
    assert!(cast(v(5, 0, 0), RIGHT, FAR, cube(1), Transform::IDENTITY).is_none());
    assert!(cast(v(0, 5, 0), UP, FAR, cube(1), Transform::IDENTITY).is_none());
    assert!(cast(v(0, 0, 5), FORWARD, FAR, cube(1), Transform::IDENTITY).is_none());
}

/// A zero-radius sphere is a point, and the vocabulary requires queries to
/// answer for one rather than refuse.
#[test]
fn a_zero_radius_sphere_is_answerable() {
    let dot = Shape::Sphere {
        radius: Fixed::ZERO,
    };
    let hit = cast(v(-5, 0, 0), RIGHT, FAR, dot, Transform::IDENTITY).expect("a point is a target");
    close(hit.distance, Fixed::from_int(5), "distance to the point");
    // A hair to the side and it misses, which makes the hit above real.
    assert!(
        cast(
            Vec3::new(Fixed::from_int(-5), Fixed::from_bits(64), Fixed::ZERO),
            RIGHT,
            FAR,
            dot,
            Transform::IDENTITY
        )
        .is_none()
    );
}

/// A world of unit cubes at the given positions, all on layer 1.
fn world_of(positions: &[(i32, i32, i32)]) -> (World, Vec<Entity>) {
    let mut entities = Entities::new();
    let mut world = World::new();
    let mut handles = Vec::new();
    for &(x, y, z) in positions {
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Kinematic, at(x, y, z));
        world.add_shape(handle, cube(1), Transform::IDENTITY, Filter::new(1, 1));
        handles.push(handle);
    }
    (world, handles)
}

fn buffer(handle: Entity) -> [Collider; 8] {
    [Collider {
        handle,
        index: ShapeIndex::from_raw(0),
    }; 8]
}

#[test]
fn a_point_finds_every_shape_containing_it() {
    let (world, handles) = world_of(&[(0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1), (9, 9, 9)]);
    let mut found = buffer(handles[0]);
    let counts = world.point_query(Vec3::ZERO, ANY, Exclude::NONE, &mut found);
    assert_eq!(counts.existed, 4, "four cubes contain the origin");
    assert_eq!(counts.written, 4);
    for window in found[..4].windows(2) {
        assert!(window[0] < window[1], "ascending, in collider order");
    }
}

#[test]
fn a_small_buffer_reports_what_it_could_not_write() {
    let (world, handles) = world_of(&[(0, 0, 0), (1, 0, 0), (0, 0, 1)]);
    let mut one = [Collider {
        handle: handles[0],
        index: ShapeIndex::from_raw(0),
    }; 1];
    let counts = world.point_query(Vec3::ZERO, ANY, Exclude::NONE, &mut one);
    assert_eq!(counts.written, 1);
    assert_eq!(counts.existed, 3);
    assert!(counts.truncated());
}

#[test]
fn a_ray_finds_the_nearest_body_and_ties_go_to_the_lower_collider() {
    let (world, handles) = world_of(&[(0, 0, 0), (5, 0, 0), (10, 0, 0)]);
    let hit = world
        .ray_query(v(-5, 0, 0), RIGHT, FAR, ANY, Exclude::NONE)
        .expect("three ahead");
    assert_eq!(hit.collider.handle, handles[0], "the nearest one");

    // Two bodies whose faces sit on the same plane, so the ray meets both at
    // once — the tie has to go somewhere stated.
    let (tied, tied_handles) = world_of(&[(0, 1, 0), (0, -1, 0)]);
    let hit = tied
        .ray_query(v(-5, 0, 0), RIGHT, FAR, ANY, Exclude::NONE)
        .expect("both are on the line");
    assert_eq!(hit.collider.handle, tied_handles[0], "the lower collider");
}

#[test]
fn a_query_mask_and_an_exclusion_both_hide_things() {
    let (mut world, handles) = world_of(&[(0, 0, 0), (5, 0, 0)]);
    let mut found = buffer(handles[0]);

    let wrong_layer = world.point_query(Vec3::ZERO, 0b10, Exclude::NONE, &mut found);
    assert_eq!(wrong_layer.existed, 0, "nothing on that layer");

    let excluded = world.ray_query(v(-5, 0, 0), RIGHT, FAR, ANY, Exclude::bodies(&[handles[0]]));
    assert_eq!(
        excluded.expect("the second is still there").collider.handle,
        handles[1]
    );

    // A shape that collides with nothing is still findable by a cast, which is
    // what makes triggers writable.
    let trigger = handles[1];
    world.set_filter(trigger, ShapeIndex::from_raw(0), Filter::new(0b10, 0b00));
    let counts = world.overlap_query(cube(1), at(5, 0, 0), 0b10, Exclude::NONE, &mut found);
    assert_eq!(counts.existed, 1, "a mask-zero shape is still castable");
}

#[test]
fn an_overlap_finds_what_a_shape_would_touch() {
    let (world, handles) = world_of(&[(0, 0, 0), (3, 0, 0), (9, 9, 9)]);
    let mut found = buffer(handles[0]);
    let wide = Shape::Box {
        half_extents: v(2, 1, 1),
    };
    let counts = world.overlap_query(wide, at(1, 0, 0), ANY, Exclude::NONE, &mut found);
    assert_eq!(counts.existed, 2, "the two near cubes, not the distant one");
    assert_eq!(found[0].handle, handles[0]);
    assert_eq!(found[1].handle, handles[1]);
}

#[test]
fn queries_against_an_empty_world_answer_rather_than_refusing() {
    let world = World::new();
    let mut entities = Entities::new();
    let mut found = buffer(entities.spawn());
    assert_eq!(
        world.point_query(Vec3::ZERO, ANY, Exclude::NONE, &mut found),
        Counts::default()
    );
    assert_eq!(
        world.overlap_query(cube(1), Transform::IDENTITY, ANY, Exclude::NONE, &mut found),
        Counts::default()
    );
    assert!(
        world
            .ray_query(Vec3::ZERO, RIGHT, FAR, ANY, Exclude::NONE)
            .is_none()
    );
}

proptest! {
    /// Whatever a ray hits, the point it reports lies on the ray at the
    /// distance it reports, and the normal faces the ray.
    #[test]
    fn a_hit_is_self_consistent(
        oy in -3i64..4, oz in -3i64..4, use_box in prop::bool::ANY,
    ) {
        let shape = if use_box { cube(1) } else { sphere(1) };
        let origin = Vec3::new(
            Fixed::from_int(-6),
            Fixed::from_bits(oy * 32768),
            Fixed::from_bits(oz * 32768),
        );
        if let Some(hit) = cast(origin, RIGHT, FAR, shape, Transform::IDENTITY) {
            prop_assert!(hit.distance >= Fixed::ZERO && hit.distance <= FAR);
            let expected = origin + RIGHT * hit.distance;
            for (a, b) in [
                (hit.point.x, expected.x),
                (hit.point.y, expected.y),
                (hit.point.z, expected.z),
            ] {
                prop_assert!((a - b).to_bits().abs() <= SLACK, "the point is off the ray");
            }
            let length_error = (hit.normal.length() - Fixed::ONE).to_bits().abs();
            prop_assert!(length_error <= 64, "the normal is not unit");
            prop_assert!(
                hit.normal.dot(RIGHT) <= Fixed::ZERO,
                "the normal must face the ray"
            );
        }
    }
}
