//! The CI-gating half of the benchmark suite: exact allocation counts
//! over the exact kernels the benches time. Exact-zero assertions have
//! zero variance in what they measure, so they can gate every push
//! where wall time never could.
//!
//! Measurement protocol: the counters are process-wide, and the test
//! harness's own thread can allocate concurrently (observed on Linux,
//! where its progress output landed inside a window). So each window
//! retries: one-shot neighbor noise rides out, while a genuine kernel
//! allocation reproduces in every window and still fails. A warmup pass
//! runs every kernel once first, so one-time lazy initialization never
//! lands in a window either. Own process on purpose; this file holds a
//! single test so no sibling test allocates alongside it.

use renew_memory::{CountingAllocator, LinearArena, Pool, counters};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Run the window until one pass shows exactly zero allocator activity.
/// Fallible on purpose: the `expect()` lives inside the test, where the
/// lint configuration scopes it.
fn quiet_window(attempts: usize, mut window: impl FnMut()) -> Result<(), String> {
    let mut last = (0u64, 0u64);
    for _ in 0..attempts {
        let before = counters::snapshot();
        window();
        let after = counters::snapshot();
        if after.allocations == before.allocations && after.deallocations == before.deallocations {
            return Ok(());
        }
        last = (
            after.allocations - before.allocations,
            after.deallocations - before.deallocations,
        );
    }
    Err(format!(
        "allocator activity in every window (last deltas: +{} allocations, +{} deallocations)",
        last.0, last.1
    ))
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn kernels_allocate_exactly_nothing() {
    // Everything that allocates happens before the windows open.
    let pairs = renew_bench::vec3_pairs(1024, 0x5EED_0001);
    let matrices = renew_bench::mat4_inputs(256, 0x5EED_0001);
    let vectors = renew_bench::vec4_inputs(1024, 0x5EED_0001);
    let rotations = renew_bench::quat_inputs(256, 0x5EED_0001);
    let points = renew_bench::vec3_inputs(1024, 0x5EED_0001);
    let (boxes, probes) = renew_bench::aabb_scene(1024, 0x5EED_0001);
    let (others, _) = renew_bench::aabb_scene(1024, 0x5EED_0002);
    let mut arena = LinearArena::with_capacity(64 * 1024);
    let scalars: Vec<u64> = (0..512).collect();
    let slice: Vec<u32> = (0..256).collect();
    let mut pool: Pool<u64> = Pool::with_capacity(1024);

    // Warmup: every kernel once, with its results sanity-checked here so
    // the windows below measure steady-state behavior only.
    let mut checksum = renew_bench::dot_sum(&pairs);
    checksum += renew_bench::cross_sum(&pairs).x;
    checksum += renew_bench::mat4_mul_chain(&matrices)
        .transform(vectors[0])
        .x;
    checksum += renew_bench::mat4_transform_sum(matrices[0], &vectors).y;
    checksum += renew_bench::quat_mul_chain(&rotations).w;
    checksum += renew_bench::quat_rotate_sum(rotations[0], &points).z;
    assert!(checksum.is_finite());
    assert!(renew_bench::aabb_hits(&boxes, &probes) > 0);
    assert!(renew_bench::aabb_overlap_count(&boxes, &others) > 0);
    assert_eq!(renew_bench::arena_frame(&arena, &scalars, &slice), 513);
    arena.reset();
    assert!(renew_bench::pool_churn(&mut pool, 1024) > 0);

    // Window 1: every math kernel, repeatedly.
    quiet_window(5, || {
        let mut sum = 0.0f32;
        for _ in 0..8 {
            sum += renew_bench::dot_sum(&pairs);
            sum += renew_bench::cross_sum(&pairs).x;
            sum += renew_bench::mat4_mul_chain(&matrices)
                .transform(vectors[0])
                .x;
            sum += renew_bench::mat4_transform_sum(matrices[0], &vectors).y;
            sum += renew_bench::quat_mul_chain(&rotations).w;
            sum += renew_bench::quat_rotate_sum(rotations[0], &points).z;
        }
        let hits = renew_bench::aabb_hits(&boxes, &probes);
        let overlaps = renew_bench::aabb_overlap_count(&boxes, &others);
        core::hint::black_box((sum, hits, overlaps));
    })
    .expect("math kernels stay heap-silent");

    // Window 2: the arena frame cycle, repeatedly.
    quiet_window(5, || {
        for _ in 0..64 {
            let leased = renew_bench::arena_frame(&arena, &scalars, &slice);
            assert_eq!(leased, 513);
            arena.reset();
        }
    })
    .expect("arena frame cycle stays heap-silent");

    // Window 3: pool churn, repeatedly.
    quiet_window(5, || {
        let mut sum = 0u64;
        for _ in 0..64 {
            sum = sum.wrapping_add(renew_bench::pool_churn(&mut pool, 1024));
        }
        core::hint::black_box(sum);
    })
    .expect("pool churn stays heap-silent");
}
