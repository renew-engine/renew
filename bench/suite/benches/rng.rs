//! Generator timings: the cost of one draw, in the four shapes a
//! simulation actually asks for.
//!
//! This crate sits in the innermost loop — a simulation that wants a
//! random number wants one per entity per step — so its cost belongs in
//! the suite rather than in an argument. The generator is pure integer
//! arithmetic over sixteen bytes of state: no clock, no allocation, no
//! entropy source, nothing to warm up.
//!
//! Two of the four cases exist because they can be slower than they look.
//! A 64-bit draw is two 32-bit steps, not one, so it should cost about
//! twice a 32-bit draw and it is worth noticing the day it does not. A
//! bounded draw rejects and retries, so its cost is an *expected* cost
//! rather than a fixed one, and the two bounds below sit deliberately at
//! the two ends of that behaviour.

use std::hint::black_box;
use std::num::NonZeroU32;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_rng::{Rng, Seed, StreamId};

/// Arbitrary but fixed: a benchmark that reseeded per iteration would be
/// timing the seeding, and one that varied its seed between runs would
/// make two runs incomparable for no benefit.
const SEED: Seed = Seed::from_u64(0x1234_5678_9abc_def0);

/// A bound that never rejects. Any power of two divides the output range
/// exactly, so the rejection threshold is zero and every draw is taken on
/// the first attempt — the floor for a bounded draw.
const BOUND_NEVER_REJECTS: u32 = 1 << 16;

/// The bound that rejects most often. Just over half the range means
/// almost half of all words are refused, so this is the worst expected
/// case rather than a typical one, and it is here to bound the cost from
/// above rather than to describe ordinary use.
const BOUND_WORST_CASE: u32 = (1u32 << 31) + 1;

fn generator(c: &mut Criterion) {
    let stream = StreamId::from_name("bench");

    c.bench_function("rng_next_u32", |b| {
        let mut rng = Rng::new(SEED, stream);
        b.iter(|| black_box(rng.next_u32()));
    });

    c.bench_function("rng_next_u64", |b| {
        let mut rng = Rng::new(SEED, stream);
        b.iter(|| black_box(rng.next_u64()));
    });

    c.bench_function("rng_below_u32_no_rejection", |b| {
        let mut rng = Rng::new(SEED, stream);
        let bound = NonZeroU32::new(BOUND_NEVER_REJECTS).expect("non-zero by construction");
        b.iter(|| black_box(rng.below_u32(black_box(bound))));
    });

    c.bench_function("rng_below_u32_worst_case_rejection", |b| {
        let mut rng = Rng::new(SEED, stream);
        let bound = NonZeroU32::new(BOUND_WORST_CASE).expect("non-zero by construction");
        b.iter(|| black_box(rng.below_u32(black_box(bound))));
    });
}

criterion_group!(benches, generator);
criterion_main!(benches);
