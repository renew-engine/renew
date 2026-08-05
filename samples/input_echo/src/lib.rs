//! Window input feeding a fixed-timestep world — with no renderer
//! anywhere in its dependency graph.
//!
//! The complementary half of the frame-loop evidence: a *running* loop
//! sample in a workspace with the GPU crate removed. What it shows is
//! the property a fixed timestep exists for — a key held for three ticks
//! moves the same distance on every machine, whatever the frame rate was
//! while it was held — and the two ways to drive it, a window and a
//! scripted trace, are the same state machine with different event
//! sources.
//!
//! # Contract
//!
//! - **`--headless` implies a synthetic time source.** The scripted
//!   driver reads no clock at all, so its digest line is a pure function
//!   of `(trace, frames, seed)` and is identical across runs, processes
//!   and machines. It exists because no CI runner supplies keystrokes,
//!   and an unexecuted binary is an untested one.
//! - **Events change what the next step does, never the state.** Input
//!   arrives whenever the OS says so; the world moves only in fixed
//!   steps. Key repeats are counted and ignored for exactly that reason.
//! - **The simulation is integer-only.** Positions are whole units and
//!   pointer coordinates are truncated on arrival: a state hash that
//!   absorbed float arithmetic would quietly become a cross-platform
//!   promise the engine does not make.
//! - **No timing section in the report.** This sample presents nothing,
//!   and a drawn-versus-skipped split for a loop that never draws would
//!   be a measurement of nothing.
//! - **An environment that cannot host the run is a skip, not a
//!   failure.** No display server is an ordinary answer on ordinary
//!   runners; the binary says `SKIP:` and exits zero. Set
//!   `RENEW_FRAME_STRICT=1` where a skip would be a lie and it becomes a
//!   failure.

use std::process::ExitCode;

mod app;
mod cli;
/// The translation and the recorder moved to `renew-replay` — any game
/// shipping a replay needs them, and a correctness property maintained
/// in two copies is maintained in one and a half. Re-exported so every
/// existing path through this crate keeps meaning what it meant.
pub use renew_replay as convert;
mod error;
mod input;
pub use renew_replay as record;
mod scripted;
mod trace;
mod world;

pub use app::{EchoApp, describe};
pub use cli::{DEFAULT_FRAMES, DEFAULT_TRACE, Options, Report, SAMPLE, USAGE, parse_args};
pub use error::SampleError;
pub use input::{Direction, Input, Intent};
pub use scripted::replay;
pub use trace::{Trace, by_name, names};
pub use world::EchoWorld;

/// Setting this to `1` turns a skip into a failure: on a lane that
/// exists to run this sample, "no display here" is a broken lane, not a
/// tolerable environment.
pub const STRICT: &str = "RENEW_FRAME_STRICT";

/// Exit code for a command line this sample cannot honour.
const USAGE_EXIT: u8 = 2;
/// Exit code for a run that failed.
const FAILURE_EXIT: u8 = 1;

/// The whole binary, minus the process boundary: parse, run, report.
///
/// Returns the process exit code — `0` for a completed run or a skip,
/// `1` for a failure, `2` for a bad command line. The last line on
/// stdout is always the digest line the determinism gate compares.
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

/// The most a trace file may be before this sample refuses to read it.
///
/// Recordings here are tens of lines; the limit exists for the file that
/// is not a recording at all.
const TRACE_BYTE_LIMIT: usize = 1 << 20;

fn run<I: IntoIterator<Item = String>>(args: I) -> Result<Report, SampleError> {
    let options = parse_args(args)?;
    let report = if let Some(path) = &options.replay_trace {
        // Bounded, because a trace is untrusted input and the parser can
        // only judge bytes it already holds. A megabyte is far more than
        // any recording this sample produces and far less than a file
        // that could hurt.
        let text = renew_platform::fs::read_to_string_bounded(path, TRACE_BYTE_LIMIT)
            .map_err(|error| SampleError::failed("reading the trace file", &error))?;
        let recorded = renew_trace::parse(&text)
            .map_err(|error| SampleError::failed("reading the trace file", &error))?;
        scripted::replay_recorded(&recorded)?
    } else if options.headless {
        scripted::run(&options)?
    } else {
        app::run(&options)?
    };
    if let Some(path) = &options.dump_stats {
        renew_platform::fs::write(path, report.json().as_bytes()).map_err(dump_failed)?;
    }
    Ok(report)
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

#[cfg(test)]
mod tests {
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
    fn a_scripted_run_reaches_the_process_exit_code_with_its_digest_line() {
        let code = run_cli(
            ["--headless", "--input-trace", "idle", "--frames", "4"]
                .iter()
                .map(|argument| (*argument).to_string()),
        );
        assert_eq!(code, 0);
    }

    #[test]
    fn a_bad_command_line_reaches_the_process_exit_code() {
        assert_eq!(run_cli(["--replay".to_string()]), USAGE_EXIT);
    }

    #[test]
    fn an_unavailable_environment_is_a_skip_until_strictness_says_otherwise() {
        let missing = SampleError::Unavailable("no display".to_string());
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
            report_error(&SampleError::Failed("window loop: gone".to_string()), false),
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
