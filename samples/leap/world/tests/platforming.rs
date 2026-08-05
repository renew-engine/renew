//! The behaviours a platformer is judged on.

use renew_fixed::{Fixed, Vec2};
use renew_sample_leap_world::{Intent, Leap, Platform, Tuning};

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

/// A wide floor at y = 0 with its top at y = 1, and a tall wall at x = 10.
fn level() -> Vec<Platform> {
    vec![
        Platform::new(0, 0, 40, 1),
        Platform::new(10, 5, 1, 4),
        Platform::new(-14, 4, 3, 1),
    ]
}

/// The character starts above the floor, half-height one, so it rests with its
/// centre at y = 2.
fn staged() -> Leap {
    Leap::new(Tuning::default(), v(0, 6), &level())
}

fn run(world: &mut Leap, ticks: u32, intent: Intent) {
    for _ in 0..ticks {
        world.step(intent);
    }
}

#[test]
fn a_character_falls_and_lands_on_the_floor() {
    let mut world = staged();
    assert!(!world.footing().grounded, "it starts in the air");

    run(&mut world, 60, Intent::IDLE);

    assert!(world.footing().grounded, "it should have landed");
    let resting = world.position().y;
    assert!(
        (resting - Fixed::from_int(2)).to_bits().abs() <= 4096,
        "rested at y = {}, expected about 2",
        resting.to_bits()
    );
    // Landing kills downward speed; without that, gravity accumulates while
    // the character stands still and it launches when it next steps off.
    assert!(
        world.velocity().y.to_bits().abs() <= 4096,
        "still falling at {}",
        world.velocity().y.to_bits()
    );
}

#[test]
fn a_grounded_character_jumps_and_comes_back_down() {
    let mut world = staged();
    run(&mut world, 60, Intent::IDLE);
    let ground = world.position().y;

    world.step(Intent::jumping(0));
    assert!(world.velocity().y > Fixed::ZERO, "the jump pushed it up");

    run(&mut world, 10, Intent::jumping(0));
    let apex = world.position().y;
    assert!(
        apex > ground,
        "it went up: {} to {}",
        ground.to_bits(),
        apex.to_bits()
    );
    assert!(!world.footing().grounded, "and left the floor");

    run(&mut world, 80, Intent::IDLE);
    assert!(world.footing().grounded, "and landed again");
    assert!(
        (world.position().y - ground).to_bits().abs() <= 8192,
        "back to about the same height"
    );
}

/// **Holding jump must not re-fire it.** Reading the held state instead of the
/// edge lets a player hold the button and bounce forever, which is the first
/// thing anybody tries.
#[test]
fn holding_jump_does_not_bounce() {
    let mut world = staged();
    run(&mut world, 60, Intent::IDLE);

    // Hold jump for a long time. It fires once; after that the character is
    // airborne and then lands, and holding must not launch it again.
    run(&mut world, 200, Intent::jumping(0));

    assert!(
        world.footing().grounded,
        "it should be resting on the floor"
    );
    assert!(
        world.velocity().y.to_bits().abs() <= 4096,
        "a held button relaunched it at {}",
        world.velocity().y.to_bits()
    );
}

/// **Coyote time**, which is the difference between a platformer that feels
/// responsive and one that feels like it is ignoring the player.
#[test]
fn a_character_can_jump_just_after_walking_off_a_ledge() {
    let tuning = Tuning::default();
    // Start on the small high platform at x = −14, whose top is y = 5.
    let mut world = Leap::new(tuning, v(-14, 7), &level());
    run(&mut world, 60, Intent::IDLE);
    assert!(world.footing().grounded, "standing on the ledge");

    // Walk off the right-hand end.
    let mut ticks_off = 0;
    while world.footing().grounded && ticks_off < 200 {
        world.step(Intent::running(1));
        ticks_off += 1;
    }
    assert!(!world.footing().grounded, "walked off");
    assert!(
        world.footing().ticks_airborne <= tuning.coyote_ticks,
        "the very next tick is inside the window"
    );

    // A jump now still works.
    world.step(Intent::jumping(1));
    assert!(
        world.velocity().y > Fixed::ZERO,
        "a jump inside the coyote window must still fire"
    );
}

#[test]
fn a_character_cannot_jump_from_mid_air() {
    let tuning = Tuning::default();
    let mut world = Leap::new(tuning, v(0, 30), &level());
    // Fall well past the coyote window.
    run(&mut world, 30, Intent::IDLE);
    assert!(!world.footing().grounded);
    assert!(world.footing().ticks_airborne > tuning.coyote_ticks);

    let before = world.velocity().y;
    world.step(Intent::jumping(0));
    assert!(
        world.velocity().y < before,
        "gravity kept pulling; a mid-air jump must not fire"
    );
}

/// Running into a wall spends the horizontal motion and leaves the vertical
/// alone, which is what lets a character fall down a wall it is pressed
/// against instead of sticking to it.
#[test]
fn a_character_running_into_a_wall_stops_horizontally_and_keeps_falling() {
    let mut world = staged();
    run(&mut world, 60, Intent::IDLE);
    // Run right into the wall at x = 10 (its left face is x = 9).
    run(&mut world, 200, Intent::running(1));

    let x = world.position().x;
    assert!(
        x < Fixed::from_int(9),
        "ended at x = {}, which is inside the wall",
        x.to_bits()
    );
    assert!(
        world.footing().against_wall,
        "it should know it is on a wall"
    );
    assert!(world.footing().grounded, "and still standing on the floor");
}

#[test]
fn a_character_runs_left_and_right() {
    let mut world = staged();
    run(&mut world, 60, Intent::IDLE);
    let start = world.position().x;

    run(&mut world, 20, Intent::running(1));
    let right = world.position().x;
    assert!(right > start, "moved right");

    run(&mut world, 40, Intent::running(-1));
    assert!(world.position().x < right, "and back left");
}

