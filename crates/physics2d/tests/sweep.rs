//! Moving a shape and finding what it meets first.

use proptest::prelude::*;
use renew_fixed::{Fixed, Vec2};
use renew_physics2d::{Shape, Transform, narrow::separation, sweep};

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

fn at(x: i32, y: i32) -> Transform {
    Transform::at(v(x, y))
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

/// A skin of one part in 256 — small against the shapes here, large enough to
/// be measurable.
const SKIN: Fixed = Fixed::from_bits(256);

#[test]
fn a_circle_moving_at_a_circle_stops_before_it() {
    // From x = −5 moving +10; a unit circle meets another unit circle at x=0
    // when their centres are 2 apart, so at x = −2, which is 3/10 of the way.
    let hit =
        sweep(circle(1), at(-5, 0), v(10, 0), circle(1), at(0, 0), SKIN).expect("straight at it");
    let expected = Fixed::from_ratio(3, 10);
    assert!(
        (hit.time - expected).to_bits().abs() <= 512,
        "stopped at {} of the way, expected about {}",
        hit.time.to_bits(),
        expected.to_bits()
    );
    // And it really is short of contact.
    let (gap, _) = separation(circle(1), Transform::at(hit.origin), circle(1), at(0, 0))
        .expect("both are circles");
    assert!(gap >= Fixed::ZERO, "the sweep must not end overlapping");
}

#[test]
fn a_box_landing_on_a_floor_stops_on_top_of_it() {
    let floor = Shape::Box {
        half_extents: v(20, 1),
    };
    // A unit box falling from y = 10 onto a floor whose top is y = 1: it
    // should stop with its centre near y = 2.
    let hit = sweep(square(1), at(0, 10), v(0, -20), floor, at(0, 0), SKIN)
        .expect("it falls onto the floor");
    let resting_y = hit.origin.y;
    assert!(
        (resting_y - Fixed::from_int(2)).to_bits().abs() <= 1024,
        "came to rest at y = {}, expected about 2",
        resting_y.to_bits()
    );
    // The normal points back at the mover: up.
    assert!(hit.normal.y > Fixed::ZERO, "the floor pushes up");
}

#[test]
fn a_shape_moving_away_meets_nothing() {
    assert!(
        sweep(circle(1), at(-5, 0), v(-10, 0), circle(1), at(0, 0), SKIN).is_none(),
        "travelling away from it"
    );
}

#[test]
fn a_shape_moving_past_meets_nothing() {
    // Along +x at y = 5, well above a unit circle at the origin.
    assert!(sweep(circle(1), at(-5, 5), v(10, 0), circle(1), at(0, 0), SKIN).is_none());
}

#[test]
fn a_shape_that_starts_overlapping_reports_zero() {
    let hit =
        sweep(circle(1), at(0, 0), v(10, 0), circle(1), at(1, 0), SKIN).expect("already inside");
    assert_eq!(hit.time, Fixed::ZERO, "no travel before contact");
    assert_eq!(hit.origin, Vec2::ZERO);
}

#[test]
fn a_zero_displacement_sweep_answers_rather_than_refusing() {
    // Touching already: a hit at zero.
    assert!(sweep(circle(1), at(0, 0), Vec2::ZERO, circle(1), at(1, 0), SKIN).is_some());
    // Apart, and going nowhere: nothing.
    assert!(sweep(circle(1), at(0, 0), Vec2::ZERO, circle(1), at(9, 0), SKIN).is_none());
}

#[test]
fn a_capsule_cannot_be_swept_yet() {
    let capsule = Shape::Capsule {
        radius: Fixed::ONE,
        half_height: Fixed::ONE,
    };
    assert!(sweep(capsule, at(-5, 0), v(10, 0), square(1), at(0, 0), SKIN).is_none());
    assert!(sweep(square(1), at(-5, 0), v(10, 0), capsule, at(0, 0), SKIN).is_none());
}

/// **The property the whole approach exists for.** A small shape crossing a
/// thin wall at speed is exactly what a per-step overlap test misses: at the
/// start of the step it is on one side, at the end it is on the other, and
/// nothing ever reports a touch.
///
/// Conservative advancement cannot do that, because every step is bounded by a
/// *lower bound* on the remaining distance — it can approach the true time of
/// impact but never pass it.
#[test]
fn a_fast_small_shape_cannot_tunnel_through_a_thin_wall() {
    let bullet = Shape::Circle {
        radius: Fixed::from_ratio(1, 16),
    };
    let wall = Shape::Box {
        half_extents: Vec2::new(Fixed::from_ratio(1, 8), Fixed::from_int(10)),
    };

    for speed in [50i32, 200, 1000, 5000] {
        let start = at(-10, 0);
        let displacement = Vec2::new(Fixed::from_int(speed), Fixed::ZERO);
        let hit = sweep(bullet, start, displacement, wall, at(0, 0), SKIN)
            .unwrap_or_else(|| panic!("a bullet at {speed} units per step tunnelled the wall"));
        assert!(hit.time >= Fixed::ZERO, "time is a fraction of the step");
        assert!(hit.time <= Fixed::ONE, "and never past its end");
        // It stopped on the near side, not somewhere past the wall.
        assert!(
            hit.origin.x < Fixed::ZERO,
            "at speed {speed} it stopped at x = {}, which is past the wall",
            hit.origin.x.to_bits()
        );
    }
}

/// A displacement that does not quite reach is not a hit, and one that just
/// reaches is — the boundary between them is where an off-by-one lives.
#[test]
fn a_sweep_that_falls_short_reports_nothing() {
    // Surfaces meet when the centres are 2 apart, so from x = −5 the gap is 3.
    assert!(
        sweep(circle(1), at(-5, 0), v(2, 0), circle(1), at(0, 0), SKIN).is_none(),
        "two units of travel across a three-unit gap"
    );
    assert!(
        sweep(circle(1), at(-5, 0), v(4, 0), circle(1), at(0, 0), SKIN).is_some(),
        "four units closes it"
    );
}

proptest! {
    /// However far a sweep travels, it never ends up overlapping what it hit.
    /// That is the guarantee a caller builds movement on: a body placed at the
    /// reported origin is outside the thing that stopped it.
    #[test]
    fn a_sweep_never_ends_inside_what_it_hit(
        start_x in -12i64..-2,
        travel in 1i64..40,
        target_y in -2i64..3,
        moving_is_box in prop::bool::ANY,
    ) {
        let moving = if moving_is_box { square(1) } else { circle(1) };
        let target = square(1);
        let from = Transform::at(Vec2::new(Fixed::from_int(
            i32::try_from(start_x).unwrap_or(-5)), Fixed::ZERO));
        let displacement = Vec2::new(
            Fixed::from_int(i32::try_from(travel).unwrap_or(1)),
            Fixed::ZERO,
        );
        let target_at = Transform::at(Vec2::new(
            Fixed::ZERO,
            Fixed::from_int(i32::try_from(target_y).unwrap_or(0)),
        ));

        let Some(hit) = sweep(moving, from, displacement, target, target_at, SKIN) else {
            return Ok(());
        };
        prop_assert!(hit.time >= Fixed::ZERO && hit.time <= Fixed::ONE, "time is a fraction");

        let (gap, _) = separation(
            moving,
            Transform::at(hit.origin),
            target,
            target_at,
        ).expect("both shapes are supported");
        // A raw unit or two of slack for the rounding in the advance.
        prop_assert!(
            gap.to_bits() >= -64,
            "ended {} raw units inside what it hit",
            -gap.to_bits()
        );
    }

    /// The reported origin lies on the path, at the reported fraction of it.
    /// A hit whose position and time disagree would move a body somewhere the
    /// sweep never said it went.
    #[test]
    fn a_sweep_origin_lies_on_its_path(
        travel in 1i64..30, target_y in -2i64..3,
    ) {
        let from = at(-8, 0);
        let displacement = Vec2::new(
            Fixed::from_int(i32::try_from(travel).unwrap_or(1)),
            Fixed::ZERO,
        );
        let target_at = Transform::at(Vec2::new(
            Fixed::ZERO,
            Fixed::from_int(i32::try_from(target_y).unwrap_or(0)),
        ));
        if let Some(hit) = sweep(circle(1), from, displacement, square(1), target_at, SKIN) {
            let expected = from.translation + displacement * hit.time;
            let gap_x = (hit.origin.x - expected.x).to_bits().abs();
            let gap_y = (hit.origin.y - expected.y).to_bits().abs();
            prop_assert!(gap_x <= 64 && gap_y <= 64, "the origin is off the path");
        }
    }
}

/// **A box sweeping at a circle**, which is the same geometry the other way
/// round and a separate arm of the separation query. Every other sweep here
/// moves the circle, so without this the box-first arm has never run.
#[test]
fn a_box_can_sweep_at_a_circle_too() {
    let hit =
        sweep(square(1), at(-5, 0), v(10, 0), circle(1), at(0, 0), SKIN).expect("straight at it");
    // Surfaces meet when the centres are 2 apart, so it stops near x = −2.
    assert!(
        (hit.origin.x - Fixed::from_int(-2)).to_bits().abs() <= 1024,
        "stopped at x = {}, expected about −2",
        hit.origin.x.to_bits()
    );
    assert!(
        hit.normal.x < Fixed::ZERO,
        "the circle pushes back along −x"
    );

    // And it does not end up inside.
    let (gap, _) = separation(square(1), Transform::at(hit.origin), circle(1), at(0, 0))
        .expect("both shapes are supported");
    assert!(
        gap.to_bits() >= -64,
        "ended inside by {} raw",
        -gap.to_bits()
    );

    // Moving away finds nothing, exercising the same arm's rejection.
    assert!(sweep(square(1), at(-5, 0), v(-10, 0), circle(1), at(0, 0), SKIN).is_none());
}
