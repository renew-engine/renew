//! Seeded stress campaign for the dispatch protocol: many dispatches,
//! varied shapes, varied pool sizes, construct/drop churn. The always-on
//! tier stays fast; the long tier is `#[ignore]`d locally and runs in
//! the scheduled sanitizer workflow.

use core::num::NonZeroUsize;
use core::sync::atomic::{AtomicU64, Ordering};

use renew_jobs::{JobPool, PoolConfig};

/// Deterministic parameter stream (seeded LCG — no ambient randomness).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 16
    }

    // Test helper: the tests-only expect allowance covers #[test] fns,
    // not their helpers; this allow extends it, same spirit.
    #[allow(clippy::expect_used)]
    fn in_range(&mut self, low: u64, high: u64) -> usize {
        usize::try_from(low + self.next() % (high - low)).expect("fits")
    }
}

// Test helper (called only from #[test] fns) — same allowance extension.
#[allow(clippy::expect_used)]
fn campaign(seed: u64, dispatches: usize, max_workers: usize) {
    let mut rng = Rng(seed);
    let mut pool_size = 0;
    let mut pool = JobPool::new(&PoolConfig::new(pool_size)).expect("pool");
    for round in 0..dispatches {
        // Periodically rebuild the pool at a new size (construct/drop
        // churn, including drop-immediately-after-new).
        if round % 37 == 0 {
            pool_size = rng.in_range(0, u64::try_from(max_workers).expect("fits") + 1);
            pool = JobPool::new(&PoolConfig::new(pool_size)).expect("pool");
            if round % 111 == 0 {
                // Immediate drop + rebuild.
                pool = JobPool::new(&PoolConfig::new(pool_size)).expect("pool");
            }
        }
        let len = rng.in_range(0, 5000);
        let grain = NonZeroUsize::new(rng.in_range(1, 512)).expect("nonzero");
        let expected: u64 = (0..len as u64).sum();
        if round % 2 == 0 {
            let total = AtomicU64::new(0);
            pool.parallel_for(0..len, grain, |chunk| {
                let mut local = 0u64;
                for index in chunk {
                    local += index as u64;
                }
                total.fetch_add(local, Ordering::Relaxed);
            });
            assert_eq!(total.load(Ordering::Relaxed), expected, "round {round}");
        } else {
            let mut data = vec![0u64; len];
            pool.parallel_for_slice_mut(&mut data, grain, |offset, chunk| {
                for (index, slot) in chunk.iter_mut().enumerate() {
                    *slot = (offset + index) as u64;
                }
            });
            let total: u64 = data.iter().sum();
            assert_eq!(total, expected, "round {round}");
        }
    }
}

#[test]
fn stress_short_tier() {
    let dispatches = if cfg!(miri) { 12 } else { 300 };
    let max_workers = if cfg!(miri) { 2 } else { 8 };
    campaign(0x5EED_0B57, dispatches, max_workers);
}

#[test]
#[ignore = "long tier: run explicitly or in the scheduled sanitizer workflow"]
fn stress_long_tier() {
    campaign(0x5EED_0B58, 5000, 16);
}
