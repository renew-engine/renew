//! The command line, and the two things a run reports.

use std::path::PathBuf;

use renew_frame::{FrameStats, FrameTiming};

use crate::error::SampleError;

/// The name this sample answers to — in its output, and in the command
/// that starts it.
pub const SAMPLE: &str = "hello_triangle";

/// What the sample accepts, in the words it accepts them.
pub const USAGE: &str =
    "usage: hello_triangle [--headless] [--frames N] [--seed N] [--dump-stats PATH]";

/// Ten seconds of simulation at 60 Hz: long enough for a frame-time
/// summary to mean something, short enough to sit inside a CI step.
pub const DEFAULT_FRAMES: u64 = 600;

/// One run's configuration, exactly as the command line describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Run with no window: an offscreen image and a synthetic clock.
    /// Everything gated in CI is measured in this mode, because it is
    /// the mode whose output is a pure function of its inputs.
    pub headless: bool,
    /// Frames to run before reporting. A windowed run stops after this
    /// many *presented* frames, so an unattended run always terminates.
    pub frames: u64,
    /// Selects the world's stride (see `World::new`); reported so a
    /// digest line names every input that produced it.
    pub seed: u64,
    /// Where to write the machine-readable report, if anywhere.
    pub dump_stats: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            headless: false,
            frames: DEFAULT_FRAMES,
            seed: 0,
            dump_stats: None,
        }
    }
}

/// Read the command line.
///
/// # Errors
///
/// [`SampleError::Usage`] for an unknown argument, a flag with no value,
/// or a count that is not a number — each naming the argument and the
/// accepted set, because a sample that answers "usage error" and stops
/// is a sample nobody runs twice.
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Options, SampleError> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--headless" => options.headless = true,
            "--frames" => options.frames = number(&mut args, "--frames")?,
            "--seed" => options.seed = number(&mut args, "--seed")?,
            "--dump-stats" => {
                options.dump_stats = Some(PathBuf::from(value(&mut args, "--dump-stats")?));
            }
            other => {
                return Err(SampleError::Usage(format!(
                    "unknown argument `{other}`; {USAGE}"
                )));
            }
        }
    }
    Ok(options)
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, SampleError> {
    args.next()
        .ok_or_else(|| SampleError::Usage(format!("{flag} needs a value; {USAGE}")))
}

fn number(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<u64, SampleError> {
    let text = value(args, flag)?;
    text.parse()
        .map_err(|_| SampleError::Usage(format!("{flag} takes a whole number, not `{text}`")))
}

/// What one finished run has to say for itself.
///
/// Split exactly where the determinism boundary is: `stats` and
/// `state_hash` are functions of the inputs and are what CI compares;
/// `timing` is measured and is only ever recorded.
#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub seed: u64,
    pub stats: FrameStats,
    pub timing: FrameTiming,
    pub state_hash: u64,
}

impl Report {
    /// The one line every run prints on stdout, and the exact string the
    /// cross-process determinism gate compares.
    ///
    /// A line rather than the JSON document: comparing it needs no
    /// parser in anybody's dependency list, and it stays readable in a
    /// CI log. Everything on it is deterministic — the measured timings
    /// are deliberately absent.
    #[must_use]
    pub fn digest_line(&self) -> String {
        format!(
            "renew-frame sample={SAMPLE} seed={} frames={} ticks={} dropped={} \
             schedule_hash={:#018x} state_hash={:#018x}",
            self.seed,
            self.stats.frames(),
            self.stats.ticks(),
            self.stats.steps_dropped(),
            self.stats.schedule_hash(),
            self.state_hash,
        )
    }

