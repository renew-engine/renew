//! The frame loop driving the renderer: a triangle on a clear colour the
//! simulation computes, in a window or into an offscreen image.
//!
//! The loop itself is a passive state machine in `renew-frame`. It owns
//! no loop, drives no application, and knows nothing about GPUs or
//! windows. This sample is the other half of that arrangement: it reads
//! the one clock (or invents one), executes the steps the plan asks for,
//! renders between them, and reports what happened. Both drivers here —
//! windowed and headless — call the same three lines:
//!
//! ```text
//! let plan = frame.begin_frame(now);
//! for step in plan.steps() { world.step(step); }
//! stats.absorb(&plan);
//! ```
//!
//! # Contract
//!
//! - **`--headless` implies a synthetic time source.** The headless
//!   driver feeds the schedule frame indices, not measurements, so its
//!   digest line is a pure function of `(frames, seed)` and is identical
//!   across runs, processes and machines. That property is what the
//!   cross-process determinism test compares, and it is why the same
//!   binary is CI's evidence rather than a special test build.
//! - **Measured numbers are quarantined.** The one clock the headless
//!   driver reads brackets each frame for the timing summary. Nothing
//!   measured reaches the schedule, the state hash, or the digest line.
//! - **The simulation is integer-only.** Bit-determinism is scoped to a
//!   single platform; a world holding a float angle would silently make
//!   the state hash a cross-platform promise the engine does not make.
//!   If the triangle ever spins, the angle is a tick count and the trig
//!   happens in the shader — render, not simulation.
//! - **Steady state is frames `[3, N)`, and it allocates nothing.**
//!   Everything that allocates happens before frame zero: device,
//!   target, pipeline, and the readback buffer. No file I/O, no logging
//!   and no serialization happens inside the loop — `--dump-stats`
//!   writes after it exits — and the one thing on the frame path that
//!   formats text, the window-title readout, formats into a buffer it
//!   owns.
//! - **An environment that cannot host the run is a skip, not a
//!   failure.** No GPU runtime and no display server are ordinary
//!   answers on ordinary machines; the binary says `SKIP:` and exits
//!   zero. Set `RENEW_FRAME_STRICT=1` where a skip would be a lie — the
//!   CI lane that exists to run this — and a skip becomes a failure.

use std::process::ExitCode;

mod cli;
mod error;
mod offscreen;
#[cfg(feature = "window")]
mod readout;
mod render;
#[cfg(feature = "window")]
mod windowed;
mod world;

pub use cli::{DEFAULT_FRAMES, Options, Report, SAMPLE, USAGE, parse_args};
pub use error::SampleError;
pub use offscreen::{Draw, EXTENT, HeadlessRun, WARMUP_FRAMES};
#[cfg(feature = "window")]
pub use readout::Readout;
pub use render::{Surface, clear_color};
pub use world::World;

/// Setting this to `1` turns a skip into a failure: on a lane that
/// exists to run this sample, "no GPU here" is a broken lane, not a
/// tolerable environment.
pub const STRICT: &str = "RENEW_FRAME_STRICT";

/// Exit code for a command line this build cannot honour.
const USAGE_EXIT: u8 = 2;
/// Exit code for a run that failed.
const FAILURE_EXIT: u8 = 1;

/// The whole binary, minus the process boundary: parse, run, report.
///
/// Returns the process exit code — `0` for a completed run or a skip,
/// `1` for a failure, `2` for a bad command line. Exactly one line
/// reaches stdout on the success path, and it is the digest line the
/// determinism gate compares.
pub fn run_cli<I: IntoIterator<Item = String>>(args: I) -> u8 {
    // Diagnostics, before anything can fail. `RENEW_LOG` names a file;
    // when it is set the engine's own error channel and any panic land
    // there, and the device is brought up with validation on.
    renew_platform::diag::log_to_file(diagnostics_path());
    let strict = std::env::var_os(STRICT).is_some_and(|value| value == "1");
    match run(args) {
        Ok(report) => {
            println!("{}", report.digest_line());
            0
        }
        Err(error) => report_error(&error, strict),
    }
}

fn run<I: IntoIterator<Item = String>>(args: I) -> Result<Report, SampleError> {
    let options = parse_args(args)?;
    let report = if options.headless {
        offscreen::run(&options)?
    } else {
        windowed_run(&options)?
    };
    if let Some(path) = &options.dump_stats {
        renew_platform::fs::write(path, report.json().as_bytes()).map_err(dump_failed)?;
    }
    Ok(report)
}

#[cfg(feature = "window")]
fn windowed_run(options: &Options) -> Result<Report, SampleError> {
    windowed::run(options)
}

/// The honest answer in a build with the windowing stack compiled out:
/// name the reason and exit non-zero. Silently pretending to open a
/// window would make the removability evidence worthless.
#[cfg(not(feature = "window"))]
fn windowed_run(_options: &Options) -> Result<Report, SampleError> {
    Err(SampleError::Usage(
        "this build has no windowing support; run with --headless".to_string(),
    ))
}

/// The stats file could not be written.
#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err hands the error over by value; a by-reference signature would need a closure at every call site"
)]
fn dump_failed(error: renew_platform::fs::FsError) -> SampleError {
    SampleError::failed("writing the stats file", &error)
}

