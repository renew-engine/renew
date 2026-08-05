//! The identity and ordering rules, lifted to three dimensions.
//!
//! These are the same properties the two-dimensional crate holds, and they are
//! tested again rather than assumed to carry over: the vocabulary is shared,
//! the implementation is not, and "it worked in 2D" is not evidence about code
//! that was written separately.

use proptest::prelude::*;
use renew_ecs::Entities;
use renew_fixed::{Fixed, Vec3};
use renew_physics3d::{
    BodyKind, Collider, Filter, HandleState, Shape, ShapeIndex, Transform, World,
};

fn v(x: i32, y: i32, z: i32) -> Vec3 {
    Vec3::new(Fixed::from_int(x), Fixed::from_int(y), Fixed::from_int(z))
}

fn at(x: i32, y: i32, z: i32) -> Transform {
    Transform::at(v(x, y, z))
}

fn sphere(units: i32) -> Shape {
    Shape::Sphere {
        radius: Fixed::from_int(units),
    }
}

fn cube(half: i32) -> Shape {
    Shape::Box {
        half_extents: v(half, half, half),
    }
}

/// A removed shape leaves a hole, and the shapes after it keep their indices.
#[test]
fn removing_a_shape_does_not_renumber_the_others() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let body = world
        .create_body(entities.spawn(), BodyKind::Kinematic, Transform::IDENTITY)
        .expect("a fresh entity has no body");

    let indices: Vec<_> = (1..=3)
        .map(|units| {
            world
                .add_shape(body, sphere(units), Transform::IDENTITY, Filter::ALL)
                .expect("live body")
        })
        .collect();
    assert_eq!(indices[2].get(), 2);

    assert!(world.remove_shape(body, indices[1]));

    let (shape, _, _) = world
        .shape(Collider {
            handle: body,
            index: indices[2],
        })
        .expect("the third shape kept its index");
    assert_eq!(shape, sphere(3));
    assert!(
        world
            .shape(Collider {
                handle: body,
                index: indices[1]
            })
            .is_none(),
        "index one must be a hole, not a shifted-down neighbour"
    );
    assert_eq!(
        world.shape_extent(body),
        Some(3),
        "the hole keeps its place"
    );
}

#[test]
fn a_new_shape_fills_the_lowest_free_hole() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let body = world
        .create_body(entities.spawn(), BodyKind::Static, Transform::IDENTITY)
        .expect("fresh");

    let indices: Vec<_> = (0..5)
        .map(|_| {
            world
                .add_shape(body, cube(1), Transform::IDENTITY, Filter::ALL)
                .expect("live")
        })
        .collect();

    // Free the higher hole first, so "lowest" is about ordering rather than
    // recency.
    assert!(world.remove_shape(body, indices[3]));
    assert!(world.remove_shape(body, indices[1]));

    let first = world
        .add_shape(body, cube(1), Transform::IDENTITY, Filter::ALL)
        .expect("live");
    assert_eq!(first, indices[1], "the lowest free hole, not the newest");
    let second = world
        .add_shape(body, cube(1), Transform::IDENTITY, Filter::ALL)
        .expect("live");
    assert_eq!(second, indices[3], "then the next one up");
}

#[test]
fn a_handle_is_live_stale_or_unknown() {
    let mut entities = Entities::new();
    let mut world = World::new();

    let handle = entities.spawn();
    assert_eq!(world.handle_state(handle), HandleState::Unknown);

    world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
    assert_eq!(world.handle_state(handle), HandleState::Live);

    entities.despawn(handle);
    let recycled = entities.spawn();
    assert_eq!(recycled.index(), handle.index(), "the slot was reused");
    assert_eq!(
        world.handle_state(recycled),
        HandleState::Stale,
        "a reused index must not reach another body's data"
    );
    assert!(!world.set_transform(recycled, at(5, 5, 5)));
    assert!(world.transform(recycled).is_none());

    // The original still works: nothing about the ECS despawn reached here.
    assert!(world.set_transform(handle, at(5, 5, 5)));
}

