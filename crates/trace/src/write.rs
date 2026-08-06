//! Writing a trace out as text.
//!
//! The writer is total: every trace that can be built can be written, and
//! everything it writes can be read back as the same trace. That is not a
//! coincidence to be tested for — it is bought by the types. A float that
//! could not be written finitely cannot be put in an event, and a header
//! field that could not survive the round trip cannot be put in a header,
//! so there is no case left for the writer to fail on and it returns text
//! rather than a result.

use crate::event::{FiniteF32, FiniteF64, TraceEvent};
use crate::grammar::{
    ASSIGN, BUDGET, BUTTON, CLOSE, DOWN, EVENT, FOCUS, FOCUS_IN, FOCUS_OUT, FORMAT_VERSION,
    HEX_PREFIX, KEY, MAGIC, MOTION, POINTER, REDRAW, REPEAT, RESIZE, SAMPLE, SCALE, SEPARATOR,
    TICKS, TIMESTEP_NS, UP, WHEEL,
};
use crate::trace::Trace;

/// A trace as text: the header line, then one line per event, each ended
/// by a newline.
///
/// The file therefore ends with a newline, the way text files do, and the
/// reader treats that last newline as a terminator rather than as an empty
/// line.
#[must_use]
pub fn write(trace: &Trace) -> String {
    let header = trace.header();
    let mut text = format!(
        "{MAGIC}{SEPARATOR}{FORMAT_VERSION}\
         {SEPARATOR}{SAMPLE}={sample}\
         {SEPARATOR}{TICKS}={ticks}\
         {SEPARATOR}{TIMESTEP_NS}={timestep_ns}\
         {SEPARATOR}{BUDGET}={budget}",
        sample = header.sample(),
        ticks = header.ticks(),
        timestep_ns = header.timestep_ns(),
        budget = header.budget(),
    );
    for (key, value) in header.keys() {
        text.push(SEPARATOR);
        text.push_str(key);
        text.push(ASSIGN);
        text.push_str(value);
    }
    text.push('\n');
    for (tick, event) in trace.events() {
        text.push_str(&event_line(*tick, event));
        text.push('\n');
    }
    text
}

/// One event line, without its newline.
fn event_line(tick: u64, event: &TraceEvent) -> String {
    match *event {
        TraceEvent::Key {
            code,
            pressed,
            repeat,
        } => {
            let mut line = format!(
                "{EVENT} {tick} {KEY} {name} {state}",
                name = code.name(),
                state = pressed_word(pressed),
            );
            if repeat {
                line.push(SEPARATOR);
                line.push_str(REPEAT);
            }
            line
        }
        TraceEvent::PointerMoved { x, y } => format!(
            "{EVENT} {tick} {POINTER} {x} {y}",
            x = hex_f64(x),
            y = hex_f64(y),
        ),
        TraceEvent::PointerMotion { dx, dy } => format!(
            "{EVENT} {tick} {MOTION} {dx} {dy}",
            dx = hex_f64(dx),
            dy = hex_f64(dy),
        ),
        TraceEvent::PointerButton { button, pressed } => format!(
            "{EVENT} {tick} {BUTTON} {button} {state}",
            state = pressed_word(pressed),
        ),
        TraceEvent::Wheel { dx, dy } => format!(
            "{EVENT} {tick} {WHEEL} {dx} {dy}",
            dx = hex_f32(dx),
            dy = hex_f32(dy),
        ),
        TraceEvent::Focused(focused) => {
            let state = if focused { FOCUS_IN } else { FOCUS_OUT };
            format!("{EVENT} {tick} {FOCUS} {state}")
        }
        TraceEvent::Resized { width, height } => {
            format!("{EVENT} {tick} {RESIZE} {width} {height}")
        }
        TraceEvent::ScaleFactorChanged { scale } => {
            format!("{EVENT} {tick} {SCALE} {scale}", scale = hex_f64(scale))
        }
        TraceEvent::RedrawRequested => format!("{EVENT} {tick} {REDRAW}"),
        TraceEvent::CloseRequested => format!("{EVENT} {tick} {CLOSE}"),
    }
}

fn pressed_word(pressed: bool) -> &'static str {
    if pressed { DOWN } else { UP }
}

/// A bit pattern, zero-padded to exactly the width of its type. The width
/// comes from the type rather than from a literal here, because a field
/// one digit short is a different number read back, silently.
fn hex_f64(value: FiniteF64) -> String {
    format!(
        "{HEX_PREFIX}{bits:0width$x}",
        bits = value.bits(),
        width = FiniteF64::HEX_DIGITS,
    )
}

fn hex_f32(value: FiniteF32) -> String {
    format!(
        "{HEX_PREFIX}{bits:0width$x}",
        bits = value.bits(),
        width = FiniteF32::HEX_DIGITS,
    )
}

#[cfg(test)]
mod tests {
    use super::{event_line, write};
    use crate::event::{FiniteF32, FiniteF64, TraceButton, TraceEvent, TraceKey};
    use crate::trace::{Trace, TraceHeader};

