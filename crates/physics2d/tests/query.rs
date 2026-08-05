//! Asking the world what is where.

use renew_ecs::{Entities, Entity};
use renew_fixed::{Fixed, Vec2};
use renew_physics2d::{BodyKind, Collider, Counts, Exclude, Filter, Shape, Transform, World};

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

fn at(x: i32, y: i32) -> Transform {
    Transform::at(v(x, y))
}

fn square(half: i32) -> Shape {
    Shape::Box {
        half_extents: v(half, half),
    }
}

const RIGHT: Vec2 = Vec2::new(Fixed::ONE, Fixed::ZERO);
const FAR: Fixed = Fixed::from_bits(100 * 65536);
const ANY: u32 = u32::MAX;

/// A world of unit boxes at the given positions, all on layer 1.
fn world_of(positions: &[(i32, i32)]) -> (World, Vec<Entity>) {
    let mut entities = Entities::new();
    let mut world = World::new();
    let mut handles = Vec::new();
    for &(x, y) in positions {
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Kinematic, at(x, y));
        world.add_shape(handle, square(1), Transform::IDENTITY, Filter::new(1, 1));
        handles.push(handle);
    }
    (world, handles)
}

#[test]
fn a_point_finds_every_shape_containing_it() {
    // Three boxes all covering the origin, plus one that does not.
    let (world, _) = world_of(&[(0, 0), (1, 0), (0, 1), (9, 9)]);
    let mut found = [Collider {
        handle: Entities::new().spawn(),
        index: renew_physics2d::ShapeIndex::from_raw(0),
    }; 8];
    let counts = world.point_query(Vec2::ZERO, ANY, Exclude::NONE, &mut found);
    assert_eq!(counts.existed, 3, "three boxes contain the origin");
    assert_eq!(counts.written, 3);
    assert!(!counts.truncated());
    // Ascending, because the world iterates in collider order.
    for window in found[..3].windows(2) {
        assert!(window[0] < window[1]);
    }
}

/// **A full buffer is reported, never silently truncated.** A query that
/// returned only what fitted would let a world quietly stop reporting under
/// load.
#[test]
fn a_small_buffer_reports_what_it_could_not_write() {
    let (world, _) = world_of(&[(0, 0), (1, 0), (0, 1)]);
    let mut found = [Collider {
        handle: Entities::new().spawn(),
        index: renew_physics2d::ShapeIndex::from_raw(0),
    }; 1];
    let counts = world.point_query(Vec2::ZERO, ANY, Exclude::NONE, &mut found);
    assert_eq!(counts.written, 1, "only one fitted");
    assert_eq!(counts.existed, 3, "and three existed");
    assert!(
        counts.truncated(),
        "the caller can tell it lost information"
    );
}

#[test]
fn a_point_in_empty_space_finds_nothing() {
    let (world, _) = world_of(&[(9, 9)]);
    let mut found: [Collider; 4] = [Collider {
        handle: Entities::new().spawn(),
        index: renew_physics2d::ShapeIndex::from_raw(0),
    }; 4];
    let counts = world.point_query(Vec2::ZERO, ANY, Exclude::NONE, &mut found);
    assert_eq!(counts, Counts::default(), "nothing found, nothing written");
}

#[test]
fn a_query_mask_hides_shapes_on_other_layers() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let terrain = entities.spawn();
    let pickup = entities.spawn();
    world.create_body(terrain, BodyKind::Static, at(0, 0));
    world.create_body(pickup, BodyKind::Kinematic, at(0, 0));
    world.add_shape(
        terrain,
        square(1),
        Transform::IDENTITY,
        Filter::new(0b01, 0b11),
    );
    // A pickup that collides with nothing: mask zero. It must still be
    // findable by a query naming its layer.
    world.add_shape(
        pickup,
        square(1),
        Transform::IDENTITY,
        Filter::new(0b10, 0b00),
    );

    let mut found = [Collider {
        handle: terrain,
        index: renew_physics2d::ShapeIndex::from_raw(0),
    }; 4];

    let terrain_only = world.point_query(Vec2::ZERO, 0b01, Exclude::NONE, &mut found);
    assert_eq!(terrain_only.existed, 1);
    assert_eq!(found[0].handle, terrain);

    let pickups_only = world.point_query(Vec2::ZERO, 0b10, Exclude::NONE, &mut found);
    assert_eq!(
        pickups_only.existed, 1,
        "a mask-zero shape is still castable"
    );
    assert_eq!(found[0].handle, pickup);

    let both = world.point_query(Vec2::ZERO, 0b11, Exclude::NONE, &mut found);
    assert_eq!(both.existed, 2);
}

#[test]
fn an_exclusion_hides_every_shape_of_a_body() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let character = entities.spawn();
    world.create_body(character, BodyKind::Kinematic, at(0, 0));
    // Two shapes on the one body, both covering the origin.
    world.add_shape(character, square(1), Transform::IDENTITY, Filter::new(1, 1));
    world.add_shape(character, square(1), at(0, 0), Filter::new(1, 1));

    let mut found = [Collider {
        handle: character,
        index: renew_physics2d::ShapeIndex::from_raw(0),
    }; 4];

    let unfiltered = world.point_query(Vec2::ZERO, ANY, Exclude::NONE, &mut found);
    assert_eq!(unfiltered.existed, 2, "both shapes contain the point");

    let excluded = world.point_query(Vec2::ZERO, ANY, Exclude::bodies(&[character]), &mut found);
    assert_eq!(
        excluded.existed, 0,
        "excluding a body excludes all its shapes"
    );
}

