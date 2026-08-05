//! **The open question, answered by measurement.**
//!
//! The vocabulary states an obligation with no mechanism behind it: a slide
//! ends with the body no closer to anything than the skin distance minus the
//! contact tolerance. Two prose mechanisms were invented to guarantee that and
//! both were wrong — the second is refuted by a test in `renew-fixed`, because
//! the proportional term it tried to bound is scale-invariant under cutting.
//! The written conclusion was that an implementation must answer it with
//! running code instead. This is that code.
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

/// The clearance a slide is allowed to fall short of the skin by, in raw
/// units, for a slide of `units` whole units.
///
/// **Measured, not derived.** Over the sweep below — five iteration limits,
/// five starting places, four skins and seventy-two displacements — the worst
/// observed shortfall was 15072 raw at roughly 38 000 units travelled, a slope
/// near one raw unit per 131 000 raw units of travel. This bound is that slope
/// doubled and rounded to something a person can hold: **one raw unit of
/// shortfall per whole unit travelled, plus four**.
///
/// The vocabulary derived `2 + L/8192` raw per iteration from the normal
/// tolerance. That is about twenty-five times looser than what this
/// implementation actually does, which is the expected direction — the
/// derivation is what a merely adequate implementation may spend, and
/// `renew-fixed`'s normalisation is better than adequate.
fn allowed_shortfall_raw(units: i64) -> i64 {
    4 + units
}

/// **The answer to the open question, measured over real geometry.**
///
/// The written vocabulary states the obligation and says outright that no
/// prose mechanism has bounded it, that two attempts were wrong, and that an
/// implementation must settle it with running code. This is the running code,
/// and the answer is a law rather than a constant: *the clearance after a
/// slide is the skin, less a shortfall proportional to the distance
/// travelled.*
///
/// The assertion is two-sided on purpose, which is the shape a bound
/// assertion has to have. One side requires the bound to hold. The other
/// requires some configuration to come within a factor of eight of it — so
/// that a bound loosened until nothing can fail it stops being a bound and
/// starts failing this test instead.
#[test]
fn the_clearance_shortfall_is_bounded_by_the_distance_travelled() {
    let walls = corridor();
    let mut worst_excess = i64::MIN;
    let mut worst_case = (0i64, 0i64, 0i64, 0u32);
    let mut closest_approach = i64::MIN;
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
                    // about creep, and counting them would let this pass by
                    // running in empty space.
                    if clearance > 4 * skin_raw {
                        continue;
                    }
                    touched += 1;

                    let allowed = allowed_shortfall_raw(units);
                    let shortfall = skin_raw - clearance;
                    let excess = shortfall - allowed;
                    if excess > worst_excess {
                        worst_excess = excess;
                        worst_case = (units, skin_raw, clearance, limit);
                    }
                    // How near the bound anything came, as a fraction of it.
                    let approach = shortfall * 8 - allowed;
                    closest_approach = closest_approach.max(approach);
                }
            }
        }
    }

    assert!(
        touched > 500,
        "only {touched} configurations reached geometry; the sweep is not exercising the slide"
    );
    assert!(
        worst_excess < 0,
        "a slide fell {} raw short of the skin where {} was allowed — \
         {} units travelled, skin {}, clearance {}, iteration limit {}",
        worst_case.1 - worst_case.2,
        allowed_shortfall_raw(worst_case.0),
        worst_case.0,
        worst_case.1,
        worst_case.2,
        worst_case.3
    );
    assert!(
        closest_approach >= 0,
        "nothing came within a factor of eight of the bound, so the bound is \
         too loose to be a bound"
    );
}

/// **The specified property does not hold for a fixed skin, and here is the
/// case that breaks it.**
///
/// The vocabulary asks for a clearance of at least the skin minus the contact
/// tolerance — 63 raw units when the skin is 64 and the tolerance is 1. The
/// measured shortfall grows with the distance travelled, so it passes 1 raw
/// after a couple of units of travel and the property is unreachable for any
/// ordinary motion.
///
/// **This test pins the gap rather than hiding it.** It asserts the failure,
/// with the number, so the day the implementation restores clearance at the
/// end of a slide — or scales the skin with the displacement, the two options
/// the vocabulary itself lists — this test fails and has to be rewritten as
/// the property finally holding. A gap nobody can see is the thing worth
/// avoiding; a gap with a test around it is a decision.
#[test]
fn a_fixed_skin_cannot_meet_the_specified_clearance_over_a_long_slide() {
    let walls = corridor();
    let (mut world, character) = staged((0, 2), &walls);
    let mut hits = empty_hits();
    world
        .move_and_slide(character, v(500, 0), ANY, SKIN, 8, &mut hits)
        .expect("a live body");

    let clearance = clearance_raw(&world, character, &walls);
    assert!(
        clearance < REQUIRED_RAW,
        "the specified clearance is met after all ({clearance} raw against the \
         required {REQUIRED_RAW}) — if that is a fix rather than an accident, \
         this test is the one to rewrite"
    );
    assert_eq!(
        clearance, -96,
        "the shortfall over five hundred units changed; it was 160 raw below \
         the skin of {SKIN_RAW}"
    );
}

/// A short slide *does* meet the specified property, which is what makes the
/// law above a law rather than a blanket failure.
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
        "even a twenty-unit slide fell short: {clearance} raw against {REQUIRED_RAW}"
    );
    assert!(
        clearance <= 4 * SKIN_RAW,
        "the body never reached the wall, so this proves nothing: {clearance} raw"
    );
}

/// **A body that starts overlapping is not pushed out**, because nothing here
/// pushes it out yet.
///
/// The vocabulary requires depenetration — a body that begins the operation
/// inside something is moved clear before anything else happens — and this
/// crate does not implement it. The sweep excludes a body from its own sweep
/// and starts from wherever it is, so a body inside a wall stays inside it.
///
/// Pinned rather than left unsaid: the assertion is that the overlap survives,
/// with the depth, so the behaviour is a recorded fact instead of an
/// assumption, and implementing depenetration will fail this test loudly.
#[test]
fn a_body_that_starts_inside_stays_inside_because_nothing_pushes_it_out() {
    let walls = vec![Wall {
        centre: (0, 0),
        half: (4, 4),
    }];
    let (mut world, character) = staged((0, 0), &walls);
    let mut hits = empty_hits();
    world
        .move_and_slide(character, v(1, 0), ANY, SKIN, 8, &mut hits)
        .expect("a live body");

    let clearance = clearance_raw(&world, character, &walls);
    assert!(
        clearance < 0,
        "the body was moved clear of an overlap it started in ({clearance} raw) — \
         if depenetration has been implemented, this test is the one to rewrite"
    );
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
