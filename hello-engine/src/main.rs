//! Prints the build identity, then drives the fixed-timestep frame loop
//! through a fixed number of simulated frames with deterministic frame
//! times. No clocks are read; every run produces identical output.
//!
//! The schedule itself lives in `renew-frame`: this is its smallest
//! client, and the numbers below are the loop's own — the tally comes
//! from the plans it returns, never from a second count kept here.

use std::fmt::Write as _;
use std::process::ExitCode;

use renew_frame::{FrameLoop, FrameStats, StepBudget, Timestamp, Timestep};

/// Number of frames to simulate.
const FRAMES: usize = 60;

/// Deterministic per-frame durations, cycled for the whole run: a fast frame,
/// an exact frame, a slow frame, and a two-tick spike.
const FRAME_PATTERN_NS: [u64; 4] = [15_000_000, 16_666_667, 18_000_000, 33_333_334];

/// Exit code for a command line this binary cannot honour, matching the
/// `renew` CLI's contract: `0` success, `2` usage error.
const USAGE_EXIT: u8 = 2;

/// What this program accepts, which is nothing.
///
/// One line per output line and no continuations: a `\` at the end of a
/// Rust string literal is correct here, and it is also the character an
/// editing pipeline is most likely to eat. Written to be un-eatable.
const USAGE: &str = concat!(
    "usage: hello-engine\n",
    "\n",
    "It drives a fixed number of frames of the fixed-timestep loop with a\n",
    "built-in frame pattern and prints the tally. For a sample that takes\n",
    "flags, run one of the samples instead."
);

/// Everything the program does, as a value.
///
/// **`main` must contain no branch, and that is a coverage constraint
/// rather than a preference.** The coverage lane runs this binary once,
/// with no arguments, so any arm `main` owns that a no-argument run does
/// not take is a line no test can reach — the first attempt at this fix
/// put the refusal in `main` and reddened the gate on exactly two lines.
/// Returning the streams instead puts every branch somewhere a unit test
/// can call, and has the side benefit that the tally is now assertable
/// at all, which it never was.
struct Outcome {
    out: String,
    err: String,
    code: u8,
}

fn run<I: Iterator<Item = String>>(mut arguments: I) -> Outcome {
    if let Some(first) = arguments.next() {
        return Outcome {
            out: String::new(),
            err: format!("error: {first}: this program takes no arguments\n\n{USAGE}"),
            code: USAGE_EXIT,
        };
    }

    let timestep = Timestep::HZ_60;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(out, "fixed timestep: {} ns", timestep.nanos());

    // Absolute timestamps, so the elapsed total is the schedule's own
    // input rather than a number accumulated beside it.
    let mut elapsed_ns: u64 = 0;
    let mut frame = FrameLoop::new(timestep, StepBudget::DEFAULT, Timestamp::from_nanos(0));
    let mut stats = FrameStats::new();

    for frame_time_ns in FRAME_PATTERN_NS.iter().copied().cycle().take(FRAMES) {
        elapsed_ns += frame_time_ns;
        let plan = frame.begin_frame(Timestamp::from_nanos(elapsed_ns));
        // A real client steps its world here, once per planned step. This
        // one has no world, so the plan goes straight to the tally.
        stats.absorb(&plan);
    }

    let _ = writeln!(out, "frames simulated: {}", stats.frames());
    let _ = writeln!(out, "time submitted: {elapsed_ns} ns");
    let _ = writeln!(out, "ticks executed: {}", stats.ticks());
    let _ = writeln!(out, "time pending: {} ns", frame.remainder().get());
    Outcome {
        out,
        err: String::new(),
        code: 0,
    }
}

fn main() -> ExitCode {
    let outcome = run(std::env::args().skip(1));
    print!("{}", outcome.out);
    eprint!("{}", outcome.err);
    ExitCode::from(outcome.code)
}

#[cfg(test)]
mod tests {
    use super::{FRAMES, run};

    fn no_args() -> super::Outcome {
        run(std::iter::empty())
    }

    #[test]
    fn a_run_with_no_arguments_reports_the_tally_and_succeeds() {
        let outcome = no_args();
        assert_eq!(outcome.code, 0);
        assert!(
            outcome.err.is_empty(),
            "nothing belongs on stderr: {}",
            outcome.err
        );
        assert!(outcome.out.contains(&format!("frames simulated: {FRAMES}")));
        for expected in [
            "fixed timestep:",
            "time submitted:",
            "ticks executed:",
            "time pending:",
        ] {
            assert!(
                outcome.out.contains(expected),
                "missing {expected} in:
{}",
                outcome.out
            );
        }
    }

    /// The loop is fed fixed frame times and reads no clock, so the tally
    /// is the same on every machine and every run. Nothing asserted this
    /// before — the binary printed numbers nobody checked.
    #[test]
    fn the_tally_is_identical_across_runs() {
        assert_eq!(no_args().out, no_args().out);
    }

    #[test]
    fn any_argument_is_refused_on_stderr_with_the_usage_code() {
        for argument in ["--json", "--frames", "--help", "nonsense"] {
            let outcome = run(std::iter::once(argument.to_string()));
            assert_eq!(outcome.code, super::USAGE_EXIT, "{argument}");
            assert!(outcome.out.is_empty(), "a refusal prints no tally");
            assert!(
                outcome.err.contains(argument),
                "the refusal must quote what it refused: {}",
                outcome.err
            );
            assert!(
                outcome.err.contains("takes no arguments"),
                "{}",
                outcome.err
            );
            assert!(
                outcome.err.contains("usage: hello-engine"),
                "{}",
                outcome.err
            );
        }
    }

    /// Only the first argument is quoted, and one is enough: the point is
    /// to say the program takes none, not to enumerate what was given.
    #[test]
    fn several_arguments_are_still_one_refusal() {
        let outcome = run(["--a".to_string(), "--b".to_string()].into_iter());
        assert!(outcome.err.contains("--a"));
        assert!(!outcome.err.contains("--b"));
    }
}
