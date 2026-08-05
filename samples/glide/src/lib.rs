//! The glide game's driver: the half that touches the world's forbidden
//! things — files in, files out — and hands the simulation nothing but a
//! seed and one resolved boolean per tick.
//!
//! Headless-first, with a windowed mode behind the `window` feature.
//! Every headless mode is a pure function of its command line, which is
//! what lets the cross-process determinism gate compare runs by their
//! one output line; a windowed run rides the real clock and marks its
//! digest line `source=window` so nothing ever compares the two.
//!
//! # Indexing
//!
//! Traces index events by tick from zero, and this driver consumes them
//! exactly so — event for tick `k` is delivered before step `k`. There
//! is no frame-numbering shift anywhere in this sample; the loader's
//! contract is the driver's contract.

mod cli;
mod error;
pub mod scene;
// The sound derivation is pure and testable with no device, no
// window, and no audio crate in the graph, so it compiles always;
// only the wiring that plays its answers is gated.
#[cfg(feature = "audio")]
mod audio;
mod scripted;
pub mod sound;
mod trace;
#[cfg(feature = "window")]
mod windowed;

pub use cli::{Options, Report};
pub use error::SampleError;
pub use scene::{SceneSprite, Tile, scene};
pub use scripted::world_at;
pub use sound::{TickSounds, tick_sounds};
pub use trace::{by_name, names};

/// Exit code for a command line this sample cannot honour.
const USAGE_EXIT: u8 = 2;
/// Exit code for a run that failed.
const FAILURE_EXIT: u8 = 1;

/// The most a trace file may be before this sample refuses to read it.
/// Recordings here are tens of lines; the limit exists for the file
/// that is not a recording at all.
const TRACE_BYTE_LIMIT: usize = 1 << 20;

/// The whole binary, minus the process boundary: parse, run, report.
///
/// The last line on stdout is always the digest line the determinism
/// gate compares.
pub fn run_cli<I: IntoIterator<Item = String>>(args: I) -> u8 {
    // Whether the run was asked for JSON is a property of the arguments,
    // and `run` consumes them — so it is read here, from a second parse
    // that cannot disagree with the first because it is the same parser.
    // A parse failure is reported by `run` below, not twice.
    let args: Vec<String> = args.into_iter().collect();
    let json = crate::cli::parse_args(args.iter().cloned()).is_ok_and(|options| options.json);
    // Diagnostics, before anything can fail. `RENEW_LOG` names a file;
    // when it is set the engine's own error channel and any panic land
    // there, and the renderer is brought up with validation on. Reading
    // the environment is the binary's job: the crate that owns the file
    // sink deliberately reads no configuration of its own.
    renew_platform::diag::log_to_file(diagnostics_path());

    match run(args) {
        Ok(report) => {
            if json {
                println!("{}", report.json_line());
            } else {
                println!("{}", report.digest_line());
            }
            0
        }
        Err(SampleError::Usage(message)) => {
            eprintln!("usage: {message}");
            USAGE_EXIT
        }
        // Variant-agnostic on purpose: Failed and the feature-gated
        // Unavailable both mean exit 1, and a cfg'd arm here would be a
        // line no lane can execute — the xvfb lane HAS a display, and no
        // test may construct an event loop to manufacture the miss.
        Err(other) => {
            eprintln!("FAIL: {other}");
            FAILURE_EXIT
        }
    }
}

fn run<I: IntoIterator<Item = String>>(args: I) -> Result<Report, SampleError> {
    let options = cli::parse_args(args)?;

    if options.window {
        return windowed_run(&options);
    }

    if let Some(path) = &options.replay_trace {
        // Untrusted input: bounded read, then the codec judges the
        // bytes. The header owns a replayed run — its seed and length
        // are facts about the recording, not options.
        let text = renew_platform::fs::read_to_string_bounded(
            std::path::Path::new(path),
            TRACE_BYTE_LIMIT,
        )
        .map_err(|error| SampleError::failed("reading the trace file", &error))?;
        let recorded = renew_trace::parse(&text)
            .map_err(|error| SampleError::failed("parsing the trace file", &error))?;
        return scripted::replay_recorded(&recorded);
    }

    let trace = trace::by_name(&options.input_trace)?;
    // The recorder lives here, beside the path it exists for, so a
    // recording that was asked for and a recording that exists are the
    // same fact — no mismatch arm, because no mismatch can be built.
    let mut recorder = options
        .record_trace
        .as_ref()
        .map(|_| renew_replay::Recorder::default());
    let report = scripted::run(&options, &trace, recorder.as_mut());
    if let (Some(path), Some(recorder)) = (&options.record_trace, recorder) {
        // The recorder produced the bytes; the driver performs the
        // write. The plumbing crate does no I/O by contract, so the
        // side effect lives here, at the seam that owns files.
        let sealed = scripted::close_recording(&report, recorder)?;
        renew_platform::fs::write(
            std::path::Path::new(path),
            renew_trace::write(&sealed).as_bytes(),
        )
        .map_err(|error| SampleError::failed("writing the trace file", &error))?;
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
        "this build has no windowing support; rebuild with --features window".to_string(),
    ))
}

/// The file `RENEW_LOG` names, if it names one.
///
/// One variable rather than a flag, so it covers a panic that happens
/// before the command line is parsed — which is exactly the failure a
/// flag cannot describe. An empty value is treated as unset, because a
/// shell that exports an empty string meant to turn it off.
#[must_use]
pub fn diagnostics_path() -> Option<std::path::PathBuf> {
    let value = std::env::var_os("RENEW_LOG")?;
    if value.is_empty() {
        return None;
    }
    Some(std::path::PathBuf::from(value))
}

/// Whether this run is logging diagnostics, which is also what decides
/// whether the renderer asks for validation.
///
/// **`IfAvailable`, never `Required`.** A machine without the validation
/// layer installed must still be able to run the game while debugging
/// something else; requiring it would turn a missing optional component
/// into a failure to start.
#[must_use]
pub fn diagnostics_enabled() -> bool {
    diagnostics_path().is_some()
}

/// Which validation policy a run asks the renderer for.
///
/// Behind the windowing feature because the rendering crate is: a build
/// with no window has no renderer to ask.
///
/// **A named function rather than a conditional at the call site**, so
/// both answers can be asserted. Inline, the diagnostics arm executes
/// only in a run that has already set the variable and opened a window,
/// which no test does — the choice would be made in a place nothing
/// could observe. `IfAvailable` rather than `Required`: a machine
/// without the validation layer must still run while something else is
/// being debugged.
#[cfg(feature = "window")]
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
#[cfg(feature = "window")]
#[must_use]
pub fn validation_for(logging: bool) -> renew_rhi::Validation {
    if logging {
        renew_rhi::Validation::IfAvailable
    } else {
        renew_rhi::Validation::Off
    }
}