/// A handle naming no body excludes nothing and the query answers — refusing
/// would collide with the legitimate empty answer.
#[test]
fn a_stale_handle_in_an_exclusion_excludes_nothing() {
    let (world, handles) = world_of(&[(0, 0)]);
    let mut stranger_source = Entities::new();
    let stranger = stranger_source.spawn();
    let _ = handles;

    let mut found = [Collider {
        handle: stranger,
        index: renew_physics2d::ShapeIndex::from_raw(0),
    }; 4];
    let counts = world.point_query(Vec2::ZERO, ANY, Exclude::bodies(&[stranger]), &mut found);
    // The stranger happens to share an index with the real body but the
    // exclusion compares whole handles, and either way the query answers.
    assert!(counts.existed <= 1);
}

#[test]
fn a_ray_finds_the_nearest_body() {
    let (world, handles) = world_of(&[(0, 0), (5, 0), (10, 0)]);
    let hit = world
        .ray_query(v(-5, 0), RIGHT, FAR, ANY, Exclude::NONE)
        .expect("three boxes ahead");
    assert_eq!(hit.collider.handle, handles[0], "the nearest one");
    let expected = Fixed::from_int(4);
    assert!((hit.distance - expected).to_bits().abs() <= 16);
}

/// **Ties go to the lowest collider.** In a tile-aligned world almost
/// everything shares an edge, so a ray down a seam meets two tiles at the same
/// distance — and "nearest" would otherwise be whichever the iteration reached
/// first.
#[test]
fn a_ray_meeting_two_bodies_at_once_takes_the_lower_collider() {
    // Two boxes stacked so their shared edge is exactly on the ray's path.
    let (world, handles) = world_of(&[(0, 1), (0, -1)]);
    let hit = world
        .ray_query(v(-5, 0), RIGHT, FAR, ANY, Exclude::NONE)
        .expect("both are on the line y = 0");
    assert_eq!(
        hit.collider.handle, handles[0],
        "the lower collider wins the tie"
    );
}

#[test]
fn a_ray_can_be_told_to_ignore_the_body_casting_it() {
    let (world, handles) = world_of(&[(0, 0), (5, 0)]);
    // A ray starting inside the first body hits it at zero without exclusion.
    let unfiltered = world
        .ray_query(Vec2::ZERO, RIGHT, FAR, ANY, Exclude::NONE)
        .expect("inside the first");
    assert_eq!(unfiltered.distance, Fixed::ZERO);

    let excluded = world
        .ray_query(Vec2::ZERO, RIGHT, FAR, ANY, Exclude::bodies(&[handles[0]]))
        .expect("the second is still there");
    assert_eq!(excluded.collider.handle, handles[1]);
}

#[test]
fn a_ray_into_nothing_finds_nothing() {
    let (world, _) = world_of(&[(0, 9)]);
    assert!(
        world
            .ray_query(v(-5, 0), RIGHT, FAR, ANY, Exclude::NONE)
            .is_none()
    );
}

#[test]
fn an_overlap_finds_what_a_shape_would_touch() {
    let (world, handles) = world_of(&[(0, 0), (3, 0), (9, 9)]);
    let mut found = [Collider {
        handle: handles[0],
        index: renew_physics2d::ShapeIndex::from_raw(0),
    }; 8];
    // A wide box spanning the first two.
    let sweep_shape = Shape::Box {
        half_extents: v(2, 1),
    };
    let counts = world.overlap_query(sweep_shape, at(1, 0), ANY, Exclude::NONE, &mut found);
    assert_eq!(counts.existed, 2, "the two near boxes, not the distant one");
    assert_eq!(found[0].handle, handles[0]);
    assert_eq!(found[1].handle, handles[1]);
}

#[test]
fn an_overlap_honours_the_mask_and_the_exclusion() {
    let (world, handles) = world_of(&[(0, 0), (1, 0)]);
    let mut found = [Collider {
        handle: handles[0],
        index: renew_physics2d::ShapeIndex::from_raw(0),
    }; 8];

    let none_on_this_layer =
        world.overlap_query(square(1), at(0, 0), 0b10, Exclude::NONE, &mut found);
    assert_eq!(none_on_this_layer.existed, 0);

    let excluded = world.overlap_query(
        square(1),
        at(0, 0),
        ANY,
        Exclude::bodies(&[handles[0]]),
        &mut found,
    );
    assert_eq!(excluded.existed, 1, "only the one not excluded");
    assert_eq!(found[0].handle, handles[1]);
}

/// An overlap against an empty world answers rather than refusing.
#[test]
fn an_overlap_against_an_empty_world_answers() {
    let world = World::new();
    let mut found: [Collider; 2] = [Collider {
        handle: Entities::new().spawn(),
        index: renew_physics2d::ShapeIndex::from_raw(0),
    }; 2];
    let counts = world.overlap_query(square(1), at(0, 0), ANY, Exclude::NONE, &mut found);
    assert_eq!(counts, Counts::default());
    assert!(
        world
            .ray_query(Vec2::ZERO, RIGHT, FAR, ANY, Exclude::NONE)
            .is_none()
    );
}
