//! Fixed-point arithmetic, for simulation code whose output has to reproduce
//! bit-for-bit on every target.
//!
//! # Why this crate exists
//!
//! Rust guarantees IEEE 754 semantics for `f32` and `f64` *operators*, and
//! that guarantee stops at the operators: `sin`, `cos` and their siblings are
//! the platform's maths library, and are permitted to differ between targets.
//! A simulation that calls them has no cross-target claim to make.
//!
//! Integer arithmetic is bit-identical everywhere, with nothing to police.
//! That is the whole argument.
//!
//! # Contract
//!
//! - **Q47.16 in an `i64`.** 16 fractional bits: a resolution of 2⁻¹⁶, and a
//!   range of ±2⁴⁷. See [`Fixed`] for why those numbers and not others.
//! - **No `f32` or `f64` in any signature this crate exposes.** Converting to
//!   a float is a presentation concern, and the conversion is written at the
//!   boundary that needs it rather than offered here. It cannot be centralised
//!   in the maths crate: that crate is core and this one is optional, so the
//!   core-closure rule refuses the edge outright. Nor is centralising it worth
//!   much — each boundary carries its own precision-loss exemption with its own
//!   reason, and one shared helper would flatten those into a single reason
//!   that fits none of them. What stops a simulation from converting is not
//!   this contract but the float-arithmetic denial it already builds under.
//! - **Every operation is deterministic and target-independent.** No operation
//!   here consults a clock, an allocator, an environment variable, or anything
//!   whose value could differ between two machines running the same build.
//! - **Overflow saturates, in every build profile, and is counted.** Never
//!   wraps, never differs between debug and release. See [`Fixed::saturations`].

// This crate is arithmetic; it does not print. And it is the crate simulation
// arithmetic is written in, so a float operator here would defeat its only
// purpose — denied rather than left to review, and there is no `allow` below.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::float_arithmetic)]

mod angle;
mod saturation;
mod scalar;
mod vector;
mod wide;

pub use angle::Angle;
pub use saturation::{Saturations, saturations};
pub use scalar::Fixed;
pub use vector::{Vec2, Vec3};
pub use wide::Wide;
