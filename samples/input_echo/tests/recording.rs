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
        "renew-trace 0 sample=input_echo ticks=4 timestep_ns=16666667 budget=8\n",
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
