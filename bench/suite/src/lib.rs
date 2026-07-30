//! Benchmark kernels and their input builders.
//!
//! The criterion benches time these functions, and the allocation-count
//! assertions in `tests/` gate the math and arena/pool subset of them —
//! sharing the functions here means a timed path and its assertion can
//! never drift apart.
//!
//! Kernels never allocate — except [`boxed_churn`], whose entire job is
//! heap round trips and which is therefore timed but never zero-gated.
//! Input builders allocate (call them outside any measurement window).
//! All inputs are seeded — same seed, same inputs.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use renew_math::{Aabb3, Mat4, Quat, Vec3, Vec4};
use renew_memory::{LinearArena, Pool};

/// Deterministic input generator (seeded linear congruential — no
/// ambient randomness anywhere in the suite).
struct Rng(u32);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        // The upper 16 bits, exactly representable in f32, scaled to [-1, 1).
        let bits = (self.0 >> 16) as u16;
        f32::from(bits) / 32_768.0 - 1.0
    }

    fn next_vec3(&mut self) -> Vec3 {
        Vec3::new(self.next_f32(), self.next_f32(), self.next_f32())
    }
}

// --- Math kernels -----------------------------------------------------

/// Sum of pairwise dot products.
#[must_use]
pub fn dot_sum(pairs: &[(Vec3, Vec3)]) -> f32 {
    pairs.iter().fold(0.0, |acc, &(a, b)| acc + a.dot(b))
}

/// Component sum of pairwise cross products.
#[must_use]
pub fn cross_sum(pairs: &[(Vec3, Vec3)]) -> Vec3 {
    pairs
        .iter()
        .fold(Vec3::ZERO, |acc, &(a, b)| acc + a.cross(b))
}

/// Left fold of the whole chain of matrix products.
#[must_use]
pub fn mat4_mul_chain(matrices: &[Mat4]) -> Mat4 {
    matrices.iter().fold(Mat4::IDENTITY, |acc, &m| acc * m)
}

/// Sum of one matrix applied to every vector.
#[must_use]
pub fn mat4_transform_sum(matrix: Mat4, vectors: &[Vec4]) -> Vec4 {
    vectors
        .iter()
        .fold(Vec4::ZERO, |acc, &v| acc + matrix.transform(v))
}

/// Left fold of the whole chain of quaternion products.
#[must_use]
pub fn quat_mul_chain(rotations: &[Quat]) -> Quat {
    rotations.iter().fold(Quat::IDENTITY, |acc, &q| acc * q)
}

/// Sum of one rotation applied to every point.
#[must_use]
pub fn quat_rotate_sum(rotation: Quat, points: &[Vec3]) -> Vec3 {
    points
        .iter()
        .fold(Vec3::ZERO, |acc, &p| acc + rotation.rotate(p))
}

/// How many `(box, point)` pairs hit.
#[must_use]
pub fn aabb_hits(boxes: &[Aabb3], points: &[Vec3]) -> usize {
    boxes
        .iter()
        .zip(points)
        .filter(|&(bounds, &point)| bounds.contains(point))
        .count()
}

/// How many box pairs overlap.
#[must_use]
pub fn aabb_overlap_count(left: &[Aabb3], right: &[Aabb3]) -> usize {
    left.iter()
        .zip(right)
        .filter(|&(a, &b)| a.intersects(b))
        .count()
}

// --- Allocator kernels ------------------------------------------------

/// One frame of arena traffic: a scalar lease per input value plus one
/// slice lease. Returns the number of successful leases so exhaustion is
/// visible to the caller instead of silent; the caller resets the arena
/// between frames.
#[must_use]
pub fn arena_frame(arena: &LinearArena, scalars: &[u64], slice: &[u32]) -> usize {
    let mut leased = 0;
    for &value in scalars {
        if arena.alloc(value).is_some() {
            leased += 1;
        }
    }
    if arena.alloc_slice(slice).is_some() {
        leased += 1;
    }
    leased
}

/// Insert/remove round trips through the pool's free list. Returns the
/// sum of removed values so the round trips cannot be optimized away.
#[must_use]
pub fn pool_churn(pool: &mut Pool<u64>, rounds: u64) -> u64 {
    let mut sum = 0u64;
    for value in 0..rounds {
        if let Ok(handle) = pool.insert(value)
            && let Some(removed) = pool.remove(handle)
        {
            sum = sum.wrapping_add(removed);
        }
    }
    sum
}

