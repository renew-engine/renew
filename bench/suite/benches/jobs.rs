//! Job-pool timings: raw dispatch overhead, the flagship parallel
//! transform measured against the serial `math_mat4_transform_4096`
//! baseline in the math group, and a deliberately skewed workload.
//! Worker counts are fixed (not detected) so numbers are comparable
//! across machines with the configuration stated in the name.
//!
//! # The skew group, and how to read it
//!
//! The uniform transform above is the case where a scheduler cannot
//! distinguish itself: every chunk costs the same, so any assignment is
//! as good as any other. The skew group exists to measure the case that
//! is actually claimed to be a problem — one chunk far more expensive
//! than its neighbours — and it is arranged so the *policy*, not the
//! arithmetic, decides the answer.
//!
//! There are 4096 elements in 16 coarse chunks. One chunk’s worth of
//! them (256) costs [`HEAVY_FACTOR`] times what the others do.
//!
//! **The pool is three configured workers plus the calling thread, which
//! participates in its own dispatch — so the effective width is four.**
//! The names below say `3_workers` because that is the configuration,
//! matching the group above; every ratio here is against four. Getting
//! this wrong is not cosmetic: it moves perfect speedup by a third.
//!
//! With `C` chunks, `W` participating threads, one heavy chunk of cost
//! `H = k·L` and `C-1` light chunks of cost `L`, no schedule can finish
//! before `max(H, total/W)`. For the heavy chunk not to be the
//! bottleneck all by itself — which would make every policy look
//! identical and measure nothing — we need `k < (C-1)/(W-1)`. At
//! `C = 16` and `W = 4` that is `k < 5`, which is why the factor below is
//! 4 and not a rounder, larger number.
//!
//! Read the four benchmarks together:
//!
//! * `jobs_skew_serial` is the total work. **Perfect speedup is that
//!   divided by four**, and that is the number the parallel runs are
//!   really being compared against — not against each other.
//! * `heavy_first` should land near perfect: the expensive chunk is
//!   claimed immediately, and the fifteen cheap ones absorb the rest.
//!   Expect about `5L` against an ideal of `4.75L`.
//! * `heavy_last` is the tail case. The cheap chunks are consumed first,
//!   the expensive one is claimed last, and it runs alone while three
//!   threads idle. Expect about `15L/4 + 4L = 7.75L` against the same
//!   `4.75L` ideal — around two thirds longer.
//! * `heavy_last_fine_grain` is the same arrangement with the grain cut
//!   to a quarter, which splits the expensive region across four chunks
//!   and should land back near the ideal.
//!
//! The comparison the last two make is the one worth having. **A shared
//! claim cursor is already dynamic load balancing** — a worker that
//! finishes early takes the next chunk immediately, which is most of
//! what stealing is usually credited with. What it cannot do is split a
//! chunk that has already been claimed, and *neither can work stealing*:
//! stealing moves whole tasks. If `heavy_last_fine_grain` closes the gap
//! that `heavy_last` opens, then the lever on tail latency is grain
//! size, which this pool already exposes to every caller.

use core::num::NonZeroUsize;
use core::sync::atomic::{AtomicU64, Ordering};
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_jobs::{JobPool, PoolConfig};
use renew_math::Vec4;

const COUNT: usize = 4096;
const SEED: u32 = 0x5EED_0001;

fn jobs_benches(c: &mut Criterion) {
    let Ok(mut pool) = JobPool::new(&PoolConfig::new(3)) else {
        // A machine that cannot spawn three threads cannot run these
        // benches meaningfully; skip rather than panic in bench code.
        return;
    };
    let grain = NonZeroUsize::MIN.saturating_add(255);

    // Pure fan-out/join cost: trivial body over one chunk per thread.
    c.bench_function("jobs_dispatch_overhead_3_workers", |b| {
        b.iter(|| {
            let sink = AtomicU64::new(0);
            pool.parallel_for(0..4, NonZeroUsize::MIN, |chunk| {
                sink.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            });
            black_box(sink.into_inner())
        });
    });

    // The flagship shape: one matrix over an array, chunked across the
    // pool — compare against math_mat4_transform_4096 (serial baseline).
    let matrices = renew_bench::mat4_inputs(COUNT, SEED);
    let vectors = renew_bench::vec4_inputs(COUNT, SEED);
    let matrix = matrices[0];
    let mut output = vec![Vec4::new(0.0, 0.0, 0.0, 0.0); COUNT];
    c.bench_function("jobs_parallel_transform_4096_3_workers", |b| {
        b.iter(|| {
            pool.parallel_for_slice_mut(black_box(&mut output), grain, |offset, chunk| {
                for (index, slot) in chunk.iter_mut().enumerate() {
                    *slot = matrix.transform(vectors[offset + index]);
                }
            });
            black_box(output[COUNT - 1]);
        });
    });
}

// --- Skewed workload ----------------------------------------------------
//
// See the module docs for the arithmetic these constants satisfy and for
// how the four measurements below are meant to be read together.

/// Elements in the skew group. A quarter of the flagship transform's
/// count, because each element here does far more work per element.
const SKEW_COUNT: usize = 4096;

/// Elements in the expensive region: exactly one coarse chunk, so the
/// skew is a property of one chunk rather than smeared across several.
const HEAVY_SPAN: usize = 256;

/// How much more an expensive element costs. Bounded above by
/// `(chunks - 1) / (threads - 1)` = 5 — sixteen chunks over four
/// participating threads — or the heavy chunk alone would set the
/// makespan and no scheduling policy could show a difference.
const HEAVY_FACTOR: u32 = 4;

