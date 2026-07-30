//! Allocator kernel timings under the system allocator. The
//! `alloc_counted` target runs the same heap kernel under the counting
//! wrapper; the wrapper's overhead is the comparison between the two.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_memory::{LinearArena, Pool};

fn alloc_benches(c: &mut Criterion) {
    let mut arena = LinearArena::with_capacity(64 * 1024);
    let scalars: Vec<u64> = (0..512).collect();
    let slice: Vec<u32> = (0..256).collect();
    c.bench_function("alloc_arena_frame_513_leases", |b| {
        b.iter(|| {
            let leased =
                renew_bench::arena_frame(black_box(&arena), black_box(&scalars), black_box(&slice));
            arena.reset();
            leased
        });
    });

    let mut pool: Pool<u64> = Pool::with_capacity(1024);
    c.bench_function("alloc_pool_churn_1024", |b| {
        b.iter(|| renew_bench::pool_churn(black_box(&mut pool), black_box(1024)));
    });

    c.bench_function("alloc_boxed_churn_system_256", |b| {
        b.iter(|| renew_bench::boxed_churn(black_box(256)));
    });
}

criterion_group!(benches, alloc_benches);
criterion_main!(benches);
