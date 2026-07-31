//! What it costs to ask what time it is.
//!
//! This number is not interesting on its own; it is interesting because
//! it is the unit price of every piece of timing instrumentation the
//! engine will ever add. Any proposal to time something *per chunk*, or
//! per job, or per draw call, is really a proposal to spend this many
//! nanoseconds that many times, and the argument for or against it
//! cannot be had until the price is on the table.
//!
//! It is a syscall-adjacent operation on every platform — a vDSO read on
//! Linux, `QueryPerformanceCounter` on Windows, `mach_absolute_time` on
//! macOS — so it is both far more expensive than arithmetic and highly
//! platform-dependent. That is exactly why it belongs in the per-platform
//! table rather than in one number quoted from one machine.
//!
//! # The self-check, and why it is here
//!
//! `clock_elapsed_nanos_x16` is not a second measurement of interest. It
//! exists to prove the first one is real. Sixteen reads must cost about
//! sixteen times one read; if the two land close together, the compiler
//! has hoisted the call out of the loop and **both numbers are measuring
//! criterion's iteration overhead instead of the clock**. A benchmark
//! that cannot fail is not evidence, and a timing benchmark whose subject
//! is a single cheap call is precisely where that happens quietly.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_platform::Clock;

/// Reads in the batched case. Enough that per-iteration loop overhead is
/// a small fraction of the total, few enough that the batch still fits
/// comfortably inside criterion's timing resolution.
const BATCH: usize = 16;

fn clock_benches(c: &mut Criterion) {
    let clock = Clock::start();

    c.bench_function("clock_elapsed_nanos", |b| {
        b.iter(|| black_box(black_box(&clock).elapsed_nanos()));
    });

    // Summed rather than discarded so every read has a consumer the
    // optimiser can see, and so a partially-elided loop changes the
    // answer rather than merely the timing.
    c.bench_function("clock_elapsed_nanos_x16", |b| {
        b.iter(|| {
            let mut total: u64 = 0;
            for _ in 0..BATCH {
                total = total.wrapping_add(black_box(&clock).elapsed_nanos());
            }
            black_box(total)
        });
    });
}

criterion_group!(benches, clock_benches);
criterion_main!(benches);
