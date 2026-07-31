//! Prints the build identity, then drives the fixed-timestep frame loop
//! through a fixed number of simulated frames with deterministic frame
//! times. No clocks are read; every run produces identical output.
//!
//! The schedule itself lives in `renew-frame`: this is its smallest
//! client, and the numbers below are the loop's own — the tally comes
//! from the plans it returns, never from a second count kept here.

use renew_frame::{FrameLoop, FrameStats, StepBudget, Timestamp, Timestep};

/// Number of frames to simulate.
const FRAMES: usize = 60;

/// Deterministic per-frame durations, cycled for the whole run: a fast frame,
/// an exact frame, a slow frame, and a two-tick spike.
const FRAME_PATTERN_NS: [u64; 4] = [15_000_000, 16_666_667, 18_000_000, 33_333_334];

fn main() {
    let timestep = Timestep::HZ_60;
    println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    println!("fixed timestep: {} ns", timestep.nanos());

    // Absolute timestamps, so the elapsed total is the schedule's own
    // input rather than a number accumulated beside it.
    let mut elapsed_ns: u64 = 0;
    let mut frame = FrameLoop::new(timestep, StepBudget::DEFAULT, Timestamp::from_nanos(0));
    let mut stats = FrameStats::new();

    for frame_time_ns in FRAME_PATTERN_NS.iter().copied().cycle().take(FRAMES) {
        elapsed_ns += frame_time_ns;
        let plan = frame.begin_frame(Timestamp::from_nanos(elapsed_ns));
        // A real client steps its world here, once per planned step. This
        // one has no world, so the plan goes straight to the tally.
        stats.absorb(&plan);
    }

    println!("frames simulated: {}", stats.frames());
    println!("time submitted: {elapsed_ns} ns");
    println!("ticks executed: {}", stats.ticks());
    println!("time pending: {} ns", frame.remainder().get());
}
