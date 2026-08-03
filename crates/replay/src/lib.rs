//! Replaying recorded input deterministically: the translation between
//! the engine's event vocabulary and the trace format, a loader for
//! stored traces, and a recorder that produces them.
//!
//! Any game shipping a replay, a demo mode, or a deterministic
//! regression harness needs exactly this — the need is not "being a
//! sample", it is a capability the engine claims. The code lived inside
//! one sample until a second consumer was committed; copying it would
//! have meant maintaining a correctness property in two places, and the
//! property here is load-bearing: an unknown event must **refuse**
//! loudly, because silently dropping one makes a replay diverge from its
//! recording in a way no test would catch.
//!
//! # What deliberately is not here
//!
//! No filesystem, no clock, no window. This crate turns text into events
//! and events into text; where the text comes from and where it goes is
//! its caller's business. That is not a style preference — the crate
//! declares `simulation = true`, so the structure check refuses it a
//! path to any OS capability, at any dependency depth.
//!
//! # Indexing: ticks, not frames
//!
//! [`events`] returns events indexed by **tick, counted from zero**,
//! because that is what the trace format itself means. Drivers that
//! number frames from one apply their own shift — the shift is a driver
//! convention, and baking one driver's convention into the shared loader
//! would silently impose it on every future consumer.

#![deny(clippy::print_stdout, clippy::print_stderr)]

pub mod convert;
mod load;
pub mod record;

pub use convert::{Unencodable, from_trace, to_trace};
pub use load::{LoadError, events};
pub use record::{RecordError, Recorder};
