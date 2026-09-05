//! The command line, and the one line a run answers with.

use renew_frame::FrameStats;
use renew_sample_glide_world::World;

use crate::SampleError;

/// The sample's name in its digest line.
const SAMPLE: &str = "glide";

/// A parsed command line.
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
    /// Open a window and play, instead of any headless mode.
    pub window: bool,
    /// The windowed run's tick bound: `None` plays until closed; the
    /// headless `frames` default must not leak into an interactive
    /// session, so this is derived from an EXPLICIT --frames only. The
    /// bound lands at the first frame boundary at or after N ticks —
    /// plans are never cut mid-frame, so the digest line's counts and
    /// the world always agree; a lagging frame may overshoot by at most
    /// the step budget minus one.
    pub window_ticks: Option<u64>,
    /// Print the run's report as one JSON object instead of the digest
    /// line. Same facts, machine-readable — what the cross-platform
    /// comparison lane collects, and what a human line should never be
    /// parsed for.
    pub json: bool,
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
    let mut json = false;
    let mut window = false;
    // Per-flag tracking, not one folded bool: refusing an explicit
    // trace flag beside --window while keeping an explicit seed needs
    // to know WHICH flags were given.
    let mut seen_seed = false;
    let mut seen_frames = false;
    let mut seen_input_trace = false;

    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let mut value_for = |flag: &str| {
            args.next()
                .ok_or_else(|| SampleError::Usage(format!("{flag} needs a value")))
        };
        match flag.as_str() {
            "--seed" => {
                seed = parse_number(&value_for("--seed")?, "--seed")?;
                seen_seed = true;
            }
            "--frames" => {
                frames = parse_number(&value_for("--frames")?, "--frames")?;
                seen_frames = true;
            }
            "--input-trace" => {
                input_trace = value_for("--input-trace")?;
                seen_input_trace = true;
            }
            "--window" => {
                refuse_repeat("--window", window)?;
                window = true;
            }
            "--json" => {
                refuse_repeat("--json", json)?;
                json = true;
            }
            "--record-trace" => {
                refuse_repeat("--record-trace", record_trace.is_some())?;
                record_trace = Some(value_for("--record-trace")?);
            }
            "--replay-trace" => {
                refuse_repeat("--replay-trace", replay_trace.is_some())?;
                replay_trace = Some(value_for("--replay-trace")?);
            }
            other => {
                return Err(SampleError::Usage(format!(
                    "unknown flag `{other}`; this sample takes --seed, --frames, \
                     --input-trace, --record-trace, --replay-trace, --window"
                )));
            }
        }
    }

    let seen_run_flags = seen_seed || seen_frames || seen_input_trace;
    if replay_trace.is_some() && (seen_run_flags || record_trace.is_some() || window) {
        return Err(SampleError::Usage(
            "--replay-trace owns the whole run; --seed, --frames, --input-trace, \
             --record-trace and --window contradict it"
                .to_string(),
        ));
    }
    if window && (seen_input_trace || record_trace.is_some()) {
        return Err(SampleError::Usage(
            "--window plays from the keyboard; --input-trace and --record-trace \
             contradict it"
                .to_string(),
        ));
    }
    // The windowed tick bound comes from an EXPLICIT --frames only: the
    // headless default of 2000 silently ending an interactive session
    // would be the exact bare-invocation surprise --window avoids. An
    // explicit zero is a contradiction, refused rather than reinterpreted.
    let window_ticks = if window && seen_frames {
        if frames == 0 {
            return Err(SampleError::Usage(
                "--window with --frames 0 is a zero-tick window; leave --frames off \
                 to play until closed"
                    .to_string(),
            ));
        }
        Some(frames)
    } else {
        None
    };

    Ok(Options {
        seed,
        frames,
        input_trace,
        record_trace,
        replay_trace,
        window,
        window_ticks,
        json,
    })
}

