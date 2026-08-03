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

    // The retry-until-quiet policy lives with the counters it reads;
    // both channels now — a tick that frees is as loud as one that
    // allocates.
    let verdict = counters::quiet_window(5, || {
        assert!(
            world.alive() && world.pipes() > 0,
            "the window must open on a live game"
        );
        for _ in 0..1_000u64 {
            let flap = world.autopilot();
            world.step(flap);
        }
        assert!(
            world.alive() && world.pipes() > 0,
            "the window must close on a live game"
        );
    });
    if let Err(activity) = verdict {
        panic!("a live tick was loud in every window (last: {activity})");
    }
}
