//! The rest of the operations, and what each refuses.
//!
//! Split from the identity tests because these are about *behaviour on bad
//! input* rather than about ordering, and the two fail for different reasons:
//! an identity bug corrupts every downstream report, while a refusal bug shows
//! up as a caller silently getting nothing.

use renew_ecs::Entities;
use renew_fixed::{Fixed, Vec2};
use renew_physics2d::{
    BodyKind, Collider, Filter, HandleState, Shape, ShapeIndex, Transform, World,
};

fn circle(units: i32) -> Shape {
    Shape::Circle {
        radius: Fixed::from_int(units),
    }
}

fn at(x: i32, y: i32) -> Transform {
    Transform::at(Vec2::new(Fixed::from_int(x), Fixed::from_int(y)))
}

/// Static bodies never move, so a contact between two of them can never change
/// — reporting it every step is noise a caller has to filter out forever.
#[test]
fn only_two_static_bodies_cannot_produce_a_contact() {
    use BodyKind::{Dynamic, Kinematic, Static};
    assert!(!Static.collides_with(Static));
    assert!(Static.collides_with(Kinematic));
    assert!(Kinematic.collides_with(Static));
    assert!(Kinematic.collides_with(Kinematic));
    // Named for the fork that grows one, even though v0 refuses to create it.
    assert!(Dynamic.collides_with(Static));
    assert!(Dynamic.collides_with(Dynamic));
}

/// A capsule needs both operands non-negative, and a zero-length one is a
/// circle rather than an error.
#[test]
fn a_capsule_validates_both_of_its_operands() {
    let ok = Shape::Capsule {
        radius: Fixed::from_int(1),
        half_height: Fixed::ZERO,
    };
    assert!(
        ok.is_valid(),
        "a zero-height capsule is a circle, not an error"
    );

    assert!(
        !Shape::Capsule {
            radius: Fixed::from_int(-1),
            half_height: Fixed::from_int(1),
        }
        .is_valid()
    );
    assert!(
        !Shape::Capsule {
            radius: Fixed::from_int(1),
            half_height: Fixed::from_int(-1),
        }
        .is_valid()
    );
}

/// Operands that are not a shape are refused at the door, rather than stored
/// and discovered by whatever tries to sweep against them.
#[test]
fn a_shape_with_negative_operands_is_refused() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let body = world
        .create_body(entities.spawn(), BodyKind::Static, Transform::IDENTITY)
        .expect("fresh");

    assert!(
        world
            .add_shape(body, circle(-1), at(0, 0), Filter::ALL)
            .is_none()
    );
    assert_eq!(world.shape_extent(body), Some(0), "and nothing was stored");
}

/// Replacing keeps the index — which is the point, since the index is identity
/// — and advances the incarnation, since the collider is not the one it was.
#[test]
fn replacing_a_shape_keeps_its_index_and_changes_its_incarnation() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
    let index = world
        .add_shape(handle, circle(1), at(0, 0), Filter::ALL)
        .expect("live");
    let collider = Collider { handle, index };
    let before = world.incarnation(collider).expect("occupied");

    assert!(world.replace_shape(handle, index, circle(7), at(2, 2)));
    let (shape, local, _) = world.shape(collider).expect("still occupied");
    assert_eq!(shape, circle(7));
    assert_eq!(local.translation.x.trunc_int(), 2);
    let after = world.incarnation(collider).expect("occupied");
    assert_ne!(after, before);
    assert_ne!(after.get(), before.get(), "and the raw counts differ too");

    // Refusals: a hole, an index past the end, an invalid shape, a dead body.
    assert!(!world.replace_shape(handle, index, circle(-1), at(0, 0)));
    assert!(!world.replace_shape(handle, ShapeIndex::from_raw(9), circle(1), at(0, 0)));
    assert!(world.remove_shape(handle, index));
    assert!(
        !world.replace_shape(handle, index, circle(1), at(0, 0)),
        "a hole is not a shape to replace"
    );
}

/// A filter change does not advance the incarnation: the collider is the same
/// one it was, so a contact that persists across the change is a persisting
/// contact rather than a new one.
#[test]
fn changing_a_filter_does_not_change_the_collider() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Kinematic, Transform::IDENTITY);
    let index = world
        .add_shape(handle, circle(1), at(0, 0), Filter::ALL)
        .expect("live");
    let collider = Collider { handle, index };
    let before = world.incarnation(collider).expect("occupied");

    assert!(world.set_filter(handle, index, Filter::NONE));
    let (_, _, filter) = world.shape(collider).expect("occupied");
    assert_eq!(filter, Filter::NONE);
    assert_eq!(
        world.incarnation(collider).expect("occupied"),
        before,
        "a filter change is not a rebuild"
    );

    // Refusals: a hole, and an index past the end.
    assert!(!world.set_filter(handle, ShapeIndex::from_raw(9), Filter::ALL));
    assert!(world.remove_shape(handle, index));
    assert!(!world.set_filter(handle, index, Filter::ALL));
}

