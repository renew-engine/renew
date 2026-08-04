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
// The determinism rule in the language standard: a simulation crate does not
// perform floating-point arithmetic whose result can reach digested state.
// Denied here rather than left to review — the lint covers operators only, so
// it is necessary and not sufficient, but what it does cover it covers with
// teeth.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::float_arithmetic)]

mod entity;
mod store;

pub use entity::{Entities, Entity};
pub use store::Store;

/// Every entity present in both stores, in ascending slot order.
///
/// Walks whichever side is cheaper to scan and probes the other, which is
/// the whole reason a sparse set is worth having: membership is an array
/// lookup, so a join costs one scan rather than the product. The order is
/// the same promise the stores make on their own.
///
/// **Cheaper means the slot span, not the component count.** [`Store::iter`]
/// walks `sparse`, so its cost is the highest slot ever occupied — a store
/// holding three components scattered across a million slots is expensive
/// to walk and still O(1) to probe. Choosing by component count would pick
/// the wrong side exactly when the difference is worth having.
///
/// Both directions yield identical sequences, so which one runs is
/// invisible to callers and cannot reach the state digest.
///
/// Deliberately a free function rather than a method: it belongs to
/// neither store, and making it one store's method would suggest an
/// asymmetry the operation does not have.
pub fn join<'a, A, B>(
    left: &'a Store<A>,
    right: &'a Store<B>,
) -> impl Iterator<Item = (u32, &'a A, &'a B)> {
    // `iter` is already slot-ordered and filtering preserves that, so both
    // arms emit ascending slots over the same intersection.
    if walks_left(left, right) {
        Join::FromLeft(
            left.iter()
                .filter_map(move |(slot, value)| Some((slot, value, right.get(slot)?))),
        )
    } else {
        Join::FromRight(
            right
                .iter()
                .filter_map(move |(slot, value)| Some((slot, left.get(slot)?, value))),
        )
    }
}

/// Whether [`join`] will walk `left` rather than `right`.
///
/// Split out and tested directly because the choice is a cost decision
/// with no observable effect on results: both arms emit the identical
/// sequence, so no test over the output can tell which one ran. Asserting
/// the predicate is the only guard that would have caught the original
/// defect, where the doc promised a choice the code never made.
fn walks_left<A, B>(left: &Store<A>, right: &Store<B>) -> bool {
    left.scan_len() <= right.scan_len()
}

/// A join walked from one side or the other.
///
/// An enum rather than a boxed iterator because the steady-state frame
/// loop allocates nothing, and rather than always walking one side because
/// that is the choice being made. Both variants carry the same item type,
/// so the branch is a cost decision and never a behavioural one.
enum Join<L, R> {
    FromLeft(L),
    FromRight(R),
}

impl<'a, A: 'a, B: 'a, L, R> Iterator for Join<L, R>
where
    L: Iterator<Item = (u32, &'a A, &'a B)>,
    R: Iterator<Item = (u32, &'a A, &'a B)>,
{
    type Item = (u32, &'a A, &'a B);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::FromLeft(iter) => iter.next(),
            Self::FromRight(iter) => iter.next(),
        }
    }
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

    /// Regression: the doc promised the join walks the cheaper side and
    /// the code walked `left` unconditionally, so a wide-span left store
    /// paid for every empty slot while a one-slot right store sat unused.
    /// Both directions must produce the identical sequence, or the choice
    /// would be observable — and a cost decision that changes results is
    /// not a cost decision.
    #[test]
    fn a_join_yields_the_same_sequence_from_either_side() {
        let mut wide: Store<u32> = Store::new();
        let mut narrow: Store<u32> = Store::new();
        // `wide` spans far more slots than it holds components; `narrow`
        // is dense and low. The join must pick `narrow` to walk.
        for slot in [0u32, 3, 40_000] {
            wide.insert(slot, slot);
        }
        for slot in [0u32, 3] {
            narrow.insert(slot, slot * 10);
        }
        assert!(narrow.scan_len() < wide.scan_len());
        // The guard that actually bites: results agree either way, so only
        // the choice itself distinguishes the fix from the defect.
        assert!(!walks_left(&wide, &narrow));
        assert!(walks_left(&narrow, &wide));

        let forward: Vec<(u32, u32, u32)> = join(&wide, &narrow)
            .map(|(slot, a, b)| (slot, *a, *b))
            .collect();
        let backward: Vec<(u32, u32, u32)> = join(&narrow, &wide)
            .map(|(slot, a, b)| (slot, *b, *a))
            .collect();

        assert_eq!(forward, vec![(0, 0, 0), (3, 3, 30)]);
        assert_eq!(forward, backward);
    }

    /// The walk cost is the slot span, not the component count, so a store
    /// with fewer components can still be the expensive side. This is the
    /// distinction that made the original comment wrong even in intent.
    #[test]
    fn scan_cost_follows_slot_span_not_component_count() {
        let mut few_but_scattered: Store<u32> = Store::new();
        few_but_scattered.insert(0, 0);
        few_but_scattered.insert(50_000, 1);

        let mut many_but_packed: Store<u32> = Store::new();
        for slot in 0..1_000u32 {
            many_but_packed.insert(slot, slot);
        }

        assert!(few_but_scattered.len() < many_but_packed.len());
        assert!(few_but_scattered.scan_len() > many_but_packed.scan_len());
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
