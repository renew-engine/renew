//! The command line, and the two things a run reports.

use std::path::PathBuf;

use renew_frame::FrameStats;

use crate::error::SampleError;
use crate::world::EchoWorld;

/// The name this sample answers to — in its output, and in the command
/// that starts it.
pub const SAMPLE: &str = "input_echo";

/// What the sample accepts, in the words it accepts them.
pub const USAGE: &str = "usage: input_echo [--headless [--input-trace NAME]] [--frames N] \
                         [--seed N] [--dump-stats PATH] [--record-trace PATH] \n                         [--replay-trace PATH]";

/// The trace a headless run replays unless told otherwise.
pub const DEFAULT_TRACE: &str = "walk";

/// Ten seconds at 60 Hz — the same run length the other sample uses, so
/// two digest lines from one CI step are comparable at a glance.
pub const DEFAULT_FRAMES: u64 = 600;

/// One run's configuration, exactly as the command line describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Run with no window: a scripted event trace and a synthetic clock.
    pub headless: bool,
    /// Frames to run before reporting. Headless, a frame is exactly one
    /// simulation step; windowed, the run ends once the simulation has
    /// advanced this many steps. A run ends earlier if the input asks it
    /// to — the close button, the escape key, the trace's own close
    /// request.
    pub frames: u64,
    /// Selects the movement speed (see `EchoWorld::new`); reported so a
    /// digest line names every input that produced it.
    pub seed: u64,
    /// Which scripted trace headless mode replays.
    pub trace: String,
    /// Where to write the machine-readable report, if anywhere.
    pub dump_stats: Option<PathBuf>,
    /// Where to write the run's input as a replayable trace, if anywhere.
    ///
    /// Recording is independent of how the run is driven: a scripted run
    /// records the script, a windowed run records the person. The file
    /// says nothing about which, because a trace is the input and not the
    /// thing that produced it.
    pub record_trace: Option<PathBuf>,
    /// A recorded trace to drive this run, instead of a named script.
    ///
    /// The file owns the run: its header carries the length, the
    /// timestep, the budget and the seed, so the flags that would set
    /// those are refused alongside it rather than silently losing.
    pub replay_trace: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            headless: false,
            frames: DEFAULT_FRAMES,
            seed: 0,
            trace: DEFAULT_TRACE.to_string(),
            dump_stats: None,
            record_trace: None,
            replay_trace: None,
        }
    }
}

/// Read the command line.
///
/// # Errors
///
/// [`SampleError::Usage`] for an unknown argument, a flag with no value,
/// a count that is not a number, or a trace asked for in a mode that
/// cannot replay one.
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Options, SampleError> {
    let mut options = Options::default();
    let mut scripted = false;
    let mut overridden: Vec<&str> = Vec::new();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--headless" => options.headless = true,
            "--frames" => {
                options.frames = number(&mut args, "--frames")?;
                overridden.push("--frames");
            }
            "--seed" => {
                options.seed = number(&mut args, "--seed")?;
                overridden.push("--seed");
            }
            "--input-trace" => {
                options.trace = value(&mut args, "--input-trace")?;
                scripted = true;
            }
            "--dump-stats" => {
                options.dump_stats = Some(PathBuf::from(value(&mut args, "--dump-stats")?));
            }
            "--record-trace" => {
                options.record_trace = Some(PathBuf::from(value(&mut args, "--record-trace")?));
            }
            "--replay-trace" => {
                options.replay_trace = Some(PathBuf::from(value(&mut args, "--replay-trace")?));
            }
            other => {
                return Err(SampleError::Usage(format!(
                    "unknown argument `{other}`; {USAGE}"
                )));
            }
        }
    }
    if let Some(path) = &options.replay_trace {
        // The header owns length, timestep, budget and seed. A flag that
        // set one of them would be silently ignored or would silently
        // win, and both make a digest line unexplainable.
        if scripted {
            overridden.push("--input-trace");
        }
        if !overridden.is_empty() {
            return Err(SampleError::Usage(format!(
                "--replay-trace owns the run, so {} cannot be given with it;                  the trace's own header carries them",
                overridden.join(" and ")
            )));
        }
        if !options.headless {
            return Err(SampleError::Usage(format!(
                "--replay-trace needs --headless: replaying {} against a live                  window would mix recorded input with real input",
                path.display()
            )));
        }
    }
    if scripted && !options.headless {
        // Silently ignoring the flag would be worse: a windowed run is
        // driven by the person at the keyboard, and a trace it quietly
        // dropped would make its digest line unexplainable.
        return Err(SampleError::Usage(
            "--input-trace only applies to --headless runs; a window is driven by real input"
                .to_string(),
        ));
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
/// Everything here is deterministic: the counters, the position and the
/// digests are functions of the input trace and the frame count. There
/// is no timing section, deliberately — this sample presents nothing, so
/// a drawn-versus-skipped split would be a measurement of nothing.
#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub seed: u64,
    /// Where the input came from: a trace name, or the window.
    pub source: &'static str,
    pub stats: FrameStats,
    pub world: EchoWorld,
}

impl Report {
    /// The one line every run prints on stdout, and the exact string the
    /// cross-process determinism gate compares.
    #[must_use]
    pub fn digest_line(&self) -> String {
        format!(
            "renew-frame sample={SAMPLE} seed={} source={} frames={} ticks={} dropped={} \
             schedule_hash={:#018x} state_hash={:#018x}",
            self.seed,
            self.source,
            self.stats.frames(),
            self.stats.ticks(),
            self.stats.steps_dropped(),
            self.stats.schedule_hash(),
            self.world.state_hash(),
        )
    }

