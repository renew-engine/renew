//! Pushing a body out of what it is inside, or too close to.
//!
//! **The clearance is read off box arithmetic done here**, never from the
//! engine, for the same reason the creep measurement does it: an operation that
//! leaves the body in the wrong place must not also be the thing that decides
//! the place was right.

use renew_ecs::{Entities, Entity};
use renew_fixed::{Fixed, Vec2};
use renew_physics2d::{BodyKind, ClearEnd, Filter, Shape, Transform, World};

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

fn at(x: i32, y: i32) -> Transform {
    Transform::at(v(x, y))
}

const SKIN_RAW: i64 = 64;
const SKIN: Fixed = Fixed::from_bits(SKIN_RAW);
const ANY: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Wall {
    centre: (i32, i32),
    half: (i32, i32),
}

fn separation_raw(centre: Vec2, half: Vec2, wall: Wall) -> i64 {
    let wall_centre = v(wall.centre.0, wall.centre.1);
    let wall_half = v(wall.half.0, wall.half.1);
    let gap_x = (centre.x - wall_centre.x).abs().to_bits() - (half.x + wall_half.x).to_bits();
    let gap_y = (centre.y - wall_centre.y).abs().to_bits() - (half.y + wall_half.y).to_bits();
    if gap_x < 0 && gap_y < 0 {
        return gap_x.max(gap_y);
    }
    let dx = gap_x.max(0);
    let dy = gap_y.max(0);
    (dx * dx + dy * dy).isqrt()
}

#[expect(
    clippy::expect_used,
    reason = "a test helper: the body is created in the same function, and a \
              panic here is the failure being reported"
)]
fn clearance_raw(world: &World, character: Entity, walls: &[Wall]) -> i64 {
    let here = world.transform(character).expect("a live body").translation;
    walls
        .iter()
        .map(|wall| separation_raw(here, v(1, 1), *wall))
        .min()
        .unwrap_or(i64::MAX)
}

fn staged(start: (i32, i32), walls: &[Wall]) -> (World, Entity) {
    let mut entities = Entities::new();
    let mut world = World::new();
    let character = entities.spawn();
    world.create_body(character, BodyKind::Kinematic, at(start.0, start.1));
    world.add_shape(
        character,
        Shape::Box {
            half_extents: v(1, 1),
        },
        Transform::IDENTITY,
        Filter::new(1, 1),
    );
    for wall in walls {
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Static, at(wall.centre.0, wall.centre.1));
        world.add_shape(
            handle,
            Shape::Box {
                half_extents: v(wall.half.0, wall.half.1),
            },
            Transform::IDENTITY,
            Filter::new(1, 1),
        );
    }
    (world, character)
}

/// **A body inside a wall comes out**, from every side and from dead centre.
///
/// Dead centre is the case worth having: the separating direction there is a
/// tie between four faces, and an implementation that picked none of them would
/// leave the body where it was while reporting success.
#[test]
fn a_body_inside_a_wall_is_pushed_clear_of_it() {
    let walls = [Wall {
        centre: (0, 0),
        half: (4, 4),
    }];
    for start in [(0, 0), (3, 0), (-3, 0), (0, 3), (0, -3), (4, 4), (-2, 3)] {
        let (mut world, character) = staged(start, &walls);
        let report = world
            .clear_of_geometry(character, ANY, SKIN, 8)
            .expect("a live body");
        let clearance = clearance_raw(&world, character, &walls);
        assert!(
            clearance >= SKIN_RAW,
            "started at {start:?} and ended {clearance} raw away, inside the skin of {SKIN_RAW}"
        );
        assert_eq!(report.end, ClearEnd::Cleared, "from {start:?}");
        assert!(report.iterations >= 1, "from {start:?}");
    }
}

/// A body already clear is left exactly where it is, and says so.
///
/// **`AlreadyClear` and `Cleared` are different answers**, because a caller
/// checking whether a spawn was safe cannot tell them apart from the
/// displacement alone: a body one raw unit inside is pushed out by a distance
/// that rounds to nothing.
#[test]
fn a_body_already_clear_is_not_moved() {
    let walls = [Wall {
        centre: (0, 0),
        half: (4, 4),
    }];
    for start in [(10, 0), (0, 20), (-30, -30), (6, 6)] {
        let (mut world, character) = staged(start, &walls);
        let before = world.transform(character).expect("a live body").translation;
        let report = world
            .clear_of_geometry(character, ANY, SKIN, 8)
            .expect("a live body");
        assert_eq!(report.end, ClearEnd::AlreadyClear, "from {start:?}");
        assert_eq!(report.iterations, 0, "from {start:?}");
        assert_eq!(report.moved, Vec2::ZERO, "from {start:?}");
        assert_eq!(
            world.transform(character).expect("a live body").translation,
            before,
            "from {start:?}"
        );
    }
}

