//! The command line, and the one line a run answers with.

use renew_frame::FrameStats;
use renew_sample_glide_world::World;

use crate::SampleError;

/// The sample's name in its digest line.
const SAMPLE: &str = "glide";

/// A parsed command line. Headless in every mode today.
#[derive(Debug)]
pub struct Options {
    pub seed: u64,
    pub frames: u64,
    /// The built-in trace driving input.
    pub input_trace: String,
    /// Record the run's input here.
    pub record_trace: Option<String>,
    /// Replay a recorded file instead; owns the whole run.
    pub replay_trace: Option<String>,
}

/// Parse, refusing combinations that would silently ignore a flag.
///
/// `--replay-trace` owns the run: the header carries the seed and the
/// length, so `--seed`, `--frames`, `--input-trace` and
/// `--record-trace` beside it are contradictions, refused by name —
/// re-recording a replay would only prove the recorder is the identity.
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Options, SampleError> {
    let mut seed = 7u64;
    let mut frames = 2_000u64;
    let mut input_trace = String::from("soar");
    let mut record_trace = None;
    let mut replay_trace = None;
    let mut seen_run_flags = false;

    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let mut value_for = |flag: &str| {
            args.next()
                .ok_or_else(|| SampleError::Usage(format!("{flag} needs a value")))
        };
        match flag.as_str() {
            "--seed" => {
                seed = parse_number(&value_for("--seed")?, "--seed")?;
                seen_run_flags = true;
            }
            "--frames" => {
                frames = parse_number(&value_for("--frames")?, "--frames")?;
                seen_run_flags = true;
            }
            "--input-trace" => {
                input_trace = value_for("--input-trace")?;
                seen_run_flags = true;
            }
            "--record-trace" => record_trace = Some(value_for("--record-trace")?),
            "--replay-trace" => replay_trace = Some(value_for("--replay-trace")?),
            other => {
                return Err(SampleError::Usage(format!(
                    "unknown flag `{other}`; this sample takes --seed, --frames, \
                     --input-trace, --record-trace, --replay-trace"
                )));
            }
        }
    }

    if replay_trace.is_some() && (seen_run_flags || record_trace.is_some()) {
        return Err(SampleError::Usage(
            "--replay-trace owns the whole run; --seed, --frames, --input-trace and \
             --record-trace contradict it"
                .to_string(),
        ));
    }

    Ok(Options {
        seed,
        frames,
        input_trace,
        record_trace,
        replay_trace,
    })
}

fn parse_number(text: &str, flag: &str) -> Result<u64, SampleError> {
    text.parse::<u64>().map_err(|_| {
        SampleError::Usage(format!("{flag} takes a non-negative integer, got `{text}`"))
    })
}

/// Everything a run answers with. Deterministic throughout: every field
/// is a function of the seed, the trace and the frame count.
pub struct Report {
    pub seed: u64,
    /// Where the input came from: a trace name, or a replayed file.
    pub source: String,
    pub stats: FrameStats,
    pub world: World,
}

impl Report {
    /// The one line every run prints on stdout, and the exact string
    /// the cross-process determinism gate compares.
    #[must_use]
    pub fn digest_line(&self) -> String {
        format!(
            "renew-frame sample={SAMPLE} seed={} source={} frames={} ticks={} dropped={} \
             score={} alive={} schedule_hash={:#018x} state_hash={:#018x}",
            self.seed,
            self.source,
            self.stats.frames(),
            self.stats.ticks(),
            self.stats.steps_dropped(),
            self.world.score(),
            u8::from(self.world.alive()),
            self.stats.schedule_hash(),
            self.world.digest().finish(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, SampleError> {
        parse_args(args.iter().map(ToString::to_string))
    }

    #[test]
    fn defaults_are_a_runnable_game() {
        let options = parse(&[]).expect("no flags is a valid run");
        assert_eq!(options.seed, 7);
        assert_eq!(options.input_trace, "soar");
    }

    #[test]
    fn replay_refuses_every_flag_it_would_silently_ignore() {
        for conflicting in [
            &["--replay-trace", "t.trace", "--seed", "3"][..],
            &["--replay-trace", "t.trace", "--frames", "9"][..],
            &["--replay-trace", "t.trace", "--input-trace", "soar"][..],
            &["--replay-trace", "t.trace", "--record-trace", "out"][..],
        ] {
            let refused = parse(conflicting);
            assert!(
                matches!(refused, Err(SampleError::Usage(_))),
                "{conflicting:?} must refuse, not ignore"
            );
        }
    }

    #[test]
    fn unknown_flags_name_themselves() {
        let refused = parse(&["--fly"]);
        assert!(
            matches!(&refused, Err(SampleError::Usage(message)) if message.contains("--fly")),
            "unknown flag must be a usage error naming itself: {refused:?}"
        );
    }

    #[test]
    fn a_number_that_is_not_one_is_refused_by_flag_name() {
        let refused = parse(&["--seed", "seven"]);
        assert!(
            matches!(&refused, Err(SampleError::Usage(message)) if message.contains("--seed")),
            "{refused:?}"
        );
    }
}
