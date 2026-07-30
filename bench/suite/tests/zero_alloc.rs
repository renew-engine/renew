//! The CI-gating half of the benchmark suite: exact allocation counts
//! over the exact kernels the benches time. Exact-zero assertions have
//! zero variance, so they can gate every push where wall time never
//! could. Own process on purpose — the counters are process-wide, and
//! this file holds a single test so nothing else allocates inside the
//! measurement windows.

use renew_memory::{CountingAllocator, LinearArena, Pool, counters};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
#[cfg_attr(
    coverage,
    ignore = "allocation counting is invalid under coverage instrumentation"
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

    // Window 1: every math kernel, repeatedly.
    let before = counters::snapshot();
    let mut checksum = 0.0f32;
    for _ in 0..8 {
        checksum += renew_bench::dot_sum(&pairs);
        checksum += renew_bench::cross_sum(&pairs).x;
        checksum += renew_bench::mat4_mul_chain(&matrices)
            .transform(vectors[0])
            .x;
        checksum += renew_bench::mat4_transform_sum(matrices[0], &vectors).y;
        checksum += renew_bench::quat_mul_chain(&rotations).w;
        checksum += renew_bench::quat_rotate_sum(rotations[0], &points).z;
    }
    let hits = renew_bench::aabb_hits(&boxes, &probes);
    let overlaps = renew_bench::aabb_overlap_count(&boxes, &others);
    let after_math = counters::snapshot();
    assert_eq!(
        after_math.allocations, before.allocations,
        "math kernels allocated (checksum {checksum}, hits {hits})"
    );
    assert_eq!(
        after_math.deallocations, before.deallocations,
        "math kernels freed heap memory"
    );
    assert!(checksum.is_finite());
    assert!(hits > 0);
    assert!(overlaps > 0);

    // Window 2: the arena frame cycle, repeatedly.
    let before_arena = counters::snapshot();
    for _ in 0..64 {
        let leased = renew_bench::arena_frame(&arena, &scalars, &slice);
        assert_eq!(leased, 513);
        arena.reset();
    }
    let after_arena = counters::snapshot();
    assert_eq!(
        after_arena.allocations, before_arena.allocations,
        "arena frame cycle allocated"
    );
    assert_eq!(
        after_arena.deallocations, before_arena.deallocations,
        "arena frame cycle freed heap memory"
    );

    // Window 3: pool churn, repeatedly.
    let before_pool = counters::snapshot();
    let mut sum = 0u64;
    for _ in 0..64 {
        sum = sum.wrapping_add(renew_bench::pool_churn(&mut pool, 1024));
    }
    let after_pool = counters::snapshot();
    assert_eq!(
        after_pool.allocations, before_pool.allocations,
        "pool churn allocated (sum {sum})"
    );
    assert_eq!(
        after_pool.deallocations, before_pool.deallocations,
        "pool churn freed heap memory"
    );
}