/// **A corner needs more than one push**, which is the whole reason this
/// iterates: coming out of one face can put the body inside the other.
#[test]
fn a_corner_takes_more_than_one_push() {
    let walls = [
        Wall {
            centre: (-5, 0),
            half: (4, 20),
        },
        Wall {
            centre: (0, -5),
            half: (20, 4),
        },
    ];
    let (mut world, character) = staged((-1, -1), &walls);
    let report = world
        .clear_of_geometry(character, ANY, SKIN, 8)
        .expect("a live body");

    let clearance = clearance_raw(&world, character, &walls);
    assert!(
        clearance >= SKIN_RAW,
        "left {clearance} raw from a corner, inside the skin of {SKIN_RAW}"
    );
    assert_eq!(report.end, ClearEnd::Cleared);
    assert!(
        report.iterations >= 2,
        "a corner came out in {} push(es), so one of the two faces was never \
         violated and this is not the case it claims to be",
        report.iterations
    );
}

/// **Geometry with no room reports that it ran out**, rather than claiming
/// success or looping forever.
#[test]
fn a_gap_too_narrow_to_fit_reports_that_it_ran_out() {
    // A two-unit body in a slot barely two units wide: no position satisfies
    // both walls by the skin distance.
    let walls = [
        Wall {
            centre: (-2, 0),
            half: (1, 20),
        },
        Wall {
            centre: (2, 0),
            half: (1, 20),
        },
    ];
    let (mut world, character) = staged((0, 0), &walls);
    let report = world
        .clear_of_geometry(character, ANY, SKIN, 6)
        .expect("a live body");
    assert_eq!(report.end, ClearEnd::IterationsExhausted);
    assert_eq!(report.iterations, 6, "it spent the whole budget trying");
}

/// The mask is obeyed: geometry the caller is not asking about does not push.
#[test]
fn geometry_outside_the_mask_does_not_push() {
    let walls = [Wall {
        centre: (0, 0),
        half: (4, 4),
    }];
    let (mut world, character) = staged((0, 0), &walls);
    let report = world
        .clear_of_geometry(character, 0, SKIN, 8)
        .expect("a live body");
    assert_eq!(
        report.end,
        ClearEnd::AlreadyClear,
        "nothing was asked about"
    );
    assert_eq!(report.moved, Vec2::ZERO);
}

/// A body does not push itself out of its own shapes.
#[test]
fn a_body_is_not_inside_itself() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let character = entities.spawn();
    world.create_body(character, BodyKind::Kinematic, Transform::IDENTITY);
    // Two overlapping shapes on one body: parts of one object.
    world.add_shape(
        character,
        Shape::Box {
            half_extents: v(1, 1),
        },
        Transform::IDENTITY,
        Filter::new(1, 1),
    );
    world.add_shape(
        character,
        Shape::Box {
            half_extents: v(1, 1),
        },
        at(1, 0),
        Filter::new(1, 1),
    );
    let report = world
        .clear_of_geometry(character, ANY, SKIN, 8)
        .expect("a live body");
    assert_eq!(report.end, ClearEnd::AlreadyClear);
    assert_eq!(report.moved, Vec2::ZERO);
}

/// A handle naming no body answers with nothing rather than panicking.
#[test]
fn a_dead_handle_answers_with_nothing() {
    let mut world = World::new();
    let stranger = Entities::new().spawn();
    assert!(world.clear_of_geometry(stranger, ANY, SKIN, 8).is_none());
}

/// **The same situation clears the same way every time**, and the answer does
/// not depend on which order the geometry was created in.
///
/// Two equal deficits must resolve identically or the operation is not
/// reproducible, and creation order is exactly the thing that differs between
/// a level loaded from a file and one built by hand.
#[test]
fn clearing_is_reproducible_and_order_independent() {
    let a = Wall {
        centre: (-5, 0),
        half: (4, 20),
    };
    let b = Wall {
        centre: (0, -5),
        half: (20, 4),
    };

    let run = |walls: &[Wall]| {
        let (mut world, character) = staged((-1, -1), walls);
        world
            .clear_of_geometry(character, ANY, SKIN, 8)
            .expect("a live body")
            .destination
    };

    let first = run(&[a, b]);
    assert_eq!(first, run(&[a, b]), "the same world cleared differently");
    assert_eq!(
        first,
        run(&[b, a]),
        "the geometry's creation order changed where the body ended up"
    );
}

/// Circles are pushed out too — the operation is not box-only.
#[test]
fn a_circle_is_pushed_clear_as_well() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let ball = entities.spawn();
    world.create_body(ball, BodyKind::Kinematic, at(1, 0));
    world.add_shape(
        ball,
        Shape::Circle { radius: Fixed::ONE },
        Transform::IDENTITY,
        Filter::new(1, 1),
    );
    let wall = entities.spawn();
    world.create_body(wall, BodyKind::Static, Transform::IDENTITY);
    world.add_shape(
        wall,
        Shape::Box {
            half_extents: v(4, 4),
        },
        Transform::IDENTITY,
        Filter::new(1, 1),
    );

    let report = world
        .clear_of_geometry(ball, ANY, SKIN, 8)
        .expect("a live body");
    assert_eq!(report.end, ClearEnd::Cleared);

    let here = world.transform(ball).expect("a live body").translation;
    let gap = here.x.abs().to_bits() - Fixed::from_int(5).to_bits();
    assert!(
        gap >= SKIN_RAW,
        "the circle rests {gap} raw from the face, inside the skin of {SKIN_RAW}"
    );
}
