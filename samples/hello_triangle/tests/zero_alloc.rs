//! The steady state allocates nothing.
//!
//! Steady state, defined: frames `[3, N)` of a headless run. Everything
//! that allocates happens before frame zero — device, offscreen target,
//! pipeline, and the readback buffer — and the three warmup frames
//! absorb whatever the driver initializes lazily on its first draws.
//!
//! Own process on purpose (the counters are process-wide); one test in
//! the file so no sibling allocates alongside it. The retry-window
//! protocol comes from a real incident: the counters see every thread in
//! the process, including the harness's own output thread, so one-shot
//! neighbour noise rides out while a genuine per-frame allocation
//! reproduces in every window and still fails.
//!
//! Three specific regressions this exists to catch: a per-frame
//! `println!` (allocates and locks — the mandated frame-time readout
//! aggregates into the timing summary's four scalars instead), a
//! per-frame `format!`, and a `Vec` of plans accumulating anywhere on
//! the frame path.

use renew_memory::{CountingAllocator, counters};
use renew_sample_hello_triangle::{Draw, HeadlessRun, SampleError, WARMUP_FRAMES};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Frames per measurement window.
const WINDOW_FRAMES: u64 = 16;

fn strict() -> bool {
    std::env::var_os("RENEW_FRAME_STRICT").is_some_and(|value| value == "1")
}

fn quiet_window(attempts: usize, mut window: impl FnMut()) -> Result<(), String> {
    let mut last = (0u64, 0u64);
    for _ in 0..attempts {
        let before = counters::snapshot();
        window();
        let after = counters::snapshot();
        if after.allocations == before.allocations && after.deallocations == before.deallocations {
            return Ok(());
        }
        last = (
            after.allocations - before.allocations,
            after.deallocations - before.deallocations,
        );
    }
    Err(format!(
        "allocator activity in every window (last deltas: +{} allocations, +{} deallocations)",
        last.0, last.1
    ))
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn steady_state_frames_allocate_nothing() {
    let mut run = match HeadlessRun::start(0, Draw::Triangle) {
        Ok(run) => run,
        Err(SampleError::Unavailable(reason)) if !strict() => {
            eprintln!("SKIP: {reason}");
            return;
        }
        Err(error) => panic!("headless bring-up failed: {error}"),
    };

    // Warmup: outside the steady state by definition, and the reason the
    // definition exists.
    run.run(WARMUP_FRAMES).expect("warmup frames");

    quiet_window(5, || {
        run.run(WINDOW_FRAMES).expect("steady-state frames");
    })
    .expect("planning, stepping, drawing and tallying stay heap-silent");

    // The window did real work: without this the test would pass on a
    // driver that returned early from every frame.
    let report = run.report();
    assert!(report.stats.frames() >= WARMUP_FRAMES + WINDOW_FRAMES);
    assert_eq!(report.stats.ticks(), report.stats.frames());
    assert_eq!(report.stats.steps_dropped(), 0);

    // Serialization is outside the steady state by design: it is the one
    // thing here that allocates, and it happens after the loop exits.
    let before = counters::snapshot();
    let json = report.json();
    assert!(json.contains("\"schedule_hash\":\"0x"));
    assert!(
        counters::snapshot().allocations > before.allocations,
        "the report is expected to allocate; only the frame path is not"
    );
}
