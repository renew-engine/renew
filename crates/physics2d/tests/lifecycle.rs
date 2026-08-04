//! The identity and ordering rules, which everything else rests on.
//!
//! These are the properties that cannot be recovered later: if a shape index
//! moves, or collider order depends on insertion order, then every contact
//! report downstream is a different report on a different machine, and no
//! amount of care in the geometry fixes it.

use proptest::prelude::*;
use renew_ecs::Entities;
use renew_fixed::{Fixed, Vec2};
use renew_physics2d::{BodyKind, Collider, Filter, HandleState, Shape, Transform, World};

fn circle(units: i32) -> Shape {
    Shape::Circle {
        radius: Fixed::from_int(units),
    }
}

fn at(x: i32, y: i32) -> Transform {
    Transform::at(Vec2::new(Fixed::from_int(x), Fixed::from_int(y)))
}

/// **The property the whole identity scheme rests on.** A removed shape leaves
/// a hole, and the shapes after it keep their indices.
///
/// The tempting implementation — the one the nearest dense-array idiom
/// suggests — moves the last shape into the hole. That renumbers a collider
/// that nobody touched, which silently re-keys its contacts.
#[test]
fn removing_a_shape_does_not_renumber_the_others() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let body = world
        .create_body(entities.spawn(), BodyKind::Kinematic, Transform::IDENTITY)
        .expect("a fresh entity has no body");

    let first = world
        .add_shape(body, circle(1), at(0, 0), Filter::ALL)
        .expect("live body");
    let second = world
        .add_shape(body, circle(2), at(1, 0), Filter::ALL)
        .expect("live body");
    let third = world
        .add_shape(body, circle(3), at(2, 0), Filter::ALL)
        .expect("live body");
    assert_eq!((first.get(), second.get(), third.get()), (0, 1, 2));

    assert!(world.remove_shape(body, second));

    // The third shape is still index 2, and it is still the radius-3 circle.
    let (shape, _, _) = world
        .shape(Collider {
            handle: body,
            index: third,
        })
        .expect("the third shape kept its index");
    assert_eq!(shape, circle(3));

    // And the hole is genuinely a hole, not a shifted-down neighbour.
    assert!(
        world
            .shape(Collider {
                handle: body,
                index: second
            })
            .is_none(),
        "index 1 must be empty, not occupied by what used to be index 2"
    );

    // The extent still spans the hole, because the highest occupied index is
    // what bounds iteration — not the count of live shapes.
    assert_eq!(world.shape_extent(body), Some(3));
}

/// A new shape fills the lowest free hole, so indices stay dense from the
/// bottom without any of them ever moving.
#[test]
fn a_new_shape_fills_the_lowest_free_hole() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let body = world
        .create_body(entities.spawn(), BodyKind::Static, Transform::IDENTITY)
        .expect("fresh");

    let indices: Vec<_> = (1..=5)
        .map(|units| {
            world
                .add_shape(body, circle(units), at(0, 0), Filter::ALL)
                .expect("live")
        })
        .collect();
    assert_eq!(indices[4].get(), 4);

    // Punch two holes, freeing the higher one first so that "lowest" is a
    // claim about ordering rather than about recency.
    assert!(world.remove_shape(body, indices[3]));
    assert!(world.remove_shape(body, indices[1]));

    let first_refill = world
        .add_shape(body, circle(6), at(0, 0), Filter::ALL)
        .expect("live");
    assert_eq!(
        first_refill, indices[1],
        "the lowest free hole, not the newest"
    );

    let second_refill = world
        .add_shape(body, circle(7), at(0, 0), Filter::ALL)
        .expect("live");
    assert_eq!(second_refill, indices[3], "then the next one up");

    // With no holes left, the next shape extends the list.
    let appended = world
        .add_shape(body, circle(8), at(0, 0), Filter::ALL)
        .expect("live");
    assert_eq!(appended.get(), 5);
    assert_eq!(world.shape_extent(body), Some(6));
}

