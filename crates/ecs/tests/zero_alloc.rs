//! What storage costs the heap in a steady state, measured rather than
//! assumed — and the one call that costs it something, pinned so the
//! cost cannot go quiet.
//!
//! **Why this crate needed a gate before it needed a fix.** The steady
//! frame loop is meant to reach the heap never, and this is the crate a
//! game touches most: every system that walks components in a defined
//! order goes through here, once per system per step. Until now nothing
//! here counted allocations at all, so the budget was stated in the
//! standards and unenforced exactly where it was most likely to be spent.
//! The measurement gap was the worse half of the problem: without it, the
//! day the budget binds, an overrun surfaces as a diffuse regression
//! across every system at once instead of as one identifiable call.
//!
//! **The ordered mutable walk allocates, and that is recorded rather than
//! hidden.** It collects the visit order before handing out any `&mut`,
//! because deciding the order borrows the same structure the values live
//! in and the honest alternative was `unsafe`. That trade was written
//! down when it was made; this file makes it *visible*, so the fix — a
//! cached visit order, invalidated on structural change — can be judged
//! against a number rather than an argument. The test that pins it fails
//! the day the allocation goes away, which is the point: it is a
//! tripwire on a known cost, not an endorsement of it.
//!
//! One `#[global_allocator]` is process-wide, so the whole file is one
//! sequence of measured windows rather than several racing tests.

use renew_ecs::{Entities, Store};
use renew_memory::{CountingAllocator, counters};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Enough slots that the sparse array is grown well before any window
/// opens, and small enough that the test stays instant.
const SLOTS: u32 = 64;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_steady_state_costs_the_heap_nothing_except_the_ordered_mutable_walk() {
    // Everything allowed to allocate happens out here: the allocator
    // grows both containers to their high-water mark, and nothing below
    // exceeds it.
    let mut entities = Entities::new();
    let mut store: Store<u32> = Store::new();
    let mut live: Vec<_> = Vec::with_capacity(SLOTS as usize);
    for value in 0..SLOTS {
        let entity = entities.spawn();
        store.insert(entity.index(), value);
        live.push(entity);
    }
    assert_eq!(store.len(), SLOTS as usize, "the fixture really filled");

    // --- Reading, in order and out of it, and by slot.
    let mut seen = 0u64;
    let verdict = counters::quiet_window(5, || {
        for _ in 0..8 {
            for (slot, value) in store.iter() {
                seen = seen
                    .wrapping_add(u64::from(slot))
                    .wrapping_add(u64::from(*value));
            }
            for value in store.iter_unordered() {
                seen = seen.wrapping_add(u64::from(*value));
            }
            for entity in &live {
                if let Some(value) = store.get(entity.index()) {
                    seen = seen.wrapping_add(u64::from(*value));
                }
            }
        }
    });
    verdict.expect("reading storage stays heap-silent");
    assert!(seen > 0, "the windowed reads really visited components");

    // --- Mutating in place, which is the other half of a system's work.
    let verdict = counters::quiet_window(5, || {
        for round in 0..8u32 {
            for entity in &live {
                if let Some(value) = store.get_mut(entity.index()) {
                    *value = value.wrapping_add(round);
                }
            }
        }
    });
    verdict.expect("mutating components in place stays heap-silent");

    // --- Churn within the high-water mark: a slot freed and refilled is
    // the shape a spawner and a despawner make between them, and it must
    // not reach for the allocator to do it.
    let verdict = counters::quiet_window(5, || {
        for _ in 0..8 {
            let doomed = live.pop().expect("the fixture is not empty");
            store.remove(doomed.index());
            assert!(entities.despawn(doomed));
            let reborn = entities.spawn();
            store.insert(reborn.index(), 7);
            live.push(reborn);
        }
    });
    verdict.expect("churn inside the high-water mark stays heap-silent");
    assert_eq!(
        store.len(),
        SLOTS as usize,
        "churn left the population where it was"
    );

    // --- And the one that does allocate. This is a tripwire on a
    // recorded cost: it fails if the walk becomes free, which is the
    // signal that the cached-order fix landed and this file should stop
    // claiming otherwise.
    let mut touched = 0u32;
    let verdict = counters::quiet_window(1, || {
        store.for_each_mut(|_, value| {
            touched += 1;
            *value = value.wrapping_add(1);
        });
    });
    assert_eq!(touched, SLOTS, "the walk really visited every component");
    assert!(
        verdict.is_err(),
        "the ordered mutable walk no longer allocates — if that is deliberate, this \
         expectation is the stale half and should become an assertion that it \
         stays free rather than one that it is not"
    );
}
