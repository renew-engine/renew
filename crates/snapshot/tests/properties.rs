//! Properties of the classification, over arbitrary capture sequences.
//!
//! The unit tests pin named cases; these pin the rules those cases are
//! instances of. The one that matters most is exclusivity: every row is
//! exactly one of living, newborn or dying, and which one is decided by
//! the generations alone.

use proptest::prelude::*;
use renew_math::Alpha;
use renew_snapshot::{Fate, Key, Snapshots};

const SLOTS: u32 = 12;

/// One capture: a set of (slot, generation, value), with slots made
/// unique because putting a slot twice is a refusal by contract rather
/// than a case to explore.
fn capture_strategy() -> impl Strategy<Value = Vec<(u32, u64, f32)>> {
    prop::collection::vec((0..SLOTS, 0..4u64, -1000.0f32..1000.0), 0..8).prop_map(|mut rows| {
        rows.sort_by_key(|&(slot, _, _)| slot);
        rows.dedup_by_key(|&mut (slot, _, _)| slot);
        rows
    })
}

/// The step length every alpha below is a fraction of. Written as a
/// total `match` rather than an `expect`: this is a helper rather than a
/// test function, so the crate's in-test allowances do not reach it, and
/// a fallible construction of a literal is worth avoiding anyway.
const STEP_NANOS: core::num::NonZeroU64 = match core::num::NonZeroU64::new(1000) {
    Some(value) => value,
    None => core::num::NonZeroU64::MIN,
};

fn alpha_strategy() -> impl Strategy<Value = Alpha> {
    (0..1000u64).prop_map(|n| Alpha::new(n, STEP_NANOS))
}

proptest! {
    /// Every row is exactly one fate, and the fate is a function of the
    /// two captures' generations at that slot — nothing else.
    #[test]
    fn classification_is_total_and_exclusive(
        first in capture_strategy(),
        second in capture_strategy(),
        alpha in alpha_strategy(),
    ) {
        let mut pair = Snapshots::<f32>::new(SLOTS);
        {
            let mut capture = pair.capture();
            for &(slot, generation, value) in &first {
                capture.put(Key::new(slot, generation), value);
            }
        }
        {
            let mut capture = pair.capture();
            for &(slot, generation, value) in &second {
                capture.put(Key::new(slot, generation), value);
            }
        }
        let rows: Vec<_> = pair.frame(alpha).collect();

        for row in &rows {
            let was = first.iter().find(|&&(slot, _, _)| slot == row.key.slot);
            let is = second.iter().find(|&&(slot, _, _)| slot == row.key.slot);
            match row.fate {
                Fate::Living => {
                    let (_, old_generation, _) = *was.expect("living implies it was there before");
                    let (_, new_generation, _) = *is.expect("living implies it is there now");
                    prop_assert_eq!(old_generation, new_generation, "living means the tenant stayed");
                    prop_assert_eq!(row.key.generation, new_generation);
                }
                Fate::Newborn => {
                    let (_, new_generation, new_value) =
                        *is.expect("a newborn is in the newer capture");
                    prop_assert_eq!(row.key.generation, new_generation);
                    prop_assert_eq!(row.value.to_bits(), new_value.to_bits(),
                        "a newborn stands at its one known tick, never blended");
                    if let Some(&(_, old_generation, _)) = was {
                        prop_assert_ne!(old_generation, new_generation,
                            "a newborn at an occupied slot means the tenant changed");
                    }
                }
                Fate::Dying => {
                    let (_, old_generation, old_value) =
                        *was.expect("a dying row is in the older capture");
                    prop_assert_eq!(row.key.generation, old_generation);
                    prop_assert_eq!(row.value.to_bits(), old_value.to_bits(),
                        "a dying row is its last capture, never blended");
                    if let Some(&(_, new_generation, _)) = is {
                        prop_assert_ne!(old_generation, new_generation);
                    }
                }
            }
        }
    }

    /// Every slot in the newer capture appears exactly once, in put
    /// order, and every dying row precedes every one of them.
    #[test]
    fn each_current_slot_appears_once_in_put_order_after_the_dying(
        first in capture_strategy(),
        second in capture_strategy(),
        alpha in alpha_strategy(),
    ) {
        let mut pair = Snapshots::<f32>::new(SLOTS);
        {
            let mut capture = pair.capture();
            for &(slot, generation, value) in &first {
                capture.put(Key::new(slot, generation), value);
            }
        }
        {
            let mut capture = pair.capture();
            for &(slot, generation, value) in &second {
                capture.put(Key::new(slot, generation), value);
            }
        }
        let rows: Vec<_> = pair.frame(alpha).collect();

        let living_order: Vec<u32> = rows
            .iter()
            .filter(|row| row.fate != Fate::Dying)
            .map(|row| row.key.slot)
            .collect();
        let put_order: Vec<u32> = second.iter().map(|&(slot, _, _)| slot).collect();
        prop_assert_eq!(living_order, put_order, "put order survives, and nothing is dropped");

        let last_dying = rows.iter().rposition(|row| row.fate == Fate::Dying);
        let first_other = rows.iter().position(|row| row.fate != Fate::Dying);
        if let (Some(last), Some(first)) = (last_dying, first_other) {
            prop_assert!(last < first, "the departing draw underneath the living");
        }
    }

    /// At the tick boundary every living value is bit-exactly its earlier
    /// capture — the guarantee committed images stand on.
    #[test]
    fn the_tick_boundary_is_the_earlier_capture_exactly(
        first in capture_strategy(),
        second in capture_strategy(),
    ) {
        let mut pair = Snapshots::<f32>::new(SLOTS);
        {
            let mut capture = pair.capture();
            for &(slot, generation, value) in &first {
                capture.put(Key::new(slot, generation), value);
            }
        }
        {
            let mut capture = pair.capture();
            for &(slot, generation, value) in &second {
                capture.put(Key::new(slot, generation), value);
            }
        }
        for row in pair.frame(Alpha::ZERO) {
            if row.fate == Fate::Living {
                let (_, _, was) = *first
                    .iter()
                    .find(|&&(slot, _, _)| slot == row.key.slot)
                    .expect("living implies it was there");
                prop_assert_eq!(row.value.to_bits(), was.to_bits());
            }
        }
    }
}
