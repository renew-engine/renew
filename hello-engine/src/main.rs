//! Prints the build identity, then drives the fixed-timestep accumulator
//! through a fixed number of simulated frames with deterministic frame times.
//! No clocks are read; every run produces identical output.

use hello_engine::Accumulator;

/// 60 Hz simulation timestep, rounded to whole nanoseconds.
const TIMESTEP_NS: u64 = 16_666_667;

/// Number of frames to simulate.
const FRAMES: usize = 60;

/// Deterministic per-frame durations, cycled for the whole run: a fast frame,
/// an exact frame, a slow frame, and a two-tick spike.
const FRAME_PATTERN_NS: [u64; 4] = [15_000_000, 16_666_667, 18_000_000, 33_333_334];

fn main() {
    println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    println!("fixed timestep: {TIMESTEP_NS} ns");

    let mut accumulator = Accumulator::new(TIMESTEP_NS);
    let mut total_ticks: u64 = 0;
    let mut total_time_ns: u64 = 0;

    for frame_time_ns in FRAME_PATTERN_NS.iter().copied().cycle().take(FRAMES) {
        total_ticks += u64::from(accumulator.advance(frame_time_ns));
        total_time_ns += frame_time_ns;
    }

    println!("frames simulated: {FRAMES}");
    println!("time submitted: {total_time_ns} ns");
    println!("ticks executed: {total_ticks}");
    println!("time pending: {} ns", accumulator.pending_ns());
}
