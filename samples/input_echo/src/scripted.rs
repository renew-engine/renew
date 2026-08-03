//! The headless driver: a scripted trace, a synthetic clock, no window.
//!
//! This mode reads no clock at all, so its digest line is a pure
//! function of `(trace, frames, seed)` and is identical across runs,
//! processes and machines. It is what makes a windowing sample provable
//! in CI.

use core::num::{NonZeroU32, NonZeroU64};

use renew_frame::{FrameLoop, FrameStats, StepBudget, Timestamp, Timestep};
use renew_trace::TraceHeader;

use crate::cli::{Options, Report};
use crate::error::SampleError;
use crate::input::Input;
use crate::trace::{self, Trace};
use crate::world::EchoWorld;
use renew_replay as convert;
use renew_replay::Recorder;

/// The synthetic frame interval: exactly one timestep, so a run of N
/// frames executes exactly N steps, banks nothing and drops nothing. The
/// expected numbers are readable without running anything, which is what
/// makes a wrong one obvious.
const FRAME_INTERVAL_NS: u64 = Timestep::HZ_60.nanos().get();

/// Replay the trace the command line named.
///
/// # Errors
///
/// [`SampleError::Usage`] when no trace goes by that name.
pub fn run(options: &Options) -> Result<Report, SampleError> {
    let trace = trace::by_name(&options.trace)?;
    let Some(path) = &options.record_trace else {
        return Ok(replay(&trace, options.seed, options.frames));
    };

    let mut recorder = Recorder::default();
    let report = replay_recording(&trace, options.seed, options.frames, Some(&mut recorder));
    // The header is written after the run, not before it, because two of
    // its fields are facts about what happened: how many ticks the run
    // actually reached, which the trace's own close request decides.
    let header = TraceHeader::new(
        SAMPLE_NAME,
        report.world.ticks(),
        FRAME_INTERVAL_NS,
        StepBudget::DEFAULT.get().get(),
    )
    .and_then(|header| header.with_key("seed", &options.seed.to_string()))
    .map_err(|error| SampleError::failed("describing the recording", &error))?;
    let written = recorder
        .finish(header)
        .map_err(|error| SampleError::failed("closing the recording", &error))?;
    renew_platform::fs::write(path, renew_trace::write(&written).as_bytes())
        .map_err(|error| SampleError::failed("writing the trace file", &error))?;
    Ok(report)
}

/// The name a recording carries so a replay can refuse the wrong sample.
const SAMPLE_NAME: &str = "input_echo";

/// Replay one trace for at most `frames` frames.
///
/// Events scheduled for a frame are delivered before that frame is
/// planned — the same order the window seam uses, where the event phase
/// runs before the update phase. The run stops early if the input asks
/// it to; a close request is a close request whether it came from a
/// script or a person.
#[must_use]
pub fn replay(trace: &Trace, seed: u64, frames: u64) -> Report {
    replay_recording(trace, seed, frames, None)
}

/// Replay a trace, optionally recording the events as they are delivered.
///
/// The recorder sees exactly what the world sees, at the tick the world
/// sees it. That is why recording lives here rather than beside the trace
/// table: a recording taken from the source data would be a copy of the
/// input, not a record of what this run did with it, and the two stop
/// agreeing the moment a driver changes.
#[must_use]
pub fn replay_recording(
    trace: &Trace,
    seed: u64,
    frames: u64,
    recorder: Option<&mut Recorder>,
) -> Report {
    replay_at_interval(trace, seed, frames, FRAME_INTERVAL_NS, recorder)
}

