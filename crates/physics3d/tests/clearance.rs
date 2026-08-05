//! What a slide leaves between a body and the geometry that stopped it.
//!
//! **The same question the two-dimensional crate answered by measurement**, put
//! to this one rather than assumed to transfer. The shapes differ, the
//! separating axes differ, and the sweep is a different routine; the only thing
//! shared is the shape of the argument, which is not evidence.
//!
//! The clearance is read off box arithmetic done here, never from the engine,
//! so a slide that stops in the wrong place cannot also rule that the place was
//! fine.

use renew_ecs::{Entities, Entity};
use renew_fixed::{Fixed, Vec3};
use renew_physics3d::{
    BodyKind, ClearEnd, Collider, Filter, Shape, ShapeIndex, SlideHit, Transform, World,
};

fn v(x: i32, y: i32, z: i32) -> Vec3 {
    Vec3::new(Fixed::from_int(x), Fixed::from_int(y), Fixed::from_int(z))
}

fn at(x: i32, y: i32, z: i32) -> Transform {
    Transform::at(v(x, y, z))
}

const SKIN_RAW: i64 = 256;
const SKIN: Fixed = Fixed::from_bits(SKIN_RAW);
const TOLERANCE_RAW: i64 = 1;
const REQUIRED_RAW: i64 = SKIN_RAW - TOLERANCE_RAW;
const ANY: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Wall {
    centre: (i32, i32, i32),
    half: (i32, i32, i32),
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

/// The separation between two axis-aligned boxes, in raw units. Positive is a
/// gap, negative is penetration.
fn separation_raw(centre: Vec3, half: Vec3, wall: Wall) -> i64 {
    let wall_centre = v(wall.centre.0, wall.centre.1, wall.centre.2);
    let wall_half = v(wall.half.0, wall.half.1, wall.half.2);
    let gaps = [
        (centre.x - wall_centre.x).abs().to_bits() - (half.x + wall_half.x).to_bits(),
        (centre.y - wall_centre.y).abs().to_bits() - (half.y + wall_half.y).to_bits(),
        (centre.z - wall_centre.z).abs().to_bits() - (half.z + wall_half.z).to_bits(),
    ];
    if gaps.iter().all(|gap| *gap < 0) {
        // Overlapping on every axis: the shallowest is the way out.
        return gaps.iter().copied().max().unwrap_or(0);
    }
    let positive: Vec<i64> = gaps.iter().map(|gap| (*gap).max(0)).collect();
    positive.iter().map(|d| d * d).sum::<i64>().isqrt()
}

#[expect(
    clippy::expect_used,
    reason = "a test helper: the body is created in the same function, and a \
              panic here is the failure being reported"
)]
fn clearance_raw(world: &World, mover: Entity, walls: &[Wall]) -> i64 {
    let here = world.transform(mover).expect("a live body").translation;
    walls
        .iter()
        .map(|wall| separation_raw(here, v(1, 1, 1), *wall))
        .min()
        .unwrap_or(i64::MAX)
}

fn staged(start: (i32, i32, i32), walls: &[Wall]) -> (World, Entity) {
    let mut entities = Entities::new();
    let mut world = World::new();
    let mover = entities.spawn();
    world.create_body(mover, BodyKind::Kinematic, at(start.0, start.1, start.2));
    world.add_shape(
        mover,
        Shape::Box {
            half_extents: v(1, 1, 1),
        },
        Transform::IDENTITY,
        Filter::new(1, 1),
    );
    for wall in walls {
        let handle = entities.spawn();
        world.create_body(
            handle,
            BodyKind::Static,
            at(wall.centre.0, wall.centre.1, wall.centre.2),
        );
        world.add_shape(
            handle,
            Shape::Box {
                half_extents: v(wall.half.0, wall.half.1, wall.half.2),
            },
            Transform::IDENTITY,
            Filter::new(1, 1),
        );
    }
    (world, mover)
}

/// A room: floor, ceiling and four walls, with free space in the middle.
fn room() -> Vec<Wall> {
    vec![
        Wall {
            centre: (0, -21, 0),
            half: (400, 20, 400),
        },
        Wall {
            centre: (0, 40, 0),
            half: (400, 20, 400),
        },
        Wall {
            centre: (60, 0, 0),
            half: (20, 200, 400),
        },
        Wall {
            centre: (-60, 0, 0),
            half: (20, 200, 400),
        },
        Wall {
            centre: (0, 0, 60),
            half: (400, 200, 20),
        },
        Wall {
            centre: (0, 0, -60),
            half: (400, 200, 20),
        },
    ]
}

