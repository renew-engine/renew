//! Translating between the window seam's event vocabulary and the one
//! the trace format uses.
//!
//! The two vocabularies mirror each other deliberately, so almost every
//! arm here is a rename. That is the point: the codec owns its own
//! vocabulary and depends on nothing, so a change to the window seam
//! cannot silently change what a file on disk means. The cost of that
//! independence is this file, and it is a cost worth paying once.
//!
//! # Why encoding returns a result
//!
//! The window vocabulary is non-exhaustive, so the match below must
//! carry a wildcard arm and the compiler will never report a new variant
//! as unhandled here. The wildcard therefore **refuses** rather than
//! silently dropping: a recording that meets an event it cannot express
//! fails, instead of writing a file that is quietly missing events and
//! replays into a different world.
//!
//! The other half of that guarantee lives in the platform crate, which
//! publishes one value of every shape and an exhaustive match over them.
//! Adding a variant breaks the build there, and the test at the bottom of
//! this file then fails until this translation learns the new shape.

use renew_platform::window::{KeyCode, PointerButton, WindowEvent};
use renew_trace::{FiniteF32, FiniteF64, TraceButton, TraceEvent, TraceKey};

/// An event this build cannot write down.
///
/// Carries the shape index rather than the event, because the index is
/// what a reader can act on: it names the arm that needs adding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unencodable {
    pub shape: usize,
}

impl core::fmt::Display for Unencodable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "event shape {} has no trace encoding; recording would silently drop it",
            self.shape
        )
    }
}

const fn key_to_trace(code: KeyCode) -> TraceKey {
    match code {
        KeyCode::Escape => TraceKey::Escape,
        KeyCode::Space => TraceKey::Space,
        KeyCode::Enter => TraceKey::Enter,
        KeyCode::Tab => TraceKey::Tab,
        KeyCode::ArrowUp => TraceKey::ArrowUp,
        KeyCode::ArrowDown => TraceKey::ArrowDown,
        KeyCode::ArrowLeft => TraceKey::ArrowLeft,
        KeyCode::ArrowRight => TraceKey::ArrowRight,
        KeyCode::KeyW => TraceKey::KeyW,
        KeyCode::KeyA => TraceKey::KeyA,
        KeyCode::KeyS => TraceKey::KeyS,
        KeyCode::KeyD => TraceKey::KeyD,
        // Every other physical key already arrived here unidentified;
        // the trace records that faithfully rather than refusing, because
        // a key this build does not name is still input that happened.
        _ => TraceKey::Unidentified,
    }
}

const fn button_to_trace(button: PointerButton) -> TraceButton {
    match button {
        PointerButton::Left => TraceButton::Left,
        PointerButton::Right => TraceButton::Right,
        PointerButton::Middle => TraceButton::Middle,
        PointerButton::Back => TraceButton::Back,
        PointerButton::Forward => TraceButton::Forward,
        PointerButton::Other(index) => TraceButton::Other(index),
        _ => TraceButton::Other(u16::MAX),
    }
}

/// Encode one event, or refuse it by name.
///
/// # Errors
///
/// [`Unencodable`] when the event is a shape this translation does not
/// know — which can only happen after a new variant is added to the
/// window vocabulary and not added here.
pub fn to_trace(event: WindowEvent) -> Result<TraceEvent, Unencodable> {
    // Taken once, up front, so every refusal below names the same
    // shape the platform does and no arm carries a magic number.
    let shape = renew_platform::window::shape_index(&event);
    let encoded = match event {
        WindowEvent::CloseRequested => TraceEvent::CloseRequested,
        WindowEvent::RedrawRequested => TraceEvent::RedrawRequested,
        WindowEvent::Focused(focused) => TraceEvent::Focused(focused),
        WindowEvent::Resized { width, height } => TraceEvent::Resized { width, height },
        WindowEvent::Key {
            code,
            pressed,
            repeat,
        } => TraceEvent::Key {
            code: key_to_trace(code),
            pressed,
            repeat,
        },
        WindowEvent::PointerButton { button, pressed } => TraceEvent::PointerButton {
            button: button_to_trace(button),
            pressed,
        },
        // The float-bearing shapes are the only ones that can fail on
        // their payload rather than their kind: the trace format holds
        // finite values only, so a non-finite coordinate is refused here
        // instead of becoming a bit pattern nothing can compare.
        WindowEvent::PointerMoved { x, y } => match (FiniteF64::new(x), FiniteF64::new(y)) {
            (Some(x), Some(y)) => TraceEvent::PointerMoved { x, y },
            _ => return Err(Unencodable { shape }),
        },
        WindowEvent::Wheel { dx, dy } => match (FiniteF32::new(dx), FiniteF32::new(dy)) {
            (Some(dx), Some(dy)) => TraceEvent::Wheel { dx, dy },
            _ => return Err(Unencodable { shape }),
        },
        WindowEvent::ScaleFactorChanged { scale } => match FiniteF64::new(scale) {
            Some(scale) => TraceEvent::ScaleFactorChanged { scale },
            None => return Err(Unencodable { shape }),
        },
        // Never `=> {}`. A wildcard that drops is how a recording starts
        // lying about what happened.
        _ => return Err(Unencodable { shape }),
    };
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::{Unencodable, to_trace};
    use renew_platform::window::{EVERY_EVENT_SHAPE, WindowEvent, shape_index};

    /// Every shape the window seam can produce has an encoding.
    ///
    /// This is the test the shape list exists for. The compiler already
    /// refuses to build the platform crate when a variant is added
    /// without an index; this refuses to build a *recording* that would
    /// have dropped it. Together they turn "did we remember?" into a
    /// question the build answers.
    #[test]
    fn every_window_event_shape_can_be_written_down() {
        for event in EVERY_EVENT_SHAPE {
            assert!(
                to_trace(*event).is_ok(),
                "shape {} has no encoding: {event:?}",
                shape_index(event)
            );
        }
    }

    /// A non-finite payload is refused rather than encoded, because the
    /// format carries finite values only and a NaN written as a bit
    /// pattern is a value nothing downstream can compare.
    #[test]
    fn a_non_finite_coordinate_is_refused_and_names_its_shape() {
        let event = WindowEvent::PointerMoved {
            x: f64::NAN,
            y: 0.0,
        };
        assert_eq!(
            to_trace(event),
            Err(Unencodable {
                shape: shape_index(&event)
            })
        );

        let wheel = WindowEvent::Wheel {
            dx: 0.0,
            dy: f32::INFINITY,
        };
        assert_eq!(
            to_trace(wheel),
            Err(Unencodable {
                shape: shape_index(&wheel)
            })
        );
    }

    /// The refusal says what a reader can act on.
    #[test]
    fn the_refusal_names_the_shape_in_its_message() {
        let shown = Unencodable { shape: 5 }.to_string();
        assert!(shown.contains('5'), "{shown}");
        assert!(shown.contains("drop"), "{shown}");
    }
}
