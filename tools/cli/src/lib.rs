//! Library half of the `renew` binary: argument parsing, the canonical
//! command table, JSON output, and environment checks.
//!
//! The binary in `main.rs` is a thin I/O shell over these modules so the
//! decision logic stays unit-testable without spawning processes.

pub mod cli;
pub mod coverage;
pub mod doctor;
pub mod json;
pub mod plan;
pub mod structure;
pub mod workspace;
