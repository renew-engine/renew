//! The retry-until-quiet policy, exercised against the real allocator:
//! this binary's global allocator is the counting wrapper, so windows
//! see genuine deltas. One test on purpose — the counters are
//! process-wide, and a sibling test's allocations would race the
//! scenarios below.

use renew_memory::{CountingAllocator, counters};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_policy_retries_reports_and_settles() {
    // A window that allocates nothing succeeds immediately.
    assert!(
        counters::quiet_window(1, || {}).is_ok(),
        "an empty window must be quiet"
    );

    // A window that allocates every attempt fails, and the reported
    // delta is the LAST window's activity, exactly — one boxed byte,
    // kept alive past the window so the deallocation half stays zero
    // and the numbers are unambiguous.
    let mut keep = Vec::with_capacity(16);
    let error = counters::quiet_window(3, || {
        keep.push(Box::new(1u8));
    })
    .expect_err("an always-allocating window must fail");
    assert_eq!(
        (error.allocations, error.deallocations),
        (1, 0),
        "the delta must be the last window's own activity"
    );
    assert_eq!(
        error.to_string(),
        "+1 allocations, +0 deallocations",
        "the report is what a failing gate prints"
    );

    // Noise that stops — the reason the policy exists: two loud
    // windows, then quiet, inside the attempt budget.
    let mut remaining_noise = 2u32;
    assert!(
        counters::quiet_window(5, || {
            if remaining_noise > 0 {
                remaining_noise -= 1;
                keep.push(Box::new(2u8));
            }
        })
        .is_ok(),
        "one-shot noise must ride out within the attempts"
    );

    // Deallocations alone are activity too: freeing inside the window
    // is a dirty window, which is what makes the both-channels check
    // stronger than counting allocations alone.
    let boxes: Vec<Box<u8>> = (0..3).map(|_| Box::new(3u8)).collect();
    let mut boxes = boxes.into_iter();
    let error = counters::quiet_window(3, || {
        drop(boxes.next());
    })
    .expect_err("a window that frees must be loud");
    assert_eq!(
        (error.allocations, error.deallocations),
        (0, 1),
        "the dealloc-only delta must be visible"
    );
    drop(keep);
}
