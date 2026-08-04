//! What the broadphase proposes, and in what order.
//!
//! The order is the part that cannot be recovered later. Everything
//! downstream — which contact is reported first, which overlap is resolved
//! first — inherits it, so a broadphase whose output depends on how the world
//! was assembled makes every report downstream machine-dependent.

use proptest::prelude::*;
use renew_ecs::Entities;
use renew_fixed::{Fixed, Vec2};
use renew_physics2d::{BodyKind, Broadphase, Collider, Filter, Shape, Transform, World};

const NO_TOLERANCE: Fixed = Fixed::ZERO;

fn box_shape(half: i32) -> Shape {
    Shape::Box {
        half_extents: Vec2::new(Fixed::from_int(half), Fixed::from_int(half)),
    }
}

fn at(x: i32, y: i32) -> Transform {
    Transform::at(Vec2::new(Fixed::from_int(x), Fixed::from_int(y)))
}

/// Put one unit box per position into a world, in the given order, and return
/// the pairs the broadphase proposes.
fn pairs_for(positions: &[(i32, i32)], order: &[usize]) -> Vec<(Collider, Collider)> {
    let mut entities = Entities::new();
    let handles: Vec<_> = (0..positions.len()).map(|_| entities.spawn()).collect();
    let mut world = World::new();
    for &which in order {
        let handle = handles[which];
        let (x, y) = positions[which];
        world.create_body(handle, BodyKind::Kinematic, at(x, y));
        world.add_shape(handle, box_shape(1), Transform::IDENTITY, Filter::ALL);
    }
    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    broadphase.pairs().to_vec()
}

#[test]
fn overlapping_boxes_are_proposed_and_distant_ones_are_not() {
    // Two unit boxes one apart overlap; one five apart do not.
    let near = pairs_for(&[(0, 0), (1, 0)], &[0, 1]);
    assert_eq!(near.len(), 1);

    let far = pairs_for(&[(0, 0), (5, 0)], &[0, 1]);
    assert!(far.is_empty(), "boxes five apart share no point");
}

/// Boxes that merely touch are proposed, because under-reporting loses a
/// contact silently and over-reporting costs one narrowphase test.
#[test]
fn boxes_that_merely_touch_are_proposed() {
    let touching = pairs_for(&[(0, 0), (2, 0)], &[0, 1]);
    assert_eq!(touching.len(), 1, "edge-to-edge is a candidate");
}

/// The tile-aligned world the sort key exists for: everything shares edges, so
/// equal coordinates are the common case rather than a corner one.
#[test]
fn a_tile_aligned_row_proposes_only_its_neighbours() {
    let row: Vec<(i32, i32)> = (0..5).map(|i| (i * 2, 0)).collect();
    let order: Vec<usize> = (0..5).collect();
    let pairs = pairs_for(&row, &order);
    // Each tile touches the next: four pairs, not ten.
    assert_eq!(
        pairs.len(),
        4,
        "only neighbours, despite every edge matching"
    );
    for window in pairs.windows(2) {
        assert!(window[0] < window[1], "pairs must ascend");
    }
}

/// A body's own shapes are parts of one object, and an object does not block
/// itself.
#[test]
fn a_body_never_pairs_with_itself() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
    // Two shapes on one body, deliberately overlapping.
    world.add_shape(handle, box_shape(1), Transform::IDENTITY, Filter::ALL);
    world.add_shape(handle, box_shape(1), at(1, 0), Filter::ALL);

    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert_eq!(broadphase.collider_count(), 2, "both shapes were swept");
    assert!(
        broadphase.pairs().is_empty(),
        "a body must not collide with itself"
    );
}

/// Two static bodies never move, so a contact between them can never change.
#[test]
fn two_static_bodies_are_not_proposed() {
    let mut entities = Entities::new();
    let mut world = World::new();
    for _ in 0..2 {
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Static, Transform::IDENTITY);
        world.add_shape(handle, box_shape(1), Transform::IDENTITY, Filter::ALL);
    }
    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert!(broadphase.pairs().is_empty());

    // But a kinematic body against a static one is proposed, or the only
    // movable kind in v0 could collide with nothing.
    let moving = entities.spawn();
    world.create_body(moving, BodyKind::Kinematic, Transform::IDENTITY);
    world.add_shape(moving, box_shape(1), Transform::IDENTITY, Filter::ALL);
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert_eq!(broadphase.pairs().len(), 2, "one against each static body");
}