/// Heap round trips through whatever global allocator the binary
/// installed — benched under both the system allocator and the counting
/// wrapper so the wrapper's overhead is a comparison between two runs.
#[must_use]
pub fn boxed_churn(rounds: u64) -> u64 {
    let mut sum = 0u64;
    for value in 0..rounds {
        let boxed = core::hint::black_box(Box::new(value));
        sum = sum.wrapping_add(*boxed);
    }
    sum
}

// --- Input builders (allocate; call outside measurement windows) ------

/// Seeded pairs of vectors in `[-1, 1)^3`.
#[must_use]
pub fn vec3_pairs(count: usize, seed: u32) -> Vec<(Vec3, Vec3)> {
    let mut rng = Rng(seed);
    (0..count)
        .map(|_| (rng.next_vec3(), rng.next_vec3()))
        .collect()
}

/// Seeded vectors in `[-1, 1)^4`.
#[must_use]
pub fn vec4_inputs(count: usize, seed: u32) -> Vec<Vec4> {
    let mut rng = Rng(seed);
    (0..count)
        .map(|_| {
            Vec4::new(
                rng.next_f32(),
                rng.next_f32(),
                rng.next_f32(),
                rng.next_f32(),
            )
        })
        .collect()
}

/// Seeded points in `[-1, 1)^3`.
#[must_use]
pub fn vec3_inputs(count: usize, seed: u32) -> Vec<Vec3> {
    let mut rng = Rng(seed);
    (0..count).map(|_| rng.next_vec3()).collect()
}

/// Seeded unit quaternions (exact unit axes, varied angles) — chain
/// products stay well conditioned.
#[must_use]
pub fn quat_inputs(count: usize, seed: u32) -> Vec<Quat> {
    let mut rng = Rng(seed);
    (0..count)
        .map(|index| {
            let axis = match index % 3 {
                0 => Vec3::X,
                1 => Vec3::Y,
                _ => Vec3::Z,
            };
            Quat::from_axis_angle(axis, rng.next_f32() * core::f32::consts::PI)
        })
        .collect()
}

/// Seeded rigid transforms (rotation then translation) — chain products
/// stay finite and well conditioned.
#[must_use]
pub fn mat4_inputs(count: usize, seed: u32) -> Vec<Mat4> {
    quat_inputs(count, seed)
        .into_iter()
        .scan(Rng(seed.wrapping_add(1)), |rng, rotation| {
            Some(Mat4::from_translation(rng.next_vec3()) * Mat4::from_quat(rotation))
        })
        .collect()
}

