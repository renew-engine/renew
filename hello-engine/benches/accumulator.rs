//! Criterion benchmarks for [`Accumulator::advance`].

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use hello_engine::Accumulator;

/// 60 Hz simulation timestep, rounded to whole nanoseconds.
const TIMESTEP_NS: u64 = 16_666_667;

fn accumulator_advance(c: &mut Criterion) {
    // Steady state: frame time just under one timestep, ticks mostly 0 or 1.
    c.bench_function("accumulator_advance_steady_60hz", |b| {
        let mut acc = Accumulator::new(TIMESTEP_NS);
        b.iter(|| acc.advance(black_box(16_000_000)));
    });

    // Spike: every frame is worth several ticks.
    c.bench_function("accumulator_advance_multi_tick", |b| {
        let mut acc = Accumulator::new(TIMESTEP_NS);
        b.iter(|| acc.advance(black_box(100_000_000)));
    });
}

criterion_group!(benches, accumulator_advance);
criterion_main!(benches);
