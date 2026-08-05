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
//! # The decisions worth knowing before reading the code
//!
//! - **A shape index is stable for the life of its body.** Removing a shape
//!   leaves a hole; the next one fills the lowest free hole.
//! - **A handle is an ECS entity, stored whole**, because `Entity`'s
//!   constructor is crate-private and this crate can neither mint one nor
//!   rebuild one from its parts.
//! - **A stale handle is refused, a foreign one is undetectable.**
//! - **Incarnation counters** advance when a collider is rebuilt at an
//!   identity it already used.

#![forbid(unsafe_code)]
// A float that reached a contact would make the world a function of the
// compiler's instruction selection, and this crate's whole obligation is that
// it is not. There is no `allow` below.
#![deny(clippy::float_arithmetic, clippy::print_stdout, clippy::print_stderr)]

pub mod bounds;
pub mod broadphase;
pub mod filter;
pub mod narrow;
pub mod query;
pub mod ray;
pub mod shape;
pub mod slide;
pub mod sweep;
pub mod world;

pub use bounds::Aabb;
pub use broadphase::Broadphase;
pub use filter::Filter;
pub use narrow::{Contact, collide, separation};
pub use query::{Counts, Exclude, Hit};
pub use ray::{RayHit, cast};
pub use shape::{Shape, Transform};
pub use slide::{SlideEnd, SlideHit, SlideReport};
pub use sweep::{MAX_ADVANCE_STEPS, SweepHit, sweep};
pub use world::{BodyKind, Collider, HandleState, Incarnation, ShapeIndex, World};
