//! Moving platforms: what the hierarchy decides, and what the character then
//! runs into.
//!
//! These are the sample's half of the parenting crate's evidence. The crate's
//! own suite proves the composition against a reference; this proves that a
//! game driving it gets a platform that is somewhere real — that the placement
//! reaches the collision world, that it reaches the digest, and that a replay
//! of the same inputs puts it in the same place.

use std::collections::BTreeSet;

use renew_fixed::{Angle, Fixed, Vec2};
use renew_sample_leap_world::{Intent, Leap, Orbit, Platform, Tuning};

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
}

fn floor() -> [Platform; 1] {
    [Platform::new(0, -2, 20, 1)]
}

/// A hub at the origin with a four-unit arm, turning a quarter of a turn every
/// four ticks — chosen so the deck visits exact axis positions rather than
/// numbers a test would have to round.
fn quarter_turn_orbit() -> Orbit {
    Orbit {
        pivot: v(0, 8),
        arm: v(4, 0),
        half_extents: Vec2::new(Fixed::from_int(2), Fixed::from_ratio(1, 2)),
        turn_per_tick: Angle::from_degrees(90),
    }
}

/// The deck starts where composition says, not where the caller guessed. The
/// body is created at the origin, so a deck that had merely been seeded with
/// `pivot + arm` and never composed would read `(0, 0)` here.
#[test]
fn a_deck_is_placed_before_the_first_tick() {
    let mut world = Leap::new(Tuning::default(), v(0, 0), &floor());
    let deck = world.add_orbit(quarter_turn_orbit());

    let (at, turn) = world.deck_placement(deck).expect("the deck is placed");
    assert_eq!(at, v(4, 8), "pivot plus arm, with the hub not yet turned");
    assert_eq!(turn, Angle::ZERO);
}

/// Four ticks of a quarter turn each walk the deck round the four axes and
/// back. This is the assertion that separates composing from adding: an
/// implementation that added the arm to the pivot would leave the deck at
/// `(4, 8)` for ever.
#[test]
fn a_deck_travels_round_its_pivot() {
    let mut world = Leap::new(Tuning::default(), v(0, 0), &floor());
    let deck = world.add_orbit(quarter_turn_orbit());

    let expected = [v(0, 12), v(-4, 8), v(0, 4), v(4, 8)];
    for (index, want) in expected.into_iter().enumerate() {
        world.step(Intent::IDLE);
        let (at, turn) = world.deck_placement(deck).expect("placed");
        assert_eq!(at, want, "after {} tick(s)", index + 1);
        let turns = u32::try_from(index + 1).expect("four ticks fit in a u32");
        assert_eq!(
            turn,
            Angle::from_bits(Angle::from_degrees(90).to_bits().wrapping_mul(turns)),
            "the deck carries the hub's turn, not just its position"
        );
    }

    // A full circuit returns to exactly the start — integer angles, so this is
    // equality rather than a tolerance.
    let (at, turn) = world.deck_placement(deck).expect("placed");
    assert_eq!(at, v(4, 8));
    assert_eq!(turn, Angle::ZERO);
}

/// The placement is not just reported — it is what the character collides
/// with. A deck parked in the fall path must catch the character above the
/// floor, at the height composition put it.
///
/// **The arm and pivot must not cancel.** `add_orbit` creates the body at the
/// origin and then composes; a hub at `(0, 3)` with an arm of `(0, -3)` puts
/// the deck at exactly the origin too, so severing the composition from the
/// physics world would change nothing and this test would pass against it.
/// Composing to `(0, 4)` is what makes the assertion mean anything.
#[test]
fn a_deck_is_solid_where_the_hierarchy_puts_it() {
    let mut world = Leap::new(Tuning::default(), v(0, 9), &floor());
    world.add_orbit(Orbit {
        pivot: v(0, 6),
        arm: v(0, -2),
        half_extents: Vec2::new(Fixed::from_int(4), Fixed::from_ratio(1, 2)),
        turn_per_tick: Angle::ZERO,
    });

    // The hub is at y = 6 with an arm reaching 2 down, so the deck sits at
    // y = 4 — a height stated nowhere, only composed. Its top face is at 4.5
    // and the character is one unit tall from its centre, so a character
    // resting on it stands near y = 5.5. The floor would leave it at 1.5,
    // and a deck left at the origin would leave it at 1.5 as well.
    for _ in 0..80 {
        world.step(Intent::IDLE);
    }

    assert!(world.footing().grounded, "the character came to rest");
    let resting = world.position().y;
    assert!(
        resting > Fixed::from_ratio(53, 10),
        "it stopped on the deck, not on the floor below it (y = {resting:?})"
    );
    assert!(
        resting < Fixed::from_ratio(57, 10),
        "and on top of the deck rather than inside it (y = {resting:?})"
    );
}

