//! Driving the world from a trace: the fixed-step loop on a synthetic
//! clock, events delivered by tick, input resolved through the same map
//! the windowed mode uses.

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

/// The bindings both modes share: space or the primary button, one
/// action. The pointer binding is invisible to committed traces (none
/// carries a pointer event) and makes the windowed mode mouse-playable;
/// binding it here keeps this doc sentence true instead of aspirational.
pub(crate) fn input_map() -> InputMap<Action> {
    let mut map = InputMap::new();
    map.bind(Binding::Key(KeyCode::Space), Action::Flap);
    map.bind(
        Binding::Pointer(renew_event::PointerButton::Left),
        Action::Flap,
    );
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
    // The header's length is the SESSION's, not the world's: with a
    // pause menu the world can stand still while frames keep coming,
    // and a recorded event's tick is a session tick. In a run that
    // never pauses the two counts are equal, which is why this line
    // could say world.tick() for as long as it did.
    let header = renew_trace::TraceHeader::new(
        "glide",
        report.stats.ticks(),
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
    replay_to(recorded, None)
}

/// The world a committed trace produces at `tick` — the scripted loop,
/// promoted so oracles need not copy it. A copy could drift (events
/// after the step, `advance` before the read) into a different
/// deterministic world whose premises still pass; one loop, however
/// many consumers, is the only shape that cannot.
///
/// # Errors
///
/// [`SampleError::Usage`] for an unknown trace name;
/// [`SampleError::Failed`] if a committed file does not parse or its
/// header refuses this driver's clock, budget, or missing seed.
pub fn world_at(name: &str, tick: u64) -> Result<World, SampleError> {
    drawn_at(name, tick).map(|(world, _)| world)
}

/// The same replay, returning the presentation effects beside the
/// world.
///
/// **What [`world_at`] cannot give an image oracle.** The effects are a
/// function of the flight, not of the world it ended at: the trail is
/// whatever the last half-second of flying shed, and the burst is an
/// edge that has already passed. A pool rebuilt from a finished world
/// is empty, so a checkpoint drawn that way shows no trail at all and
/// its committed picture proves nothing about one. This returns the
/// pools the run actually accumulated, out of the same loop, for the
/// same reason `world_at` was promoted rather than copied.
///
/// # Errors
///
/// As [`world_at`].
pub fn drawn_at(name: &str, tick: u64) -> Result<(World, crate::effects::Effects), SampleError> {
    let recorded = renew_trace::parse(crate::trace::text_by_name(name)?)
        .map_err(|error| SampleError::Failed(format!("built-in trace: {error}")))?;
    replay_to(&recorded, Some(tick)).map(|report| (report.world, report.effects))
}

/// The recorded-replay loop with an optional frame override: the header
/// still rules on clock, budget and seed; `frames` merely stops early
/// for checkpoint oracles.
fn replay_to(recorded: &renew_trace::Trace, frames: Option<u64>) -> Result<Report, SampleError> {
    // The header carries four facts and this loop honours all four or
    // refuses: replaying a 120Hz recording at this driver's fixed 60Hz
    // would be a different run wearing the recording's name.
    if recorded.header().timestep_ns() != FRAME_INTERVAL_NS {
        return Err(SampleError::Failed(format!(
            "the trace was recorded at timestep_ns={}, this driver runs {}",
            recorded.header().timestep_ns(),
            FRAME_INTERVAL_NS
        )));
    }
    if recorded.header().budget() != StepBudget::DEFAULT.get().get() {
        return Err(SampleError::Failed(format!(
            "the trace was recorded with budget={}, this driver runs {}",
            recorded.header().budget(),
            StepBudget::DEFAULT.get().get()
        )));
    }
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
        frames.unwrap_or_else(|| recorded.header().ticks()),
        "replay".to_string(),
        &events,
        None,
    ))
}

