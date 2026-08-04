//! Two-dimensional collision detection that reproduces bit-for-bit.
//!
//! # What this is
//!
//! The collider set and the operations that change it, in fixed-point
//! arithmetic with no floating point anywhere. Bodies own shapes; shapes carry
//! a placement and a filter; every ordering this crate emits is a function of
//! the collider set rather than of how it was built.
//!
//! # Status
//!
//! `bootstrap`. The lifecycle and identity rules are here; the broadphase,
//! narrowphase, queries and move-and-slide are not yet.
//!
//! # The decisions worth knowing before reading the code
//!
//! - **A shape index is stable for the life of its body.** Removing a shape
//!   leaves a hole; the next one fills the lowest free hole. The index is half
//!   of every collider identity and part of the broadphase sort key, so
//!   renumbering would give a different contact order for the same removal
//!   history.
//! - **A handle is an ECS entity, stored whole.** `Entity`'s constructor is
//!   crate-private to `renew-ecs`, so this crate can neither mint a handle nor
//!   rebuild one from its parts — body identity is stored state, not something
//!   derived.
//! - **A stale handle is refused, a foreign one is undetectable.** Two entity
//!   allocators hand out identical handles by construction, so a handle from
//!   another world cannot be told from a correct one. It is a caller warrant.
//! - **Incarnation counters** advance when a collider is rebuilt at an
//!   identity it already used, so an identifier derived from a collider pair
//!   cannot outlive the pair.
//! - **Nothing here is generic over dimension.** A shared vocabulary is
//!   implemented twice rather than abstracted once, so the generic bounds do
//!   not infect every signature.

#![forbid(unsafe_code)]
// The claim above — that there is no floating point anywhere — is enforced
// rather than reviewed. A float that reached a contact would make the world a
// function of the compiler's instruction selection, and this crate's whole
// obligation is that it is not. There is no `allow` below.
#![deny(clippy::float_arithmetic, clippy::print_stdout, clippy::print_stderr)]

pub mod bounds;
pub mod broadphase;
pub mod filter;
pub mod shape;
pub mod world;

pub use bounds::Aabb;
pub use broadphase::Broadphase;
pub use filter::Filter;
pub use shape::{Shape, Transform};
pub use world::{BodyKind, Collider, HandleState, Incarnation, ShapeIndex, World};