/// A filtered pair never existed — it is not a result that was dropped — so
/// the count has to agree between implementations.
#[test]
fn a_filtered_pair_never_existed() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let a = entities.spawn();
    let b = entities.spawn();
    world.create_body(a, BodyKind::Kinematic, Transform::IDENTITY);
    world.create_body(b, BodyKind::Kinematic, Transform::IDENTITY);
    world.add_shape(
        a,
        box_shape(1),
        Transform::IDENTITY,
        Filter::new(0b01, 0b10),
    );
    let shape_b = world
        .add_shape(
            b,
            box_shape(1),
            Transform::IDENTITY,
            Filter::new(0b10, 0b01),
        )
        .expect("live");

    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert_eq!(broadphase.pairs().len(), 1, "they name each other");

    // Break one direction only: the rule is symmetric, so the pair goes.
    world.set_filter(b, shape_b, Filter::new(0b10, 0b00));
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert!(
        broadphase.pairs().is_empty(),
        "one-sided interest is not eligibility"
    );
    assert_eq!(
        broadphase.collider_count(),
        2,
        "both colliders were still swept — the pair is what vanished"
    );
}

/// The tolerance is what lets a contact at depth zero exist at all: without
/// the inflation, a pair separated by less than the tolerance never reaches
/// narrowphase and the contact the vocabulary requires is never generated.
#[test]
fn the_tolerance_makes_near_misses_into_candidates() {
    let mut entities = Entities::new();
    let mut world = World::new();
    for x in [0, 3] {
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Kinematic, at(x, 0));
        world.add_shape(handle, box_shape(1), Transform::IDENTITY, Filter::ALL);
    }

    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert!(
        broadphase.pairs().is_empty(),
        "one unit apart, no tolerance"
    );

    broadphase.rebuild(&world, Fixed::from_ratio(1, 2));
    assert_eq!(
        broadphase.pairs().len(),
        1,
        "half a unit of tolerance on each side closes a one-unit gap"
    );
}

/// Rebuilding is idempotent, which is what makes the structure derived state
/// rather than something a save has to carry.
#[test]
fn rebuilding_twice_gives_the_same_answer() {
    let mut entities = Entities::new();
    let mut world = World::new();
    for x in 0..6 {
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Kinematic, at(x, 0));
        world.add_shape(handle, box_shape(1), Transform::IDENTITY, Filter::ALL);
    }
    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    let first = broadphase.pairs().to_vec();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert_eq!(first, broadphase.pairs(), "a rebuild is a pure function");
}

/// A zero-extent shape has both endpoints at the same coordinate, which is the
/// case the begin/end flag in the sort key exists for.
#[test]
fn zero_extent_shapes_do_not_confuse_the_sweep() {
    let mut entities = Entities::new();
    let mut world = World::new();
    for _ in 0..3 {
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
        world.add_shape(
            handle,
            Shape::Circle {
                radius: Fixed::ZERO,
            },
            Transform::IDENTITY,
            Filter::ALL,
        );
    }
    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    // Three coincident points: every pair overlaps, and there are three.
    assert_eq!(broadphase.pairs().len(), 3);
    for window in broadphase.pairs().windows(2) {
        assert!(window[0] < window[1], "still ascending");
    }
}

proptest! {
    /// **The property the emitted-order rule exists for.** Two worlds holding
    /// the same colliders propose the same pairs in the same order, however
    /// they were assembled — so a contact array cannot leak the sequence of
    /// operations that built the world.
    #[test]
    fn proposed_pairs_do_not_depend_on_insertion_order(
        order in Just((0usize..7).collect::<Vec<_>>()).prop_shuffle(),
        offsets in prop::collection::vec(-3i32..4, 7),
    ) {
        let positions: Vec<(i32, i32)> = offsets
            .iter()
            .enumerate()
            .map(|(i, &dy)| (i32::try_from(i).unwrap_or(0), dy))
            .collect();
        let ascending: Vec<usize> = (0..7).collect();
        let expected = pairs_for(&positions, &ascending);
        let shuffled = pairs_for(&positions, &order);
        prop_assert_eq!(expected, shuffled, "insertion order reached the output");
    }

    /// Whatever the world contains, the emitted pairs ascend strictly and each
    /// names its lower collider first.
    #[test]
    fn emitted_pairs_are_ordered_and_canonical(
        offsets in prop::collection::vec(-2i32..3, 1..8),
    ) {
        let positions: Vec<(i32, i32)> = offsets
            .iter()
            .enumerate()
            .map(|(i, &dy)| (i32::try_from(i).unwrap_or(0), dy))
            .collect();
        let order: Vec<usize> = (0..positions.len()).collect();
        let pairs = pairs_for(&positions, &order);

        for &(low, high) in &pairs {
            prop_assert!(low < high, "a pair names its lower collider first");
        }
        for window in pairs.windows(2) {
            prop_assert!(window[0] <= window[1], "pairs ascend");
        }
        // And no pair is proposed twice, which a sweep that mishandled the
        // active set would do.
        let mut seen = pairs.clone();
        seen.dedup();
        prop_assert_eq!(seen.len(), pairs.len(), "a pair was proposed twice");
    }
}