/// The four handle cases, each with the answer the vocabulary gives it.
#[test]
fn a_handle_is_live_stale_or_unknown() {
    let mut entities = Entities::new();
    let mut world = World::new();

    let handle = entities.spawn();
    assert_eq!(
        world.handle_state(handle),
        HandleState::Unknown,
        "an entity with no body here"
    );

    let body = world
        .create_body(handle, BodyKind::Kinematic, Transform::IDENTITY)
        .expect("fresh");
    assert_eq!(world.handle_state(body), HandleState::Live);

    // The case the stored generation exists for: despawn, and the allocator
    // hands the same *index* back at a new generation.
    entities.despawn(handle);
    let recycled = entities.spawn();
    assert_eq!(
        recycled.index(),
        handle.index(),
        "the allocator reused the slot, which is the scenario"
    );
    assert_ne!(recycled.generation(), handle.generation());
    assert_eq!(
        world.handle_state(recycled),
        HandleState::Stale,
        "a reused index must not reach another body's data"
    );

    // And the operations refuse it rather than answering.
    assert!(!world.set_transform(recycled, at(5, 5)));
    assert!(
        world
            .add_shape(recycled, circle(1), at(0, 0), Filter::ALL)
            .is_none()
    );
    assert!(world.transform(recycled).is_none());

    // The original handle still works, because nothing about the ECS despawn
    // reached this world.
    assert_eq!(world.handle_state(handle), HandleState::Live);
    assert!(world.set_transform(handle, at(5, 5)));
}

/// Two bodies sharing a handle would make collider order non-total, and every
/// emitted ordering rests on it being total.
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

/// v0 has no solver, so a dynamic body is refused explicitly rather than
/// accepted and quietly treated as something else.
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

/// Destroying and recreating against a live entity gives back the same handle.
/// The incarnation is what stops the rebuilt collider being indistinguishable
/// from the one it replaced.
#[test]
fn a_rebuilt_collider_is_not_the_one_it_replaced() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();

    world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
    let index = world
        .add_shape(handle, circle(1), at(0, 0), Filter::ALL)
        .expect("live");
    let collider = Collider { handle, index };
    let before = world.incarnation(collider).expect("occupied");

    // Same index back, by the lowest-free-hole rule.
    assert!(world.remove_shape(handle, index));
    let again = world
        .add_shape(handle, circle(1), at(0, 0), Filter::ALL)
        .expect("live");
    assert_eq!(again, index, "the identity is reused, which is the problem");
    assert_ne!(
        world.incarnation(collider).expect("occupied"),
        before,
        "and the incarnation is what distinguishes them"
    );

    // Same again across a body destroy/create cycle.
    let across = world.incarnation(collider).expect("occupied");
    world.destroy_body(handle);
    world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
    world.add_shape(handle, circle(1), at(0, 0), Filter::ALL);
    assert_ne!(world.incarnation(collider).expect("occupied"), across);
}

/// A shape's placement is composed onto its body's, which is what lets one
/// body own two shapes in different places.
#[test]
fn a_shape_sits_where_its_local_transform_puts_it() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Kinematic, at(10, 0));

    let index = world
        .add_shape(handle, circle(1), at(0, 3), Filter::ALL)
        .expect("live");
    let world_transform = world
        .world_transform(Collider { handle, index })
        .expect("occupied");
    assert_eq!(world_transform.translation.x.trunc_int(), 10);
    assert_eq!(world_transform.translation.y.trunc_int(), 3);
}

proptest! {
    /// **Collider order must be a function of the collider set, not of the
    /// order it was built in.** Two worlds holding the same bodies and shapes
    /// iterate identically however they were assembled — which is what makes
    /// a contact array reproducible across machines that reached the same
    /// state by different routes.
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
                world.add_shape(handle, circle(1), at(0, 0), Filter::ALL);
            }
            world.colliders().collect::<Vec<_>>()
        };

        let ascending: Vec<usize> = (0..6).collect();
        let a = build(&ascending);
        let b = build(&permutation);
        // Same set of bodies, whatever order they were added in.
        prop_assert_eq!(a.len(), b.len());
        prop_assert_eq!(a, b, "iteration order leaked the insertion order");
    }

    /// However a body's shapes are added and removed, every live shape keeps a
    /// distinct index and the collider order stays strictly ascending.
    #[test]
    fn collider_order_is_strictly_ascending_under_any_history(
        operations in prop::collection::vec(prop::bool::ANY, 0..24)
    ) {
        let mut entities = Entities::new();
        let mut world = World::new();
        let handle = entities.spawn();
        world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);

        let mut added: Vec<renew_physics2d::ShapeIndex> = Vec::new();
        for add in operations {
            if add || added.is_empty() {
                if let Some(index) = world.add_shape(handle, circle(1), at(0, 0), Filter::ALL) {
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
