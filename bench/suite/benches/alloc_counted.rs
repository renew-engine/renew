//! The same heap kernel as `alloc`, with the counting wrapper installed
//! as this binary's global allocator — the delta against
//! `alloc_boxed_churn_system_256` is the wrapper's measured overhead.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_memory::CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn counted_benches(c: &mut Criterion) {
    c.bench_function("alloc_boxed_churn_counted_256", |b| {
        b.iter(|| renew_bench::boxed_churn(black_box(256)));
    });
}

criterion_group!(benches, counted_benches);
criterion_main!(benches);
