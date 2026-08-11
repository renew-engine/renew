# renew-bench

The workspace benchmark suite: criterion timings over the math and
allocator kernels, paired with the assertions that actually gate CI.

## How gating works

Hosted-runner wall time is too noisy to gate a build, so this crate
splits the two jobs benchmarks usually conflate:

- **Timings** (`benches/`) measure; they never gate. `math` covers the
  vector/matrix/quaternion/AABB kernels on seeded array inputs; `alloc`
  covers the arena frame cycle, pool churn, and heap round trips;
  `alloc_counted` reruns the heap kernel under the counting allocator so
  its overhead is a comparison between two runs; `particles` times the
  particle pool's step and instance pack at 1,024 and 4,096 particles
  (its allocation gate lives with the pool's own crate, which asserts
  exact zero on every push).
- **Allocation counts** (`tests/zero_alloc.rs`) gate. The math kernels,
  the arena frame cycle, and pool churn must allocate **exactly zero** —
  an assertion with zero run-to-run variance, checked on every push as
  part of the ordinary test suite.

For the math and allocator kernels, both halves call the same kernel
functions in `src/lib.rs`, so what is timed and what is asserted can
never drift apart. Benches over other crates (jobs, ecs, rng, frame,
particles) call those crates directly, and their gating — where they
have it — lives with the crate that owns the behaviour.

## Running

- All benches: `cargo bench --workspace` (the canonical bench command).
- Smoke (each bench once, no statistics): `cargo bench --workspace -- --test`.
- The gating assertions run with the normal test suite.

Inputs are seeded — identical across runs. The vector and box builders
are bit-identical across platforms too; the quaternion and matrix
builders go through `sin_cos`, so their values are deterministic per
platform rather than across platforms. The `[package.metadata.renew]`
table in [Cargo.toml](Cargo.toml) is authoritative for manifest
metadata.
