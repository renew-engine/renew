//! Linear algebra value types: vectors, a column-major matrix, a
//! quaternion, and an axis-aligned bounding box, all `f32`.
//!
//! # Contract
//!
//! - **Documented layout.** Every type is `#[repr(C)]`; [`Vec4`] and
//!   [`Mat4`] are 16-byte aligned. The layouts below are part of the API.
//! - **Branchless kernels.** Per-component operations contain no branches:
//!   `min`/`max`/`clamp`/`lerp` are arithmetic on every path, so call
//!   sites inside hot loops stay vectorizable.
//! - **Deterministic.** Pure scalar IEEE-754 arithmetic — bit-identical
//!   results for the same build on the same platform. No clock, no
//!   filesystem, no allocation, no hashing anywhere in this crate.
//! - **`normalize` requires a positive squared length** (caller contract,
//!   checked by a debug assertion); the vector types offer
//!   [`Vec3::try_normalize`]-style fallible variants where the caller
//!   cannot promise. [`Quat`] has no fallible variant yet — no consumer
//!   has needed one.

// Diagnostics are not this crate's job; the standard output macros are
// banned by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod aabb;
mod mat4;
mod quat;
mod vec;

pub use aabb::Aabb3;
pub use mat4::Mat4;
pub use quat::Quat;
pub use vec::{Vec2, Vec3, Vec4};
