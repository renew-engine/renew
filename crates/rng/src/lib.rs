//! Seeded, reproducible pseudo-random numbers for simulation: PCG32 with
//! per-domain streams derived from one master seed, and bounded draws with
//! no modulo bias.
//!
//! Randomness in a deterministic simulation is not a source of surprise —
//! it is a *function of the seed*, and the whole job of this crate is to
//! keep it that way. There is no ambient generator, no way to seed from
//! the clock, and no dependency that could offer either: a run's numbers
//! come from the seed the application supplied, or the crate does not
//! produce them.
//!
//! ```
//! use renew_rng::{Rng, Seed, StreamId};
//!
//! // Streams are named where they are used. Two systems never have to
//! // agree on a number, and neither can silently draw from the other's
//! // sequence.
//! const LOOT: StreamId = StreamId::from_name("loot");
//! const SPAWN: StreamId = StreamId::from_name("spawn");
//!
//! let seed = Seed::from_u64(0x5eed);
//! let mut loot = Rng::new(seed, LOOT);
//! let mut spawn = Rng::new(seed, SPAWN);
//!
//! // Per-entity sequences hang off a system's stream by index, so an
//! // entity's rolls are the same however the world was iterated.
//! let mut enemy = Rng::new(seed, SPAWN.child(17));
//!
//! let _roll = loot.next_u32();
//! let _where = spawn.next_u64();
//! let _crit = enemy.next_bool();
//! ```
//!
//! Bounded draws take a non-zero bound; [`Rng::below_u32`] shows the
//! idiom and says why the type is spelled that way.
//!
//! # Contract
//!
//! - **A run is a pure function of its seed.** For a fixed build and
//!   platform, every number this crate produces is determined by the
//!   `(Seed, StreamId)` pair it came from and the number of draws taken
//!   before it. Nothing here reads a clock, allocates, spawns a thread,
//!   or holds iteration-order-dependent state.
//! - **Derivation is order-independent.** [`Rng::new`] is a pure function
//!   of its arguments. Building a stream early, late, or twice gives the
//!   same generator, which is what lets a replay reconstruct one entity's
//!   sequence without replaying every other entity's.
//! - **Distinct streams under one seed cannot collide.** The derivation is
//!   a bijection, so two different [`StreamId`]s under one [`Seed`] always
//!   start from different internal states. What is *not* claimed is proven
//!   statistical independence between streams: the generator's designers
//!   do not claim it, and neither does this crate. What it gives instead
//!   is that the correlations known to exist between this algorithm's
//!   streams cannot be reached by adjacent or patterned identifiers,
//!   because no identifier reaches the generator unmixed.
//! - **Bounded draws are exactly uniform.** Not nearly uniform. See
//!   [`Rng::below_u32`] for the technique and what it costs.
//! - **Nothing can fail.** Non-zero bounds by type, a total constructor,
//!   saturating nothing because there is nothing to saturate: no method
//!   here returns a `Result`, nothing panics, and nothing unwinds.
//! - **No floating point.** Not in the generator, not in the draws, not
//!   in the tests. The crate denies float arithmetic at its root, so the
//!   claim is checked by the compiler rather than by review.
//!
//! # No float draws in v0, on purpose
//!
//! There is no `next_f32`. A float in `[0, 1)` *can* be built exactly —
//! `f32::from_bits(0x3f80_0000 | (bits >> 9)) - 1.0` is bit-exact on every
//! target this engine supports, because the bit pattern is assembled with
//! integers and the subtraction is exact — so the objection is not that it
//! cannot be done. The objection is that this crate has no consumer yet,
//! so shipping a float faucet now means guessing the precision, the
//! interval convention (open, half-open, closed) and the rounding
//! behaviour that some future gameplay system wants, and then owning that
//! guess in a reproducibility contract that spans platforms this engine
//! has not yet proven bit-identical.
//!
//! Until a caller exists with a stated requirement, the recipe above lives
//! in documentation, where it can be read and argued with, rather than in
//! an API, where it would be depended on. Simulation code that needs
//! fractions should prefer fixed point anyway: draw an integer and divide
//! by a constant scale.
//!
//! # Extension points
//!
//! None. No trait, no `dyn`, no runtime polymorphism — the manifest says
//! so and CI holds the crate to it. Swapping generators is not a designed
//! extension point and should not become one lightly: the algorithm is
//! part of what "reproducible" means here, and every recorded trace and
//! golden state hash in the tree is stated against this one.

// This crate computes; it does not print, and it does not do arithmetic on
// floats. Both are denied here rather than left to review.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::float_arithmetic)]

mod generator;
mod mix;
mod seed;

pub use generator::Rng;
pub use seed::{Seed, StreamId};
