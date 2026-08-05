//! **The open question, answered by measurement.**
//!
//! The vocabulary states an obligation with no mechanism behind it: a slide
//! ends with the body no closer to anything than the skin distance minus the
//! contact tolerance. Two prose mechanisms were invented to guarantee that and
//! both were wrong — the second is refuted by a test in `renew-fixed`, because
//! the proportional term it tried to bound is scale-invariant under cutting.
//! The written conclusion was that an implementation must answer it with
//! running code instead. This is that code, and it has done both halves of the
//! job: it measured a shortfall proportional to the distance travelled, and
//! then — once a clearance-restoring step existed to answer it — it measured
//! the shortfall away. **What these tests assert changed when the behaviour
//! did**, because each was pinned to a number and the numbers moved.
//!
//! **The clearance is measured, not asked for.** Every number below comes from
//! axis-aligned box arithmetic done in this file, over the positions the slide
//! actually returned. Nothing here calls the distance code that the slide
//! itself uses, so a slide that stops in the wrong place cannot also decide
//! that the place was fine.

use renew_ecs::{Entities, Entity};
use renew_fixed::{Fixed, Vec2};
use renew_physics2d::{BodyKind, Collider, Filter, Shape, ShapeIndex, SlideHit, Transform, World};

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

fn at(x: i32, y: i32) -> Transform {
    Transform::at(v(x, y))
}

/// The skin the slide is asked to keep, in raw units.
const SKIN_RAW: i64 = 64;
/// The contact tolerance the vocabulary pairs it with, in raw units.
const TOLERANCE_RAW: i64 = 1;
/// What the vocabulary says the body must never be closer than.
const REQUIRED_RAW: i64 = SKIN_RAW - TOLERANCE_RAW;

const SKIN: Fixed = Fixed::from_bits(SKIN_RAW);
const ANY: u32 = u32::MAX;

/// A static box in the world: centre and half-extents.
#[derive(Clone, Copy)]
struct Wall {
    centre: (i32, i32),
    half: (i32, i32),
}

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

/// The separation between two axis-aligned boxes, in raw units.
///
/// Positive is a gap and negative is penetration. **Computed here from the
/// centres and half-extents rather than by asking the engine**, so this is an
/// independent reading of where the slide left the body.
fn separation_raw(centre: Vec2, half: Vec2, wall: Wall) -> i64 {
    let wall_centre = v(wall.centre.0, wall.centre.1);
    let wall_half = v(wall.half.0, wall.half.1);
    let gap_x = (centre.x - wall_centre.x).abs().to_bits() - (half.x + wall_half.x).to_bits();
    let gap_y = (centre.y - wall_centre.y).abs().to_bits() - (half.y + wall_half.y).to_bits();

    if gap_x < 0 && gap_y < 0 {
        // Overlapping: the shallower axis is the penetration depth.
        return gap_x.max(gap_y);
    }
    // Separated on at least one axis: the true distance is the length of the
    // positive part, which for boxes is the corner-to-corner case when both
    // are positive.
    let dx = gap_x.max(0);
    let dy = gap_y.max(0);
    (dx * dx + dy * dy).isqrt()
}

/// The closest anything is to the body, in raw units.
#[expect(
    clippy::expect_used,
    reason = "a test helper: the body is created two lines away, and a panic               here is the failure being reported"
)]
fn clearance_raw(world: &World, character: Entity, walls: &[Wall]) -> i64 {
    let here = world.transform(character).expect("a live body").translation;
    walls
        .iter()
        .map(|wall| separation_raw(here, v(1, 1), *wall))
        .min()
        .unwrap_or(i64::MAX)
}

/// A corridor: floor, ceiling and a wall to run into. Deliberately a corner
/// rather than a plane, because a corner is where two constraints must both
/// hold and where a slide iterates.
fn corridor() -> Vec<Wall> {
    vec![
        Wall {
            centre: (0, -21),
            half: (400, 20),
        },
        Wall {
            centre: (0, 40),
            half: (400, 20),
        },
        Wall {
            centre: (60, 0),
            half: (20, 200),
        },
        Wall {
            centre: (-60, 0),
            half: (20, 200),
        },
    ]
}

