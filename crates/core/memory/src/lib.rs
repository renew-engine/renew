//! The engine's allocation seam: explicit allocators passed as context,
//! plus an instrumented global allocator for counting.
//!
//! # Contract
//!
//! - **Hot-path allocation is explicit.** [`LinearArena`] and [`Pool`] are
//!   passed to the code that allocates from them; ownership and lifetime
//!   are visible at every call site.
//! - **Backing storage comes from the process's global allocator at
//!   construction** — never from platform APIs — and is acquired up
//!   front: neither allocator grows.
//! - **[`CountingAllocator`] counts everything.** A binary that installs
//!   it as its global allocator gets process-wide allocation counters,
//!   read through [`counters::snapshot`]; the counters are diagnostics,
//!   never control flow.
//! - **This crate never reads a clock and never touches the filesystem**
//!   (rejected at lint time), and [`LinearArena`] is deliberately not
//!   `Sync` — sharing an arena across threads is a design error here.
//!
//! `unsafe` is confined to the allocator internals (aligned writes into
//! owned storage; the `GlobalAlloc` wrapper) with the invariant stated at
//! every block.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]
// Allocator internals cannot be expressed in safe Rust; the exception is
// scoped to this crate's library code and every block carries SAFETY.
#![allow(unsafe_code)]

mod arena;
pub mod counters;
mod counting;
mod pool;

pub use arena::LinearArena;
pub use counting::CountingAllocator;
pub use pool::{Handle, Pool};