#[test]
fn one_entity_gets_at_most_one_body() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    assert!(
        world
            .create_body(handle, BodyKind::Static, Transform::IDENTITY)
            .is_some()
    );
    assert!(
        world
            .create_body(handle, BodyKind::Static, Transform::IDENTITY)
            .is_none(),
        "a second body against the same entity must be refused"
    );
    assert_eq!(world.body_count(), 1);
}

#[test]
fn a_dynamic_body_is_refused_rather_than_silently_reinterpreted() {
    let mut entities = Entities::new();
    let mut world = World::new();
    assert!(
        world
            .create_body(entities.spawn(), BodyKind::Dynamic, Transform::IDENTITY)
            .is_none()
    );
    assert_eq!(world.body_count(), 0);
}

#[test]
fn a_rebuilt_collider_is_not_the_one_it_replaced() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
    let index = world
        .add_shape(handle, sphere(1), Transform::IDENTITY, Filter::ALL)
        .expect("live");
    let collider = Collider { handle, index };
    let before = world.incarnation(collider).expect("occupied");

    assert!(world.remove_shape(handle, index));
    let again = world
        .add_shape(handle, sphere(1), Transform::IDENTITY, Filter::ALL)
        .expect("live");
    assert_eq!(again, index, "the identity is reused, which is the problem");
    assert_ne!(
        world.incarnation(collider).expect("occupied"),
        before,
        "and the incarnation is what distinguishes them"
    );
}

/// A shape's placement is added to its body's, which is what lets one body own
/// several in different places — a chunk of terrain, or a door and its frame.
#[test]
fn a_shape_sits_where_its_local_transform_puts_it() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Kinematic, at(10, 0, -5));
    let index = world
        .add_shape(handle, cube(1), at(0, 3, 2), Filter::ALL)
        .expect("live");
    let placed = world
        .world_transform(Collider { handle, index })
        .expect("occupied");
    assert_eq!(placed.translation, v(10, 3, -3));
}

#[test]
fn every_operation_refuses_a_handle_this_world_never_saw() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let stranger = entities.spawn();

    assert_eq!(world.handle_state(stranger), HandleState::Unknown);
    assert!(!world.destroy_body(stranger));
    assert!(!world.set_transform(stranger, at(1, 1, 1)));
    assert!(
        world
            .add_shape(stranger, cube(1), Transform::IDENTITY, Filter::ALL)
            .is_none()
    );
    assert!(!world.remove_shape(stranger, ShapeIndex::from_raw(0)));
    assert!(!world.replace_shape(
        stranger,
        ShapeIndex::from_raw(0),
        cube(1),
        Transform::IDENTITY
    ));
    assert!(!world.set_filter(stranger, ShapeIndex::from_raw(0), Filter::ALL));
    assert!(world.kind(stranger).is_none());
    assert!(world.shape_extent(stranger).is_none());
}

#[test]
fn destroying_a_body_takes_its_shapes_and_stops_answering() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Kinematic, at(1, 2, 3));
    let index = world
        .add_shape(handle, cube(1), Transform::IDENTITY, Filter::ALL)
        .expect("live");

    assert_eq!(world.kind(handle), Some(BodyKind::Kinematic));
    assert!(world.destroy_body(handle));
    assert_eq!(world.body_count(), 0);
    assert_eq!(world.handle_state(handle), HandleState::Unknown);
    assert!(world.shape(Collider { handle, index }).is_none());
    assert!(world.world_transform(Collider { handle, index }).is_none());
    assert_eq!(world.colliders().count(), 0);
    assert!(!world.destroy_body(handle), "twice is a no-op");
}

