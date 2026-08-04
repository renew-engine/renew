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
