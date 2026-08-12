//! The counting allocator, installed for real: this binary's global
//! allocator is the counting wrapper, and the counters must move with
//! actual allocations. Own process on purpose — the counters are
//! process-wide.

use renew_memory::{CountingAllocator, counters};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Bytes the harness may allocate inside a measured window without this
/// test caring. Two orders above the 48 bytes observed under coverage,
/// and still a quarter of the 4,096 being measured, so a box that was
/// never released cannot hide underneath it.
const NOISE: usize = 1024;

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
    // **The window is not quiescent, and this comment used to say it
    // was.** It claimed a single test in its own file left nothing else
    // allocating between the snapshots, "observed stable across repeated
    // runs" — which is what a flaky test looks like from the inside. Under
    // `cargo llvm-cov` the profiling runtime allocates on this thread: the
    // counters showed exactly one extra allocation of 48 bytes in the
    // window, so a release of 4,048 failed a demand for 4,096 by those 48
    // bytes. The identical commit passed on re-run, which is the whole
    // signature of a gate reddening for reasons no change caused.
    //
    // The counter is process-wide and no test can stop the harness using
    // it. So this asserts what it can actually own: the deallocation is
    // counted, and the bytes released are the box's size within a stated
    // noise floor. The floor is named rather than fudged, and the
    // interference is *checked* rather than assumed — if the window ever
    // gets noisier than a handful of allocations, that fails on its own
    // terms instead of being absorbed.
    let after_drop = counters::snapshot();
    assert!(
        after_drop.deallocations > after_alloc.deallocations,
        "deallocation not counted"
    );
    let released = after_alloc
        .bytes_in_use
        .saturating_sub(after_drop.bytes_in_use);
    assert!(
        released + NOISE >= 4096,
        "bytes not released: {after_alloc:?} -> {after_drop:?} \
         (released {released}, noise floor {NOISE})"
    );
    let interference = after_drop.allocations - after_alloc.allocations;
    assert!(
        interference <= 4,
        "the window allocated {interference} times while this test held it; \
         the noise floor above assumes a handful, so it can no longer be \
         trusted to bound what {after_alloc:?} -> {after_drop:?} means"
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