/// The same driver, with the frame interval left to the caller.
///
/// Private, and it exists for one test. Every shipping path uses
/// [`FRAME_INTERVAL_NS`], where a frame is always exactly one step — and
/// that is precisely why the shipping path cannot show that a recorded
/// tick is the world's step count rather than the frame number. The two
/// are equal there. An interval that is not a whole number of timesteps
/// makes frames carry one step and two by turns, which is the only shape
/// that tells those numbers apart.
#[must_use]
fn replay_at_interval(
    trace: &Trace,
    seed: u64,
    frames: u64,
    interval_ns: u64,
    mut recorder: Option<&mut Recorder>,
) -> Report {
    let mut world = EchoWorld::new(seed);
    let mut input = Input::new();
    let mut frame = FrameLoop::new(
        Timestep::HZ_60,
        StepBudget::DEFAULT,
        Timestamp::from_nanos(0),
    );
    let mut stats = FrameStats::new();
    for index in 1..=frames {
        for (at, event) in &trace.events {
            if *at == index {
                // The tick the next step will carry is however many steps
                // the world has already run, which is what the format
                // means by an event's tick: delivered before that step.
                //
                // Deliberately NOT `index - 1`. That is the same number
                // only while every frame carries exactly one step — true
                // of this driver by construction, and not a property of
                // the format or of the loop. Asking the world removes the
                // assumption rather than documenting it: a frame that
                // banks no step, or runs two, still records the tick its
                // event actually precedes.
                if let Some(recorder) = recorder.as_deref_mut() {
                    recorder.event(world.ticks(), *event);
                }
                world.event(*event);
                input.handle(*event);
            }
        }
        let now = Timestamp::from_nanos(interval_ns.saturating_mul(index));
        let plan = frame.begin_frame(now);
        let intent = input.intent();
        for step in plan.steps() {
            world.step(step, intent);
        }
        input.advance();
        stats.absorb(&plan);
        if world.close_requested() {
            break;
        }
    }
    Report {
        seed,
        source: trace.name,
        stats,
        world,
    }
}