/// Rounds of mixing one ordinary element does. Large enough that the
/// per-element call is not measuring call overhead, small enough that
/// the whole sweep stays in the sub-millisecond range criterion likes.
const BASE_ROUNDS: u32 = 64;

/// One element's work: `rounds` rounds of an integer mix.
///
/// `#[inline(never)]` and a returned value the caller stores, because
/// the entire measurement is the *time* this takes — an optimiser that
/// hoisted, unrolled against a constant, or elided it would leave four
/// benchmarks that all measure dispatch overhead and agree beautifully.
/// `rounds` arrives from a runtime table for the same reason.
#[inline(never)]
fn burn(seed: u64, rounds: u32) -> u64 {
    let mut state = seed;
    for _ in 0..rounds {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        state ^= state >> 33;
    }
    state
}

/// Per-element round counts with the expensive region at one end.
///
/// Both arrangements carry identical total work by construction, so the
/// serial baseline describes either one and the two parallel runs differ
/// only in *where* the expensive region sits.
fn skew_profile(heavy_at_end: bool) -> Vec<u32> {
    (0..SKEW_COUNT)
        .map(|index| {
            let heavy = if heavy_at_end {
                index >= SKEW_COUNT - HEAVY_SPAN
            } else {
                index < HEAVY_SPAN
            };
            if heavy {
                BASE_ROUNDS * HEAVY_FACTOR
            } else {
                BASE_ROUNDS
            }
        })
        .collect()
}

fn skew_benches(c: &mut Criterion) {
    let Ok(mut pool) = JobPool::new(&PoolConfig::new(3)) else {
        return;
    };
    let coarse = NonZeroUsize::MIN.saturating_add(HEAVY_SPAN - 1);
    let fine = NonZeroUsize::MIN.saturating_add(HEAVY_SPAN / 4 - 1);
    let at_end = skew_profile(true);
    let at_start = skew_profile(false);
    let mut output = vec![0_u64; SKEW_COUNT];

    // The total work, and so the perfect-speedup bound: this divided by
    // the worker count is what the three parallel runs are measured
    // against. Without it they can only be compared to each other, which
    // says which is faster but never how much is left on the table.
    c.bench_function("jobs_skew_serial_4096", |b| {
        b.iter(|| {
            for (index, slot) in output.iter_mut().enumerate() {
                *slot = burn(index as u64, at_end[index]);
            }
            black_box(output[SKEW_COUNT - 1]);
        });
    });

    for (name, profile, grain) in [
        (
            "jobs_skew_heavy_first_grain256_3_workers",
            &at_start,
            coarse,
        ),
        ("jobs_skew_heavy_last_grain256_3_workers", &at_end, coarse),
        ("jobs_skew_heavy_last_grain64_3_workers", &at_end, fine),
    ] {
        c.bench_function(name, |b| {
            b.iter(|| {
                pool.parallel_for_slice_mut(black_box(&mut output), grain, |offset, chunk| {
                    for (index, slot) in chunk.iter_mut().enumerate() {
                        let at = offset + index;
                        *slot = burn(at as u64, profile[at]);
                    }
                });
                black_box(output[SKEW_COUNT - 1]);
            });
        });
    }
}

// --- Batching: what many small dispatches cost -------------------------
//
// Two costs have long been suspected of this pool. The skew group above
// measures the first: how badly a single dispatch can balance. This
// measures the second, which no benchmark had touched -- *many small
// batches each pay one wakeup* -- and answers whether splitting the same
// work across more dispatches costs anything worth avoiding.
//
// The design is one fixed quantity of work, split four ways. Total
// elements, grain, and the per-element body are identical in all four
// cases; only the number of `parallel_for` calls changes. **Any
// difference between them is dispatch overhead and nothing else** --
// which is the only way to price a wakeup without guessing at it.
//
// One case is not like the others and is included deliberately: at 64
// dispatches each covers a single chunk, and the pool documents that a
// single-chunk dispatch runs inline on the caller with no workers woken
// at all. So that row prices the *absence* of the herd, and the spread
// between it and the middle rows is what a wakeup actually costs.

/// Elements processed in every batching case, so the total work is a
/// constant and only the dispatch count varies.
const BATCH_ELEMENTS: usize = 4096;

/// Chunk size, fixed across the group: 64 chunks in total, however they
/// are divided between dispatches.
const BATCH_GRAIN: usize = 64;

/// Rounds per element. Deliberately small -- the complaint being priced
/// is about batches whose work is slight enough that per-dispatch cost
/// could dominate, and a heavy body would bury exactly the effect under
/// measurement.
const BATCH_ROUNDS: u32 = 8;

fn batching_benches(c: &mut Criterion) {
    let Ok(mut pool) = JobPool::new(&PoolConfig::new(3)) else {
        return;
    };
    let grain = NonZeroUsize::MIN.saturating_add(BATCH_GRAIN - 1);
    let mut output = vec![0_u64; BATCH_ELEMENTS];

    for dispatches in [1_usize, 4, 16, 64] {
        let span = BATCH_ELEMENTS / dispatches;
        let name = format!("jobs_batching_{dispatches}_dispatches_3_workers");
        c.bench_function(&name, |b| {
            b.iter(|| {
                for slice in output.chunks_mut(span) {
                    pool.parallel_for_slice_mut(black_box(slice), grain, |offset, chunk| {
                        for (index, slot) in chunk.iter_mut().enumerate() {
                            *slot = burn((offset + index) as u64, BATCH_ROUNDS);
                        }
                    });
                }
                black_box(output[BATCH_ELEMENTS - 1]);
            });
        });
    }
}

criterion_group!(benches, jobs_benches, skew_benches, batching_benches);
criterion_main!(benches);