/// The deck's placement reaches the digest, so two worlds whose platforms are
/// in different places are different worlds — even when the character is doing
/// exactly the same thing in both.
#[test]
fn a_moving_deck_changes_the_digest() {
    let mut still = Leap::new(Tuning::default(), v(0, 0), &floor());
    still.add_orbit(Orbit {
        turn_per_tick: Angle::ZERO,
        ..quarter_turn_orbit()
    });
    let mut turning = Leap::new(Tuning::default(), v(0, 0), &floor());
    turning.add_orbit(quarter_turn_orbit());

    // Premise: the character must be doing the same thing in both, or the
    // digests would differ for a reason that has nothing to do with decks.
    for _ in 0..3 {
        still.step(Intent::IDLE);
        turning.step(Intent::IDLE);
    }
    assert_eq!(
        still.position(),
        turning.position(),
        "premise: the characters must agree, so only the decks can differ"
    );
    assert_ne!(
        still.digest(),
        turning.digest(),
        "a platform somewhere else is a different world"
    );
}

/// The sharper half of the same claim: two decks that are turned *identically*
/// and differ only in where they are must still digest differently.
///
/// Without this the absorption of a deck's translation is never load-bearing —
/// the test above contrasts a still deck with a turning one, whose rotations
/// already differ, so deleting the two translation lines from the digest leaves
/// the whole suite green.
#[test]
fn two_decks_in_different_places_digest_differently() {
    let build = |reach: i32| {
        let mut world = Leap::new(Tuning::default(), v(0, 0), &floor());
        world.add_orbit(Orbit {
            pivot: v(0, 30),
            arm: v(reach, 0),
            half_extents: Vec2::new(Fixed::from_int(2), Fixed::from_ratio(1, 2)),
            turn_per_tick: Angle::from_degrees(90),
        });
        for _ in 0..3 {
            world.step(Intent::IDLE);
        }
        world
    };

    let near = build(4);
    let far = build(9);

    // Premise: same turn rate from the same start means identical orientation
    // every tick, so only the place can account for a difference.
    let (near_at, near_turn) = near
        .deck_placement(near.decks().next().expect("one deck"))
        .expect("placed");
    let (far_at, far_turn) = far
        .deck_placement(far.decks().next().expect("one deck"))
        .expect("placed");
    assert_eq!(near_turn, far_turn, "premise: the orientations must agree");
    assert_ne!(near_at, far_at, "premise: the places must differ");

    assert_ne!(near.digest(), far.digest());
}

/// And the mirror: two decks in the same place, turned differently, must also
/// digest differently — which is what covers each hub's own angle.
#[test]
fn two_decks_turned_differently_digest_differently() {
    let build = |turn: i32| {
        let mut world = Leap::new(Tuning::default(), v(0, 0), &floor());
        world.add_orbit(Orbit {
            pivot: v(0, 30),
            arm: Vec2::ZERO,
            half_extents: Vec2::new(Fixed::from_int(2), Fixed::from_ratio(1, 2)),
            turn_per_tick: Angle::from_degrees(turn),
        });
        for _ in 0..3 {
            world.step(Intent::IDLE);
        }
        world
    };

    let slow = build(10);
    let fast = build(40);

    // Premise: a zero-length arm means the deck never moves, so place cannot
    // account for the difference and only the turn can.
    let slow_at = slow
        .deck_placement(slow.decks().next().expect("one deck"))
        .expect("placed")
        .0;
    let fast_at = fast
        .deck_placement(fast.decks().next().expect("one deck"))
        .expect("placed")
        .0;
    assert_eq!(slow_at, fast_at, "premise: the places must agree");

    assert_ne!(slow.digest(), fast.digest());
}

