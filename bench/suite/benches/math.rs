//! Math kernel timings. Inputs are seeded and array-sized so the
//! branchless kernels' auto-vectorization is what gets measured; CI
//! gating lives in the allocation assertions, never in these timings.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

const COUNT: usize = 4096;
const SEED: u32 = 0x5EED_0001;

fn math_benches(c: &mut Criterion) {
    let pairs = renew_bench::vec3_pairs(COUNT, SEED);
    c.bench_function("math_vec3_dot_4096", |b| {
        b.iter(|| renew_bench::dot_sum(black_box(&pairs)));
    });
    c.bench_function("math_vec3_cross_4096", |b| {
        b.iter(|| renew_bench::cross_sum(black_box(&pairs)));
    });

    let matrices = renew_bench::mat4_inputs(COUNT, SEED);
    c.bench_function("math_mat4_mul_chain_4096", |b| {
        b.iter(|| renew_bench::mat4_mul_chain(black_box(&matrices)));
    });

    let vectors = renew_bench::vec4_inputs(COUNT, SEED);
    c.bench_function("math_mat4_transform_4096", |b| {
        b.iter(|| renew_bench::mat4_transform_sum(black_box(matrices[0]), black_box(&vectors)));
    });

    let rotations = renew_bench::quat_inputs(COUNT, SEED);
    c.bench_function("math_quat_mul_chain_4096", |b| {
        b.iter(|| renew_bench::quat_mul_chain(black_box(&rotations)));
    });

    let points = renew_bench::vec3_inputs(COUNT, SEED);
    c.bench_function("math_quat_rotate_4096", |b| {
        b.iter(|| renew_bench::quat_rotate_sum(black_box(rotations[0]), black_box(&points)));
    });

    let (boxes, probes) = renew_bench::aabb_scene(COUNT, SEED);
    c.bench_function("math_aabb_contains_4096", |b| {
        b.iter(|| renew_bench::aabb_hits(black_box(&boxes), black_box(&probes)));
    });

    let (others, _) = renew_bench::aabb_scene(COUNT, SEED.wrapping_add(1));
    c.bench_function("math_aabb_intersects_4096", |b| {
        b.iter(|| renew_bench::aabb_overlap_count(black_box(&boxes), black_box(&others)));
    });
}

criterion_group!(benches, math_benches);
criterion_main!(benches);
