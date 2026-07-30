# renew-math

Linear algebra value types for the engine: `Vec2`/`Vec3`/`Vec4`, a
column-major `Mat4`, a rotation `Quat`, and an axis-aligned `Aabb3` — all
`f32`, all plain data.

## Contract

- **Documented layout.** Every type is `#[repr(C)]` with its exact layout
  stated in its docs; `Vec4` and `Mat4` are 16-byte aligned, so the shapes
  are ready for SIMD without committing to intrinsics.
- **Branchless kernels.** Per-component operations (`min`, `max`, `clamp`,
  `lerp`, the predicates on `Aabb3`) contain no data-dependent branches —
  call sites inside hot loops stay vectorizable.
- **Deterministic.** Pure scalar IEEE-754 arithmetic: bit-identical
  results for the same build on the same platform. No clock, no
  filesystem, no allocation, no hashing anywhere in the crate — the
  clock/filesystem half is rejected at lint time via
  [clippy.toml](clippy.toml), not by review.
- **`normalize` requires a positive squared length** (debug assertion);
  the vector types offer `try_normalize`, returning `Option` where absence
  of a usable direction is a normal outcome — accurate across the full
  finite range, subnormals included.

## Public API and thread safety

Six value types, one operation set: constructors and constants
(`ZERO`, unit axes, `IDENTITY`), component arithmetic and the operator
impls, `dot`/`cross`/length/normalization on vectors, transform and
composition on `Mat4`/`Quat`, containment/intersection/union on
`Aabb3`. Exact layouts and per-method contracts live in the doc
comments — the source is the reference, this README doesn't duplicate
it. Everything is `Copy` plain data with no interior state, so every
type is trivially `Send + Sync`: sharing is copying, and there is
nothing to synchronize, ever.

## Testing note

Math's specialized regime applies and is in place: property-based
tests in two strictly separated tiers — bit-exact laws (compared via
`to_bits`, on a stated binding domain) and labeled tolerance
properties. The workspace benchmark suite times the kernels and
asserts they never touch the heap.

## Status

Settled enough to depend on: the type surface is what other crates in
this workspace build against, and breaking it is avoided; operations
are still added when a consumer needs them, not speculatively —
examples and a performance narrative are still to come. The
`[package.metadata.renew]` table in [Cargo.toml](Cargo.toml) is
authoritative for maturity and manifest metadata.

## Key decisions

- **Scalar code, SIMD-ready shape.** Portable SIMD is not available on the
  stable toolchain, and the design favors what compilers already vectorize
  well — branchless single-pass shapes — so scalar code expresses the
  intent fully while the aligned layouts keep the intrinsics door open.
- **Column-major `Mat4`**, matching the convention of the graphics APIs
  this engine targets; `a * b` applies `b` first.
- **`f32` only.** Double-precision variants arrive with a consumer that
  needs them, not before.