    /// The machine-readable report, one JSON object.
    ///
    /// The gated half (`frame`, `state_hash`) and the recorded half
    /// (`timing`) are separate members, so a consumer that diffs runs
    /// knows which half is allowed to move.
    #[must_use]
    pub fn json(&self) -> String {
        format!(
            "{{\"schema_version\":1,\"sample\":\"{SAMPLE}\",\"seed\":{},\
             \"frame\":{},\"state_hash\":\"{:#018x}\",\"timing\":{}}}",
            self.seed,
            self.stats.json(),
            self.state_hash,
            self.timing.json(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_FRAMES, Options, Report, parse_args};
    use crate::error::SampleError;
    use renew_frame::{FrameLoop, FrameStats, FrameTiming, Nanos, StepBudget, Timestamp, Timestep};
    use std::path::PathBuf;

    fn parse(args: &[&str]) -> Result<Options, SampleError> {
        parse_args(args.iter().map(|argument| (*argument).to_string()))
    }

    fn usage_message(args: &[&str]) -> String {
        let error = parse(args).expect_err("a usage error");
        assert!(matches!(error, SampleError::Usage(_)), "{error:?}");
        error.to_string()
    }

    #[test]
    fn an_empty_command_line_is_the_windowed_default_run() {
        assert_eq!(parse(&[]).expect("no arguments"), Options::default());
        assert!(!Options::default().headless);
        assert_eq!(Options::default().frames, DEFAULT_FRAMES);
    }

    #[test]
    fn every_flag_lands_where_it_says_it_does() {
        let options = parse(&[
            "--headless",
            "--frames",
            "8",
            "--seed",
            "17",
            "--dump-stats",
            "target/stats.json",
        ])
        .expect("a full command line");
        assert_eq!(
            options,
            Options {
                headless: true,
                frames: 8,
                seed: 17,
                dump_stats: Some(PathBuf::from("target/stats.json")),
            }
        );
    }

    #[test]
    fn an_unknown_argument_names_itself_and_the_accepted_set() {
        let message = usage_message(&["--turbo"]);
        assert!(message.contains("--turbo"), "{message}");
        assert!(message.contains("--headless"), "{message}");
    }

    #[test]
    fn a_flag_with_no_value_says_which_flag() {
        for flag in ["--frames", "--seed", "--dump-stats"] {
            let message = usage_message(&[flag]);
            assert!(message.contains(flag), "{message}");
            assert!(message.contains("needs a value"), "{message}");
        }
    }

    #[test]
    fn a_count_that_is_not_a_number_is_refused_with_the_text_that_was_given() {
        let message = usage_message(&["--frames", "soon"]);
        assert!(message.contains("soon"), "{message}");
        assert!(message.contains("whole number"), "{message}");
    }

    fn report() -> Report {
        let mut frame = FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(0),
        );
        let mut stats = FrameStats::new();
        let mut timing = FrameTiming::new();
        for k in 1..=4u64 {
            let plan = frame.begin_frame(Timestamp::from_nanos(k * 16_666_667));
            stats.absorb(&plan);
            timing.record(Nanos::from_nanos(1_000_000 * k), true);
        }
        Report {
            seed: 17,
            stats,
            timing,
            state_hash: 0x0123_4567_89ab_cdef,
        }
    }

    #[test]
    fn the_digest_line_carries_every_deterministic_number_and_no_measured_one() {
        let line = report().digest_line();
        assert_eq!(
            line,
            format!(
                "renew-frame sample=hello_triangle seed=17 frames=4 ticks=4 dropped=0 \
                 schedule_hash={:#018x} state_hash=0x0123456789abcdef",
                report().stats.schedule_hash()
            )
        );
        assert!(!line.contains("min_ns"), "measured timing is not gated");
    }

    #[test]
    fn the_json_report_nests_the_gated_half_apart_from_the_measured_half() {
        let json = report().json();
        assert!(
            json.starts_with("{\"schema_version\":1,\"sample\":\"hello_triangle\",\"seed\":17,")
        );
        assert!(json.contains("\"frame\":{\"frames\":4,\"ticks\":4,\"steps_dropped\":0,"));
        assert!(json.contains("\"state_hash\":\"0x0123456789abcdef\""));
        assert!(json.contains("\"timing\":{\"count\":4,\"min_ns\":1000000,"));
        assert!(json.ends_with("}}"), "{json}");
    }
}
