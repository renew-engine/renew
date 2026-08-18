//! Recording a sequence of events produces exactly the expected file.
//!
//! The assertion is against a string typed by hand, not one the writer
//! produced. That distinction is the whole value of the test: a
//! round-trip through a writer and reader that are inverse to each other
//! passes even when both are wrong in the same way, and only text a
//! person wrote down can catch the pair agreeing on something incorrect.

use renew_platform::window::{KeyCode, PointerButton, WindowEvent};
use renew_sample_input_echo::record::Recorder;
use renew_trace::{TraceHeader, write};

/// The header every test here records against.
///
/// Returns the result rather than unwrapping it: the lint that forbids
/// `expect` outside tests reaches helpers in a test file too, because
/// the exemption follows `#[test]` rather than the file.
fn header() -> Result<TraceHeader, renew_trace::TraceError> {
    TraceHeader::new("input_echo", 4, 16_666_667, 8)
}

#[test]
fn a_recorded_sequence_is_written_exactly_as_expected() {
    let mut recorder = Recorder::default();
    assert!(recorder.is_empty(), "a fresh recorder has nothing in it");
    recorder.event(
        0,
        WindowEvent::Key {
            code: KeyCode::KeyD,
            pressed: true,
            repeat: false,
        },
    );
    recorder.event(
        2,
        WindowEvent::Resized {
            width: 640,
            height: 360,
        },
    );
    // Two events on one tick: their recorded order is part of the trace,
    // so this also pins that the recorder does not sort or coalesce.
    recorder.event(
        2,
        WindowEvent::PointerButton {
            button: PointerButton::Left,
            pressed: true,
        },
    );
    // The trailing bucket: an event delivered after the final step. This
    // is the common case for a close request, not an edge case.
    recorder.event(4, WindowEvent::CloseRequested);

    let trace = recorder
        .finish(header().expect("a well-formed header"))
        .expect("a recordable sequence");
    let expected = concat!(
        "renew-trace 2 sample=input_echo ticks=4 timestep_ns=16666667 budget=8\n",
        "e 0 key key-d down\n",
        "e 2 resize 640 360\n",
        "e 2 button left down\n",
        "e 4 close\n",
    );
    assert_eq!(write(&trace), expected);
}

/// Recording never silently drops. A payload the format cannot carry
/// fails the whole recording rather than producing a file that is
/// quietly missing an event and replays into a different world.
#[test]
fn a_non_finite_payload_fails_the_recording_rather_than_the_event() {
    let mut recorder = Recorder::default();
    recorder.event(
        0,
        WindowEvent::PointerMoved {
            x: f64::NAN,
            y: 0.0,
        },
    );
    recorder.event(1, WindowEvent::CloseRequested);
    // The good event was still accepted — the refusal is deferred so a
    // live session is not abandoned mid-frame — but the recording as a
    // whole refuses to close.
    assert_eq!(recorder.len(), 1);
    let refused = recorder
        .finish(header().expect("a well-formed header"))
        .expect_err("must refuse");
    assert!(refused.to_string().contains("drop"), "{refused}");
}

/// A tick past the header's count is refused by the same rule a reader
/// would apply, so a recorder cannot write a file its own reader rejects.
#[test]
fn an_event_past_the_final_tick_is_refused_at_recording_time() {
    let mut recorder = Recorder::default();
    recorder.event(5, WindowEvent::CloseRequested);
    let refused = recorder
        .finish(header().expect("a well-formed header"))
        .expect_err("must refuse");
    assert!(refused.to_string().contains("trace"), "{refused}");
}