/// The loop itself. Events for session tick `k` are delivered before
/// frame `k` — the loader's own indexing, no shift anywhere. The
/// session tick is the frame counter, not the world's: while the
/// pause menu is open the world stands still and the session keeps
/// counting, so events keep arriving. In a run that never pauses the
/// two counters are equal, which is why every trace recorded before
/// the menu existed replays unchanged.
fn drive(
    seed: u64,
    frames: u64,
    source: String,
    events: &[(u64, WindowEvent)],
    mut recorder: Option<&mut Recorder>,
) -> Report {
    let mut world = World::new(seed);
    let mut effects = crate::effects::Effects::new(&world);
    let mut menu = crate::menu::Menu::new();
    let mut input = input_map();
    let mut frame = FrameLoop::new(
        Timestep::HZ_60,
        StepBudget::DEFAULT,
        Timestamp::from_nanos(0),
    );
    let mut stats = FrameStats::new();

    for index in 1..=frames {
        let session_tick = index - 1;
        for (at, event) in events {
            if *at == session_tick {
                if let Some(recorder) = recorder.as_deref_mut() {
                    recorder.event(session_tick, *event);
                }
                // The menu hears everything; gameplay hears an event
                // only when the menu was closed as it arrived, so the
                // click that presses Resume never also flaps.
                let was_open = menu.is_open();
                menu.handle(event);
                if !was_open && menu.is_open() {
                    // Opening releases gameplay input: a key held into
                    // the pause must not wedge the map with a press
                    // whose release the menu will swallow.
                    input.release_all();
                }
                if !was_open {
                    input.handle(*event);
                }
            }
        }
        for action in menu.drain() {
            if action == crate::menu::MenuAction::Restart {
                world = World::new(seed);
                // The sparks belong to that world too — the same rule
                // the windowed driver follows, and for the same reason:
                // a trail and a burst kept across a restart are the
                // record of a flight that no longer happened.
                effects = crate::effects::Effects::new(&world);
            }
        }
        let now = Timestamp::from_nanos(FRAME_INTERVAL_NS.saturating_mul(index));
        let plan = frame.begin_frame(now);
        // One frame is exactly one timestep by construction, so one
        // flap edge feeds exactly one step. A multi-step plan here
        // would double-fire every press; the loop's whole shape rests
        // on this, so it is asserted rather than assumed.
        debug_assert!(
            plan.steps().len() == 1,
            "a synthetic frame must plan exactly one step"
        );
        let flap = input.state(Action::Flap).just_pressed;
        for _step in plan.steps() {
            // An open menu pauses the world: the session advances,
            // the world does not, and the pause bit is digested.
            if !menu.is_open() {
                world.step(flap);
                // Once per **executed** step, after the step — the rule
                // `Effects::observe` states and the windowed driver
                // already follows. A paused frame steps nothing and so
                // observes nothing: the trail freezes with the world
                // rather than shedding into a still picture.
                effects.observe(&world);
            }
        }
        input.advance();
        stats.absorb(&plan);
    }

    let session_hash = menu.absorb(world.digest()).finish();
    Report {
        seed,
        source,
        stats,
        world,
        session_hash,
        effects,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The menu trace's structure, pinned: the pause really pauses
    /// and the restart really restarts, visible in the world's own
    /// tick count. At session 240 the world has stepped 228 times —
    /// 240 minus the twelve frames the first pause held it (opened by
    /// the escape at session 120, closed by the Resume release at
    /// 132). At session 320 it has stepped 8 — a NEW world, made by
    /// the Restart click at session 312, stepped once per session
    /// since. Wrong routing (a click that also flaps), a pause that
    /// leaks steps, or a restart that keeps the old world each move
    /// one of these two integers.
    #[test]
    fn the_menu_trace_pauses_and_restarts_the_world() {
        let mid = world_at("menu", 240).expect("trace runs").tick();
        let after = world_at("menu", 320).expect("trace runs").tick();
        assert_eq!((mid, after), (228, 8));
    }

    #[test]
    fn both_bindings_resolve_to_the_one_action() {
        // The doc above the map promises "space or the primary button";
        // this is the featureless test that keeps the sentence true.
        let mut map = input_map();
        map.handle(WindowEvent::Key {
            code: KeyCode::Space,
            pressed: true,
            repeat: false,
        });
        assert!(map.state(Action::Flap).just_pressed, "space flaps");
        map.advance();
        map.handle(WindowEvent::Key {
            code: KeyCode::Space,
            pressed: false,
            repeat: false,
        });
        map.advance();
        map.handle(WindowEvent::PointerButton {
            button: renew_event::PointerButton::Left,
            pressed: true,
        });
        assert!(
            map.state(Action::Flap).just_pressed,
            "the primary button flaps"
        );
    }
}