#[cfg(test)]
mod window_flag_tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, SampleError> {
        parse_args(args.iter().map(ToString::to_string))
    }

    #[test]
    fn the_json_flag_is_off_by_default_and_refuses_repetition() {
        // Off unless asked: the human line is what a person running the
        // sample expects to see, and a flag that flipped the default
        // would break every eye reading it.
        assert!(!parse(&[]).expect("a bare run parses").json);
        assert!(parse(&["--json"]).expect("--json parses").json);
        // Repetition is refused like every other flag here, so a caller
        // who typed it twice is told rather than quietly obliged.
        let error = parse(&["--json", "--json"]).expect_err("a repeat is refused");
        assert!(
            matches!(error, SampleError::Usage(ref m) if m.contains("--json")),
            "{error:?}"
        );
    }

    #[test]
    fn a_bare_window_run_is_unbounded() {
        // The centerpiece: the headless default of 2000 frames must not
        // leak into an interactive session and silently end it mid-play.
        let options = parse(&["--window"]).expect("bare --window parses");
        assert!(options.window);
        assert_eq!(options.window_ticks, None, "no bound unless asked for");
    }

    #[test]
    fn an_explicit_frames_bounds_the_window_in_ticks() {
        let options = parse(&["--window", "--frames", "30"]).expect("parses");
        assert_eq!(options.window_ticks, Some(30));
    }

    #[test]
    fn seed_stays_legal_beside_window() {
        // The positive half the per-flag seen-set exists for: refusing
        // trace flags must not take --seed down with them.
        let options = parse(&["--window", "--seed", "3"]).expect("parses");
        assert!(options.window);
        assert_eq!(options.seed, 3);
        assert_eq!(options.window_ticks, None, "seed alone adds no bound");
    }
}

/// A repeated flag silently last-winning is the same defect as a
/// contradicted one silently losing: the caller meant something and the
/// run does something else.
fn refuse_repeat(flag: &str, seen: bool) -> Result<(), SampleError> {
    if seen {
        return Err(SampleError::Usage(format!("{flag} was given twice")));
    }
    Ok(())
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
    /// The session digest: the world's own fold, then the menu's —
    /// the pause bit and every decision the tree made. A menu that
    /// can restart the run is gameplay, so the reported hash covers
    /// it; the world's own digest stays what it always was.
    ///
    /// **What the fold leaves out, named so the exclusion cannot go
    /// quietly vacuous:** the input map's latch state and the
    /// windowed driver's pending-flap counter. Both change future
    /// behaviour mid-run, and both are excluded soundly only because
    /// this fold is terminal — it happens once, when the run is over
    /// and no future remains. A mid-run session checkpoint would
    /// have to absorb them or inherit the gap; this sentence is where
    /// that obligation is written.
    pub session_hash: u64,
    /// The presentation effects as the run left them.
    ///
    /// **Carried on the report so an image oracle draws what the game
    /// draws.** The trail is a function of the whole flight, not of the
    /// world's final state, so a checkpoint that rebuilt the pool from
    /// the world it stopped at would show an empty trail and a committed
    /// picture would prove nothing about it. Reading it back off the
    /// same loop the runs use is the same argument that promoted
    /// `world_at` in the first place.
    ///
    /// Nothing here reaches [`Report::session_hash`]: the effects are
    /// presentation, and the digest is the world's and the menu's.
    pub effects: crate::effects::Effects,
}

impl Report {
    /// The same facts as [`Report::digest_line`], as one JSON object.
    ///
    /// The comparison lane reads this rather than the human line, for the
    /// reason every tool in this tree emits both: a line built for a
    /// person changes when a person's needs change, and a gate that
    /// parses one breaks silently when it does. `schema_version` is here
    /// from the first release because a consumer that cannot tell which
    /// shape it is holding has to guess.
    ///
    /// Hashes are hex strings, not numbers. A `u64` digest exceeds what
    /// JSON's number type is guaranteed to carry exactly, and a consumer
    /// that silently rounds one would compare two digests as equal that
    /// are not — which is the single failure this whole lane exists to
    /// prevent.
    #[must_use]
    pub fn json_line(&self) -> String {
        format!(
            "{{\"schema_version\":1,\"sample\":\"{SAMPLE}\",\"seed\":{},\
             \"source\":\"{}\",\"frames\":{},\"ticks\":{},\"dropped\":{},\
             \"score\":{},\"alive\":{},\"schedule_hash\":\"{:#018x}\",\
             \"state_hash\":\"{:#018x}\"}}",
            self.seed,
            self.source,
            self.stats.frames(),
            self.stats.ticks(),
            self.stats.steps_dropped(),
            self.world.score(),
            self.world.alive(),
            self.stats.schedule_hash(),
            self.session_hash,
        )
    }

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
            self.session_hash,
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