#[test]
fn replacing_and_refiltering_a_shape_behave_as_they_do_in_two_dimensions() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
    let index = world
        .add_shape(handle, sphere(1), Transform::IDENTITY, Filter::ALL)
        .expect("live");
    let collider = Collider { handle, index };
    let before = world.incarnation(collider).expect("occupied");

    assert!(world.replace_shape(handle, index, cube(2), at(1, 1, 1)));
    let (shape, local, _) = world.shape(collider).expect("still occupied");
    assert_eq!(shape, cube(2));
    assert_eq!(local.translation, v(1, 1, 1));
    assert_ne!(world.incarnation(collider).expect("occupied"), before);

    // A filter change is not a rebuild.
    let after_replace = world.incarnation(collider).expect("occupied");
    assert!(world.set_filter(handle, index, Filter::NONE));
    assert_eq!(
        world.incarnation(collider).expect("occupied"),
        after_replace
    );
    let (_, _, filter) = world.shape(collider).expect("occupied");
    assert_eq!(filter, Filter::NONE);

    // Refusals.
    assert!(!world.replace_shape(handle, index, cube(-1), Transform::IDENTITY));
    assert!(!world.replace_shape(
        handle,
        ShapeIndex::from_raw(9),
        cube(1),
        Transform::IDENTITY
    ));
    assert!(!world.set_filter(handle, ShapeIndex::from_raw(9), Filter::ALL));
    assert!(world.remove_shape(handle, index));
    assert!(!world.remove_shape(handle, index));
    assert!(!world.set_filter(handle, index, Filter::ALL));
}

#[test]
fn a_shape_with_a_negative_extent_is_refused() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let body = world
        .create_body(entities.spawn(), BodyKind::Static, Transform::IDENTITY)
        .expect("fresh");
    assert!(
        world
            .add_shape(body, cube(-1), Transform::IDENTITY, Filter::ALL)
            .is_none()
    );
    assert_eq!(world.shape_extent(body), Some(0), "and nothing was stored");
}

/// Bodies live against entity indices, and a sparse index space must not
/// invent bodies in the gaps or lose the order between them.
#[test]
fn a_sparse_index_space_does_not_invent_bodies() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let first = entities.spawn();
    for _ in 0..5 {
        let _ = entities.spawn();
    }
    let distant = entities.spawn();

    world.create_body(first, BodyKind::Static, Transform::IDENTITY);
    world.create_body(distant, BodyKind::Static, Transform::IDENTITY);
    world.add_shape(first, cube(1), Transform::IDENTITY, Filter::ALL);
    world.add_shape(distant, cube(1), Transform::IDENTITY, Filter::ALL);

    assert_eq!(world.body_count(), 2);
    let seen: Vec<_> = world.colliders().collect();
    assert_eq!(seen.len(), 2, "the gap contributed nothing");
    assert!(seen[0] < seen[1], "and they came out in index order");
}

proptest! {
    /// Collider order is a function of the collider set, not of the order it
    /// was built in — which is what makes a contact array reproducible across
    /// machines that reached the same state by different routes.
    #[test]
    fn collider_order_does_not_depend_on_insertion_order(
        permutation in Just((0usize..6).collect::<Vec<_>>()).prop_shuffle()
    ) {
        let build = |order: &[usize]| {
            let mut entities = Entities::new();
            let handles: Vec<_> = (0..6).map(|_| entities.spawn()).collect();
            let mut world = World::new();
            for &which in order {
                let handle = handles[which];
                world.create_body(handle, BodyKind::Static, Transform::IDENTITY);
                world.add_shape(handle, cube(1), Transform::IDENTITY, Filter::ALL);
            }
            world.colliders().collect::<Vec<_>>()
        };
        let ascending: Vec<usize> = (0..6).collect();
        prop_assert_eq!(build(&ascending), build(&permutation));
    }

    /// However a body's shapes are added and removed, every live one keeps a
    /// distinct index and collider order stays strictly ascending.
    #[test]
    fn collider_order_is_strictly_ascending_under_any_history(
        operations in prop::collection::vec(prop::bool::ANY, 0..24)
    ) {
        let mut entities = Entities::new();
        let mut world = World::new();
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);

        let mut added: Vec<ShapeIndex> = Vec::new();
        for add in operations {
            if add || added.is_empty() {
                if let Some(index) = world.add_shape(handle, cube(1), Transform::IDENTITY, Filter::ALL) {
                    added.push(index);
                }
            } else {
                let victim = added.remove(added.len() / 2);
                world.remove_shape(handle, victim);
            }
        }

        let seen: Vec<Collider> = world.colliders().collect();
        for pair in seen.windows(2) {
            prop_assert!(pair[0] < pair[1], "collider order must be strictly ascending");
        }
        prop_assert_eq!(seen.len(), added.len(), "every live shape appears exactly once");
    }
}