fn displacements() -> Vec<(Vec3, i64)> {
    let mut out = Vec::new();
    for length in [1i64, 5, 40, 400, 5000] {
        for (dx, dy, dz) in [
            (1i64, 0i64, 0i64),
            (0, -1, 0),
            (1, -1, 0),
            (1, -1, 1),
            (4, -1, 2),
            (-3, -2, 5),
            (7, -3, -1),
        ] {
            let (x, y, z) = (length * dx, length * dy, length * dz);
            let units = (x * x + y * y + z * z).isqrt() + 1;
            out.push((
                Vec3::new(
                    Fixed::from_int(i32::try_from(x).unwrap_or(0)),
                    Fixed::from_int(i32::try_from(y).unwrap_or(0)),
                    Fixed::from_int(i32::try_from(z).unwrap_or(0)),
                ),
                units,
            ));
        }
    }
    out
}

/// **The property, over a sweep wide enough to break it.**
///
/// Four iteration limits, four starting places, three skin distances and
/// thirty-five displacements. In two dimensions the equivalent sweep found a
/// shortfall that grew with the distance travelled, and the fix was to
/// re-establish the clearance at the end rather than to bound the drift.
#[test]
fn a_slide_never_ends_closer_than_the_skin_minus_the_tolerance() {
    let walls = room();
    let mut touched = 0u32;

    for limit in [1u32, 2, 4, 8] {
        for start in [(0, 2, 0), (-30, 10, 20), (30, 5, -20), (20, 15, 30)] {
            for skin_raw in [256i64, 4096, 65536] {
                for (displacement, units) in displacements() {
                    let (mut world, mover) = staged(start, &walls);
                    let mut hits = empty_hits();
                    world
                        .move_and_slide(
                            mover,
                            displacement,
                            ANY,
                            Fixed::from_bits(skin_raw),
                            limit,
                            &mut hits,
                        )
                        .expect("a live body");

                    let clearance = clearance_raw(&world, mover, &walls);
                    if clearance > 4 * skin_raw {
                        continue;
                    }
                    touched += 1;
                    assert!(
                        clearance >= skin_raw - TOLERANCE_RAW,
                        "a slide ended {clearance} raw from geometry against a skin of \
                         {skin_raw} — {units} units travelled, iteration limit {limit}, \
                         from {start:?}"
                    );
                }
            }
        }
    }

    assert!(
        touched > 200,
        "only {touched} configurations reached geometry; the sweep is not exercising the slide"
    );
}

/// **The distance travelled must not appear in the answer**, which is the
/// property the two-dimensional crate could only reach by restoring the
/// clearance rather than bounding the drift.
#[test]
fn the_clearance_does_not_depend_on_the_distance_travelled() {
    let walls = room();
    let mut readings = Vec::new();
    for length in [1i32, 4, 16, 64, 256, 1024, 4096] {
        let (mut world, mover) = staged((-35, 10, 0), &walls);
        let mut hits = empty_hits();
        world
            .move_and_slide(
                mover,
                Vec3::new(
                    Fixed::from_int(length * 3),
                    Fixed::from_int(-length),
                    Fixed::ZERO,
                ),
                ANY,
                SKIN,
                8,
                &mut hits,
            )
            .expect("a live body");
        readings.push((length, clearance_raw(&world, mover, &walls)));
    }

    let touching: Vec<(i32, i64)> = readings
        .iter()
        .copied()
        .filter(|(_, clearance)| *clearance < 4 * SKIN_RAW)
        .collect();
    assert!(
        touching.len() >= 4,
        "not enough runs reached the geometry to say anything: {readings:?}"
    );
    for (length, clearance) in &touching {
        assert!(
            *clearance >= REQUIRED_RAW,
            "a {length}-unit slide ended {clearance} raw away: {touching:?}"
        );
    }
    assert_eq!(
        touching.first().expect("checked above").1,
        touching.last().expect("checked above").1,
        "the distance travelled changed where the body came to rest: {touching:?}"
    );
}

/// A body that starts overlapping is pushed out before it moves.
#[test]
fn a_body_that_starts_inside_is_pushed_out_before_it_moves() {
    let walls = vec![Wall {
        centre: (0, 0, 0),
        half: (4, 4, 4),
    }];
    for start in [
        (0, 0, 0),
        (3, 0, 0),
        (0, 3, 0),
        (0, 0, 3),
        (4, 4, 4),
        (-3, 2, 1),
    ] {
        let (mut world, mover) = staged(start, &walls);
        let mut hits = empty_hits();
        world
            .move_and_slide(mover, v(1, 0, 0), ANY, SKIN, 8, &mut hits)
            .expect("a live body");
        let clearance = clearance_raw(&world, mover, &walls);
        assert!(
            clearance >= REQUIRED_RAW,
            "started inside at {start:?} and ended {clearance} raw away, \
             inside the required {REQUIRED_RAW}"
        );
    }
}

