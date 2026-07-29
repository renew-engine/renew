//! The counting allocator, installed for real: this binary's global
//! allocator is the counting wrapper, and the counters must move with
//! actual allocations. Own process on purpose — the counters are
//! process-wide.

use renew_memory::{CountingAllocator, counters};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn counters_track_real_allocations() {
    let before = counters::snapshot();

    let boxed = Box::new([0u8; 4096]);
    let after_alloc = counters::snapshot();
    assert!(
        after_alloc.allocations > before.allocations,
        "allocation not counted: {before:?} -> {after_alloc:?}"
    );
    assert!(
        after_alloc.bytes_in_use >= before.bytes_in_use + 4096,
        "bytes not tracked: {before:?} -> {after_alloc:?}"
    );
    assert!(after_alloc.peak_bytes >= after_alloc.bytes_in_use);

    drop(boxed);
    // The exact-release assertion below assumes the harness is quiescent
    // between the snapshots; this file holds a single test precisely so
    // nothing else allocates in the window (observed stable across
    // repeated runs).
    let after_drop = counters::snapshot();
    assert!(
        after_drop.deallocations > after_alloc.deallocations,
        "deallocation not counted"
    );
    assert!(
        after_drop.bytes_in_use <= after_alloc.bytes_in_use - 4096,
        "bytes not released: {after_alloc:?} -> {after_drop:?}"
    );

    // Growth reallocates: both halves of realloc count.
    let mut vector: Vec<u64> = Vec::with_capacity(4);
    for i in 0..1024 {
        vector.push(i);
    }
    let after_growth = counters::snapshot();
    assert!(after_growth.allocations >= after_drop.allocations + 2);
    assert!(after_growth.peak_bytes >= 1024 * 8);
}