/// A level with no moving parts must digest exactly as it did before moving
/// parts existed — the property that let this land without restating a single
/// recorded hash.
///
/// **Pinned to a literal, because nothing else in the tree pins one.** The
/// original version of this test asserted only that a pure function equals
/// itself and that a world nobody added an orbit to has no orbits; both are
/// tautologies, and adding a constant to the digest left them green. The
/// number below is the whole assertion: it was computed before moving
/// platforms existed, and every quantity reaching it is fixed-point, so it is
/// the same on every target and in every profile.
#[test]
fn a_level_without_orbits_digests_as_if_they_did_not_exist() {
    let mut plain = Leap::new(Tuning::default(), v(0, 6), &floor());
    for _ in 0..20 {
        plain.step(Intent::running(1));
    }
    assert_eq!(plain.decks().count(), 0, "premise: nothing was added");
    assert_eq!(
        plain.digest(),
        ORBIT_FREE_DIGEST,
        "an orbit-free world reaches none of the deck absorption"
    );
}

/// A twenty-tick run to the right on the floor alone.
///
/// Measured on the revision before moving platforms existed, by building this
/// same world there and printing its digest — not copied from a run of the
/// code it is meant to constrain, which would only prove that the code agrees
/// with itself.
const ORBIT_FREE_DIGEST: u64 = 0xab37_f973_3b94_049b;

/// Same inputs, same placements, bit for bit — the replay property, extended
/// to cover the hierarchy.
#[test]
fn orbits_replay_exactly() {
    let run = || {
        let mut world = Leap::new(Tuning::default(), v(0, 6), &floor());
        let deck = world.add_orbit(Orbit {
            turn_per_tick: Angle::from_degrees(7),
            ..quarter_turn_orbit()
        });
        let mut trace = Vec::new();
        for tick in 0..120 {
            world.step(if tick % 3 == 0 {
                Intent::running(1)
            } else {
                Intent::jumping(-1)
            });
            trace.push((world.digest(), world.deck_placement(deck)));
        }
        trace
    };

    let first = run();
    let second = run();
    assert_eq!(first, second);

    // Premise: a deck that never moved would replay exactly too, so the trace
    // has to be shown to contain many *different* placements. Comparing whole
    // trace entries would not do it — each carries the digest, which absorbs
    // the tick counter and so differs unconditionally. Only the placements are
    // compared here, and they are counted rather than sampled.
    let places: BTreeSet<_> = first
        .iter()
        .map(|(_, at)| {
            at.map(|(translation, rotation)| {
                (
                    translation.x.to_bits(),
                    translation.y.to_bits(),
                    rotation.to_bits(),
                )
            })
        })
        .collect();
    assert_eq!(
        first.iter().filter(|(_, at)| at.is_some()).count(),
        first.len(),
        "every tick placed the deck"
    );
    assert!(
        places.len() > 100,
        "premise: the deck must actually have travelled, and it visited {} places",
        places.len()
    );
}

/// The two crates hold the same composition formula — this one in
/// `renew_scene::Global`, the collision world's in its own `Transform` — and a
/// deck's placement is copied field-for-field from one into the other. If those
/// ever stopped agreeing, a deck would collide somewhere other than where it
/// says it is, which is the single bug this whole arrangement exists to
/// prevent, and nothing else in either crate would notice.
#[test]
fn the_collision_world_agrees_with_the_composed_placement() {
    let mut world = Leap::new(Tuning::default(), v(0, 40), &floor());
    let deck = world.add_orbit(Orbit {
        pivot: v(-5, 12),
        arm: v(3, 2),
        half_extents: Vec2::new(Fixed::from_int(2), Fixed::from_ratio(1, 2)),
        turn_per_tick: Angle::from_degrees(11),
    });

    for _ in 0..40 {
        world.step(Intent::IDLE);
        let (at, turn) = world.deck_placement(deck).expect("placed");
        let body = world.deck_transform(deck).expect("the body exists");
        assert_eq!(body.0, at, "the collider is where composition says");
        assert_eq!(body.1, turn, "and turned the way composition says");
    }
}

