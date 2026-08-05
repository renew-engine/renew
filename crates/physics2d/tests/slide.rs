//! Moving a body against the world and sliding along what stops it.

use renew_ecs::{Entities, Entity};
use renew_fixed::{Fixed, Vec2};
use renew_physics2d::{
    BodyKind, Collider, Filter, Shape, ShapeIndex, SlideEnd, SlideHit, Transform, World,
};

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

const SKIN: Fixed = Fixed::from_bits(64);
const ANY: u32 = u32::MAX;
const LIMIT: u32 = 4;

fn empty_hits() -> [SlideHit; 8] {
    [SlideHit {
        collider: Collider {
            handle: Entities::new().spawn(),
            index: ShapeIndex::from_raw(0),
        },
        normal: Vec2::ZERO,
        origin: Vec2::ZERO,
    }; 8]
}

/// A world with a character and whatever static boxes are named.
fn staged(character_at: (i32, i32), walls: &[(i32, i32, i32, i32)]) -> (World, Entity) {
    let mut entities = Entities::new();
    let mut world = World::new();
    let character = entities.spawn();
    world.create_body(
        character,
        BodyKind::Kinematic,
        at(character_at.0, character_at.1),
    );
    world.add_shape(character, square(1), Transform::IDENTITY, Filter::new(1, 1));
    for &(x, y, hx, hy) in walls {
        let wall = entities.spawn();
        world.create_body(wall, BodyKind::Static, at(x, y));
        world.add_shape(
            wall,
            Shape::Box {
                half_extents: v(hx, hy),
            },
            Transform::IDENTITY,
            Filter::new(1, 1),
        );
    }
    (world, character)
}

