//! Frame-schedule timings: the cost of deciding what a frame must do.
//!
//! These replace the accumulator benchmarks that lived beside the
//! walking-proof binary until the schedule became a crate of its own.
//! The two cases are the same two: the steady frame, where the answer is
//! usually zero or one step, and the spike, where one frame is worth
//! several. Both names carry their timestep so a number read years from
//! now still says what it measured.
//!
//! `begin_frame` is a pure state transition over integers, so a
//! measurement here is the scheduling decision and nothing else — no
//! clock, no allocation, no driver.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_frame::{FrameLoop, StepBudget, Timestamp, Timestep};

/// The anchor. Non-zero so a frame is never measured against the
/// degenerate origin.
const ORIGIN: u64 = 1_000_000_000;
/// Just under a 60 Hz step: the frame that mostly banks time and
/// occasionally spends it, which is what a healthy loop does.
const STEADY_FRAME_NS: u64 = 16_000_000;
/// Six steps' worth in one frame: a stall, where the budget's clamp is
/// the code under test.
const SPIKE_FRAME_NS: u64 = 100_000_000;

fn frame_benches(c: &mut Criterion) {
    c.bench_function("frame_begin_steady_60hz", |b| {
        let mut loop_ = FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(ORIGIN),
        );
        let mut now = ORIGIN;
        b.iter(|| {
            now += STEADY_FRAME_NS;
            black_box(loop_.begin_frame(Timestamp::from_nanos(black_box(now))))
        });
    });

    c.bench_function("frame_begin_multi_step_60hz", |b| {
        let mut loop_ = FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(ORIGIN),
        );
        let mut now = ORIGIN;
        b.iter(|| {
            now += SPIKE_FRAME_NS;
            black_box(loop_.begin_frame(Timestamp::from_nanos(black_box(now))))
        });
    });
}

criterion_group!(benches, frame_benches);
criterion_main!(benches);
