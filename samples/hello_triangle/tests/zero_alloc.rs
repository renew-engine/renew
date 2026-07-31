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
//! Four specific regressions this exists to catch: a per-frame
//! `println!` (allocates and locks — the frame-time capture aggregates
//! into the timing summary's four scalars instead), a per-frame
//! `format!`, a `Vec` of plans accumulating anywhere on the frame path,
//! and a window-title readout that builds its text with `format!`. That
//! last one has no window in this process to give it away, so it is
//! driven directly, before the part of the test a machine without a GPU
//! skips.

#[cfg(feature = "window")]
use renew_frame::{Nanos, Timestamp};
use renew_memory::{CountingAllocator, counters};
#[cfg(feature = "window")]
use renew_sample_hello_triangle::Readout;
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
    // The window-title readout, driven over a synthetic timeline that
    // crosses several relabel intervals. It is the only thing on the
    // frame path that turns numbers into text, which makes it the
    // likeliest place for a `format!` to appear — and there is no window
    // in this process, so nothing below the seam would give one away.
    // Before the GPU half, so a machine without a driver still checks it.
    #[cfg(feature = "window")]
    {
        // A short interval so one window of frames spans several
        // relabels; the interval the sample actually uses is asserted by
        // the readout's own unit tests.
        let mut readout = Readout::new("renew", Nanos::from_nanos(50_000_000));
        let mut frame = 0u64;
        let mut relabels = 0u32;
        let mut malformed = 0u32;
        quiet_window(5, || {
            for _ in 0..WINDOW_FRAMES {
                frame += 1;
                let now = Timestamp::from_nanos(frame.saturating_mul(16_666_667));
                if let Some(title) = readout.record(Nanos::from_nanos(16_600_000), now) {
                    relabels += 1;
                    if !title.starts_with("renew — 16.6") {
                        malformed += 1;
                    }
                }
            }
        })
        .expect("the readout formats into its own buffer and never onto the heap");
        // Without these the windows above could have been sixteen early
        // returns each: no text formatted, and nothing proved.
        assert!(relabels > 0, "no relabel interval elapsed inside a window");
        assert_eq!(malformed, 0, "a relabel produced text nobody expected");
    }

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
