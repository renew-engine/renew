//! What the three-dimensional broadphase proposes, and in what order.

use proptest::prelude::*;
use renew_ecs::Entities;
use renew_fixed::{Fixed, Vec3};
use renew_physics3d::{BodyKind, Broadphase, Collider, Filter, Shape, Transform, World};

const NO_TOLERANCE: Fixed = Fixed::ZERO;

fn v(x: i32, y: i32, z: i32) -> Vec3 {
    Vec3::new(Fixed::from_int(x), Fixed::from_int(y), Fixed::from_int(z))
}

fn at(x: i32, y: i32, z: i32) -> Transform {
    Transform::at(v(x, y, z))
}

fn cube(half: i32) -> Shape {
    Shape::Box {
        half_extents: v(half, half, half),
    }
}

fn pairs_for(positions: &[(i32, i32, i32)], order: &[usize]) -> Vec<(Collider, Collider)> {
    let mut entities = Entities::new();
    let handles: Vec<_> = (0..positions.len()).map(|_| entities.spawn()).collect();
    let mut world = World::new();
    for &which in order {
        let handle = handles[which];
        let (x, y, z) = positions[which];
        world.create_body(handle, BodyKind::Kinematic, at(x, y, z));
        world.add_shape(handle, cube(1), Transform::IDENTITY, Filter::ALL);
    }
    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    broadphase.pairs().to_vec()
}

#[test]
fn overlapping_cubes_are_proposed_and_distant_ones_are_not() {
    assert_eq!(pairs_for(&[(0, 0, 0), (1, 0, 0)], &[0, 1]).len(), 1);
    assert!(pairs_for(&[(0, 0, 0), (5, 0, 0)], &[0, 1]).is_empty());
}

/// **The mistake a lifted two-dimensional sweep makes.** Sweeping *x* alone
/// prunes nothing between bodies stacked in *y* or *z*, so the overlap test
/// that follows has to check all three — otherwise every pair in a column is
/// proposed.
#[test]
fn a_column_of_cubes_is_not_proposed_as_every_pair() {
    // Six cubes stacked on the y axis, two apart: each touches its neighbour
    // and nothing else. They share every x and z coordinate, so the swept axis
    // separates none of them.
    let column: Vec<(i32, i32, i32)> = (0..6).map(|i| (0, i * 2, 0)).collect();
    let order: Vec<usize> = (0..6).collect();
    let pairs = pairs_for(&column, &order);
    assert_eq!(
        pairs.len(),
        5,
        "only neighbours, not the fifteen a single-axis test would give"
    );
    for window in pairs.windows(2) {
        assert!(window[0] < window[1], "pairs must ascend");
    }
}

/// The same, along the swept axis itself, so the sweep is doing its job too.
#[test]
fn a_row_along_the_swept_axis_proposes_only_neighbours() {
    let row: Vec<(i32, i32, i32)> = (0..6).map(|i| (i * 2, 0, 0)).collect();
    let order: Vec<usize> = (0..6).collect();
    assert_eq!(pairs_for(&row, &order).len(), 5);
}

#[test]
fn a_body_never_pairs_with_itself() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
    world.add_shape(handle, cube(1), Transform::IDENTITY, Filter::ALL);
    world.add_shape(handle, cube(1), at(1, 0, 0), Filter::ALL);

    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert_eq!(broadphase.collider_count(), 2, "both shapes were swept");
    assert!(
        broadphase.pairs().is_empty(),
        "and neither blocks the other"
    );
}

#[test]
fn two_static_bodies_are_not_proposed_and_a_kinematic_one_is() {
    let mut entities = Entities::new();
    let mut world = World::new();
    for _ in 0..2 {
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Static, Transform::IDENTITY);
        world.add_shape(handle, cube(1), Transform::IDENTITY, Filter::ALL);
    }
    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert!(broadphase.pairs().is_empty());

    let moving = entities.spawn();
    world.create_body(moving, BodyKind::Kinematic, Transform::IDENTITY);
    world.add_shape(moving, cube(1), Transform::IDENTITY, Filter::ALL);
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert_eq!(broadphase.pairs().len(), 2, "one against each static body");
}

#[test]
fn a_filtered_pair_never_existed() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let a = entities.spawn();
    let b = entities.spawn();
    world.create_body(a, BodyKind::Kinematic, Transform::IDENTITY);
    world.create_body(b, BodyKind::Kinematic, Transform::IDENTITY);
    world.add_shape(a, cube(1), Transform::IDENTITY, Filter::new(0b01, 0b10));
    let shape_b = world
        .add_shape(b, cube(1), Transform::IDENTITY, Filter::new(0b10, 0b01))
        .expect("live");

    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert_eq!(broadphase.pairs().len(), 1);

    world.set_filter(b, shape_b, Filter::new(0b10, 0b00));
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert!(broadphase.pairs().is_empty(), "the rule is symmetric");
    assert_eq!(
        broadphase.collider_count(),
        2,
        "both were still swept — the pair is what vanished"
    );
}

