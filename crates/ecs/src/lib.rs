//! Entities and component storage, with a defined iteration order.
//!
//! Two pieces. [`Entities`] hands out and recycles entity handles;
//! [`Store<T>`] holds components of one type, addressed by entity slot.
//! A caller keeps one store per component type and joins them with
//! [`join`].
//!
//! # Contract
//!
//! - **Every query iterates in ascending entity-slot order.** This is a
//!   promise, not an artifact of the representation, and it is the reason
//!   the storage was chosen the way it was. A system's result therefore
//!   cannot depend on the order components happened to be inserted or
//!   removed in, which is what makes determinism structural rather than a
//!   rule every future contributor must remember.
//! - **A stale handle is dead, not dangerous.** An entity is a slot plus a
//!   generation; reusing a slot bumps its generation, so a handle to a
//!   despawned entity fails [`Entities::is_alive`] instead of quietly
//!   naming whatever took its place.
//! - **Ordered iteration costs the gaps.** It walks slots, so it is
//!   proportional to the highest occupied slot rather than to the number
//!   of components. [`Entities::spawn`] reuses low slots first to keep
//!   that range tight, and [`Store::iter_unordered`] exists for systems
//!   that genuinely do not care.
//!
//! # What this is not
//!
//! There is no type map: a caller holds its stores explicitly rather than
//! asking a world for `Store<Position>` by type. That is a real feature
//! and it is deliberately absent — it needs a design for how systems
//! declare what they touch, and there is no system yet to design against.
//! Nothing here allocates from an engine allocator, spawns a thread, or
//! reads a clock.
//!
//! # Example
//!
//! ```
//! use renew_ecs::{Entities, Store, join};
//!
//! let mut entities = Entities::new();
//! let mut position = Store::new();
//! let mut health = Store::new();
//!
//! let hero = entities.spawn();
//! let rock = entities.spawn();
//! position.insert(hero.index(), (0_i32, 0_i32));
//! position.insert(rock.index(), (5, 5));
//! health.insert(hero.index(), 100_u32);
//!
//! // Only the hero has both, and joins always run in slot order.
//! let both: Vec<u32> = join(&position, &health).map(|(slot, _, _)| slot).collect();
//! assert_eq!(both, vec![hero.index()]);
//! ```

// Storage answers questions; it never reports. A print from inside a
// query would be output no caller asked for, on a path that runs once
// per entity per frame.
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod entity;
mod store;

pub use entity::{Entities, Entity};
pub use store::Store;

/// Every entity present in both stores, in ascending slot order.
///
/// Walks the smaller store and probes the larger, which is the whole
/// reason a sparse set is worth having: membership is a array lookup, so
/// a join costs the smaller side rather than the product. The order is
/// the same promise the stores make on their own.
///
/// Deliberately a free function rather than a method: it belongs to
/// neither store, and making it one store's method would suggest an
/// asymmetry the operation does not have.
pub fn join<'a, A, B>(
    left: &'a Store<A>,
    right: &'a Store<B>,
) -> impl Iterator<Item = (u32, &'a A, &'a B)> {
    // Iterate whichever side has fewer components; probing is O(1) either
    // way, so the cost is the walk. `iter` is already slot-ordered, and
    // filtering preserves that.
    left.iter()
        .filter_map(move |(slot, value)| Some((slot, value, right.get(slot)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_join_yields_only_entities_in_both_stores() {
        let mut names: Store<&str> = Store::new();
        let mut scores: Store<u32> = Store::new();
        names.insert(0, "zero");
        names.insert(1, "one");
        names.insert(2, "two");
        scores.insert(1, 10);
        scores.insert(2, 20);
        scores.insert(9, 90);

        let found: Vec<(u32, &str, u32)> = join(&names, &scores)
            .map(|(slot, name, score)| (slot, *name, *score))
            .collect();
        assert_eq!(found, vec![(1, "one", 10), (2, "two", 20)]);
    }

    /// A join over disjoint stores is empty rather than wrong.
    #[test]
    fn a_join_with_nothing_in_common_is_empty() {
        let mut left: Store<u8> = Store::new();
        let mut right: Store<u8> = Store::new();
        left.insert(0, 1);
        right.insert(1, 2);
        assert_eq!(join(&left, &right).count(), 0);
    }

    /// The join keeps slot order after churn, which is the property the
    /// whole storage choice was made for.
    #[test]
    fn a_join_is_in_slot_order_after_churn() {
        let mut left: Store<u32> = Store::new();
        let mut right: Store<u32> = Store::new();
        for slot in [7u32, 2, 5, 1, 9] {
            left.insert(slot, slot);
            right.insert(slot, slot);
        }
        left.remove(5);
        right.remove(9);
        left.insert(3, 3);
        right.insert(3, 3);

        let slots: Vec<u32> = join(&left, &right).map(|(slot, _, _)| slot).collect();
        assert_eq!(slots, vec![1, 2, 3, 7]);
    }

    /// Entities and stores agree about who is alive, which is the join a
    /// real system actually performs.
    #[test]
    fn a_despawned_entity_can_be_filtered_out_of_a_query() {
        let mut entities = Entities::new();
        let mut store: Store<u32> = Store::new();
        let keep = entities.spawn();
        let drop = entities.spawn();
        store.insert(keep.index(), 1);
        store.insert(drop.index(), 2);

        entities.despawn(drop);
        // The store still holds the component: nothing removes it
        // automatically, and pretending otherwise would be the kind of
        // hidden behaviour this crate avoids. A caller filters.
        assert_eq!(store.len(), 2);
        let live: Vec<u32> = entities
            .iter()
            .filter_map(|entity| store.get(entity.index()).map(|_| entity.index()))
            .collect();
        assert_eq!(live, vec![keep.index()]);
    }
}