/// A seeded box/point scene with a mixed hit rate: each point is offset
/// by up to one unit from the center of a half-extent-0.5 box, so about
/// one point in eight lands inside its box — enough hits and misses that
/// neither branch of a containment test degenerates away.
#[must_use]
pub fn aabb_scene(count: usize, seed: u32) -> (Vec<Aabb3>, Vec<Vec3>) {
    let mut rng = Rng(seed);
    let boxes: Vec<Aabb3> = (0..count)
        .map(|_| {
            let center = rng.next_vec3();
            let half = Vec3::new(0.5, 0.5, 0.5);
            Aabb3::new(center - half, center + half)
        })
        .collect();
    let points = (0..count)
        .map(|index| boxes[index].center() + rng.next_vec3())
        .collect();
    (boxes, points)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_sum_matches_hand_computation() {
        let pairs = [(Vec3::X, Vec3::X), (Vec3::X, Vec3::Y)];
        assert!((dot_sum(&pairs) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cross_sum_matches_the_right_hand_rule() {
        let pairs = [(Vec3::X, Vec3::Y)];
        let sum = cross_sum(&pairs);
        assert!((sum - Vec3::Z).dot(sum - Vec3::Z) < 1e-12);
    }

    #[test]
    fn chains_of_identities_are_identities() {
        let matrices = vec![Mat4::IDENTITY; 8];
        let product = mat4_mul_chain(&matrices);
        let probe = Vec4::new(1.0, 2.0, 3.0, 4.0);
        let mapped = product.transform(probe);
        assert!((mapped - probe).dot(mapped - probe) < 1e-12);

        let rotations = vec![Quat::IDENTITY; 8];
        let composed = quat_mul_chain(&rotations);
        let point = Vec3::new(1.0, 2.0, 3.0);
        let rotated = composed.rotate(point);
        assert!((rotated - point).dot(rotated - point) < 1e-10);
    }

    #[test]
    fn transform_sums_are_identity_stable() {
        let vectors = [
            Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(-1.0, 0.5, 0.0, 1.0),
        ];
        let summed = mat4_transform_sum(Mat4::IDENTITY, &vectors);
        let expected = vectors[0] + vectors[1];
        assert!((summed - expected).dot(summed - expected) < 1e-12);

        let points = [Vec3::X, Vec3::Y, Vec3::Z];
        let rotated = quat_rotate_sum(Quat::IDENTITY, &points);
        let expected = Vec3::new(1.0, 1.0, 1.0);
        assert!((rotated - expected).dot(rotated - expected) < 1e-10);
    }

    #[test]
    fn builders_are_deterministic_for_the_same_seed() {
        let bits3 = |v: Vec3| (v.x.to_bits(), v.y.to_bits(), v.z.to_bits());
        let bits4 = |v: Vec4| (v.x.to_bits(), v.y.to_bits(), v.z.to_bits(), v.w.to_bits());

        let first = vec3_pairs(64, 7);
        let second = vec3_pairs(64, 7);
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(bits3(a.0), bits3(b.0));
            assert_eq!(bits3(a.1), bits3(b.1));
        }

        for (a, b) in vec3_inputs(32, 11).iter().zip(&vec3_inputs(32, 11)) {
            assert_eq!(bits3(*a), bits3(*b));
        }
        for (a, b) in vec4_inputs(32, 9).iter().zip(&vec4_inputs(32, 9)) {
            assert_eq!(bits4(*a), bits4(*b));
        }
        // Quaternion and matrix builders route through sin_cos: exact
        // within one process, which is all same-seed identity needs.
        for (a, b) in quat_inputs(32, 13).iter().zip(&quat_inputs(32, 13)) {
            assert_eq!(a.x.to_bits(), b.x.to_bits());
            assert_eq!(a.w.to_bits(), b.w.to_bits());
        }
        let probe = Vec4::new(1.0, 2.0, 3.0, 1.0);
        for (a, b) in mat4_inputs(16, 17).iter().zip(&mat4_inputs(16, 17)) {
            assert_eq!(bits4(a.transform(probe)), bits4(b.transform(probe)));
        }
    }

    #[test]
    fn aabb_scene_produces_a_mixed_hit_rate() {
        let (boxes, points) = aabb_scene(256, 42);
        let hits = aabb_hits(&boxes, &points);
        assert!(hits > 0 && hits < 256, "degenerate scene: {hits}/256 hits");
    }

    #[test]
    fn overlap_count_sees_identical_scenes_fully_and_disjoint_scenes_not_at_all() {
        let (boxes, _) = aabb_scene(64, 42);
        assert_eq!(aabb_overlap_count(&boxes, &boxes), 64);

        let far = Vec3::new(1000.0, 1000.0, 1000.0);
        let shifted: Vec<Aabb3> = boxes
            .iter()
            .map(|b| Aabb3::new(b.min() + far, b.max() + far))
            .collect();
        assert_eq!(aabb_overlap_count(&boxes, &shifted), 0);
    }

    #[test]
    fn pool_churn_survives_a_pool_with_no_capacity() {
        let mut pool: Pool<u64> = Pool::with_capacity(0);
        assert_eq!(pool_churn(&mut pool, 16), 0);
    }

    #[test]
    fn arena_frame_reports_every_lease() {
        let mut arena = LinearArena::with_capacity(16 * 1024);
        let scalars: Vec<u64> = (0..64).collect();
        let slice: Vec<u32> = (0..32).collect();
        assert_eq!(arena_frame(&arena, &scalars, &slice), 65);
        assert!(arena.used() > 0);
        arena.reset();
        assert_eq!(arena.used(), 0);
    }

    #[test]
    fn arena_frame_reports_exhaustion_by_count() {
        let arena = LinearArena::with_capacity(8);
        let scalars: Vec<u64> = (0..4).collect();
        let slice: Vec<u32> = (0..4).collect();
        assert!(arena_frame(&arena, &scalars, &slice) < 5);
    }

    #[test]
    fn pool_churn_returns_the_sum_of_round_trips() {
        let mut pool: Pool<u64> = Pool::with_capacity(4);
        assert_eq!(pool_churn(&mut pool, 4), 6);
        assert!(pool.is_empty());
    }

    #[test]
    fn boxed_churn_sums_what_it_boxes() {
        assert_eq!(boxed_churn(4), 6);
    }
}
