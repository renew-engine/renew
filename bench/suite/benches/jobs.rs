//! Job-pool timings: raw dispatch overhead, and the flagship parallel
//! transform measured against the serial `math_mat4_transform_4096`
//! baseline in the math group. Worker counts are fixed (not detected)
//! so numbers are comparable across machines with the configuration
//! stated in the name.

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

criterion_group!(benches, jobs_benches);
criterion_main!(benches);