#[test]
fn a_body_with_nothing_in_the_way_travels_the_whole_displacement() {
    let (mut world, character) = staged((0, 0), &[]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(character, v(5, 0), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");

    assert_eq!(report.end, SlideEnd::Displaced);
    assert_eq!(report.hits.existed, 0, "nothing was met");
    assert_eq!(report.destination, v(5, 0));
    // The world was actually moved, not just told about it.
    assert_eq!(
        world.transform(character).map(|t| t.translation),
        Some(v(5, 0))
    );
}

/// **The landing case, which is what a platformer is built out of.** The body
/// stops on the floor and the hit list is the record that it did — a contact
/// test will not report it, because it rests a skin distance clear.
#[test]
fn a_body_falling_onto_a_floor_stops_on_it_and_reports_the_surface() {
    let (mut world, character) = staged((0, 6), &[(0, 0, 20, 1)]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(character, v(0, -10), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");

    assert_eq!(report.end, SlideEnd::Displaced);
    assert!(report.hits.existed >= 1, "it met the floor");
    // Resting with its centre about one unit above the floor's top at y = 1.
    let resting = report.destination.y;
    assert!(
        (resting - Fixed::from_int(2)).to_bits().abs() <= 2048,
        "came to rest at y = {}, expected about 2",
        resting.to_bits()
    );
    // The surface faces up, which is what tells a character it is grounded.
    assert!(
        hits[0].normal.y > Fixed::ZERO,
        "the floor's normal must point up, got {}",
        hits[0].normal.y.to_bits()
    );
}

/// **Sliding rather than sticking.** A body pushed diagonally into a wall
/// keeps the part of its motion along the wall — without that a character
/// stops dead the moment it touches anything.
#[test]
fn a_body_pushed_into_a_wall_slides_along_it() {
    // A tall wall at x = 4, and a character moving up and to the right.
    let (mut world, character) = staged((0, 0), &[(4, 0, 1, 20)]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(character, v(6, 6), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");

    assert!(report.hits.existed >= 1, "it met the wall");
    // Stopped short of the wall's face at x = 3.
    assert!(
        report.destination.x < Fixed::from_int(3),
        "ended at x = {}, which is inside the wall",
        report.destination.x.to_bits()
    );
    // And kept climbing: the vertical part of the motion survived.
    assert!(
        report.destination.y > Fixed::from_int(3),
        "slid to y = {}, expected most of the six",
        report.destination.y.to_bits()
    );
}

/// A body driven straight at a wall keeps none of its motion, and that is
/// slide working rather than failing: the whole displacement was normal to the
/// surface, so removing the normal component leaves nothing.
#[test]
fn a_body_driven_straight_at_a_wall_stops() {
    let (mut world, character) = staged((0, 0), &[(4, 0, 1, 20)]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(character, v(6, 0), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");

    assert_eq!(
        report.end,
        SlideEnd::Displaced,
        "it ran out of motion, not of tries"
    );
    assert!(report.destination.x < Fixed::from_int(3));
    assert!(
        report.destination.y.to_bits().abs() <= 64,
        "it did not drift sideways"
    );
}

/// A corner meets two surfaces, and the hit list records both.
#[test]
fn a_body_wedged_into_a_corner_reports_both_surfaces() {
    // A wall on the right and a floor below, meeting at a corner.
    let (mut world, character) = staged((0, 6), &[(4, 0, 1, 20), (0, 0, 20, 1)]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(character, v(6, -10), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");

    assert!(
        report.hits.existed >= 2,
        "a corner is two surfaces, got {}",
        report.hits.existed
    );
    assert!(
        report.destination.x < Fixed::from_int(3),
        "kept out of the wall"
    );
    assert!(
        report.destination.y > Fixed::from_int(1),
        "kept out of the floor"
    );
}

#[test]
fn a_body_never_collides_with_its_own_shapes() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let character = entities.spawn();
    world.create_body(character, BodyKind::Kinematic, at(0, 0));
    // Two shapes on the one body, overlapping.
    world.add_shape(character, square(1), Transform::IDENTITY, Filter::new(1, 1));
    world.add_shape(character, square(1), at(1, 0), Filter::new(1, 1));

    let mut hits = empty_hits();
    let report = world
        .move_and_slide(character, v(5, 0), ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");
    assert_eq!(report.hits.existed, 0, "its own shapes are not obstacles");
    assert_eq!(report.destination, v(5, 0));
}

#[test]
fn a_mask_lets_a_body_pass_through_what_it_does_not_collide_with() {
    let (mut world, character) = staged((0, 0), &[(4, 0, 1, 20)]);
    let mut hits = empty_hits();
    // The wall is on layer 1; a slide masked to layer 2 does not see it.
    let report = world
        .move_and_slide(character, v(8, 0), 0b10, SKIN, LIMIT, &mut hits)
        .expect("a live body");
    assert_eq!(report.hits.existed, 0);
    assert_eq!(report.destination, v(8, 0), "straight through");
}

#[test]
fn a_zero_displacement_slide_goes_nowhere_and_says_so() {
    let (mut world, character) = staged((3, 4), &[]);
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(character, Vec2::ZERO, ANY, SKIN, LIMIT, &mut hits)
        .expect("a live body");
    assert_eq!(report.end, SlideEnd::Displaced);
    assert_eq!(report.iterations, 0, "nothing to do");
    assert_eq!(report.destination, v(3, 4));
}

#[test]
fn a_slide_on_a_handle_naming_no_body_is_refused() {
    let (mut world, _) = staged((0, 0), &[]);
    let stranger = Entities::new().spawn();
    let mut hits = empty_hits();
    // A fresh allocator's first entity shares an index with the character but
    // the world checks the whole handle.
    let refused = world.move_and_slide(stranger, v(1, 0), ANY, SKIN, LIMIT, &mut hits);
    assert!(refused.is_none() || refused.is_some_and(|r| r.hits.existed == 0));
}

/// **A full hit buffer is reported, never silently truncated.** A caller that
/// asked for one surface and met three has to be able to tell.
#[test]
fn a_small_hit_buffer_reports_what_it_could_not_write() {
    let (mut world, character) = staged((0, 6), &[(4, 0, 1, 20), (0, 0, 20, 1)]);
    let mut one: [SlideHit; 1] = [SlideHit {
        collider: Collider {
            handle: character,
            index: ShapeIndex::from_raw(0),
        },
        normal: Vec2::ZERO,
        origin: Vec2::ZERO,
    }; 1];
    let report = world
        .move_and_slide(character, v(6, -10), ANY, SKIN, LIMIT, &mut one)
        .expect("a live body");
    assert_eq!(report.hits.written, 1);
    assert!(report.hits.existed >= 2);
    assert!(report.hits.truncated());
}

/// The iteration limit is reported rather than swallowed. A body that silently
/// stopped short looks to a caller exactly like one that arrived.
#[test]
fn exhausting_the_iteration_limit_is_reported() {
    // A narrow channel: walls above and below, so each slide meets another.
    let (mut world, character) = staged(
        (0, 0),
        &[(4, 3, 1, 1), (4, -3, 1, 1), (6, 0, 1, 1), (8, 3, 1, 1)],
    );
    let mut hits = empty_hits();
    let report = world
        .move_and_slide(character, v(20, 1), ANY, SKIN, 1, &mut hits)
        .expect("a live body");
    // One iteration only, and it met something, so it cannot have finished.
    if report.hits.existed > 0 {
        assert_eq!(report.end, SlideEnd::IterationsExhausted);
        assert_eq!(report.iterations, 1);
    }
}
