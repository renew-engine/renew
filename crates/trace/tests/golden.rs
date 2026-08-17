//! One trace, written out by hand, asserted byte for byte in both
//! directions.
//!
//! This is the anchor under the round-trip property, and it is here
//! because round-tripping alone proves less than it looks like it proves:
//! a writer and a reader that made the *same* mistake — a swapped pair of
//! coordinates, a tick shifted by one, a state word inverted — are still
//! exact inverses of each other, and every round-trip assertion in the
//! world stays green. The text below was typed by a person from the
//! documented grammar, and no part of it was produced by the code it
//! tests. The bit patterns were worked out from the format of the numbers
//! rather than printed by the writer.
//!
//! Read it in both directions, and read it as prose: if this file stops
//! matching what the codec does, one of the two is wrong, and which one is
//! a conversation rather than a rewrite of this constant.

use renew_trace::{
    FiniteF32, FiniteF64, Trace, TraceButton, TraceEvent, TraceHeader, TraceKey, TraceTouchPhase,
    parse, write,
};

/// A short session: focus arrives, the pointer moves to (1.5, -2.0) and
/// the device reports the same motion raw, the left button goes down and
/// up, the wheel turns half a notch back, a key is held with one
/// auto-repeat and an `a` is typed on its release tick, a native button
/// goes down while a finger lands at the pointer's spot, the window is
/// resized and rescaled, a redraw is served, and the window is closed
/// after the final step.
const GOLDEN: &str = "\
renew-trace 2 sample=input_echo ticks=12 timestep_ns=16666667 budget=5 seed=3 extent=640x480
e 0 focus in
e 1 pointer 0x3ff8000000000000 0xc000000000000000
e 1 motion 0x3ff8000000000000 0xc000000000000000
e 1 button left down
e 2 button left up
e 2 wheel 0x00000000 0xbf000000
e 3 key arrow-right down
e 4 key arrow-right down repeat
e 5 key arrow-right up
e 5 text 97
e 6 button other:9 down
e 6 touch 7 start 0x3ff8000000000000 0xc000000000000000
e 7 resize 1280 720
e 7 scale 0x4000000000000000
e 8 redraw
e 12 close
";

/// The same trace, assembled from typed values. Nothing here is derived
/// from the text above: the floats are written as numbers, the ticks as
/// numbers, and the states as `true` and `false`.
// A test helper, called only from `#[test]` fns: the tests-only unwrap
// allowance covers those, not their helpers, and this extends it in the
// same spirit. Every value below is finite, in range and in order by
// inspection, so a failure here is a broken constructor and belongs at
// the top of the failure output.
#[allow(clippy::unwrap_used)]
fn golden_trace() -> Trace {
    let header = TraceHeader::new("input_echo", 12, 16_666_667, 5)
        .unwrap()
        .with_key("seed", "3")
        .unwrap()
        .with_key("extent", "640x480")
        .unwrap();
    let arrow_right = |pressed, repeat| TraceEvent::Key {
        code: TraceKey::ArrowRight,
        pressed,
        repeat,
    };
    Trace::new(
        header,
        vec![
            (0, TraceEvent::Focused(true)),
            (
                1,
                TraceEvent::PointerMoved {
                    x: FiniteF64::new(1.5).unwrap(),
                    y: FiniteF64::new(-2.0).unwrap(),
                },
            ),
            (
                1,
                TraceEvent::PointerMotion {
                    dx: FiniteF64::new(1.5).unwrap(),
                    dy: FiniteF64::new(-2.0).unwrap(),
                },
            ),
            (
                1,
                TraceEvent::PointerButton {
                    button: TraceButton::Left,
                    pressed: true,
                },
            ),
            (
                2,
                TraceEvent::PointerButton {
                    button: TraceButton::Left,
                    pressed: false,
                },
            ),
            (
                2,
                TraceEvent::Wheel {
                    dx: FiniteF32::new(0.0).unwrap(),
                    dy: FiniteF32::new(-0.5).unwrap(),
                },
            ),
            (3, arrow_right(true, false)),
            (4, arrow_right(true, true)),
            (5, arrow_right(false, false)),
            (5, TraceEvent::TextEntered { ch: 97 }),
            (
                6,
                TraceEvent::PointerButton {
                    button: TraceButton::Other(9),
                    pressed: true,
                },
            ),
            (
                6,
                TraceEvent::Touch {
                    finger: 7,
                    phase: TraceTouchPhase::Started,
                    x: FiniteF64::new(1.5).unwrap(),
                    y: FiniteF64::new(-2.0).unwrap(),
                },
            ),
            (
                7,
                TraceEvent::Resized {
                    width: 1280,
                    height: 720,
                },
            ),
            (
                7,
                TraceEvent::ScaleFactorChanged {
                    scale: FiniteF64::new(2.0).unwrap(),
                },
            ),
            (8, TraceEvent::RedrawRequested),
            (12, TraceEvent::CloseRequested),
        ],
    )
    .unwrap()
}

