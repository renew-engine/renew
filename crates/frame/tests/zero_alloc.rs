//! The allocation contract, pinned: planning frames, iterating their
//! steps and tallying them performs zero heap allocations. Own process on
//! purpose (the counters are process-wide); single test so no sibling
//! allocates alongside.
//!
//! Measurement protocol: warmup frames first (lazy initialization measured
//! out), then retry windows — the counters see every thread in the
//! process, including the test harness's own output thread, so one-shot
//! neighbor noise rides out while a genuine per-frame allocation
//! reproduces in every window and still fails.
//!
//! This crate has no runtime allocation to remove — it holds five integers
//! and returns a `Copy` value — so the test is a tripwire rather than a
//! discovery: it fails the day someone adds a `Vec` of pending steps, a
//! `format!` in a hot path, or a boxed callback.

use renew_frame::{FrameLoop, FrameStats, FrameTiming, Nanos, StepBudget, Timestamp, Timestep};
use renew_memory::counters::quiet_window;
use renew_memory::{CountingAllocator, counters};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// 60 Hz in whole nanoseconds.
const DT: u64 = 16_666_667;

/// One frame of the full steady-state path: plan, execute, interpolate,
/// tally. Every call a caller makes per frame is here, so a future
/// allocation anywhere on that path is caught by this one test.
fn frame_body(
    frame: &mut FrameLoop,
    stats: &mut FrameStats,
    timing: &mut FrameTiming,
    sink: &mut u64,
    now: u64,
) {
    let plan = frame.begin_frame(Timestamp::from_nanos(now));
    for step in plan.steps() {
        *sink = sink
            .wrapping_add(step.tick)
            .wrapping_add(step.sim_time.get());
    }
    *sink = sink
        .wrapping_add(plan.remainder().get())
        .wrapping_add(plan.timestep().nanos().get());
    stats.absorb(&plan);
    timing.record(Nanos::from_nanos(now % 4_000_000), plan.step_count() > 0);
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_frame_schedule_allocates_exactly_nothing() {
    let mut frame = FrameLoop::new(
        Timestep::HZ_60,
        StepBudget::DEFAULT,
        Timestamp::from_nanos(0),
    );
    let mut stats = FrameStats::new();
    let mut timing = FrameTiming::new();
    let mut sink = 0u64;
    let mut now = 0u64;

    // Warmup: every shape the window exercises, once, so any lazy
    // initialization behind them happens before a window opens.
    for delta in [DT, 200_000_000, 0] {
        now += delta;
        frame_body(&mut frame, &mut stats, &mut timing, &mut sink, now);
    }
    frame.resync(Timestamp::from_nanos(now));

    quiet_window(5, || {
        for _ in 0..64 {
            // Ordinary frames, a stall the budget clamps, a frame with no
            // elapsed time, a backwards clock, and a resync — the whole
            // branch set, because a plan is only heap-silent if every arm
            // of it is.
            for delta in [DT, DT / 2, DT / 2, 200_000_000, 0] {
                now += delta;
                frame_body(&mut frame, &mut stats, &mut timing, &mut sink, now);
            }
            frame_body(
                &mut frame,
                &mut stats,
                &mut timing,
                &mut sink,
                now - 5_000_000,
            );
            frame.resync(Timestamp::from_nanos(now));
        }
    })
    .expect("planning, stepping and tallying stay heap-silent");

    // The window did real work: without this the test would pass on a
    // loop that returned an empty plan every frame.
    assert!(stats.frames() >= 64 * 6);
    assert!(stats.ticks() > 0);
    assert!(stats.steps_dropped() > 0, "the budget never engaged");
    assert_ne!(sink, 0);

    // Serialization is outside the steady state by design: it is the one
    // thing here that allocates, and it happens after the loop exits.
    let before = counters::snapshot();
    let json = format!("{} {}", stats.json(), timing.json());
    assert!(json.contains("\"schedule_hash\":\"0x"));
    assert!(
        counters::snapshot().allocations > before.allocations,
        "the JSON adapters are expected to allocate; only the frame path is not"
    );
}
