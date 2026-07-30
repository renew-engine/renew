//! The panic policy, exercised end to end. Test code — `catch_unwind`
//! and panics are legal here (tests are not engine targets); the pool
//! itself never catches.
//!
//! Determinism note: the worker-panic test gates its caller chunk on a
//! flag a worker sets before panicking, so "a worker panicked while the
//! dispatcher waits" is constructed, not raced for.

use core::num::NonZeroUsize;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::panic::{AssertUnwindSafe, catch_unwind};

use renew_jobs::{JobPool, PoolConfig};

// Test helper: the tests-only expect allowance covers #[test] fns, not
// their helpers; this allow extends it, same spirit, visible scope.
#[allow(clippy::expect_used)]
fn grain(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).expect("nonzero")
}

fn on_worker() -> bool {
    std::thread::current()
        .name()
        .is_some_and(|name| name.starts_with("renew-jobs"))
}

/// Yield-based gate: bounded, and scheduler-friendly under Miri (yields
/// are switch points for its interpreter scheduler).
fn wait_for(flag: &AtomicBool) {
    let mut yields = 0u32;
    while !flag.load(Ordering::Acquire) {
        yields += 1;
        assert!(yields < 10_000_000, "gated worker event never happened");
        std::thread::yield_now();
    }
}

const SLOW_ITERATIONS: usize = if cfg!(miri) { 50 } else { 200_000 };

#[test]
fn a_worker_panic_surfaces_in_the_dispatching_call() {
    let mut pool = JobPool::new(&PoolConfig::new(2)).expect("pool");
    let worker_panicked = AtomicBool::new(false);
    let result = catch_unwind(AssertUnwindSafe(|| {
        pool.parallel_for(0..4, grain(1), |_| {
            if on_worker() {
                worker_panicked.store(true, Ordering::Release);
                panic!("deliberate job defect");
            }
            // Caller chunks: wait until a worker has committed to
            // panicking, so the defect deterministically happens while
            // this dispatch is live.
            wait_for(&worker_panicked);
        });
    }));
    // The dispatch itself must observe the defect: the debug assertion
    // names the contract. (The worker's own panic happened on the worker
    // thread; what reaches THIS thread is the dispatcher's assertion.)
    let payload = result.expect_err("the dispatching call must raise");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
        .unwrap_or_default();
    assert!(
        message.contains("jobs must never panic"),
        "unexpected panic payload: {message}"
    );
}

#[test]
fn a_poisoned_pool_refuses_the_next_dispatch() {
    let mut pool = JobPool::new(&PoolConfig::new(2)).expect("pool");
    let worker_panicked = AtomicBool::new(false);
    let _ = catch_unwind(AssertUnwindSafe(|| {
        pool.parallel_for(0..4, grain(1), |_| {
            if on_worker() {
                worker_panicked.store(true, Ordering::Release);
                panic!("deliberate job defect");
            }
            wait_for(&worker_panicked);
        });
    }));

    // Sticky: the pool must refuse to run silently degraded.
    let ran = AtomicUsize::new(0);
    let second = catch_unwind(AssertUnwindSafe(|| {
        pool.parallel_for(0..100, grain(10), |_| {
            ran.fetch_add(1, Ordering::Relaxed);
        });
    }));
    let payload = second.expect_err("dispatch on a poisoned pool must assert");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
        .unwrap_or_default();
    assert!(
        message.contains("poisoned"),
        "unexpected payload: {message}"
    );
    assert_eq!(ran.load(Ordering::Relaxed), 0, "no chunk may run");

    // The INLINE path (single chunk) must honor the poison contract too
    // — no dispatch shape runs quietly after a defect.
    let inline_ran = AtomicUsize::new(0);
    let third = catch_unwind(AssertUnwindSafe(|| {
        pool.parallel_for(0..5, grain(10), |_| {
            inline_ran.fetch_add(1, Ordering::Relaxed);
        });
    }));
    assert!(third.is_err(), "inline dispatch on a poisoned pool asserts");
    assert_eq!(inline_ran.load(Ordering::Relaxed), 0);
}

#[test]
fn an_inline_unwind_poisons_the_pool() {
    // Zero-worker pool: every dispatch takes the inline path. A panic
    // there must poison exactly like a pooled one — no dispatch shape
    // escapes the contract.
    let mut pool = JobPool::new(&PoolConfig::new(0)).expect("inline pool");
    let first = catch_unwind(AssertUnwindSafe(|| {
        pool.parallel_for(0..10, grain(100), |_| panic!("inline defect"));
    }));
    assert!(first.is_err(), "the inline panic propagates");

    let ran = AtomicUsize::new(0);
    let second = catch_unwind(AssertUnwindSafe(|| {
        pool.parallel_for(0..10, grain(100), |_| {
            ran.fetch_add(1, Ordering::Relaxed);
        });
    }));
    assert!(second.is_err(), "inline unwind must poison the pool");
    assert_eq!(ran.load(Ordering::Relaxed), 0);
}

#[test]
fn a_caller_chunk_panic_drains_workers_before_the_frame_dies() {
    let mut pool = JobPool::new(&PoolConfig::new(2)).expect("pool");
    // Deterministic in every interleaving: each worker parks inside its
    // first claimed chunk waiting for the caller's panic commitment.
    // With more chunks than workers, the caller MUST claim one (workers
    // hold at most one each while gated), commits, and panics — so the
    // barrier's unwind path is exercised while workers are demonstrably
    // mid-flight (Miri/TSan pin the soundness half; this pins behavior).
    let caller_committed = AtomicBool::new(false);
    let slow_chunks = AtomicUsize::new(0);
    let result = catch_unwind(AssertUnwindSafe(|| {
        pool.parallel_for(0..6, grain(1), |_| {
            if on_worker() {
                wait_for(&caller_committed);
                for _ in 0..SLOW_ITERATIONS {
                    std::hint::spin_loop();
                }
                slow_chunks.fetch_add(1, Ordering::Relaxed);
            } else {
                caller_committed.store(true, Ordering::Release);
                panic!("caller-side defect");
            }
        });
    }));
    let payload = result.expect_err("the caller's panic propagates");
    let message = payload
        .downcast_ref::<&str>()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        message.contains("caller-side defect"),
        "unexpected payload: {message}"
    );

    // And the pool is poisoned afterwards: a dispatch died mid-flight.
    let second = catch_unwind(AssertUnwindSafe(|| {
        pool.parallel_for(0..10, grain(1), |_| {});
    }));
    assert!(second.is_err(), "pool must be poisoned after an unwind");
}