#[test]
fn the_writer_produces_the_golden_text_byte_for_byte() {
    assert_eq!(write(&golden_trace()), GOLDEN);
}

#[test]
fn the_reader_produces_the_golden_trace_from_the_golden_text() {
    assert_eq!(parse(GOLDEN), Ok(golden_trace()));
}

/// Every line shape the format defines appears above. Whenever one is
/// added, this is where it has to show up too — a shape with no golden
/// line is a shape whose text nobody has ever read. (`motion` and
/// `text` were each exactly that for as long as they had existed:
/// defined, written, and absent from this file while its name promised
/// otherwise.)
#[test]
fn the_golden_text_exercises_every_line_shape() {
    for shape in [
        " key ",
        " pointer ",
        " motion ",
        " button ",
        " wheel ",
        " focus ",
        " text ",
        " resize ",
        " scale ",
        " redraw",
        " close",
        " touch ",
    ] {
        assert!(GOLDEN.contains(shape), "no {shape} line in the golden text");
    }
}

/// The trailing bucket, asserted on the golden rather than only in a
/// unit test: the closing event carries the run's own tick count, which
/// means *after the final step*, and is where a terminating event
/// normally lands.
#[test]
fn the_golden_trace_closes_after_its_final_step() {
    let trace = golden_trace();
    let last = *trace.events().last().unwrap();
    assert_eq!(last, (trace.header().ticks(), TraceEvent::CloseRequested));
    assert!(GOLDEN.contains("ticks=12 "));
    assert!(GOLDEN.ends_with("e 12 close\n"));
}

/// A line ending is not a change to what a line says. The same golden
/// text with Windows line endings reads as the same trace — and writes
/// back out with newlines, because the writer has one spelling.
#[test]
fn the_same_text_with_carriage_returns_is_the_same_trace() {
    let with_carriage_returns = GOLDEN.replace('\n', "\r\n");
    assert_eq!(parse(&with_carriage_returns), Ok(golden_trace()));
    assert_eq!(write(&golden_trace()), GOLDEN);
}

/// Deleting one line changes the trace, and changes it in a way an
/// application would witness: the key that was released stays held.
#[test]
fn deleting_a_release_leaves_a_key_held() {
    let mutated = GOLDEN.replace("e 5 key arrow-right up\n", "");
    let trace = parse(&mutated).unwrap();
    assert_eq!(trace.events().len(), golden_trace().events().len() - 1);
    assert!(!trace.events().iter().any(|(_, event)| *event
        == TraceEvent::Key {
            code: TraceKey::ArrowRight,
            pressed: false,
            repeat: false,
        }));
}

/// Shifting the tick column one later is a different trace, not a
/// re-spelling of the same one — the header is untouched and every event
/// keeps its shape, so the column under test is the only thing that
/// moved.
///
/// The closing event stays where it is, because it already sits on the
/// last legal tick. That is not an exception carved out to make the test
/// pass; it is the bound, and the second half of this test is what says
/// so.
#[test]
fn shifting_the_tick_column_produces_a_different_trace() {
    let ticks = golden_trace().header().ticks();
    let shifted: String = GOLDEN
        .lines()
        .map(|line| match line.strip_prefix("e ") {
            None => format!("{line}\n"),
            Some(rest) => {
                let (tick, tail) = rest.split_once(' ').unwrap();
                let tick = tick.parse::<u64>().unwrap();
                let moved = if tick < ticks { tick + 1 } else { tick };
                format!("e {moved} {tail}\n")
            }
        })
        .collect();
    let trace = parse(&shifted).unwrap();
    assert_ne!(trace, golden_trace());
    let mut moved = 0;
    for (shifted, original) in trace.events().iter().zip(golden_trace().events()) {
        assert_eq!(shifted.1, original.1, "an event changed shape, not tick");
        assert!(shifted.0 == original.0 + 1 || original.0 == ticks);
        moved += usize::from(shifted.0 != original.0);
    }
    assert_eq!(moved, trace.events().len() - 1, "the shift was not applied");
}

/// And the bound is real: pushing the closing event one tick further is
/// refused, on its own line.
#[test]
fn shifting_the_last_event_past_the_end_is_refused() {
    let past_end = GOLDEN.replace("e 12 close", "e 13 close");
    let error = parse(&past_end).unwrap_err();
    assert_eq!(error.line(), 17);
    assert_eq!(
        error.kind(),
        &renew_trace::TraceErrorKind::TickBeyondHeader {
            tick: 13,
            ticks: 12
        }
    );
}