/// Every displacement, paired with its length in whole units rounded up.
///
/// **The length is computed from the integers the displacement was built
/// from**, not from the vector, so the bound below is not stated in terms of
/// a number the code under test produced.
#[expect(
    clippy::expect_used,
    reason = "a test helper over literal constants: the conversions cannot fail               for the values written below, and a panic would be the failure"
)]
fn displacements_with_length() -> Vec<(Vec2, i64)> {
    let mut out = Vec::new();
    for length in [1i64, 2, 5, 13, 40, 100, 400, 1000, 5000] {
        for (dx, dy) in [
            (1i64, 0i64),
            (0, -1),
            (1, -1),
            (4, -1),
            (1, -4),
            (-1, -1),
            (7, -3),
            (-5, -2),
        ] {
            let (x, y) = (length * dx, length * dy);
            let units = (x * x + y * y).isqrt() + 1;
            out.push((
                Vec2::new(
                    Fixed::from_int(i32::try_from(x).expect("small")),
                    Fixed::from_int(i32::try_from(y).expect("small")),
                ),
                units,
            ));
        }
    }
    out
}

/// **The property, now that it holds.**
///
/// Before the clearance-restoring step existed this assertion was impossible:
/// the shortfall grew with the distance travelled and a five-hundred-unit slide
/// ended ninety-six raw units inside the wall. The sweep below is the one that
/// measured that, unchanged — five iteration limits, five starting places, four
/// skin distances and seventy-two displacements — and it now passes.
///
/// The clearance is read off box arithmetic done in this file, so a slide that
/// stops in the wrong place cannot also rule that the place was fine.
#[test]
fn a_slide_never_ends_closer_than_the_skin_minus_the_tolerance() {
    let walls = corridor();
    let mut worst = i64::MAX;
    let mut worst_case = (0i64, 0i64, 0i64);
    let mut touched = 0u32;

    for limit in [1u32, 2, 4, 8, 16] {
        for start in [(0, 2), (-30, 10), (30, 5), (20, 15), (-20, 3)] {
            for skin_raw in [64i64, 256, 4096, 65536] {
                for (displacement, units) in displacements_with_length() {
                    let (mut world, character) = staged(start, &walls);
                    let mut hits = empty_hits();
                    world
                        .move_and_slide(
                            character,
                            displacement,
                            ANY,
                            Fixed::from_bits(skin_raw),
                            limit,
                            &mut hits,
                        )
                        .expect("a live body");

                    let clearance = clearance_raw(&world, character, &walls);
                    // Configurations that never reached anything say nothing
                    // about clearance, and counting them would let this pass
                    // by running in empty space.
                    if clearance > 4 * skin_raw {
                        continue;
                    }
                    touched += 1;

                    let shortfall = skin_raw - clearance;
                    if shortfall > i64::MIN && clearance < worst {
                        worst = clearance;
                        worst_case = (units, skin_raw, i64::from(limit));
                    }
                    assert!(
                        clearance >= skin_raw - TOLERANCE_RAW,
                        "a slide ended {clearance} raw from geometry against a skin of \
                         {skin_raw} — {units} units travelled, iteration limit {limit}"
                    );
                }
            }
        }
    }

    assert!(
        touched > 500,
        "only {touched} configurations reached geometry; the sweep is not exercising the slide"
    );
    // The worst case is worth naming even when it passes, because a sweep that
    // stopped reaching the interesting configurations would still pass.
    assert!(
        worst < i64::MAX,
        "nothing was measured at all: {worst_case:?}"
    );
}

