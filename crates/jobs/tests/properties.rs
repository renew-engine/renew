//! Property-based coverage of the public dispatch surface: for arbitrary
//! (range, grain, workers), every index is visited exactly once, chunks
//! stay in bounds, and nothing panics — the exactly-once law is the
//! crate's core promise.

use core::num::NonZeroUsize;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use renew_jobs::{JobPool, PoolConfig};

proptest! {
    // Fixed RNG seed: the suite explores the same inputs on every run
    // and every machine, so a property failure anywhere reproduces
    // everywhere. Fresh exploration is a deliberate act (change the
    // seed), never an ambient one.
    #![proptest_config(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x0000_1085),
        cases: 48,
        ..ProptestConfig::default()
    })]

    #[test]
    fn every_index_visited_exactly_once(
        start in 0usize..1000,
        len in 0usize..2000,
        grain in 1usize..300,
        workers in 0usize..4,
    ) {
        let mut pool = JobPool::new(&PoolConfig::new(workers)).expect("pool");
        let cells: Vec<AtomicU8> = (0..len).map(|_| AtomicU8::new(0)).collect();
        let end = start + len;
        let out_of_bounds = AtomicUsize::new(0);
        pool.parallel_for(
            start..end,
            NonZeroUsize::new(grain).expect("nonzero"),
            |chunk| {
                if chunk.start < start || chunk.end > end || chunk.start > chunk.end {
                    out_of_bounds.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                for index in chunk {
                    cells[index - start].fetch_add(1, Ordering::Relaxed);
                }
            },
        );
        prop_assert_eq!(out_of_bounds.load(Ordering::Relaxed), 0);
        for cell in &cells {
            prop_assert_eq!(cell.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn slice_chunks_partition_the_slice(
        len in 0usize..2000,
        grain in 1usize..300,
        workers in 0usize..4,
    ) {
        let mut pool = JobPool::new(&PoolConfig::new(workers)).expect("pool");
        let mut data = vec![0u8; len];
        pool.parallel_for_slice_mut(
            &mut data,
            NonZeroUsize::new(grain).expect("nonzero"),
            |_, chunk| {
                for slot in chunk {
                    *slot = slot.wrapping_add(1);
                }
            },
        );
        prop_assert!(data.iter().all(|&value| value == 1));
    }
}