/// Drive a run from a recorded trace.
///
/// The header owns the run: its tick count is the length, its timestep
/// and budget configure the loop, and its seed picks the world. Nothing
/// on the command line may override them, because a replay that took its
/// length from one place and its input from another would not be a replay
/// of anything.
///
/// A close request inside the file does **not** end the run early. The
/// recorded run's own length is already in the header, so honouring the
/// event as well would shorten a replay that the recording says ran
/// longer.
///
/// # Errors
///
/// [`SampleError::Usage`] when the file describes a different sample, or
/// when a header field the frame loop cannot accept — a zero timestep or
/// a zero budget — reaches it. The codec stores those numbers without
/// interpreting them, so refusing them is this driver's job.
pub fn replay_recorded(recorded: &renew_trace::Trace) -> Result<Report, SampleError> {
    let header = recorded.header();
    if header.sample() != SAMPLE_NAME {
        return Err(SampleError::Usage(format!(
            "this trace was recorded by `{}`, not `{SAMPLE_NAME}`",
            header.sample()
        )));
    }
    let timestep = NonZeroU64::new(header.timestep_ns())
        .map(Timestep::from_nanos)
        .ok_or_else(|| {
            SampleError::Usage("a trace with a zero timestep cannot be replayed".into())
        })?;
    let budget = NonZeroU32::new(header.budget())
        .map(StepBudget::new)
        .ok_or_else(|| {
            SampleError::Usage("a trace with a zero step budget cannot be replayed".into())
        })?;
    let seed = header
        .value("seed")
        .map_or(Ok(0), str::parse)
        .map_err(|_| SampleError::Usage("the trace's seed is not a number".to_string()))?;

    let interval = timestep.nanos().get();
    let mut world = EchoWorld::new(seed);
    let mut input = Input::new();
    let mut frame = FrameLoop::new(timestep, budget, Timestamp::from_nanos(0));
    let mut stats = FrameStats::new();
    let deliver = |world: &mut EchoWorld, input: &mut Input, tick: u64| {
        for (at, event) in recorded.events() {
            if *at == tick {
                let event = convert::from_trace(*event);
                world.event(event);
                input.handle(event);
            }
        }
    };
    for tick in 0..header.ticks() {
        deliver(&mut world, &mut input, tick);
        let plan = frame.begin_frame(Timestamp::from_nanos(interval.saturating_mul(tick + 1)));
        let intent = input.intent();
        for step in plan.steps() {
            world.step(step, intent);
        }
        input.advance();
        stats.absorb(&plan);
    }
    // The trailing bucket: events recorded at the run's own tick count
    // arrived after the final step, which is where a close request almost
    // always lands.
    deliver(&mut world, &mut input, header.ticks());

    Ok(Report {
        seed,
        source: "replay",
        stats,
        world,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        FRAME_INTERVAL_NS, SAMPLE_NAME, StepBudget, Timestep, TraceHeader, replay,
        replay_at_interval, replay_recorded, run,
    };
    use crate::cli::Options;
    use crate::error::SampleError;
    use crate::trace;
    use renew_replay::Recorder;

    /// A recorded tick is the number of steps that had already run, which
    /// is what the format means and what `record.rs` promises — **not**
    /// the frame the event arrived in.
    ///
    /// Under the shipping driver those are the same number, so no amount
    /// of recording through it can tell them apart: the committed traces
    /// are byte-identical either way, which is exactly how the frame
    /// number survived here unnoticed. Driving at one and a half
    /// timesteps makes frames carry one step and two by turns and the
    /// two numbers come apart — by five ticks over twelve frames.
    ///
    /// The `assert_ne!` is the load-bearing line. Without it this test
    /// would keep passing if the interval were ever "simplified" back to
    /// a whole timestep, and would then be asserting that a number equals
    /// itself. A test whose premise can quietly evaporate is worse than
    /// no test, because it still reports success.
    #[test]
    fn a_recorded_tick_counts_steps_that_ran_not_frames_that_passed() {
        let source = trace::by_name("walk").expect("the walk trace");
        let timestep = Timestep::HZ_60.nanos().get();
        let interval = timestep * 3 / 2;
        let frames = 12;

        let mut recorder = Recorder::default();
        let report = replay_at_interval(&source, 0, frames, interval, Some(&mut recorder));
        let header = TraceHeader::new(
            SAMPLE_NAME,
            report.world.ticks(),
            timestep,
            StepBudget::DEFAULT.get().get(),
        )
        .expect("the header describes a legal run");
        let written = recorder
            .finish(header)
            .expect("the recording is well formed");

        let delivered: Vec<u64> = source
            .events
            .iter()
            .filter(|(at, _)| *at <= frames)
            .map(|(at, _)| *at)
            .collect();
        // Message deliberately literal: a formatted one would put the
        // only code on this assertion's failure path, which never runs,
        // and the coverage gate would have to exempt a line whose whole
        // purpose is to never execute.
        assert!(
            delivered.len() > 3,
            "too few of the walk trace's events land in the first frames to prove anything"
        );

        // Steps completed before frame N is floor((N-1) * interval / timestep).
        let by_steps: Vec<u64> = delivered
            .iter()
            .map(|at| (at - 1) * interval / timestep)
            .collect();
        let by_frames: Vec<u64> = delivered.iter().map(|at| at - 1).collect();
        assert_ne!(
            by_steps, by_frames,
            "this interval makes steps and frames agree, so the test proves nothing"
        );

        let actual: Vec<u64> = written.events().iter().map(|(at, _)| *at).collect();
        assert_eq!(
            actual, by_steps,
            "recorded ticks must follow the steps that ran, not the frames that passed"
        );
    }

    /// The shipping interval really is one step per frame — the premise
    /// every other expectation in this file rests on, including the one
    /// above that deliberately breaks it.
    #[test]
    fn the_shipping_interval_is_exactly_one_timestep() {
        assert_eq!(FRAME_INTERVAL_NS, Timestep::HZ_60.nanos().get());
    }

    fn walk(frames: u64) -> crate::cli::Report {
        replay(&trace::by_name("walk").expect("the walk trace"), 0, frames)
    }

    /// The whole sample in one assertion: the key held from frame two to
    /// frame fourteen moved twelve ticks' worth, and the one held from
    /// four to eight moved four — distances the tick count decides, not
    /// the number of events.
    #[test]
    fn the_walk_trace_moves_exactly_as_far_as_the_ticks_it_was_held_for() {
        let report = walk(600);
        assert_eq!(report.world.position(), (12, 4));
        assert_eq!(report.world.ticks(), 20);
        assert_eq!(report.stats.ticks(), 20);
        assert_eq!(report.stats.steps_dropped(), 0);
        assert_eq!(report.world.keys(), (2, 2, 1), "presses, releases, repeats");
        assert_eq!(report.world.pointer(), (12, 34));
        assert_eq!(report.world.buttons(), 1);
        assert_eq!(report.world.wheel(), 16);
        assert_eq!(report.world.extent(), (640, 360));
        assert!(report.world.focused());
    }

    /// The close request lands at frame twenty, and the run stops there
    /// rather than at the six hundred it was offered.
    #[test]
    fn the_traces_close_request_ends_the_run_before_the_frame_count_does() {
        assert_eq!(walk(600).stats.frames(), 20);
        assert!(walk(600).world.close_requested());
        // Asked for fewer frames than the trace needs, the run ends
        // where it was told to and never sees the close.
        let short = walk(10);
        assert_eq!(short.stats.frames(), 10);
        assert!(!short.world.close_requested());
        assert_eq!(short.world.position(), (9, 4), "nine ticks held, four down");
    }

    #[test]
    fn an_empty_trace_runs_the_loop_and_nothing_else() {
        let idle = replay(&trace::by_name("idle").expect("the idle trace"), 0, 30);
        assert_eq!(idle.stats.frames(), 30);
        assert_eq!(idle.stats.ticks(), 30);
        assert_eq!(idle.world.events(), 0);
        assert_eq!(idle.world.position(), (0, 0));
        // A frameless run is legal and reports an empty schedule.
        let nothing = replay(&trace::by_name("idle").expect("the idle trace"), 0, 0);
        assert_eq!(nothing.stats.frames(), 0);
    }

    #[test]
    fn two_replays_of_one_trace_agree_to_the_last_bit() {
        assert_eq!(walk(600).digest_line(), walk(600).digest_line());
        // The seed is an input like any other, and it shows.
        let seeded = replay(&trace::by_name("walk").expect("the walk trace"), 3, 600);
        assert_ne!(seeded.digest_line(), walk(600).digest_line());
        assert_eq!(seeded.world.position(), (48, 16), "speed four");
    }

    #[test]
    fn the_driver_takes_its_trace_from_the_command_line() {
        let options = Options {
            headless: true,
            frames: 30,
            seed: 0,
            trace: "idle".to_string(),
            ..Options::default()
        };
        let report = run(&options).expect("a listed trace");
        assert_eq!(report.source, "idle");
        assert_eq!(report.stats.frames(), 30);

        let unknown = Options {
            trace: "moonwalk".to_string(),
            ..options
        };
        assert!(run(&unknown).is_err());
    }

    /// The driver refuses what the codec is not asked to judge.
    ///
    /// The codec stores the timestep and the budget without interpreting
    /// them, so a zero reaches this driver intact — and the frame loop's
    /// types cannot hold one. Refusing here is not defensive duplication;
    /// it is the only place the check can live.
    #[test]
    fn a_header_the_frame_loop_cannot_accept_is_refused() {
        let header = |sample: &str, timestep: u64, budget: u32| {
            renew_trace::TraceHeader::new(sample, 1, timestep, budget)
                .expect("a well-formed header")
        };
        let trace = |header| renew_trace::Trace::new(header, Vec::new()).expect("no events");

        let wrong = replay_recorded(&trace(header("hello_triangle", 1, 1)));
        assert!(
            matches!(&wrong, Err(SampleError::Usage(message)) if message.contains("hello_triangle")),
            "{wrong:?}"
        );

        let no_timestep = replay_recorded(&trace(header(SAMPLE_NAME, 0, 1)));
        assert!(
            matches!(&no_timestep, Err(SampleError::Usage(message)) if message.contains("timestep")),
            "{no_timestep:?}"
        );

        let no_budget = replay_recorded(&trace(header(SAMPLE_NAME, 1, 0)));
        assert!(
            matches!(&no_budget, Err(SampleError::Usage(message)) if message.contains("budget")),
            "{no_budget:?}"
        );
    }

    /// A seed that is not a number is refused rather than silently zero.
    #[test]
    fn a_seed_that_is_not_a_number_is_refused() {
        let header = renew_trace::TraceHeader::new(SAMPLE_NAME, 1, 1, 1)
            .and_then(|header| header.with_key("seed", "later"))
            .expect("a well-formed header");
        let trace = renew_trace::Trace::new(header, Vec::new()).expect("no events");
        let refused = replay_recorded(&trace);
        assert!(
            matches!(&refused, Err(SampleError::Usage(message)) if message.contains("seed")),
            "{refused:?}"
        );
    }

    /// With no seed key at all the run is seed zero, not an error: the
    /// key is the caller's, and a trace that omits it is still a trace.
    #[test]
    fn a_trace_without_a_seed_replays_at_zero() {
        let header =
            renew_trace::TraceHeader::new(SAMPLE_NAME, 1, 16_666_667, 5).expect("a header");
        let trace = renew_trace::Trace::new(header, Vec::new()).expect("no events");
        let report = replay_recorded(&trace).expect("a seedless trace still replays");
        assert_eq!(report.seed, 0);
        assert_eq!(report.world.ticks(), 1);
    }
}