/// The clearing operation on its own: out of a box, from every side and from
/// dead centre, where the separating direction is a six-way tie.
#[test]
fn a_body_inside_a_box_is_pushed_clear_of_it() {
    let walls = [Wall {
        centre: (0, 0, 0),
        half: (4, 4, 4),
    }];
    for start in [(0, 0, 0), (3, 0, 0), (0, -3, 0), (0, 0, 3), (-2, 3, 1)] {
        let (mut world, mover) = staged(start, &walls);
        let report = world
            .clear_of_geometry(mover, ANY, SKIN, 8)
            .expect("a live body");
        let clearance = clearance_raw(&world, mover, &walls);
        assert!(
            clearance >= SKIN_RAW,
            "started at {start:?} and ended {clearance} raw away, inside the skin of {SKIN_RAW}"
        );
        assert_eq!(report.end, ClearEnd::Cleared, "from {start:?}");
    }
}

/// A body already clear is left where it is, and says which of the two answers
/// it is giving.
#[test]
fn a_body_already_clear_is_not_moved() {
    let walls = [Wall {
        centre: (0, 0, 0),
        half: (4, 4, 4),
    }];
    let (mut world, mover) = staged((20, 0, 0), &walls);
    let before = world.transform(mover).expect("a live body").translation;
    let report = world
        .clear_of_geometry(mover, ANY, SKIN, 8)
        .expect("a live body");
    assert_eq!(report.end, ClearEnd::AlreadyClear);
    assert_eq!(report.moved, Vec3::ZERO);
    assert_eq!(
        world.transform(mover).expect("a live body").translation,
        before
    );
}

/// **A corner in three dimensions needs more than one push**, and the third
/// axis is the reason this is not simply the two-dimensional case again.
#[test]
fn a_three_way_corner_takes_more_than_one_push() {
    let walls = [
        Wall {
            centre: (-5, 0, 0),
            half: (4, 20, 20),
        },
        Wall {
            centre: (0, -5, 0),
            half: (20, 4, 20),
        },
        Wall {
            centre: (0, 0, -5),
            half: (20, 20, 4),
        },
    ];
    let (mut world, mover) = staged((-1, -1, -1), &walls);
    let report = world
        .clear_of_geometry(mover, ANY, SKIN, 8)
        .expect("a live body");

    let clearance = clearance_raw(&world, mover, &walls);
    assert!(
        clearance >= SKIN_RAW,
        "left {clearance} raw from a three-way corner, inside the skin of {SKIN_RAW}"
    );
    assert_eq!(report.end, ClearEnd::Cleared);
    assert!(
        report.iterations >= 2,
        "a three-way corner came out in {} push(es)",
        report.iterations
    );
}

/// The same situation clears the same way, whatever order the geometry was
/// created in.
#[test]
fn clearing_is_reproducible_and_order_independent() {
    let a = Wall {
        centre: (-5, 0, 0),
        half: (4, 20, 20),
    };
    let b = Wall {
        centre: (0, -5, 0),
        half: (20, 4, 20),
    };
    let run = |walls: &[Wall]| {
        let (mut world, mover) = staged((-1, -1, 0), walls);
        world
            .clear_of_geometry(mover, ANY, SKIN, 8)
            .expect("a live body")
            .destination
    };
    let first = run(&[a, b]);
    assert_eq!(first, run(&[a, b]));
    assert_eq!(
        first,
        run(&[b, a]),
        "the geometry's creation order changed where the body ended up"
    );
}

/// The separation arithmetic this file relies on, checked against cases whose
/// answers are obvious. The oracle needs its own oracle.
#[test]
fn the_separation_arithmetic_is_right() {
    let wall = Wall {
        centre: (0, 0, 0),
        half: (2, 2, 2),
    };
    let half = v(1, 1, 1);
    let one = Fixed::ONE.to_bits();

    assert_eq!(separation_raw(v(3, 0, 0), half, wall), 0);
    assert_eq!(separation_raw(v(4, 0, 0), half, wall), one);
    assert_eq!(separation_raw(v(0, 0, 4), half, wall), one);
    assert_eq!(separation_raw(v(2, 0, 0), half, wall), -one);
    // Deep inside: the shallowest axis wins, being the way out.
    assert_eq!(separation_raw(v(0, 0, 0), half, wall), -3 * one);
    // A three-way corner gap of one unit each way is the space diagonal.
    let corner = separation_raw(v(4, 4, 4), half, wall);
    assert!(
        corner > one && corner < 2 * one,
        "a corner gap of one unit on three axes should be about 1.73 units, was {corner} raw"
    );
}