/// Recording a scripted run produces a file that says what the run did.
///
/// The expected text is written out here in full rather than compared to
/// something the recorder produced, so this fails if the recorder, the
/// codec, or the driver's idea of which tick an event belongs to changes
/// — which is the point. It also pins the whole vocabulary in one place:
/// every event shape the sample can deliver appears below.
#[test]
fn a_scripted_run_records_the_input_it_was_given() {
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("walk-recording.trace");
    let _ = std::fs::remove_file(&path);

    let code = renew_sample_input_echo::run_cli(
        [
            "--headless",
            "--input-trace",
            "walk",
            "--seed",
            "3",
            "--record-trace",
            &path.to_string_lossy(),
        ]
        .into_iter()
        .map(str::to_string),
    );
    assert_eq!(code, 0, "the run should succeed");

    let recorded = std::fs::read_to_string(&path).expect("the trace the run was asked for");
    let expected = concat!(
        "renew-trace 2 sample=input_echo ticks=20 timestep_ns=16666667 budget=5 seed=3\n",
        "e 1 key arrow-right down\n",
        "e 1 pointer 0x4029000000000000 0x4041400000000000\n",
        "e 3 key arrow-down down\n",
        "e 5 button left down\n",
        "e 5 button left up\n",
        "e 7 key arrow-down up\n",
        "e 8 wheel 0x00000000 0x41800000\n",
        "e 9 focus in\n",
        "e 10 resize 640 360\n",
        "e 11 redraw\n",
        "e 12 key arrow-right down repeat\n",
        "e 13 key arrow-right up\n",
        "e 15 scale 0x4000000000000000\n",
        "e 19 close\n",
    );
    assert_eq!(recorded, expected);
}

/// What the recorder writes, its own reader accepts — and reproduces.
///
/// A recorder that emitted something its reader refused would be the
/// defect the format's tick rules exist to prevent, and the only way to
/// know is to read the file back.
#[test]
fn the_recorded_file_reads_back_and_rewrites_identically() {
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("walk-roundtrip.trace");
    let _ = std::fs::remove_file(&path);
    let code = renew_sample_input_echo::run_cli(
        [
            "--headless",
            "--input-trace",
            "walk",
            "--record-trace",
            &path.to_string_lossy(),
        ]
        .into_iter()
        .map(str::to_string),
    );
    assert_eq!(code, 0);

    let text = std::fs::read_to_string(&path).expect("the recorded trace");
    let parsed =
        renew_trace::parse(&text).expect("the recorder must not write what it cannot read");
    assert_eq!(renew_trace::write(&parsed), text);
    // The trailing bucket is the common case, not an edge case: the walk
    // trace ends by asking to close, and that arrives after the last step.
    assert_eq!(parsed.header().ticks(), 20);
}

/// Each committed trace file is the fixed point of its own round trip:
/// loading it, running it, and recording the result reproduces the file
/// byte for byte.
///
/// **This is a guard, not an anchor, and the difference matters.** The
/// loader shifts a file's tick to a frame and the recorder shifts it
/// back, so a pair of shifts wrong in the same direction would satisfy
/// this test perfectly. What it does catch is the committed file drifting
/// away from what the code produces — a hand edit, a reordered event, a
/// header nobody meant to change — which is the realistic failure for a
/// checked-in fixture. The assertion that the shift itself is correct is
/// the hand-typed expectation above, and the one anchored to the file's
/// own text in `trace.rs`.
#[test]
fn every_committed_trace_is_the_fixed_point_of_its_own_recording() {
    for (name, committed) in [
        ("walk", include_str!("../traces/walk.trace")),
        ("idle", include_str!("../traces/idle.trace")),
    ] {
        let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("{name}-fixed-point.trace"));
        let _ = std::fs::remove_file(&path);
        // No `--seed` and no `--frames`: the committed files were
        // captured from the defaults, and naming them here would let the
        // fixture and the test drift apart while both looked deliberate.
        let code = renew_sample_input_echo::run_cli(
            [
                "--headless",
                "--input-trace",
                name,
                "--record-trace",
                &path.to_string_lossy(),
            ]
            .into_iter()
            .map(str::to_string),
        );
        assert_eq!(code, 0, "`{name}` should run");

        let recorded = std::fs::read_to_string(&path).expect("the recorded trace");
        assert_eq!(
            recorded, committed,
            "`{name}.trace` is no longer what recording it produces"
        );
    }
}
