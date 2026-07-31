//! The headless driver: a scripted trace, a synthetic clock, no window.
//!
//! This mode reads no clock at all, so its digest line is a pure
//! function of `(trace, frames, seed)` and is identical across runs,
//! processes and machines. It is what makes a windowing sample provable
//! in CI.

use renew_frame::{FrameLoop, FrameStats, StepBudget, Timestamp, Timestep};

use crate::cli::{Options, Report};
use crate::error::SampleError;
use crate::trace::{self, Trace};
use crate::world::EchoWorld;

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
    Ok(replay(trace, options.seed, options.frames))
}

/// Replay one trace for at most `frames` frames.
///
/// Events scheduled for a frame are delivered before that frame is
/// planned — the same order the window seam uses, where the event phase
/// runs before the update phase. The run stops early if the input asks
/// it to; a close request is a close request whether it came from a
/// script or a person.
#[must_use]
pub fn replay(trace: &Trace, seed: u64, frames: u64) -> Report {
    let mut world = EchoWorld::new(seed);
    let mut frame = FrameLoop::new(
        Timestep::HZ_60,
        StepBudget::DEFAULT,
        Timestamp::from_nanos(0),
    );
    let mut stats = FrameStats::new();
    for index in 1..=frames {
        for (at, event) in trace.events {
            if *at == index {
                world.event(*event);
            }
        }
        let now = Timestamp::from_nanos(FRAME_INTERVAL_NS.saturating_mul(index));
        let plan = frame.begin_frame(now);
        for step in plan.steps() {
            world.step(step);
        }
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

#[cfg(test)]
mod tests {
    use super::{replay, run};
    use crate::cli::Options;
    use crate::trace;

    fn walk(frames: u64) -> crate::cli::Report {
        replay(trace::by_name("walk").expect("the walk trace"), 0, frames)
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
        let idle = replay(trace::by_name("idle").expect("the idle trace"), 0, 30);
        assert_eq!(idle.stats.frames(), 30);
        assert_eq!(idle.stats.ticks(), 30);
        assert_eq!(idle.world.events(), 0);
        assert_eq!(idle.world.position(), (0, 0));
        // A frameless run is legal and reports an empty schedule.
        let nothing = replay(trace::by_name("idle").expect("the idle trace"), 0, 0);
        assert_eq!(nothing.stats.frames(), 0);
    }

    #[test]
    fn two_replays_of_one_trace_agree_to_the_last_bit() {
        assert_eq!(walk(600).digest_line(), walk(600).digest_line());
        // The seed is an input like any other, and it shows.
        let seeded = replay(trace::by_name("walk").expect("the walk trace"), 3, 600);
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
}
