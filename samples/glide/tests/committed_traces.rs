//! The committed traces are recordings, and the tests hold them to it.
//!
//! `soar.trace` is not hand-authored: it is the autopilot's own run,
//! recorded through the same pieces a live recording uses. The builder
//! below is the single source of truth for its bytes — the fixed-point
//! test asserts the committed file matches it exactly, and the ignored
//! regenerator rewrites the file from it when the rules change.
//!
//! Regenerate after a deliberate rules change with:
//!
//! ```text
//! cargo test -p renew-sample-glide --test committed_traces -- --ignored
//! ```

use renew_event::{KeyCode, WindowEvent};
use renew_replay::Recorder;
use renew_sample_glide_world::World;

const FRAMES: u64 = 2_000;
const SEED: u64 = 7;

/// The soar trace's exact bytes: an autopilot run, its flaps written
/// down as the space presses a person would have made.
fn soar_bytes() -> Result<String, String> {
    let mut world = World::new(SEED);
    let mut recorder = Recorder::default();
    let mut release_due = None;
    for _ in 0..FRAMES {
        let tick = world.tick();
        if release_due == Some(tick) {
            recorder.event(
                tick,
                WindowEvent::Key {
                    code: KeyCode::Space,
                    pressed: false,
                    repeat: false,
                },
            );
            release_due = None;
        }
        let flap = world.autopilot();
        if flap {
            recorder.event(
                tick,
                WindowEvent::Key {
                    code: KeyCode::Space,
                    pressed: true,
                    repeat: false,
                },
            );
            // The pilot never flaps twice in a row — a flap sets the
            // bird rising and the pilot only flaps while falling — so
            // the release the next tick can never collide with a press.
            release_due = Some(tick + 1);
        }
        world.step(flap);
    }
    assert!(world.alive(), "the recorded run must survive its length");
    assert!(
        world.score() > 3,
        "the recorded run must clear several pipes"
    );

    let header = renew_trace::TraceHeader::new(
        "glide",
        FRAMES,
        renew_frame::Timestep::HZ_60.nanos().get(),
        renew_frame::StepBudget::DEFAULT.get().get(),
    )
    .and_then(|header| header.with_key("seed", &SEED.to_string()))
    .map_err(|error| error.to_string())?;
    let sealed = recorder.finish(header).map_err(|error| error.to_string())?;
    Ok(renew_trace::write(&sealed))
}

#[test]
fn soar_is_the_fixed_point_of_its_own_recording() {
    let committed = include_str!("../traces/soar.trace").replace("\r\n", "\n");
    assert_eq!(
        committed,
        soar_bytes().expect("the autopilot recording builds"),
        "the committed soar trace is not the autopilot's recording; \
         regenerate it with the ignored test in this file"
    );
}

#[test]
#[ignore = "writes the committed trace; run deliberately after a rules change"]
fn regenerate_soar() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/traces/soar.trace");
    let bytes = soar_bytes().expect("the autopilot recording builds");
    std::fs::write(path, bytes).expect("write the committed trace");
}
