//! Property-based tests for the allocators (required for this crate
//! class). Models are plain vectors; the properties are exact.

use proptest::prelude::*;
use renew_memory::{LinearArena, Pool};

proptest! {
    #[test]
    fn arena_round_trips_every_value(values in prop::collection::vec(any::<u64>(), 0..64)) {
        let arena = LinearArena::with_capacity(1024);
        let mut stored = Vec::new();
        for value in &values {
            match arena.alloc(*value) {
                Some(slot) => stored.push((*value, slot)),
                None => break,
            }
        }
        // Everything that fit reads back exactly, through disjoint slots.
        for (expected, slot) in &stored {
            prop_assert_eq!(*expected, **slot);
        }
        prop_assert!(arena.used() <= arena.capacity());
        prop_assert!(arena.high_water() >= arena.used());
    }

    #[test]
    fn arena_slices_round_trip(chunks in prop::collection::vec(prop::collection::vec(any::<u16>(), 0..32), 0..16)) {
        let arena = LinearArena::with_capacity(4096);
        let mut stored = Vec::new();
        for chunk in &chunks {
            match arena.alloc_slice(chunk) {
                Some(slice) => stored.push((chunk.clone(), slice)),
                None => break,
            }
        }
        for (expected, slice) in &stored {
            prop_assert_eq!(expected.as_slice(), &**slice);
        }
    }

    #[test]
    fn arena_addresses_are_always_aligned(
        capacity in 0usize..512,
        operations in prop::collection::vec(any::<bool>(), 0..64),
    ) {
        // Interleave u8 and u64 allocations at arbitrary capacities:
        // every returned u64 address must be 8-aligned, every u64 slice
        // likewise — including the zero-capacity and empty-slice edges.
        let arena = LinearArena::with_capacity(capacity);
        for wide in operations {
            if wide {
                if let Some(slot) = arena.alloc(0xAAu64) {
                    prop_assert_eq!((core::ptr::from_mut(slot) as usize) % 8, 0);
                }
            } else if let Some(byte) = arena.alloc(0x55u8) {
                prop_assert_eq!(*byte, 0x55);
            }
        }
        let empty = arena.alloc_slice::<u64>(&[]);
        if let Some(slice) = empty {
            prop_assert_eq!((slice.as_ptr() as usize) % 8, 0);
        }
    }

    #[test]
    fn arena_exhaustion_is_exact_for_uniform_allocations(capacity in 0usize..256) {
        // With an aligned base and eight-byte values, exactly
        // capacity / 8 allocations fit — no more, no fewer — and the
        // first failure leaves less than eight bytes of headroom.
        let arena = LinearArena::with_capacity(capacity);
        let mut fitted = 0usize;
        while arena.alloc(fitted as u64).is_some() {
            fitted += 1;
            prop_assert!(fitted <= capacity / 8, "over-fitted");
        }
        prop_assert_eq!(fitted, capacity / 8);
        prop_assert!(capacity - arena.used() < 8);
        // Exhaustion is total: nothing of that size fits again...
        prop_assert!(arena.alloc(0u64).is_none());
        // ...until reset, after which the same count fits again.
        let mut arena = arena;
        arena.reset();
        let mut refitted = 0usize;
        while arena.alloc(0u64).is_some() {
            refitted += 1;
        }
        prop_assert_eq!(refitted, capacity / 8);
    }

    #[test]
    fn arena_reset_makes_room_again(sizes in prop::collection::vec(1usize..64, 1..32)) {
        let mut arena = LinearArena::with_capacity(256);
        for &size in &sizes {
            let _ = arena.alloc_slice(&vec![0u8; size]);
        }
        let water = arena.high_water();
        arena.reset();
        prop_assert_eq!(arena.used(), 0);
        prop_assert_eq!(arena.high_water(), water);
        // After reset the full capacity is available again.
        prop_assert!(arena.alloc_slice(&vec![7u8; 256]).is_some());
    }

    #[test]
    fn pool_agrees_with_a_model_under_random_operations(
        operations in prop::collection::vec(prop::option::of(any::<u64>()), 1..128),
    ) {
        // Some(value) = insert; None = remove the oldest live handle.
        let mut pool: Pool<u64> = Pool::with_capacity(16);
        let mut model: Vec<(renew_memory::Handle, u64)> = Vec::new();
        let mut retired: Vec<renew_memory::Handle> = Vec::new();

        for operation in operations {
            match operation {
                Some(value) => match pool.insert(value) {
                    Ok(handle) => model.push((handle, value)),
                    Err(returned) => {
                        prop_assert_eq!(returned, value);
                        prop_assert_eq!(model.len(), 16);
                    }
                },
                None => {
                    if !model.is_empty() {
                        let (handle, value) = model.remove(0);
                        prop_assert_eq!(pool.remove(handle), Some(value));
                        retired.push(handle);
                    }
                }
            }
            // Invariants after every step.
            prop_assert_eq!(pool.len(), model.len());
            for (handle, value) in &model {
                prop_assert_eq!(pool.get(*handle), Some(value));
            }
            for handle in &retired {
                prop_assert_eq!(pool.get(*handle), None);
            }
        }
    }
}
