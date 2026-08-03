//! The steady-state tick allocates nothing.
//!
//! The world owns one scratch allocation for its whole life, made at
//! construction; the feasibility version collected sweep candidates
//! into fresh `Vec`s every tick, and this gate is what keeps that from
//! coming back. Warmup covers construction and the scratch's first
//! growth to its high-water mark; the measured window is pure `step`.
//!
//! Own process on purpose (the counters are process-wide); one test in
//! the file so no sibling allocates alongside it. The retry-window
//! protocol is the sample convention: one-shot harness noise rides out,
//! a real per-tick allocation reproduces in every window.

use renew_memory::{CountingAllocator, counters};
use renew_sample_glide_world::World;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocations() -> u64 {
    counters::snapshot().allocations
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn steady_state_ticks_allocate_nothing() {
    let mut world = World::new(7);
    // Warmup: construction, plus enough ticks that pipes have spawned,
    // scored and despawned — the scratch list has seen its worst case.
    for tick in 0..2_000u64 {
        let flap = tick.is_multiple_of(23);
        world.step(flap);
    }

    let mut last_delta = 0u64;
    let mut observed_zero = false;
    for _ in 0..5 {
        let before = allocations();
        for tick in 0..1_000u64 {
            let flap = tick.is_multiple_of(23);
            world.step(flap);
        }
        let after = allocations();
        last_delta = after - before;
        if last_delta == 0 {
            observed_zero = true;
            break;
        }
    }
    assert!(
        observed_zero,
        "the step allocated in every window (last delta: +{last_delta})"
    );
}