#[test]
fn the_tolerance_makes_near_misses_into_candidates() {
    let mut entities = Entities::new();
    let mut world = World::new();
    for z in [0, 3] {
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Kinematic, at(0, 0, z));
        world.add_shape(handle, cube(1), Transform::IDENTITY, Filter::ALL);
    }
    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert!(broadphase.pairs().is_empty(), "one unit apart on z");

    broadphase.rebuild(&world, Fixed::from_ratio(1, 2));
    assert_eq!(
        broadphase.pairs().len(),
        1,
        "and the tolerance closes it, on an axis that is not the swept one"
    );
}

#[test]
fn rebuilding_twice_gives_the_same_answer() {
    let positions: Vec<(i32, i32, i32)> = (0..6).map(|i| (i, i % 2, i % 3)).collect();
    let order: Vec<usize> = (0..6).collect();
    let first = pairs_for(&positions, &order);
    let second = pairs_for(&positions, &order);
    assert_eq!(first, second, "a rebuild is a pure function");
}

#[test]
fn zero_extent_shapes_do_not_confuse_the_sweep() {
    let mut entities = Entities::new();
    let mut world = World::new();
    for _ in 0..3 {
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
        world.add_shape(
            handle,
            Shape::Sphere {
                radius: Fixed::ZERO,
            },
            Transform::IDENTITY,
            Filter::ALL,
        );
    }
    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert_eq!(broadphase.pairs().len(), 3, "three coincident points");
    for window in broadphase.pairs().windows(2) {
        assert!(window[0] < window[1]);
    }
}

/// Every other test lays bodies out in the same order as their handles, so the
/// collider opening an interval always outranks the ones already active. A
/// world laid out backwards reaches the other side of the pair ordering.
#[test]
fn a_pair_names_its_lower_collider_first_whichever_arrives_last() {
    let mut entities = Entities::new();
    let handles: Vec<_> = (0..3).map(|_| entities.spawn()).collect();
    let mut world = World::new();
    for (step, &handle) in handles.iter().enumerate() {
        let x = 4 - 2 * i32::try_from(step).unwrap_or(0);
        world.create_body(handle, BodyKind::Kinematic, at(x, 0, 0));
        world.add_shape(handle, cube(1), Transform::IDENTITY, Filter::ALL);
    }
    let mut broadphase = Broadphase::new();
    broadphase.rebuild(&world, NO_TOLERANCE);
    assert_eq!(broadphase.pairs().len(), 2);
    for &(low, high) in broadphase.pairs() {
        assert!(low < high, "a pair names its lower collider first");
    }
}

proptest! {
    /// Two worlds holding the same colliders propose the same pairs in the
    /// same order, however they were assembled.
    #[test]
    fn proposed_pairs_do_not_depend_on_insertion_order(
        order in Just((0usize..7).collect::<Vec<_>>()).prop_shuffle(),
        heights in prop::collection::vec(-2i32..3, 7),
    ) {
        let positions: Vec<(i32, i32, i32)> = heights
            .iter()
            .enumerate()
            .map(|(i, &y)| (i32::try_from(i).unwrap_or(0), y, y % 2))
            .collect();
        let ascending: Vec<usize> = (0..7).collect();
        prop_assert_eq!(pairs_for(&positions, &ascending), pairs_for(&positions, &order));
    }

    /// Whatever the world holds, pairs ascend, name their lower collider
    /// first, and none is proposed twice.
    #[test]
    fn emitted_pairs_are_ordered_and_unique(
        heights in prop::collection::vec(-2i32..3, 1..8),
    ) {
        let positions: Vec<(i32, i32, i32)> = heights
            .iter()
            .enumerate()
            .map(|(i, &y)| (i32::try_from(i).unwrap_or(0), y, 0))
            .collect();
        let order: Vec<usize> = (0..positions.len()).collect();
        let pairs = pairs_for(&positions, &order);

        for &(low, high) in &pairs {
            prop_assert!(low < high);
        }
        for window in pairs.windows(2) {
            prop_assert!(window[0] <= window[1]);
        }
        let mut seen = pairs.clone();
        seen.dedup();
        prop_assert_eq!(seen.len(), pairs.len(), "a pair was proposed twice");
    }
}
