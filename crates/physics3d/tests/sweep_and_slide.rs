//! Sweeping a shape and sliding a body, in three dimensions.

use renew_ecs::{Entities, Entity};
use renew_fixed::{Fixed, Vec3};
use renew_physics3d::{
    BodyKind, Collider, Filter, Shape, ShapeIndex, SlideEnd, SlideHit, Transform, World,
    narrow::separation, sweep,
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

const SKIN: Fixed = Fixed::from_bits(256);
const ANY: u32 = u32::MAX;
const LIMIT: u32 = 4;

#[test]
fn a_sphere_moving_at_a_sphere_stops_before_it() {
    let hit = sweep(
        sphere(1),
        at(-5, 0, 0),
        v(10, 0, 0),
        sphere(1),
        Transform::IDENTITY,
        SKIN,
    )
    .expect("straight at it");
    let expected = Fixed::from_ratio(3, 10);
    assert!(
        (hit.time - expected).to_bits().abs() <= 512,
        "stopped at {} of the way",
        hit.time.to_bits()
    );
    let (gap, _) = separation(
        sphere(1),
        Transform::at(hit.origin),
        sphere(1),
        Transform::IDENTITY,
    );
    assert!(gap >= Fixed::ZERO, "the sweep must not end overlapping");
}

/// A body falling onto a floor stops on top of it, on every axis a floor could
/// be on — because a voxel world has walls and ceilings too.
#[test]
fn a_cube_swept_at_a_slab_stops_against_it_on_every_axis() {
    let cases = [
        (
            v(0, 10, 0),
            v(0, -20, 0),
            Shape::Box {
                half_extents: v(20, 1, 20),
            },
        ),
        (
            v(10, 0, 0),
            v(-20, 0, 0),
            Shape::Box {
                half_extents: v(1, 20, 20),
            },
        ),
        (
            v(0, 0, 10),
            v(0, 0, -20),
            Shape::Box {
                half_extents: v(20, 20, 1),
            },
        ),
    ];
    for (start, displacement, slab) in cases {
        let hit = sweep(
            cube(1),
            Transform::at(start),
            displacement,
            slab,
            Transform::IDENTITY,
            SKIN,
        )
        .expect("it meets the slab");
        let (gap, _) = separation(
            cube(1),
            Transform::at(hit.origin),
            slab,
            Transform::IDENTITY,
        );
        assert!(
            gap.to_bits() >= -64,
            "ended {} raw inside the slab",
            -gap.to_bits()
        );
        // And the normal points back at the mover.
        assert!(
            hit.normal.dot(displacement) < Fixed::ZERO,
            "the surface must face the mover"
        );
    }
}

#[test]
fn a_shape_moving_away_or_past_meets_nothing() {
    assert!(
        sweep(
            sphere(1),
            at(-5, 0, 0),
            v(-10, 0, 0),
            sphere(1),
            Transform::IDENTITY,
            SKIN
        )
        .is_none(),
        "travelling away"
    );
    assert!(
        sweep(
            sphere(1),
            at(-5, 5, 0),
            v(10, 0, 0),
            sphere(1),
            Transform::IDENTITY,
            SKIN
        )
        .is_none(),
        "passing above"
    );
    assert!(
        sweep(
            sphere(1),
            at(-5, 0, 5),
            v(10, 0, 0),
            sphere(1),
            Transform::IDENTITY,
            SKIN
        )
        .is_none(),
        "and passing in front"
    );
}

#[test]
fn a_shape_that_starts_overlapping_reports_zero() {
    let hit = sweep(
        sphere(1),
        Transform::IDENTITY,
        v(10, 0, 0),
        sphere(1),
        at(1, 0, 0),
        SKIN,
    )
    .expect("already inside");
    assert_eq!(hit.time, Fixed::ZERO);
}

/// **The property the whole approach exists for.** A small shape crossing a
/// thin wall at speed is what a per-step overlap test misses.
#[test]
fn a_fast_small_shape_cannot_tunnel_through_a_thin_wall() {
    let bullet = Shape::Sphere {
        radius: Fixed::from_ratio(1, 16),
    };
    let wall = Shape::Box {
        half_extents: Vec3::new(
            Fixed::from_ratio(1, 8),
            Fixed::from_int(10),
            Fixed::from_int(10),
        ),
    };
    for speed in [50i32, 200, 1000, 5000] {
        let hit = sweep(
            bullet,
            at(-10, 0, 0),
            Vec3::new(Fixed::from_int(speed), Fixed::ZERO, Fixed::ZERO),
            wall,
            Transform::IDENTITY,
            SKIN,
        )
        .unwrap_or_else(|| panic!("a bullet at {speed} per step tunnelled the wall"));
        assert!(hit.time >= Fixed::ZERO && hit.time <= Fixed::ONE);
        assert!(
            hit.origin.x < Fixed::ZERO,
            "at {speed} it stopped past the wall, at x = {}",
            hit.origin.x.to_bits()
        );
    }
}

fn empty_hits() -> [SlideHit; 8] {
    [SlideHit {
        collider: Collider {
            handle: Entities::new().spawn(),
            index: ShapeIndex::from_raw(0),
        },
        normal: Vec3::ZERO,
        origin: Vec3::ZERO,
    }; 8]
}

/// Where a block sits, and how big it is — both in whole units.
///
/// Named because the bare nested tuple is unreadable at the call site and
/// clippy is right to say so: `((4, 0, 0), (1, 20, 20))` gives a reader no way
/// to tell a position from a half-extent.
type Block = ((i32, i32, i32), (i32, i32, i32));

/// A world with a mover and whatever static blocks are named.
fn staged(start: (i32, i32, i32), blocks: &[Block]) -> (World, Entity) {
    let mut entities = Entities::new();
    let mut world = World::new();
    let mover = entities.spawn();
    world.create_body(mover, BodyKind::Kinematic, at(start.0, start.1, start.2));
    world.add_shape(mover, cube(1), Transform::IDENTITY, Filter::new(1, 1));
    for &((x, y, z), (hx, hy, hz)) in blocks {
        let block = entities.spawn();
        world.create_body(block, BodyKind::Static, at(x, y, z));
        world.add_shape(
            block,
            Shape::Box {
                half_extents: v(hx, hy, hz),
            },
            Transform::IDENTITY,
            Filter::new(1, 1),
        );
    }
    (world, mover)
}

#[test]
fn a_body_with_nothing_in_the_way_travels_the_whole_displacement() {
    let (mut world, mover) = staged((0, 0, 0), &[]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(mover, v(5, 0, 0), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");
    assert_eq!(report.end, SlideEnd::Displaced);
    assert_eq!(report.hits.existed, 0);
    assert_eq!(report.destination, v(5, 0, 0));
    assert_eq!(
        world.transform(mover).map(|t| t.translation),
        Some(v(5, 0, 0))
    );
}

/// **Sliding rather than sticking**, which is the bug the platformer found in
/// two dimensions and which this crate inherits the fix for.
#[test]
fn a_body_pushed_into_a_wall_slides_along_it() {
    // A wall at x = 4, and a mover heading up and to the right.
    let (mut world, mover) = staged((0, 0, 0), &[((4, 0, 0), (1, 20, 20))]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(mover, v(6, 6, 0), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");

    assert!(report.hits.existed >= 1, "it met the wall");
    assert!(
        report.destination.x < Fixed::from_int(3),
        "ended at x = {}, inside the wall",
        report.destination.x.to_bits()
    );
    assert!(
        report.destination.y > Fixed::from_int(3),
        "and kept climbing, to y = {}",
        report.destination.y.to_bits()
    );
}

/// Sliding along a *third* axis, which two dimensions cannot test at all: a
/// body pressed into a wall while moving diagonally in the plane of that wall
/// keeps both of the components that are not into it.
#[test]
fn a_body_slides_in_the_plane_of_the_surface_it_meets() {
    let (mut world, mover) = staged((0, 0, 0), &[((4, 0, 0), (1, 20, 20))]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(mover, v(6, 4, 4), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");

    assert!(
        report.destination.x < Fixed::from_int(3),
        "kept out of the wall"
    );
    assert!(
        report.destination.y > Fixed::from_int(2),
        "kept its y motion, reached {}",
        report.destination.y.to_bits()
    );
    assert!(
        report.destination.z > Fixed::from_int(2),
        "and its z motion, reached {}",
        report.destination.z.to_bits()
    );
}

#[test]
fn a_body_never_collides_with_its_own_shapes() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let mover = entities.spawn();
    world.create_body(mover, BodyKind::Kinematic, Transform::IDENTITY);
    world.add_shape(mover, cube(1), Transform::IDENTITY, Filter::new(1, 1));
    world.add_shape(mover, cube(1), at(1, 0, 0), Filter::new(1, 1));

    let mut hits = empty_hits();
    let report = world
        .move_and_slide(mover, v(5, 0, 0), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");
    assert_eq!(report.hits.existed, 0, "its own shapes are not obstacles");
    assert_eq!(report.destination, v(5, 0, 0));
}

#[test]
fn a_mask_lets_a_body_pass_through_what_it_does_not_collide_with() {
    let (mut world, mover) = staged((0, 0, 0), &[((4, 0, 0), (1, 20, 20))]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(mover, v(8, 0, 0), 0b10, SKIN, LIMIT, &mut hits)
        .expect("a live body");
    assert_eq!(report.hits.existed, 0);
    assert_eq!(report.destination, v(8, 0, 0), "straight through");
}

#[test]
fn a_zero_displacement_slide_goes_nowhere_and_says_so() {
    let (mut world, mover) = staged((3, 4, 5), &[]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(mover, Vec3::ZERO, ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");
    assert_eq!(report.end, SlideEnd::Displaced);
    assert_eq!(report.iterations, 0);
    assert_eq!(report.destination, v(3, 4, 5));
}

#[test]
fn a_slide_on_a_destroyed_body_is_refused() {
    let (mut world, _) = staged((0, 0, 0), &[]);
    let mut entities = Entities::new();
    let _first = entities.spawn();
    let doomed = entities.spawn();
    world.create_body(doomed, BodyKind::Kinematic, Transform::IDENTITY);
    world.destroy_body(doomed);

    let mut hits = empty_hits();
    assert!(
        world
            .move_and_slide(doomed, v(1, 0, 0), ANY, SKIN, LIMIT, &mut hits)
            .is_none(),
        "a destroyed body has nothing to move"
    );
}

#[test]
fn a_small_hit_buffer_reports_what_it_could_not_write() {
    // A corner of three slabs, so a diagonal run meets more than one.
    let (mut world, mover) = staged(
        (0, 6, 0),
        &[((4, 0, 0), (1, 20, 20)), ((0, 0, 0), (20, 1, 20))],
    );
    let mut one = [SlideHit {
        collider: Collider {
            handle: mover,
            index: ShapeIndex::from_raw(0),
        },
        normal: Vec3::ZERO,
        origin: Vec3::ZERO,
    }; 1];
    let report = world
        .move_and_slide(mover, v(6, -10, 0), ANY, SKIN, LIMIT, &mut one)
        .expect("a live body");
    assert_eq!(report.hits.written, 1);
    if report.hits.existed > 1 {
        assert!(report.hits.truncated());
    }
}

#[test]
fn a_body_with_a_removed_shape_sweeps_only_its_live_ones() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let mover = entities.spawn();
    world.create_body(mover, BodyKind::Kinematic, Transform::IDENTITY);
    let doomed = world
        .add_shape(mover, cube(1), Transform::IDENTITY, Filter::new(1, 1))
        .expect("live");
    world.add_shape(mover, cube(1), Transform::IDENTITY, Filter::new(1, 1));
    assert!(world.remove_shape(mover, doomed));

    let wall = entities.spawn();
    world.create_body(wall, BodyKind::Static, at(4, 0, 0));
    world.add_shape(
        wall,
        Shape::Box {
            half_extents: v(1, 20, 20),
        },
        Transform::IDENTITY,
        Filter::new(1, 1),
    );

    let mut hits = empty_hits();
    let report = world
        .move_and_slide(mover, v(8, 0, 0), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");
    assert_eq!(report.hits.existed, 1, "the surviving shape met the wall");
    assert!(report.destination.x < Fixed::from_int(3));
}
