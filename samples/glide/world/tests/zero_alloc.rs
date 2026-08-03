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
    // The pilot, not a blind schedule: the first version's schedule
    // killed the bird at tick 305, so every measured window was a dead
    // world executing nothing but its counter — and a dead world's tick
    // proves nothing about the game's. The window now asserts its own
    // premises: alive with pipes on screen at both ends.
    let mut world = World::new(7);
    for _ in 0..2_000u64 {
        let flap = world.autopilot();
        world.step(flap);
    }

    let mut last_delta = 0u64;
    let mut observed_zero = false;
    for _ in 0..5 {
        assert!(
            world.alive() && world.pipes() > 0,
            "the window must open on a live game"
        );
        let before = allocations();
        for _ in 0..1_000u64 {
            let flap = world.autopilot();
            world.step(flap);
        }
        let after = allocations();
        assert!(
            world.alive() && world.pipes() > 0,
            "the window must close on a live game"
        );
        last_delta = after - before;
        if last_delta == 0 {
            observed_zero = true;
            break;
        }
    }
    assert!(
        observed_zero,
        "a live tick allocated in every window (last delta: +{last_delta})"
    );
}