/// Destroying takes the body's shapes with it, and the handle stops answering.
#[test]
fn destroying_a_body_takes_its_shapes_and_stops_answering() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Kinematic, at(1, 2));
    let index = world
        .add_shape(handle, circle(1), at(0, 0), Filter::ALL)
        .expect("live");

    assert_eq!(world.kind(handle), Some(BodyKind::Kinematic));
    assert!(world.destroy_body(handle));
    assert_eq!(world.body_count(), 0);
    assert_eq!(world.handle_state(handle), HandleState::Unknown);
    assert!(world.kind(handle).is_none());
    assert!(world.transform(handle).is_none());
    assert!(world.shape(Collider { handle, index }).is_none());
    assert!(world.world_transform(Collider { handle, index }).is_none());
    assert!(world.shape_extent(handle).is_none());
    assert_eq!(world.colliders().count(), 0);

    // And destroying twice is a no-op rather than an error, because two
    // systems both deciding something should go is ordinary.
    assert!(!world.destroy_body(handle));
}

/// Every operation refuses a handle this world has never seen, rather than
/// panicking or growing storage for it.
#[test]
fn an_unknown_handle_is_refused_by_every_operation() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let stranger = entities.spawn();

    assert_eq!(world.handle_state(stranger), HandleState::Unknown);
    assert!(!world.destroy_body(stranger));
    assert!(!world.set_transform(stranger, at(1, 1)));
    assert!(
        world
            .add_shape(stranger, circle(1), at(0, 0), Filter::ALL)
            .is_none()
    );
    assert!(!world.remove_shape(stranger, ShapeIndex::from_raw(0)));
    assert!(!world.replace_shape(stranger, ShapeIndex::from_raw(0), circle(1), at(0, 0)));
    assert!(!world.set_filter(stranger, ShapeIndex::from_raw(0), Filter::ALL));
    assert!(world.kind(stranger).is_none());
    assert!(world.shape_extent(stranger).is_none());
    assert!(
        world
            .incarnation(Collider {
                handle: stranger,
                index: ShapeIndex::from_raw(0)
            })
            .is_none()
    );
}

/// Removing something that is not there is a no-op rather than an error, and
/// removing past the end does not grow the list.
#[test]
fn removing_nothing_is_a_no_op() {
    let mut entities = Entities::new();
    let mut world = World::new();
    let handle = entities.spawn();
    world.create_body(handle, BodyKind::Static, Transform::IDENTITY);

    assert!(!world.remove_shape(handle, ShapeIndex::from_raw(0)));
    assert!(!world.remove_shape(handle, ShapeIndex::from_raw(4096)));
    assert_eq!(world.shape_extent(handle), Some(0));

    let index = world
        .add_shape(handle, circle(1), at(0, 0), Filter::ALL)
        .expect("live");
    assert!(world.remove_shape(handle, index));
    assert!(!world.remove_shape(handle, index), "twice is a no-op");
}

/// Bodies are stored by entity index, and creating one at a high index must not
/// disturb the ones below it or invent bodies in the gap.
#[test]
fn a_sparse_index_space_does_not_invent_bodies() {
    let mut entities = Entities::new();
    let mut world = World::new();

    // Burn some indices so the third entity is not adjacent to the first.
    let first = entities.spawn();
    for _ in 0..5 {
        let _ = entities.spawn();
    }
    let distant = entities.spawn();
    assert!(distant.index() > first.index() + 1);

    world.create_body(first, BodyKind::Static, Transform::IDENTITY);
    world.create_body(distant, BodyKind::Static, Transform::IDENTITY);
    world.add_shape(first, circle(1), at(0, 0), Filter::ALL);
    world.add_shape(distant, circle(1), at(0, 0), Filter::ALL);

    assert_eq!(world.body_count(), 2);
    let seen: Vec<_> = world.colliders().collect();
    assert_eq!(seen.len(), 2, "the gap contributed nothing");
    assert!(seen[0] < seen[1], "and they came out in index order");
}
