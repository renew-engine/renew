//! The scripted event traces headless mode replays.
//!
//! No runner supplies keystrokes, so a windowing sample that could only
//! be driven by hand would be a sample CI can never execute — and an
//! unexecuted binary is an untested one. These traces are the same
//! events the OS would deliver, on a schedule the sample chooses, fed to
//! the same state machine a window feeds.

use renew_platform::window::{KeyCode, PointerButton, WindowEvent};

use crate::error::SampleError;

/// A named sequence of events, each scheduled at a frame index counted
/// from one — the same index the synthetic clock uses, so "frame 4" in
/// the table is the frame the event lands in.
#[derive(Debug)]
pub struct Trace {
    pub name: &'static str,
    pub summary: &'static str,
    pub events: &'static [(u64, WindowEvent)],
}

const fn press(code: KeyCode) -> WindowEvent {
    WindowEvent::Key {
        code,
        pressed: true,
        repeat: false,
    }
}

const fn release(code: KeyCode) -> WindowEvent {
    WindowEvent::Key {
        code,
        pressed: false,
        repeat: false,
    }
}

/// A short walk: right for twelve ticks, down for four of them, with
/// every other kind of event the window seam can deliver mixed in, and a
/// close request at the end.
const WALK: Trace = Trace {
    name: "walk",
    summary: "keys, pointer, wheel, focus and resize, ending in a close request",
    events: &[
        (2, press(KeyCode::ArrowRight)),
        (2, WindowEvent::PointerMoved { x: 12.5, y: 34.5 }),
        (4, press(KeyCode::ArrowDown)),
        (
            6,
            WindowEvent::PointerButton {
                button: PointerButton::Left,
                pressed: true,
            },
        ),
        (
            6,
            WindowEvent::PointerButton {
                button: PointerButton::Left,
                pressed: false,
            },
        ),
        (8, release(KeyCode::ArrowDown)),
        (9, WindowEvent::Wheel { dx: 0.0, dy: 16.0 }),
        (10, WindowEvent::Focused(true)),
        (
            11,
            WindowEvent::Resized {
                width: 640,
                height: 360,
            },
        ),
        // A repaint request this sample has nothing to paint for, and a
        // key repeat it deliberately ignores: both are counted, neither
        // moves anything.
        (12, WindowEvent::RedrawRequested),
        (
            13,
            WindowEvent::Key {
                code: KeyCode::ArrowRight,
                pressed: true,
                repeat: true,
            },
        ),
        (14, release(KeyCode::ArrowRight)),
        (16, WindowEvent::ScaleFactorChanged { scale: 2.0 }),
        (20, WindowEvent::CloseRequested),
    ],
};

/// No input at all: the loop running on its own, which is the shape a
/// dedicated server or a determinism harness has.
const IDLE: Trace = Trace {
    name: "idle",
    summary: "no input at all — the loop running on its own",
    events: &[],
};

/// Every trace this sample can replay.
pub const TRACES: &[Trace] = &[WALK, IDLE];

/// The trace by that name.
///
/// # Errors
///
/// [`SampleError::Usage`] naming every trace that does exist — a sample
/// that answers "no such trace" and stops is a sample nobody runs twice.
pub fn by_name(name: &str) -> Result<&'static Trace, SampleError> {
    TRACES
        .iter()
        .find(|trace| trace.name == name)
        .ok_or_else(|| SampleError::Usage(format!("no trace named `{name}`; {}", names())))
}

/// The traces, named and summarized, for a usage message.
#[must_use]
pub fn names() -> String {
    let listed: Vec<String> = TRACES
        .iter()
        .map(|trace| format!("{} ({})", trace.name, trace.summary))
        .collect();
    format!("available traces: {}", listed.join(", "))
}

#[cfg(test)]
mod tests {
    use super::{TRACES, by_name, names, press, release};
    use crate::error::SampleError;
    use renew_platform::window::{KeyCode, WindowEvent};

    /// The two shorthands the tables above are written with. They are
    /// const-evaluated into those tables, so nothing else ever executes
    /// them — and a shorthand that meant the opposite of its name would
    /// silently invert a whole trace.
    #[test]
    fn the_key_shorthands_mean_what_they_say() {
        assert_eq!(
            press(KeyCode::Space),
            WindowEvent::Key {
                code: KeyCode::Space,
                pressed: true,
                repeat: false
            }
        );
        assert_eq!(
            release(KeyCode::Space),
            WindowEvent::Key {
                code: KeyCode::Space,
                pressed: false,
                repeat: false
            }
        );
    }

    #[test]
    fn every_trace_is_findable_by_the_name_it_carries() {
        for trace in TRACES {
            let found = by_name(trace.name).expect("a listed trace");
            assert_eq!(found.name, trace.name);
            assert!(!found.summary.is_empty(), "{} has no summary", trace.name);
        }
    }

    #[test]
    fn an_unknown_trace_lists_the_ones_that_exist() {
        let error = by_name("moonwalk").expect_err("no such trace");
        assert!(matches!(error, SampleError::Usage(_)), "{error:?}");
        let message = error.to_string();
        assert!(message.contains("moonwalk"), "{message}");
        for trace in TRACES {
            assert!(message.contains(trace.name), "{message}");
        }
    }

    #[test]
    fn the_scripted_events_are_ordered_and_land_inside_a_short_run() {
        for trace in TRACES {
            let mut previous = 0;
            for (frame, _) in trace.events {
                assert!(*frame >= previous, "{} is out of order", trace.name);
                previous = *frame;
            }
        }
        assert!(names().contains("walk"));
    }
}