    fn line(event: TraceEvent) -> String {
        event_line(4, &event)
    }

    #[test]
    fn a_header_with_no_events_is_one_line_ending_in_a_newline() {
        let trace = Trace::new(
            TraceHeader::new("input_echo", 30, 16_666_667, 5).unwrap(),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            write(&trace),
            "renew-trace 0 sample=input_echo ticks=30 timestep_ns=16666667 budget=5\n"
        );
    }

    #[test]
    fn caller_keys_follow_the_four_the_codec_owns_in_their_own_order() {
        let header = TraceHeader::new("input_echo", 1, 2, 3)
            .unwrap()
            .with_key("seed", "7")
            .unwrap()
            .with_key("extent", "640x480")
            .unwrap();
        let trace = Trace::new(header, Vec::new()).unwrap();
        assert_eq!(
            write(&trace),
            "renew-trace 0 sample=input_echo ticks=1 timestep_ns=2 budget=3 seed=7 extent=640x480\n"
        );
    }

    #[test]
    fn every_line_shape_is_written_the_way_the_format_documents_it() {
        assert_eq!(
            line(TraceEvent::Key {
                code: TraceKey::ArrowRight,
                pressed: true,
                repeat: false,
            }),
            "e 4 key arrow-right down"
        );
        assert_eq!(
            line(TraceEvent::Key {
                code: TraceKey::Space,
                pressed: false,
                repeat: true,
            }),
            "e 4 key space up repeat"
        );
        assert_eq!(
            line(TraceEvent::PointerMoved {
                x: FiniteF64::new(1.5).unwrap(),
                y: FiniteF64::new(-2.0).unwrap(),
            }),
            "e 4 pointer 0x3ff8000000000000 0xc000000000000000"
        );
        // A delta gets a token of its own. Stored under `pointer` it
        // would read back as a cursor teleporting to the origin.
        assert_eq!(
            line(TraceEvent::PointerMotion {
                dx: FiniteF64::new(1.5).unwrap(),
                dy: FiniteF64::new(-2.0).unwrap(),
            }),
            "e 4 motion 0x3ff8000000000000 0xc000000000000000"
        );
        assert_eq!(
            line(TraceEvent::PointerButton {
                button: TraceButton::Left,
                pressed: true,
            }),
            "e 4 button left down"
        );
        assert_eq!(
            line(TraceEvent::PointerButton {
                button: TraceButton::Other(9),
                pressed: false,
            }),
            "e 4 button other:9 up"
        );
        assert_eq!(
            line(TraceEvent::Wheel {
                dx: FiniteF32::new(0.0).unwrap(),
                dy: FiniteF32::new(-0.5).unwrap(),
            }),
            "e 4 wheel 0x00000000 0xbf000000"
        );
        assert_eq!(line(TraceEvent::Focused(true)), "e 4 focus in");
        assert_eq!(line(TraceEvent::Focused(false)), "e 4 focus out");
        assert_eq!(
            line(TraceEvent::Resized {
                width: 1280,
                height: 720,
            }),
            "e 4 resize 1280 720"
        );
        assert_eq!(
            line(TraceEvent::ScaleFactorChanged {
                scale: FiniteF64::new(2.0).unwrap(),
            }),
            "e 4 scale 0x4000000000000000"
        );
        assert_eq!(line(TraceEvent::RedrawRequested), "e 4 redraw");
        assert_eq!(line(TraceEvent::CloseRequested), "e 4 close");
    }

    /// A bit pattern is padded to the full width of its type. Negative
    /// zero is the case that proves it: it differs from the ordinary zero
    /// in the leading digit, which a writer that trimmed would drop.
    #[test]
    fn bit_patterns_are_padded_to_the_width_of_their_type() {
        assert_eq!(
            line(TraceEvent::ScaleFactorChanged {
                scale: FiniteF64::new(-0.0).unwrap(),
            }),
            "e 4 scale 0x8000000000000000"
        );
        assert_eq!(
            line(TraceEvent::Wheel {
                dx: FiniteF32::from_bits(1).unwrap(),
                dy: FiniteF32::new(-0.0).unwrap(),
            }),
            "e 4 wheel 0x00000001 0x80000000"
        );
    }

    #[test]
    fn each_event_is_its_own_line_in_the_order_it_was_recorded() {
        let trace = Trace::new(
            TraceHeader::new("input_echo", 2, 1, 1).unwrap(),
            vec![
                (0, TraceEvent::RedrawRequested),
                (0, TraceEvent::Focused(true)),
                (2, TraceEvent::CloseRequested),
            ],
        )
        .unwrap();
        assert_eq!(
            write(&trace),
            "renew-trace 0 sample=input_echo ticks=2 timestep_ns=1 budget=1\n\
             e 0 redraw\n\
             e 0 focus in\n\
             e 2 close\n"
        );
    }
}
