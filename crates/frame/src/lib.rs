//! Fixed-timestep frame scheduling: the deterministic accumulator, the
//! step budget that bounds a stall, and the interpolation factor for
//! rendering between steps.
//!
//! The loop is a passive integer state machine. It owns no loop, drives no
//! application, knows nothing of rendering, GPUs or windows, and never
//! reads a clock — it *cannot*, having no dependency that offers one. Its
//! whole job is one total function: [`FrameLoop::begin_frame`] answers
//! *given the schedule so far and this instant, how many fixed steps are
//! due, how many did the budget refuse, and how far between steps is the
//! renderer.* The caller reads the one clock, executes the steps, and
//! renders.
//!
//! ```
//! use renew_frame::{FrameLoop, FrameStats, StepBudget, Timestamp, Timestep};
//!
//! let mut frame = FrameLoop::new(
//!     Timestep::HZ_60,
//!     StepBudget::DEFAULT,
//!     Timestamp::from_nanos(0),
//! );
//! let mut stats = FrameStats::new();
//!
//! // A headless driver: no clock is read, so the whole run is a pure
//! // function of the timestamp sequence and is byte-comparable across
//! // runs, processes and machines.
//! for k in 1..=600u64 {
//!     let now = Timestamp::from_nanos(k.saturating_mul(16_666_667));
//!     let plan = frame.begin_frame(now);
//!     for step in plan.steps() {
//!         let _ = (step.tick, step.dt, step.sim_time); // advance the world here
//!     }
//!     // Render between steps with `renew_math::Alpha::new(...)`,
//!     // built from `plan.remainder()` and `plan.timestep()`.
//!     stats.absorb(&plan);
//! }
//!
//! assert_eq!(stats.frames(), 600);
//! assert_eq!(stats.ticks(), 600);
//! assert_eq!(stats.steps_dropped(), 0);
//! ```
//!
//! # Contract
//!
//! - **Deterministic.** For a fixed build and platform, [`FrameLoop`]
//!   is a pure function of `(timestep, budget, start, the sequence of
//!   timestamps passed to begin_frame)`. It reads no clock, allocates
//!   nothing, spawns nothing, and holds no iteration-order-dependent
//!   state. A headless run supplies that sequence synthetically and is
//!   reproducible; a realtime run supplies a measured one, which is a
//!   different *input trace*, not nondeterministic *code*.
//! - **Nothing can fail.** Non-zero types, a saturating bank and a
//!   saturating delta between them leave no error to report, so
//!   `begin_frame` returns no `Result` — an uninhabitable error variant
//!   would be a lie about the API. Nothing here panics and nothing
//!   unwinds.
//! - **The plan must be executed.** The one available contract violation —
//!   a caller that ignores its plan — is unobservable from inside, so it
//!   is contract text with `#[must_use]` as the mitigation rather than an
//!   assertion. A skipped plan silently desynchronizes the simulation from
//!   the tick counter.
//! - **Clamp and discard, always reported.** Steps beyond the budget are
//!   discarded, never banked: keeping the surplus *is* the spiral of
//!   death. Simulation time therefore falls permanently behind the wall
//!   clock, and [`FramePlan::dropped`] is the exact, non-optional record
//!   of by how much.
//! - **`alpha` is never an input to simulation.** It is a render-side hint
//!   in `[0, 1)`, and it is deliberately excluded from the schedule
//!   digest.
//! - **Zero dependencies, and this crate never logs.** A dropped step is
//!   reported through the returned plan; whether that is a log line is the
//!   caller's decision.
//!
//! # Extension points
//!
//! None. There is no trait, no `dyn`, and no runtime polymorphism here —
//! the manifest says so and CI holds the crate to it. The growth point is
//! named rather than pre-built: a trait arrives when a second
//! implementation exists.

// This crate reports; it does not print. Diagnostics belong to the caller,
// which is what keeps the dependency list empty.
// The determinism rule in the language standard: a simulation crate does not
// perform floating-point arithmetic whose result can reach digested state.
// Denied here rather than left to review — the lint covers operators only, so
// it is necessary and not sufficient, but what it does cover it covers with
// teeth.
//
// This crate held the tree's only exemption: the interpolation factor was
// computed here, with an `allow` at the expression. It is gone. The
// factor moved to `renew-math` — a crate a simulation is mechanically
// forbidden from reaching — and this crate now performs no floating-point
// arithmetic at all. There is no `allow` below, and adding one would be a
// change to the language standard, not a local decision.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::float_arithmetic)]

mod digest;
mod report;
mod schedule;
mod time;

pub use digest::StateHash;
pub use report::{FrameStats, FrameStatsJson, FrameTiming, FrameTimingJson};
pub use schedule::{FrameLoop, FramePlan, Step, Steps};
pub use time::{Nanos, StepBudget, Timestamp, Timestep};