/// **A moving deck's travel does not reach its rider, and that is measured
/// rather than assumed.** A kinematic body is placed by writing its transform;
/// nothing transfers that motion to whatever is standing on it. The character
/// keeps touching the deck while the surface is still beneath it, so it does
/// not simply drop — but the deck's travel never reaches it, and it slides off
/// the way the geometry tilts.
///
/// **This is a statement about travel, not about the rider being undisturbed.**
/// A deck that moves *into* a character is a different matter: the next sweep
/// depenetrates the overlap, which can displace a rider instantly and by any
/// distance. `a_deck_that_moves_into_a_rider_displaces_it_at_once` measures
/// that, and the two together are the honest description.
///
/// The assertion here is about horizontal transfer rather than the grounded
/// flag, because a tilting surface makes contact flicker on and off from tick
/// to tick; travel is the unambiguous quantity, and it is the one a rider would
/// gain if this were fixed.
///
/// Pinned as an assertion rather than described in a comment, so the day rider
/// transfer arrives this test fails and the record has to be corrected instead
/// of quietly going stale.
#[test]
fn a_rider_is_not_carried_by_a_moving_deck() {
    let mut world = Leap::new(Tuning::default(), v(0, 12), &[Platform::new(0, -20, 40, 1)]);
    let deck = world.add_orbit(Orbit {
        pivot: v(0, 9),
        arm: v(0, -3),
        half_extents: Vec2::new(Fixed::from_int(4), Fixed::from_ratio(1, 2)),
        turn_per_tick: Angle::from_degrees(1),
    });

    // Land on it first — without this the test would prove only that a
    // character in mid-air falls.
    for _ in 0..20 {
        world.step(Intent::IDLE);
    }
    assert!(
        world.footing().grounded,
        "premise: the character must be standing on the deck before it is asked \
         whether the deck takes it anywhere"
    );
    let rider_before = world.position().x;
    // Measured from the *collider*, not the hierarchy. Reading the composed
    // placement here would let this test pass on a world where the deck the
    // character stands on never moved at all — the hierarchy would report
    // travel, the rider would report none, and the conclusion would be right
    // for entirely the wrong reason.
    let deck_before = world.deck_transform(deck).expect("the body exists").0.x;

    for _ in 0..30 {
        world.step(Intent::IDLE);
    }
    let rider_travel = world.position().x - rider_before;
    let deck_travel = world.deck_transform(deck).expect("the body exists").0.x - deck_before;

    // Premise: the deck must actually have gone somewhere.
    assert!(
        deck_travel > Fixed::ONE,
        "premise: the deck travelled {deck_travel:?}, too little to conclude anything"
    );
    assert!(
        rider_travel.abs() < Fixed::from_ratio(1, 10),
        "the deck's travel did not reach the rider (deck {deck_travel:?}, \
         rider {rider_travel:?})"
    );
}

/// The other half of the rider story, and the sharper one: a deck placed into a
/// standing character does not push it gradually, it teleports it.
///
/// `set_transform` moves a kinematic body with no sweep, so an overlap simply
/// exists on the next tick and the character's own move resolves it in one
/// step. Measured: a character resting at the origin is lifted to exactly the
/// deck's top face in a single tick, having travelled 1.5 units with nothing
/// in between. This is why the record says a deck's *travel* is not
/// transferred rather than that a deck cannot move its rider.
#[test]
fn a_deck_that_moves_into_a_rider_displaces_it_at_once() {
    // The floor sits high on purpose, so the character settles at y = 5 rather
    // than at the origin. A deck composed to the origin is where `add_orbit`
    // creates the body regardless, so a case built down there would pass even
    // if the composition never reached the collision world.
    let mut world = Leap::new(Tuning::default(), v(0, 11), &[Platform::new(0, 3, 40, 1)]);
    for _ in 0..60 {
        world.step(Intent::IDLE);
    }
    let settled = world.position().y;
    assert!(
        world.footing().grounded,
        "premise: the character must be standing on the floor first"
    );
    assert!(
        settled > Fixed::from_int(4),
        "premise: it must have settled well away from the origin ({settled:?})"
    );

    // A deck materialising exactly where the character is standing — composed
    // to (0, 5) from a hub three units above it.
    world.add_orbit(Orbit {
        pivot: v(0, 8),
        arm: v(0, -3),
        half_extents: Vec2::new(Fixed::from_int(4), Fixed::from_ratio(1, 2)),
        turn_per_tick: Angle::ZERO,
    });

    world.step(Intent::IDLE);
    let lifted = world.position().y - settled;
    assert!(
        lifted > Fixed::ONE,
        "one tick moved the character {lifted:?}, with no sweep in between"
    );
}
