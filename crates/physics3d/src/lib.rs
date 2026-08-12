//! Three-dimensional collision detection that reproduces bit-for-bit.
//!
//! # What this is, and what it is not yet
//!
//! The collider set and the operations that change it, in fixed-point
//! arithmetic with no floating point anywhere. **Axis-aligned only**: a
//! placement here is a translation, because rotating in three dimensions needs
//! a fixed-point orientation representation — a quaternion or a matrix — and
//! that decision has not been taken.
//!
//! That is a restriction with a named unblocker rather than a shape of the
//! world. It is also enough to be useful now: a voxel world never asks for a
//! rotated collider, which is why it is the right thing to build first.
//!
//! # Why this is a second implementation rather than a generic one
//!
//! The shared thing between the two dimensions is the *vocabulary* — what a
//! body is, what a contact reports, what order pairs come out in — not the
//! code. A dimension-generic version was considered and rejected: the generic
//! bounds infect every signature, and the result is an API nobody can read for
//! a saving that is mostly in files that differ anyway. The broadphase
//! structure, the narrowphase algorithms and the storage layout are all
//! expected to diverge; the meanings are not.
//!
//! # Two halves, and why the seam is where it is
//!
//! The **geometry** — shapes, bounds, separation, ray casts, swept moves —
//! answers questions about figures in space. It knows nothing about bodies
//! and needs no storage: given two boxes and a displacement it says whether
//! and when they meet.
//!
//! The **collider world** — bodies, the broadphase, the queries that report
//! *which body* was hit — is built on top of it and needs an identity for
//! each body. That identity is an ECS entity, so the world half depends on
//! the entity storage and the geometry half does not.
//!
//! The `world` feature (on by default) is that seam. Turning it off leaves
//! the geometry, for callers that hold their own: a voxel volume, a
//! heightfield, a static mesh. Such a caller has cells or triangles rather
//! than bodies, and making it compile the entity storage to ask whether two
//! boxes overlap would be a dependency it can never use.
//!
//! # The decisions worth knowing before reading the code
//!
//! - **A shape index is stable for the life of its body.** Removing a shape
//!   leaves a hole; the next one fills the lowest free hole.
//! - **A handle is an ECS entity, stored whole**, because `Entity`'s
//!   constructor is crate-private and this crate can neither mint one nor
//!   rebuild one from its parts. This is also what puts the seam above
//!   where it is: the coupling is real, not incidental, so the honest
//!   response is to name which half carries it.
//! - **A stale handle is refused, a foreign one is undetectable.**
//! - **Incarnation counters** advance when a collider is rebuilt at an
//!   identity it already used.

#![forbid(unsafe_code)]
// A float that reached a contact would make the world a function of the
// compiler's instruction selection, and this crate's whole obligation is that
// it is not. There is no `allow` below.
#![deny(clippy::float_arithmetic, clippy::print_stdout, clippy::print_stderr)]
// The README is the crate front page, included so it is built with the
// crate rather than drifting from it.
#![doc = include_str!("../README.md")]

// The geometry: figures in space, no storage and no identities.
pub mod bounds;
pub mod filter;
pub mod narrow;
pub mod ray;
pub mod shape;
pub mod sweep;

// The collider world, behind the `world` feature. Everything here either
// holds a body handle or hands one back.
#[cfg(feature = "world")]
pub mod broadphase;
#[cfg(feature = "world")]
pub mod clear;
#[cfg(feature = "world")]
pub mod query;
#[cfg(feature = "world")]
pub mod slide;
#[cfg(feature = "world")]
pub mod world;

pub use bounds::Aabb;
pub use filter::Filter;
pub use narrow::{Contact, collide, separation};
pub use ray::{RayHit, cast};
pub use shape::{Shape, Transform};
pub use sweep::{MAX_ADVANCE_STEPS, SweepHit, sweep};

#[cfg(feature = "world")]
pub use broadphase::Broadphase;
#[cfg(feature = "world")]
pub use clear::{ClearEnd, ClearReport};
#[cfg(feature = "world")]
pub use query::{Counts, Exclude, Hit};
#[cfg(feature = "world")]
pub use slide::{SlideEnd, SlideHit, SlideReport};
#[cfg(feature = "world")]
pub use world::{BodyKind, Collider, HandleState, Incarnation, ShapeIndex, World};
