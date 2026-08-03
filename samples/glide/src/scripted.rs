//! Driving the world from a trace: the fixed-step loop on a synthetic
//! clock, events delivered by tick, input resolved through the same map
//! a windowed mode will use.

use renew_event::{KeyCode, WindowEvent};
use renew_frame::{FrameLoop, FrameStats, StepBudget, Timestamp, Timestep};
use renew_input::{Binding, InputMap};
use renew_replay::Recorder;
use renew_sample_glide_world::{Action, World};

use crate::cli::{Options, Report};
use crate::{SampleError, trace::Trace};

/// One synthetic frame is exactly one timestep, so a run of N frames
/// executes exactly N steps, banks nothing and drops nothing — the
/// expected numbers are readable without running anything.
const FRAME_INTERVAL_NS: u64 = Timestep::HZ_60.nanos().get();

/// The bindings a windowed mode will share: space or the primary
/// button, one action.
fn input_map() -> InputMap<Action> {
    let mut map = InputMap::new();
    map.bind(Binding::Key(KeyCode::Space), Action::Flap);
    map
}

/// Drive a run from a built-in trace, recording into `recorder` if the
/// caller brought one. The caller owns the recorder and the files it
/// will become; this module computes.
pub fn run(options: &Options, trace: &Trace, recorder: Option<&mut Recorder>) -> Report {
    drive(
        options.seed,
        options.frames,
        trace.name.to_string(),
        &trace.events,
        recorder,
    )
}

/// Seal a recording against the run that produced it: the header's
/// facts — length, seed — come from the report, which is why this
/// takes both.
pub fn close_recording(
    report: &Report,
    recorder: Recorder,
) -> Result<renew_trace::Trace, SampleError> {
    let header = renew_trace::TraceHeader::new(
        "glide",
        report.world.tick(),
        FRAME_INTERVAL_NS,
        StepBudget::DEFAULT.get().get(),
    )
    .and_then(|header| header.with_key("seed", &report.seed.to_string()))
    .map_err(|error| SampleError::failed("describing the recording", &error))?;
    recorder
        .finish(header)
        .map_err(|error| SampleError::failed("closing the recording", &error))
}

/// Drive a run a recorded file owns: its header carries the length and
/// the seed, and only its event lines steer.
pub fn replay_recorded(recorded: &renew_trace::Trace) -> Result<Report, SampleError> {
    let seed = recorded
        .header()
        .value("seed")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            SampleError::Failed("the trace header carries no readable seed".to_string())
        })?;
    let events: Vec<(u64, WindowEvent)> = recorded
        .events()
        .iter()
        .map(|(tick, event)| (*tick, renew_replay::from_trace(*event)))
        .collect();
    Ok(drive(
        seed,
        recorded.header().ticks(),
        "replay".to_string(),
        &events,
        None,
    ))
}

/// The loop itself. Events for tick `k` are delivered before step `k` —
/// the loader's own indexing, no shift anywhere.
fn drive(
    seed: u64,
    frames: u64,
    source: String,
    events: &[(u64, WindowEvent)],
    mut recorder: Option<&mut Recorder>,
) -> Report {
    let mut world = World::new(seed);
    let mut input = input_map();
    let mut frame = FrameLoop::new(
        Timestep::HZ_60,
        StepBudget::DEFAULT,
        Timestamp::from_nanos(0),
    );
    let mut stats = FrameStats::new();

    for index in 1..=frames {
        for (at, event) in events {
            if *at == world.tick() {
                if let Some(recorder) = recorder.as_deref_mut() {
                    recorder.event(world.tick(), *event);
                }
                input.handle(*event);
            }
        }
        let now = Timestamp::from_nanos(FRAME_INTERVAL_NS.saturating_mul(index));
        let plan = frame.begin_frame(now);
        let flap = input.state(Action::Flap).just_pressed;
        for _step in plan.steps() {
            world.step(flap);
        }
        input.advance();
        stats.absorb(&plan);
    }

    Report {
        seed,
        source,
        stats,
        world,
    }
}
