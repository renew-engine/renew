//! Operating-system seams: the rest of the engine reaches the OS only
//! through this crate.
//!
//! # Contract
//!
//! - **This crate is the doorway, not a hallway.** Engine code never
//!   touches `std::time`, `std::fs`, or `std::thread` directly; it takes
//!   a [`Clock`], calls [`fs`], or spawns through [`thread`]. The same
//!   holds for the devices behind the feature-gated seams: the window
//!   and the sound card are reached only through `window` and `audio`,
//!   and neither lets its third-party vocabulary out.
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
/// The event vocabulary, re-exported from the crate that owns it.
///
/// **It is a neighbour, not a resident.** The types are plain data with
/// no way to reach the operating system, and they used to live here
/// only because everything in this crate was "platform-ish" — which is
/// exactly what made the dependency edge onto this crate impossible to
/// forbid. They now have their own crate with no dependencies, so a
/// consumer that needs the vocabulary and nothing else can take it
/// without taking a clock, a filesystem and thread spawning as well.
///
/// Re-exported so every path a consumer already uses keeps working. The
/// code that *produces* these values from the OS stays here, in
/// [`window`], and does need a windowing library.
pub use renew_event as event;
/// A diagnostics sink that writes to a file.
///
/// Here because the reporting crate forbids filesystem access in its own
/// lint configuration and this crate is the filesystem's doorway. It
/// reads no environment and installs nothing: a binary decides whether
/// to log and where.
pub mod diag;
pub mod fs;
pub mod thread;

/// Audio output: the default device, a negotiated stream shape, and a
/// callback the operating system's audio thread drives. Behind the
/// `audio-out` feature, which is off by default — a build that plays
/// nothing compiles no audio stack at all.
#[cfg(feature = "audio-out")]
pub mod audio;

#[cfg(feature = "window")]
pub mod window;

/// UDP datagrams: bind, send, and a non-blocking receive. Behind the
/// `net` feature, which is off by default — a build that talks to nobody
/// compiles no socket at all.
#[cfg(feature = "net")]
pub mod net;

pub use clock::Clock;
/// The error-classification vocabulary, re-exported so consumers match
/// on kinds without importing `std::io` themselves — the doorway stays
/// a doorway.
pub use std::io::ErrorKind;