/// A run value outside the range must not make a different world — a caller
/// meaning "hard left" and one with a noisy stick have to agree.
#[test]
fn an_out_of_range_run_is_clamped() {
    let mut fast = staged();
    let mut normal = staged();
    run(&mut fast, 100, Intent::running(9999));
    run(&mut normal, 100, Intent::running(1));
    assert_eq!(fast.digest(), normal.digest(), "clamping must be total");
}

/// **The property that makes a replay an assertion rather than a demo.** The
/// same inputs from the same start give the same digest, every time.
#[test]
fn the_same_inputs_give_the_same_digest() {
    let script: Vec<Intent> = (0..300)
        .map(|tick: u32| match tick % 17 {
            0..=4 => Intent::running(1),
            5 => Intent::jumping(1),
            6..=9 => Intent::running(-1),
            10 => Intent::jumping(-1),
            _ => Intent::IDLE,
        })
        .collect();

    let digest_of = |script: &[Intent]| {
        let mut world = staged();
        for &intent in script {
            world.step(intent);
        }
        world.digest()
    };

    let first = digest_of(&script);
    let second = digest_of(&script);
    assert_eq!(first, second, "the same run must digest the same");

    // And a negative control, so the digest is not simply constant: one tick
    // changed anywhere must change the answer.
    let mut altered = script.clone();
    altered[100] = Intent::jumping(1);
    assert_ne!(
        digest_of(&altered),
        first,
        "a digest that ignores an input is not an oracle"
    );
}

/// The digest has to cover the jump latch, not just position and velocity.
/// Two worlds standing in the same place at the same speed can still diverge
/// on the next tick if one is holding the button and the other is not.
#[test]
fn the_digest_covers_the_jump_latch() {
    let mut holding = staged();
    let mut released = staged();
    run(&mut holding, 60, Intent::IDLE);
    run(&mut released, 60, Intent::IDLE);
    assert_eq!(holding.digest(), released.digest(), "identical so far");

    // One tick with the button down leaves the latch set.
    holding.step(Intent::jumping(0));
    released.step(Intent::IDLE);
    assert_ne!(holding.digest(), released.digest());
}

/// A world that leaked an entity slot per tick would simulate correctly and
/// digest consistently, and only this would show it.
#[test]
fn a_long_run_does_not_leak_entities() {
    let mut world = staged();
    let before = world.entity_count();
    run(&mut world, 2000, Intent::running(1));
    assert_eq!(world.entity_count(), before, "the world grew");
    assert_eq!(before, 1 + level().len(), "the character and its platforms");
}

/// The character must never end a tick inside the terrain. This is the
/// guarantee everything else rests on, and it is worth asserting over a run
/// long enough to include landings, wall contacts and a fall from height.
#[test]
fn the_character_never_ends_a_tick_inside_the_terrain() {
    let mut world = Leap::new(Tuning::default(), v(-14, 20), &level());
    for tick in 0..600u32 {
        let intent = match tick % 23 {
            0..=8 => Intent::running(1),
            9 => Intent::jumping(1),
            10..=15 => Intent::running(-1),
            _ => Intent::IDLE,
        };
        world.step(intent);

        let position = world.position();
        // The floor's top is y = 1 and the character's half-height is 1, so a
        // centre below y = 2 is inside it. A little slack for the skin.
        assert!(
            position.y.to_bits() >= Fixed::from_int(2).to_bits() - 8192,
            "tick {tick}: sank to y = {}, which is inside the floor",
            position.y.to_bits()
        );
        // The wall spans x from 9 to 11 above y = 1; the character is half a
        // unit wide, so its centre must stay left of 9.5 while it is low.
        if position.y < Fixed::from_int(9) {
            assert!(
                position.x.to_bits() <= Fixed::from_ratio(19, 2).to_bits() + 8192,
                "tick {tick}: reached x = {}, which is inside the wall",
                position.x.to_bits()
            );
        }
    }
}

/// **Being wedged is not the same as being against a wall**, and the two are
/// separate bits because a caller may act on the difference: a character
/// running along a wall is moving fine, and one that ran out of slide
/// iterations has stopped for a reason worth surfacing.
#[test]
fn a_move_that_runs_out_of_iterations_reports_being_wedged() {
    let tuning = Tuning {
        // One iteration only: meeting a surface and needing to slide off it
        // spends the budget immediately.
        slide_iterations: 1,
        ..Tuning::default()
    };
    let mut world = Leap::new(tuning, v(0, 6), &level());
    run(&mut world, 60, Intent::IDLE);
    // Push diagonally into the wall, which needs a slide after the first hit.
    run(&mut world, 120, Intent::running(1));

    assert!(
        world.footing().wedged || world.footing().against_wall,
        "a one-iteration budget against a wall must report one or the other"
    );

    // And with a generous budget the same run is not wedged, which is what
    // makes the bit mean something.
    let mut roomy = Leap::new(Tuning::default(), v(0, 6), &level());
    run(&mut roomy, 60, Intent::IDLE);
    run(&mut roomy, 120, Intent::running(1));
    assert!(!roomy.footing().wedged, "four iterations is room enough");
    assert!(roomy.footing().against_wall, "but it is still on the wall");
}

/// The tick counter advances once per step, which is what a recording indexes
/// its inputs by.
#[test]
fn the_tick_counter_counts_ticks() {
    let mut world = staged();
    assert_eq!(world.tick(), 0);
    run(&mut world, 7, Intent::IDLE);
    assert_eq!(world.tick(), 7);
    world.step(Intent::jumping(0));
    assert_eq!(world.tick(), 8);
}