/// **The clearance no longer decays with the distance travelled**, which is the
/// question four prose mechanisms failed to settle and one measurement did.
///
/// The shortfall used to run at about one raw unit per hundred and thirty-one
/// thousand raw units of travel, so a long slide ended measurably deeper than a
/// short one. Restoring the clearance at the end of the operation removes the
/// dependence rather than bounding it: the distance travelled no longer appears
/// in the answer.
#[test]
fn the_clearance_does_not_decay_with_the_distance_travelled() {
    let walls = corridor();
    let mut readings = Vec::new();

    for length in [1i32, 4, 16, 64, 256, 1024, 4096] {
        let (mut world, character) = staged((-35, 10), &walls);
        let mut hits = empty_hits();
        world
            .move_and_slide(
                character,
                Vec2::new(Fixed::from_int(length * 3), Fixed::from_int(-length)),
                ANY,
                SKIN,
                8,
                &mut hits,
            )
            .expect("a live body");
        readings.push((length, clearance_raw(&world, character, &walls)));
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

    // **The distance must not appear in the answer at all**, not merely be
    // bounded: the shortest and the longest run must agree exactly.
    let shortest = touching.first().expect("checked above").1;
    let longest = touching.last().expect("checked above").1;
    assert_eq!(
        shortest, longest,
        "four thousand units of travel ended somewhere different from four: {touching:?}"
    );
}

/// A five-hundred-unit slide — the case that used to end ninety-six raw units
/// inside the wall.
///
/// **Kept as a named case rather than folded into the sweep**, because it is
/// the one a person can check by hand and the one the write-up quotes.
#[test]
fn the_case_that_used_to_end_inside_the_wall_no_longer_does() {
    let walls = corridor();
    let (mut world, character) = staged((0, 2), &walls);
    let mut hits = empty_hits();
    world
        .move_and_slide(character, v(500, 0), ANY, SKIN, 8, &mut hits)
        .expect("a live body");

    let clearance = clearance_raw(&world, character, &walls);
    assert_eq!(
        clearance, SKIN_RAW,
        "it rests {clearance} raw from the wall; it used to be -96, and the skin is {SKIN_RAW}"
    );
}

/// A short slide meets it too, which it always did.
#[test]
fn a_short_slide_meets_the_specified_clearance() {
    let walls = corridor();
    let (mut world, character) = staged((30, 2), &walls);
    let mut hits = empty_hits();
    world
        .move_and_slide(character, v(20, 0), ANY, SKIN, 8, &mut hits)
        .expect("a live body");

    let clearance = clearance_raw(&world, character, &walls);
    assert!(
        clearance >= REQUIRED_RAW,
        "a twenty-unit slide fell short: {clearance} raw against {REQUIRED_RAW}"
    );
    assert!(
        clearance <= 4 * SKIN_RAW,
        "the body never reached the wall, so this proves nothing: {clearance} raw"
    );
}

/// **A body that starts overlapping is pushed out before it moves**, which it
/// was not until the clearing operation existed.
#[test]
fn a_body_that_starts_inside_is_pushed_out_before_it_moves() {
    let walls = vec![Wall {
        centre: (0, 0),
        half: (4, 4),
    }];
    for start in [(0, 0), (3, 0), (0, 3), (4, 4), (-3, 2)] {
        let (mut world, character) = staged(start, &walls);
        let mut hits = empty_hits();
        world
            .move_and_slide(character, v(1, 0), ANY, SKIN, 8, &mut hits)
            .expect("a live body");
        let clearance = clearance_raw(&world, character, &walls);
        assert!(
            clearance >= REQUIRED_RAW,
            "started inside at {start:?} and ended {clearance} raw away, \
             inside the required {REQUIRED_RAW}"
        );
    }
}

/// The separation arithmetic this file relies on, checked against cases whose
/// answers are obvious.
///
/// **The oracle needs its own oracle.** Every number in this file is only as
/// good as this function, and it is the one piece of arithmetic here that
/// nothing else checks.
#[test]
fn the_separation_arithmetic_is_right() {
    let wall = Wall {
        centre: (0, 0),
        half: (2, 2),
    };
    let half = v(1, 1);
    let one = Fixed::ONE.to_bits();

    // Face to face, three units apart along x: 3 - (1 + 2) = 0.
    assert_eq!(separation_raw(v(3, 0), half, wall), 0);
    // A unit further out.
    assert_eq!(separation_raw(v(4, 0), half, wall), one);
    // Overlapping by one unit.
    assert_eq!(separation_raw(v(2, 0), half, wall), -one);
    // Diagonally clear by one unit each way: the distance is the diagonal.
    let diagonal = separation_raw(v(4, 4), half, wall);
    assert!(
        diagonal > one && diagonal < 2 * one,
        "a corner gap of one unit each way should be about 1.41 units, was {diagonal} raw"
    );
    // Deep inside: the shallower axis wins, being the way out.
    assert_eq!(separation_raw(v(0, 0), half, wall), -3 * one);
}
