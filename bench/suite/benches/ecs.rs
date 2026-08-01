//! Entity and component-store timings: the four shapes a system asks for.
//!
//! Recorded baselines for the entity and component stores were missing:
//! the storage decision was settled on measurements that were never
//! committed, so nothing here could detect the day the shipped
//! implementation regressed.
//!
//! What is measured, and why each one rather than a round number of
//! operations:
//!
//! - **Ordered iteration is the cost of the determinism guarantee.** The
//!   store walks its sparse array so a query visits slots in ascending
//!   order regardless of insertion history. That walk is proportional to
//!   the *highest occupied slot*, not to the number of components, which
//!   is the whole trade — and the sparse case below is where it shows.
//! - **`iter_unordered` is the same query without the guarantee**, so the
//!   pair prices the guarantee directly rather than describing it.
//! - **Churn** is insert-and-remove at a steady population, the shape a
//!   spawner produces, and the case where slot reuse either keeps the
//!   occupied range tight or does not.
//! - **Spawn** is the allocator's own cost with the free list warm.
//!
//! No clock, no allocation inside the timed region beyond what the store
//! itself does, and every input fixed: two runs of this file are
//! comparable or one of them is broken.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_ecs::{Entities, Store};

/// Population size. Large enough that per-element cost dominates the
/// call, small enough that the whole working set stays in cache — this
/// measures the data structure, not the memory system.
const N: u32 = 4096;

/// How far apart the sparse case spreads its live slots. Ordered
/// iteration walks the gaps, so this is the multiplier on the thing the
/// ordering guarantee actually costs.
const SPREAD: u32 = 8;

fn dense_store() -> Store<u64> {
    let mut store = Store::new();
    for slot in 0..N {
        store.insert(slot, u64::from(slot));
    }
    store
}

/// The same component count, spread over `SPREAD`× the slot range. The
/// dense and sparse pair is the point: same work for `iter_unordered`,
/// very different work for `iter`.
fn sparse_store() -> Store<u64> {
    let mut store = Store::new();
    for index in 0..N {
        store.insert(index * SPREAD, u64::from(index));
    }
    store
}

fn benches(c: &mut Criterion) {
    let dense = dense_store();
    let sparse = sparse_store();

    c.bench_function("ecs_iter_ordered_dense_4096", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for (slot, value) in dense.iter() {
                sum = sum.wrapping_add(u64::from(slot)).wrapping_add(*value);
            }
            black_box(sum)
        });
    });

    c.bench_function("ecs_iter_unordered_dense_4096", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for value in dense.iter_unordered() {
                sum = sum.wrapping_add(*value);
            }
            black_box(sum)
        });
    });

    c.bench_function("ecs_iter_ordered_sparse_4096_over_32768", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for (slot, value) in sparse.iter() {
                sum = sum.wrapping_add(u64::from(slot)).wrapping_add(*value);
            }
            black_box(sum)
        });
    });

    c.bench_function("ecs_iter_unordered_sparse_4096_over_32768", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for value in sparse.iter_unordered() {
                sum = sum.wrapping_add(*value);
            }
            black_box(sum)
        });
    });

    c.bench_function("ecs_spawn_despawn_churn_4096", |b| {
        b.iter(|| {
            let mut entities = Entities::new();
            let live: Vec<_> = (0..N).map(|_| entities.spawn()).collect();
            for entity in &live {
                entities.despawn(*entity);
            }
            // Respawn into the freed slots: this is where reuse either
            // keeps the occupied range tight or lets it grow.
            for _ in 0..N {
                black_box(entities.spawn());
            }
            black_box(entities.len())
        });
    });

    c.bench_function("ecs_insert_remove_churn_4096", |b| {
        b.iter(|| {
            let mut store: Store<u64> = Store::new();
            for slot in 0..N {
                store.insert(slot, u64::from(slot));
            }
            for slot in 0..N {
                black_box(store.remove(slot));
            }
            black_box(store.len())
        });
    });
}

criterion_group!(ecs, benches);
criterion_main!(ecs);
