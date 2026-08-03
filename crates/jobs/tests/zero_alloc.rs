//! The allocation contract, pinned: steady-state dispatch performs zero
//! heap allocations. Own process on purpose (the counters are
//! process-wide); single test so no sibling allocates alongside.
//!
//! Measurement protocol: warmup dispatches first (lazy initialization
//! measured out), then retry windows — the counters see every thread in
//! the process, including the test harness's own output thread, so
//! one-shot neighbor noise rides out while a genuine per-dispatch
//! allocation reproduces in every window and still fails.

use core::num::NonZeroUsize;
use core::sync::atomic::{AtomicUsize, Ordering};

use renew_jobs::{JobPool, PoolConfig};
use renew_memory::CountingAllocator;
use renew_memory::counters::quiet_window;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn dispatch_allocates_exactly_nothing() {
    let grain = NonZeroUsize::new(64).expect("nonzero");
    let mut pool = JobPool::new(&PoolConfig::new(3)).expect("pool");
    let mut data = vec![0u64; 4096];
    let sink = AtomicUsize::new(0);

    // Warmup: both entry points once, so any lazy initialization in the
    // pool, the OS synchronization primitives, or the workers happens
    // before a window opens.
    pool.parallel_for(0..4096, grain, |chunk| {
        sink.fetch_add(chunk.len(), Ordering::Relaxed);
    });
    pool.parallel_for_slice_mut(&mut data, grain, |offset, chunk| {
        for (index, slot) in chunk.iter_mut().enumerate() {
            *slot = (offset + index) as u64;
        }
    });

    quiet_window(5, || {
        for _ in 0..16 {
            pool.parallel_for(0..4096, grain, |chunk| {
                sink.fetch_add(chunk.len(), Ordering::Relaxed);
            });
        }
    })
    .expect("parallel_for stays heap-silent");

    quiet_window(5, || {
        for _ in 0..16 {
            pool.parallel_for_slice_mut(&mut data, grain, |offset, chunk| {
                for (index, slot) in chunk.iter_mut().enumerate() {
                    *slot = (offset + index) as u64;
                }
            });
        }
    })
    .expect("parallel_for_slice_mut stays heap-silent");

    assert!(sink.load(Ordering::Relaxed) > 0);
    assert_eq!(data[4095], 4095);
}