    /// The machine-readable report, one JSON object: the frame
    /// schedule, the state digest, and what the input added up to.
    #[must_use]
    pub fn json(&self) -> String {
        let (pressed, released, repeats) = self.world.keys();
        let (pointer_x, pointer_y) = self.world.pointer();
        let (x, y) = self.world.position();
        let (width, height) = self.world.extent();
        format!(
            "{{\"schema_version\":1,\"sample\":\"{SAMPLE}\",\"seed\":{},\"source\":\"{}\",\
             \"frame\":{},\"state_hash\":\"{:#018x}\",\
             \"input\":{{\"events\":{},\"keys_pressed\":{pressed},\"keys_released\":{released},\
             \"key_repeats\":{repeats},\"pointer_moves\":{},\"pointer\":[{pointer_x},{pointer_y}],\
             \"buttons\":{},\"wheel\":{},\"position\":[{x},{y}],\"extent\":[{width},{height}],\
             \"focused\":{}}}}}",
            self.seed,
            self.source,
            self.stats.json(),
            self.world.state_hash(),
            self.world.events(),
            self.world.pointer_moves(),
            self.world.buttons(),
            self.world.wheel(),
            self.world.focused(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_FRAMES, DEFAULT_TRACE, Options, Report, parse_args};
    use crate::error::SampleError;
    use crate::world::EchoWorld;
    use renew_frame::{FrameLoop, FrameStats, StepBudget, Timestamp, Timestep};
    use renew_platform::window::{KeyCode, WindowEvent};
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
        assert_eq!(Options::default().trace, DEFAULT_TRACE);
    }

    #[test]
    fn every_flag_lands_where_it_says_it_does() {
        let options = parse(&[
            "--headless",
            "--input-trace",
            "idle",
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
                trace: "idle".to_string(),
                dump_stats: Some(PathBuf::from("target/stats.json")),
                replay_trace: None,
                record_trace: None,
            }
        );
    }

    #[test]
    fn a_trace_without_headless_is_refused_rather_than_ignored() {
        let message = usage_message(&["--input-trace", "walk"]);
        assert!(message.contains("--headless"), "{message}");
    }

    #[test]
    fn an_unknown_argument_names_itself_and_the_accepted_set() {
        let message = usage_message(&["--replay"]);
        assert!(message.contains("--replay"), "{message}");
        assert!(message.contains("--input-trace"), "{message}");
    }

    #[test]
    fn a_flag_with_no_value_says_which_flag() {
        for flag in ["--frames", "--seed", "--input-trace", "--dump-stats"] {
            let message = usage_message(&[flag]);
            assert!(message.contains(flag), "{message}");
            assert!(message.contains("needs a value"), "{message}");
        }
    }

    #[test]
    fn a_count_that_is_not_a_number_is_refused_with_the_text_that_was_given() {
        let message = usage_message(&["--frames", "later"]);
        assert!(message.contains("later"), "{message}");
        assert!(message.contains("whole number"), "{message}");
    }

    fn report() -> Report {
        let mut frame = FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(0),
        );
        let mut stats = FrameStats::new();
        let mut world = EchoWorld::new(0);
        world.event(WindowEvent::Key {
            code: KeyCode::ArrowRight,
            pressed: true,
            repeat: false,
        });
        for k in 1..=4u64 {
            let plan = frame.begin_frame(Timestamp::from_nanos(k * 16_666_667));
            for step in plan.steps() {
                world.step(step);
            }
            stats.absorb(&plan);
        }
        Report {
            seed: 0,
            source: "walk",
            stats,
            world,
        }
    }

    #[test]
    fn the_digest_line_names_every_input_that_produced_it() {
        let report = report();
        assert_eq!(
            report.digest_line(),
            format!(
                "renew-frame sample=input_echo seed=0 source=walk frames=4 ticks=4 dropped=0 \
                 schedule_hash={:#018x} state_hash={:#018x}",
                report.stats.schedule_hash(),
                report.world.state_hash()
            )
        );
    }

    #[test]
    fn the_json_report_carries_the_schedule_the_digest_and_the_input_tally() {
        let json = report().json();
        assert!(json.starts_with("{\"schema_version\":1,\"sample\":\"input_echo\",\"seed\":0,"));
        assert!(json.contains("\"source\":\"walk\""));
        assert!(json.contains("\"frame\":{\"frames\":4,\"ticks\":4,"));
        assert!(json.contains("\"input\":{\"events\":1,\"keys_pressed\":1"));
        assert!(json.contains("\"position\":[4,0]"), "{json}");
        assert!(json.contains("\"focused\":false"), "{json}");
        // No timing section: this sample presents nothing.
        assert!(!json.contains("timing"), "{json}");
        assert!(json.ends_with("}}"), "{json}");
    }

    /// The two trace flags are mutually exclusive, and the message names
    /// the one the header already owns.
    #[test]
    fn a_replay_cannot_also_name_a_built_in_trace() {
        let refused = parse_args(
            [
                "--headless",
                "--replay-trace",
                "run.trace",
                "--input-trace",
                "walk",
            ]
            .into_iter()
            .map(str::to_string),
        );
        // One assertion rather than a let-else: the `else` arm is a line
        // no passing run can reach, and an uncovered line in a test is
        // still an uncovered line.
        assert!(
            matches!(&refused, Err(SampleError::Usage(message)) if message.contains("--input-trace")),
            "{refused:?}"
        );
    }
}
