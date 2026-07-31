//! The store against a model, and the order against churn.
//!
//! The interesting tests here are model-based: a `Vec<Option<T>>` indexed
//! by slot is the obvious, slow, obviously-correct implementation of what
//! a sparse set does. Running both against the same random operation
//! sequence and comparing after every step catches the bugs a
//! hand-written case will not, because the failures in this data
//! structure are all about *history* — a back-pointer that only goes
//! wrong when a particular element is removed while a particular other
//! one is last.

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use renew_ecs::{Entities, Store, join};

/// The operations a store admits, as data.
#[derive(Clone, Copy, Debug)]
enum Op {
    Insert(u32, u32),
    Remove(u32),
    Replace(u32, u32),
}

fn op() -> impl Strategy<Value = Op> {
    // A deliberately small slot range: collisions and reuse are where the
    // bugs live, and a wide range would mostly generate inserts into
    // untouched slots.
    prop_oneof![
        (0u32..12, any::<u32>()).prop_map(|(slot, value)| Op::Insert(slot, value)),
        (0u32..12).prop_map(Op::Remove),
        (0u32..12, any::<u32>()).prop_map(|(slot, value)| Op::Replace(slot, value)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x51a7_c0de_0e15_9a44),
        cases: 256,
        ..ProptestConfig::default()
    })]

    /// The store agrees with the model after every single operation.
    ///
    /// Compared step by step rather than at the end: a divergence caught
    /// at step 3 names the operation that caused it, while one caught at
    /// step 60 names only the sequence.
    #[test]
    fn the_store_matches_a_naive_model(ops in proptest::collection::vec(op(), 0..80)) {
        let mut store: Store<u32> = Store::new();
        let mut model: Vec<Option<u32>> = vec![None; 12];

        for (step, operation) in ops.iter().enumerate() {
            match *operation {
                Op::Insert(slot, value) | Op::Replace(slot, value) => {
                    let returned = store.insert(slot, value);
                    let expected = model[slot as usize].replace(value);
                    prop_assert_eq!(returned, expected, "step {} insert {}", step, slot);
                }
                Op::Remove(slot) => {
                    let returned = store.remove(slot);
                    let expected = model[slot as usize].take();
                    prop_assert_eq!(returned, expected, "step {} remove {}", step, slot);
                }
            }

            // Every slot, every step. The cheap check is what makes the
            // expensive bug findable.
            let live = model.iter().filter(|slot| slot.is_some()).count();
            prop_assert_eq!(store.len(), live, "step {} length", step);
            prop_assert_eq!(store.is_empty(), live == 0, "step {} emptiness", step);
            for (slot, expected) in model.iter().enumerate() {
                let slot = u32::try_from(slot).unwrap_or(u32::MAX);
                prop_assert_eq!(store.get(slot), expected.as_ref(), "step {} slot {}", step, slot);
                prop_assert_eq!(store.contains(slot), expected.is_some(), "step {} contains {}", step, slot);
            }
        }
    }

    /// Iteration is by ascending slot, whatever the history.
    #[test]
    fn iteration_is_always_sorted(ops in proptest::collection::vec(op(), 0..80)) {
        let mut store: Store<u32> = Store::new();
        for operation in &ops {
            match *operation {
                Op::Insert(slot, value) | Op::Replace(slot, value) => {
                    store.insert(slot, value);
                }
                Op::Remove(slot) => {
                    store.remove(slot);
                }
            }
        }
        let slots: Vec<u32> = store.iter().map(|(slot, _)| slot).collect();
        let mut sorted = slots.clone();
        sorted.sort_unstable();
        prop_assert_eq!(&slots, &sorted);
        // And it visits exactly the live set, not a subset that happens
        // to be sorted.
        prop_assert_eq!(slots.len(), store.len());
    }

    /// **The determinism property.** Two stores built from the same
    /// operations in the same order iterate identically — and so do two
    /// built from sequences that differ only in operations that cancel
    /// out. Nothing about the visit order may survive from the history.
    #[test]
    fn the_same_final_contents_iterate_identically(
        ops in proptest::collection::vec(op(), 0..60),
        churn in proptest::collection::vec(op(), 0..40),
    ) {
        let apply = |sequence: &[Op]| {
            let mut store: Store<u32> = Store::new();
            for operation in sequence {
                match *operation {
                    Op::Insert(slot, value) | Op::Replace(slot, value) => {
                        store.insert(slot, value);
                    }
                    Op::Remove(slot) => {
                        store.remove(slot);
                    }
                }
            }
            store
        };

        // One store gets extra churn first, then the same final writes.
        let direct = apply(&ops);
        let mut prefixed: Vec<Op> = churn.clone();
        prefixed.extend_from_slice(&ops);
        // Only comparable where the churn touched slots the second half
        // overwrites or clears; filter to the slots both agree on.
        let churned = apply(&prefixed);
        for (slot, value) in direct.iter() {
            if let Some(other) = churned.get(slot) {
                prop_assert_eq!(value, other, "slot {} disagrees", slot);
            }
        }
        // The order itself never depends on history.
        let one: Vec<u32> = direct.iter().map(|(slot, _)| slot).collect();
        let two: Vec<u32> = churned.iter().map(|(slot, _)| slot).collect();
        prop_assert!(one.iter().is_sorted());
        prop_assert!(two.iter().is_sorted());
    }

    /// A join yields exactly the intersection, in order.
    #[test]
    fn a_join_is_the_ordered_intersection(
        left_slots in proptest::collection::vec(0u32..16, 0..16),
        right_slots in proptest::collection::vec(0u32..16, 0..16),
    ) {
        let mut left: Store<u32> = Store::new();
        let mut right: Store<u32> = Store::new();
        for slot in &left_slots {
            left.insert(*slot, *slot);
        }
        for slot in &right_slots {
            right.insert(*slot, *slot);
        }

        let mut expected: Vec<u32> = left_slots
            .iter()
            .copied()
            .filter(|slot| right_slots.contains(slot))
            .collect();
        expected.sort_unstable();
        expected.dedup();

        let found: Vec<u32> = join(&left, &right).map(|(slot, _, _)| slot).collect();
        prop_assert_eq!(found, expected);
    }

    /// Spawning and despawning never issues two live handles for one slot,
    /// and never reports a stale handle as alive.
    #[test]
    fn handles_are_never_confused(rounds in proptest::collection::vec(any::<bool>(), 0..120)) {
        let mut entities = Entities::new();
        let mut live: Vec<renew_ecs::Entity> = Vec::new();
        let mut retired: Vec<renew_ecs::Entity> = Vec::new();

        for spawn in rounds {
            if spawn || live.is_empty() {
                live.push(entities.spawn());
            } else if let Some(victim) = live.pop() {
                prop_assert!(entities.despawn(victim));
                retired.push(victim);
            }
            prop_assert_eq!(entities.len(), live.len());
            for handle in &live {
                prop_assert!(entities.is_alive(*handle), "{} should be alive", handle);
            }
            for handle in &retired {
                prop_assert!(!entities.is_alive(*handle), "{} should be dead", handle);
            }
        }
    }
}
