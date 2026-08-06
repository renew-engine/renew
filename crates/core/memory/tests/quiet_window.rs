//! The retry-until-quiet policy, exercised against the real allocator:
//! this binary's global allocator is the counting wrapper, so windows
//! see genuine deltas. One test on purpose — the counters are
//! process-wide, and a sibling test's allocations would race the
//! scenarios below.
//!
//! # Why nothing here asserts an exact count
//!
//! One test stops *sibling tests* racing the counters. It does not stop
//! the **harness**, which runs its own threads beside this one and
//! allocates on them; a window here measures the process, not the
//! thread. This test went red once on a lane where nothing it covers had
//! changed, on an exact `(0, 1)`.
//!
//! So each assertion below states the property it is actually about and
//! tolerates activity it is not about. What matters is that freeing
//! alone makes a window loud, that a reported delta is the last window's
//! rather than every window's summed, and that an empty window settles —
//! none of which needs a number nobody controls to be exact.

use renew_memory::{CountingAllocator, counters};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_policy_retries_reports_and_settles() {
    // A window that allocates nothing settles. Several attempts rather
    // than one: a single stray allocation from the harness during the
    // one attempt would otherwise report a policy failure that is
    // nothing of the kind.
    assert!(
        counters::quiet_window(5, || {}).is_ok(),
        "an empty window must settle"
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
    // **The subject is that the delta is the last window's, not every
    // window's summed.** Three attempts allocate one box each, so a
    // cumulative delta would report three. Anything under that proves
    // the window is measured afresh — and leaves room for an allocation
    // from elsewhere in the process, which would otherwise fail a test
    // that is not about it.
    assert!(
        error.allocations >= 1 && error.allocations < 3,
        "the delta must be the last window's own activity, got {error}"
    );

    // The report's wording, from a delta built by hand. Taking it from
    // the window above would make a formatting assertion depend on what
    // every other thread in the process happened to do.
    assert_eq!(
        counters::ActivityDelta {
            allocations: 1,
            deallocations: 0,
        }
        .to_string(),
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
    // **The subject is that freeing alone makes a window loud**, which
    // is what makes the both-channels check stronger than counting
    // allocations. The allocation count is not the subject: the window
    // allocates nothing, but the process might.
    assert!(
        error.deallocations >= 1,
        "a window that freed must show it on the deallocation channel, got {error}"
    );
    drop(keep);
}
