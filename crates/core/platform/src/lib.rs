//! Operating-system seams: the rest of the engine reaches the OS only
//! through this crate.
//!
//! # Contract
//!
//! - **This crate is the doorway, not a hallway.** Engine code never
//!   touches `std::time`, `std::fs`, or `std::thread` directly; it takes
//!   a [`Clock`], calls [`fs`], or spawns through [`thread`].
//! - **No ambient state.** A [`Clock`] is a value the caller owns and
//!   passes; there is no global "current time", and nothing here reads
//!   configuration from the environment.
//! - **Errors carry context.** Every filesystem error names its path;
//!   every thread error names its thread. Crate-local enums only.
//! - **Nothing here is simulation state.** The clock feeds frame pacing
//!   and diagnostics; simulation time is fixed-step by construction and
//!   never reads a wall clock.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod clock;
pub mod fs;
pub mod thread;
#[cfg(feature = "window")]
pub mod window;

pub use clock::Clock;
/// The error-classification vocabulary, re-exported so consumers match
/// on kinds without importing `std::io` themselves — the doorway stays
/// a doorway.
pub use std::io::ErrorKind;