/// Say what went wrong, in the channel that matches what it means.
fn report_error(error: &SampleError, strict: bool) -> u8 {
    match error {
        // A skip goes to stdout: it is a result, not a diagnostic, and
        // the reader is usually a CI log or a test harness.
        SampleError::Unavailable(reason) if !strict => {
            println!("SKIP: {reason}");
            0
        }
        SampleError::Unavailable(reason) => {
            eprintln!("FAIL: {STRICT}=1 but the run cannot happen here: {reason}");
            FAILURE_EXIT
        }
        SampleError::Usage(message) => {
            eprintln!("{message}");
            USAGE_EXIT
        }
        SampleError::Failed(message) => {
            eprintln!("FAIL: {message}");
            FAILURE_EXIT
        }
    }
}

/// The binary's entry point, as a value: the process boundary in
/// `main.rs` is one line so that everything above it is testable.
#[must_use]
pub fn exit_code(code: u8) -> ExitCode {
    ExitCode::from(code)
}

/// The file `RENEW_LOG` names, if it names one.
///
/// One variable rather than a flag, so it covers a panic that happens
/// before the command line is parsed — which is exactly the failure a
/// flag cannot describe. An empty value is treated as unset, because a
/// shell that exports an empty string meant to turn it off.
#[must_use]
pub fn diagnostics_path() -> Option<std::path::PathBuf> {
    renew_platform::diag::path_from_value(std::env::var_os("RENEW_LOG"))
}

/// Whether this run is logging diagnostics, which also decides whether
/// the renderer asks for validation.
///
/// **`IfAvailable`, never `Required`.** A machine without the validation
/// layer installed must still run while something else is being
/// debugged; requiring it would turn a missing optional component into a
/// failure to start.
#[must_use]
pub fn diagnostics_enabled() -> bool {
    diagnostics_path().is_some()
}

/// Which validation policy a run asks the renderer for.
///
/// **A named function rather than a conditional at the call site**, so
/// both answers can be asserted. Inline, the diagnostics arm executes
/// only in a run that has already set the variable and opened a window,
/// which no test does — the choice would be made in a place nothing
/// could observe. `IfAvailable` rather than `Required`: a machine
/// without the validation layer must still run while something else is
/// being debugged.
#[must_use]
pub fn validation_policy() -> renew_rhi::Validation {
    validation_for(diagnostics_enabled())
}

/// The policy a run asks for, given whether it is logging.
///
/// Split from the reader so both answers are reachable without touching
/// the environment — which is process-wide, and which a test that wrote
/// it would be racing every other test in its binary. `IfAvailable`
/// rather than `Required`: a machine without the validation layer must
/// still run while something else is being debugged.
#[must_use]
pub fn validation_for(logging: bool) -> renew_rhi::Validation {
    if logging {
        renew_rhi::Validation::IfAvailable
    } else {
        renew_rhi::Validation::Off
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_validation_policy_answers_both_ways() {
        assert_eq!(
            super::validation_for(true),
            renew_rhi::Validation::IfAvailable,
            "a logged run asks for validation, which is what can name a fault in the driver"
        );
        assert_eq!(
            super::validation_for(false),
            renew_rhi::Validation::Off,
            "an ordinary run pays nothing for a layer it will not read"
        );
    }
    use super::{FAILURE_EXIT, SampleError, USAGE_EXIT, dump_failed, report_error, run_cli};
    use renew_platform::fs::FsError;
    use std::path::PathBuf;

    #[test]
    fn a_stats_file_that_cannot_be_written_names_the_path_and_the_reason() {
        let error = dump_failed(FsError::NotFound {
            path: PathBuf::from("nowhere/stats.json"),
        });
        let message = error.to_string();
        assert!(message.starts_with("writing the stats file:"), "{message}");
        assert!(message.contains("stats.json"), "{message}");
    }

    #[test]
    fn a_bad_command_line_reaches_the_process_exit_code() {
        assert_eq!(run_cli(["--turbo".to_string()]), USAGE_EXIT);
    }

    #[test]
    fn an_unavailable_environment_is_a_skip_until_strictness_says_otherwise() {
        let missing = SampleError::Unavailable("no GPU runtime".to_string());
        assert_eq!(report_error(&missing, false), 0, "a skip is not a failure");
        assert_eq!(
            report_error(&missing, true),
            FAILURE_EXIT,
            "a lane that exists to run this must not pass by skipping"
        );
    }

    #[test]
    fn usage_and_failure_have_their_own_codes() {
        assert_eq!(
            report_error(&SampleError::Usage("no such flag".to_string()), false),
            USAGE_EXIT
        );
        assert_eq!(
            report_error(
                &SampleError::Failed("render: device lost".to_string()),
                false
            ),
            FAILURE_EXIT
        );
    }

    #[test]
    fn the_exit_code_conversion_is_the_one_the_binary_uses() {
        // `ExitCode` has no equality to assert against; what matters is
        // that the conversion exists here rather than in the binary,
        // where no test could reach it.
        let _ = super::exit_code(0);
        let _ = super::exit_code(FAILURE_EXIT);
    }
}
