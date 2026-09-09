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
//! The other half of that guarantee lives in the event vocabulary crate,
//! which publishes one value of every shape and an exhaustive match over
//! them. It used to live in the platform crate; the vocabulary moved out
//! into a crate of its own, and the forcing function moved with it —
//! deliberately, because splitting the two would have meant adding a
//! wildcard arm to that match, which compiles green and silently ends
//! the guarantee this file depends on.
//! Adding a variant breaks the build there, and the test at the bottom of
//! this file then fails until this translation learns the new shape.

use renew_event::{KeyCode, PointerButton, TouchPhase, WindowEvent};
use renew_trace::{FiniteF32, FiniteF64, TraceButton, TraceEvent, TraceKey, TraceTouchPhase};

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
        KeyCode::Backspace => TraceKey::Backspace,
        KeyCode::Delete => TraceKey::Delete,
        KeyCode::Home => TraceKey::Home,
        KeyCode::End => TraceKey::End,
        KeyCode::ArrowUp => TraceKey::ArrowUp,
        KeyCode::ArrowDown => TraceKey::ArrowDown,
        KeyCode::ArrowLeft => TraceKey::ArrowLeft,
        KeyCode::ArrowRight => TraceKey::ArrowRight,
        KeyCode::KeyW => TraceKey::KeyW,
        KeyCode::KeyA => TraceKey::KeyA,
        KeyCode::KeyS => TraceKey::KeyS,
        KeyCode::KeyD => TraceKey::KeyD,
        KeyCode::KeyB => TraceKey::KeyB,
        KeyCode::KeyC => TraceKey::KeyC,
        KeyCode::KeyE => TraceKey::KeyE,
        KeyCode::KeyF => TraceKey::KeyF,
        KeyCode::KeyG => TraceKey::KeyG,
        KeyCode::KeyH => TraceKey::KeyH,
        KeyCode::KeyI => TraceKey::KeyI,
        KeyCode::KeyJ => TraceKey::KeyJ,
        KeyCode::KeyK => TraceKey::KeyK,
        KeyCode::KeyL => TraceKey::KeyL,
        KeyCode::KeyM => TraceKey::KeyM,
        KeyCode::KeyN => TraceKey::KeyN,
        KeyCode::KeyO => TraceKey::KeyO,
        KeyCode::KeyP => TraceKey::KeyP,
        KeyCode::KeyQ => TraceKey::KeyQ,
        KeyCode::KeyR => TraceKey::KeyR,
        KeyCode::KeyT => TraceKey::KeyT,
        KeyCode::KeyU => TraceKey::KeyU,
        KeyCode::KeyV => TraceKey::KeyV,
        KeyCode::KeyX => TraceKey::KeyX,
        KeyCode::KeyY => TraceKey::KeyY,
        KeyCode::KeyZ => TraceKey::KeyZ,
        KeyCode::Digit0 => TraceKey::Digit0,
        KeyCode::Digit1 => TraceKey::Digit1,
        KeyCode::Digit2 => TraceKey::Digit2,
        KeyCode::Digit3 => TraceKey::Digit3,
        KeyCode::Digit4 => TraceKey::Digit4,
        KeyCode::Digit5 => TraceKey::Digit5,
        KeyCode::Digit6 => TraceKey::Digit6,
        KeyCode::Digit7 => TraceKey::Digit7,
        KeyCode::Digit8 => TraceKey::Digit8,
        KeyCode::Digit9 => TraceKey::Digit9,
        KeyCode::F1 => TraceKey::F1,
        KeyCode::F2 => TraceKey::F2,
        KeyCode::F3 => TraceKey::F3,
        KeyCode::F4 => TraceKey::F4,
        KeyCode::F5 => TraceKey::F5,
        KeyCode::F6 => TraceKey::F6,
        KeyCode::F7 => TraceKey::F7,
        KeyCode::F8 => TraceKey::F8,
        KeyCode::F9 => TraceKey::F9,
        KeyCode::F10 => TraceKey::F10,
        KeyCode::F11 => TraceKey::F11,
        KeyCode::F12 => TraceKey::F12,
        KeyCode::ShiftLeft => TraceKey::ShiftLeft,
        KeyCode::ShiftRight => TraceKey::ShiftRight,
        KeyCode::ControlLeft => TraceKey::ControlLeft,
        KeyCode::ControlRight => TraceKey::ControlRight,
        KeyCode::AltLeft => TraceKey::AltLeft,
        KeyCode::AltRight => TraceKey::AltRight,
        KeyCode::PageUp => TraceKey::PageUp,
        KeyCode::PageDown => TraceKey::PageDown,
        KeyCode::Insert => TraceKey::Insert,
        KeyCode::Minus => TraceKey::Minus,
        KeyCode::Equal => TraceKey::Equal,
        KeyCode::BracketLeft => TraceKey::BracketLeft,
        KeyCode::BracketRight => TraceKey::BracketRight,
        KeyCode::Semicolon => TraceKey::Semicolon,
        KeyCode::Quote => TraceKey::Quote,
        KeyCode::Comma => TraceKey::Comma,
        KeyCode::Period => TraceKey::Period,
        KeyCode::Slash => TraceKey::Slash,
        KeyCode::Backslash => TraceKey::Backslash,
        KeyCode::Backquote => TraceKey::Backquote,
        // A key this build does not name is still input that happened,
        // so the trace records it faithfully rather than refusing. Named
        // rather than wildcarded: the vocabulary is exhaustive, so a new
        // key must arrive here as a compile error and be decided on, not
        // fall into this arm because it was the only one left.
        KeyCode::Unidentified => TraceKey::Unidentified,
    }
}

/// Every button this build knows, which since the vocabulary became
/// exhaustive is every button there is.
///
/// This returned `Option` while the vocabulary was non-exhaustive, so the
/// required wildcard could **refuse** rather than fold an unknown button
/// into `Other` — a silent fold would have been the writer-side drop this
/// module exists to prevent. The refusal is now the compiler's: a new
/// button breaks this match, here, and somebody decides what it records
/// as. The `Option` went with the wildcard, because a `None` no caller
/// can receive is a branch every caller still has to write.
const fn button_to_trace(button: PointerButton) -> TraceButton {
    match button {
        PointerButton::Left => TraceButton::Left,
        PointerButton::Right => TraceButton::Right,
        PointerButton::Middle => TraceButton::Middle,
        PointerButton::Back => TraceButton::Back,
        PointerButton::Forward => TraceButton::Forward,
        PointerButton::Other(index) => TraceButton::Other(index),
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
    let shape = renew_event::shape_index(&event);
    let encoded = match event {
        WindowEvent::CloseRequested => TraceEvent::CloseRequested,
        WindowEvent::RedrawRequested => TraceEvent::RedrawRequested,
        WindowEvent::Focused(focused) => TraceEvent::Focused(focused),
        // The one shape whose payload can be refused for not being a
        // value at all: the window vocabulary carries a `u32` so a
        // driver forwards without converting, and a trace holds only
        // scalars, so a surrogate or an out-of-range code point is
        // refused here rather than recorded as something no reader can
        // turn back into a character.
        WindowEvent::TextEntered { ch } => match char::from_u32(ch) {
            Some(_) => TraceEvent::TextEntered { ch },
            None => return Err(Unencodable { shape }),
        },
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
        WindowEvent::PointerMotion { dx, dy } => match (FiniteF64::new(dx), FiniteF64::new(dy)) {
            (Some(dx), Some(dy)) => TraceEvent::PointerMotion { dx, dy },
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
        WindowEvent::Touch {
            finger,
            phase,
            x,
            y,
        } => match (FiniteF64::new(x), FiniteF64::new(y)) {
            (Some(x), Some(y)) => TraceEvent::Touch {
                finger,
                phase: phase_to_trace(phase),
                x,
                y,
            },
            _ => return Err(Unencodable { shape }),
        },
    };
    Ok(encoded)
}

const fn phase_to_trace(phase: TouchPhase) -> TraceTouchPhase {
    match phase {
        TouchPhase::Started => TraceTouchPhase::Started,
        TouchPhase::Moved => TraceTouchPhase::Moved,
        TouchPhase::Ended => TraceTouchPhase::Ended,
        TouchPhase::Cancelled => TraceTouchPhase::Cancelled,
    }
}

const fn phase_from_trace(phase: TraceTouchPhase) -> TouchPhase {
    match phase {
        TraceTouchPhase::Started => TouchPhase::Started,
        TraceTouchPhase::Moved => TouchPhase::Moved,
        TraceTouchPhase::Ended => TouchPhase::Ended,
        TraceTouchPhase::Cancelled => TouchPhase::Cancelled,
    }
}

const fn key_from_trace(code: TraceKey) -> KeyCode {
    match code {
        TraceKey::Escape => KeyCode::Escape,
        TraceKey::Space => KeyCode::Space,
        TraceKey::Enter => KeyCode::Enter,
        TraceKey::Tab => KeyCode::Tab,
        TraceKey::Backspace => KeyCode::Backspace,
        TraceKey::Delete => KeyCode::Delete,
        TraceKey::Home => KeyCode::Home,
        TraceKey::End => KeyCode::End,
        TraceKey::ArrowUp => KeyCode::ArrowUp,
        TraceKey::ArrowDown => KeyCode::ArrowDown,
        TraceKey::ArrowLeft => KeyCode::ArrowLeft,
        TraceKey::ArrowRight => KeyCode::ArrowRight,
        TraceKey::KeyW => KeyCode::KeyW,
        TraceKey::KeyA => KeyCode::KeyA,
        TraceKey::KeyS => KeyCode::KeyS,
        TraceKey::KeyD => KeyCode::KeyD,
        TraceKey::KeyB => KeyCode::KeyB,
        TraceKey::KeyC => KeyCode::KeyC,
        TraceKey::KeyE => KeyCode::KeyE,
        TraceKey::KeyF => KeyCode::KeyF,
        TraceKey::KeyG => KeyCode::KeyG,
        TraceKey::KeyH => KeyCode::KeyH,
        TraceKey::KeyI => KeyCode::KeyI,
        TraceKey::KeyJ => KeyCode::KeyJ,
        TraceKey::KeyK => KeyCode::KeyK,
        TraceKey::KeyL => KeyCode::KeyL,
        TraceKey::KeyM => KeyCode::KeyM,
        TraceKey::KeyN => KeyCode::KeyN,
        TraceKey::KeyO => KeyCode::KeyO,
        TraceKey::KeyP => KeyCode::KeyP,
        TraceKey::KeyQ => KeyCode::KeyQ,
        TraceKey::KeyR => KeyCode::KeyR,
        TraceKey::KeyT => KeyCode::KeyT,
        TraceKey::KeyU => KeyCode::KeyU,
        TraceKey::KeyV => KeyCode::KeyV,
        TraceKey::KeyX => KeyCode::KeyX,
        TraceKey::KeyY => KeyCode::KeyY,
        TraceKey::KeyZ => KeyCode::KeyZ,
        TraceKey::Digit0 => KeyCode::Digit0,
        TraceKey::Digit1 => KeyCode::Digit1,
        TraceKey::Digit2 => KeyCode::Digit2,
        TraceKey::Digit3 => KeyCode::Digit3,
        TraceKey::Digit4 => KeyCode::Digit4,
        TraceKey::Digit5 => KeyCode::Digit5,
        TraceKey::Digit6 => KeyCode::Digit6,
        TraceKey::Digit7 => KeyCode::Digit7,
        TraceKey::Digit8 => KeyCode::Digit8,
        TraceKey::Digit9 => KeyCode::Digit9,
        TraceKey::F1 => KeyCode::F1,
        TraceKey::F2 => KeyCode::F2,
        TraceKey::F3 => KeyCode::F3,
        TraceKey::F4 => KeyCode::F4,
        TraceKey::F5 => KeyCode::F5,
        TraceKey::F6 => KeyCode::F6,
        TraceKey::F7 => KeyCode::F7,
        TraceKey::F8 => KeyCode::F8,
        TraceKey::F9 => KeyCode::F9,
        TraceKey::F10 => KeyCode::F10,
        TraceKey::F11 => KeyCode::F11,
        TraceKey::F12 => KeyCode::F12,
        TraceKey::ShiftLeft => KeyCode::ShiftLeft,
        TraceKey::ShiftRight => KeyCode::ShiftRight,
        TraceKey::ControlLeft => KeyCode::ControlLeft,
        TraceKey::ControlRight => KeyCode::ControlRight,
        TraceKey::AltLeft => KeyCode::AltLeft,
        TraceKey::AltRight => KeyCode::AltRight,
        TraceKey::PageUp => KeyCode::PageUp,
        TraceKey::PageDown => KeyCode::PageDown,
        TraceKey::Insert => KeyCode::Insert,
        TraceKey::Minus => KeyCode::Minus,
        TraceKey::Equal => KeyCode::Equal,
        TraceKey::BracketLeft => KeyCode::BracketLeft,
        TraceKey::BracketRight => KeyCode::BracketRight,
        TraceKey::Semicolon => KeyCode::Semicolon,
        TraceKey::Quote => KeyCode::Quote,
        TraceKey::Comma => KeyCode::Comma,
        TraceKey::Period => KeyCode::Period,
        TraceKey::Slash => KeyCode::Slash,
        TraceKey::Backslash => KeyCode::Backslash,
        TraceKey::Backquote => KeyCode::Backquote,
        TraceKey::Unidentified => KeyCode::Unidentified,
    }
}

const fn button_from_trace(button: TraceButton) -> PointerButton {
    match button {
        TraceButton::Left => PointerButton::Left,
        TraceButton::Right => PointerButton::Right,
        TraceButton::Middle => PointerButton::Middle,
        TraceButton::Back => PointerButton::Back,
        TraceButton::Forward => PointerButton::Forward,
        TraceButton::Other(index) => PointerButton::Other(index),
    }
}

/// Decode one event.
///
/// Total, and it needs no result: the trace vocabulary is this tree's own
/// and every one of its shapes has a window event to become. That is the
/// asymmetry the two directions are supposed to have — encoding can meet
/// a window event the format has no word for, decoding cannot meet a word
/// the format did not define.
#[must_use]
pub const fn from_trace(event: TraceEvent) -> WindowEvent {
    match event {
        TraceEvent::CloseRequested => WindowEvent::CloseRequested,
        TraceEvent::RedrawRequested => WindowEvent::RedrawRequested,
        TraceEvent::Focused(focused) => WindowEvent::Focused(focused),
        TraceEvent::TextEntered { ch } => WindowEvent::TextEntered { ch },
        TraceEvent::Resized { width, height } => WindowEvent::Resized { width, height },
        TraceEvent::Key {
            code,
            pressed,
            repeat,
        } => WindowEvent::Key {
            code: key_from_trace(code),
            pressed,
            repeat,
        },
        TraceEvent::PointerButton { button, pressed } => WindowEvent::PointerButton {
            button: button_from_trace(button),
            pressed,
        },
        TraceEvent::PointerMotion { dx, dy } => WindowEvent::PointerMotion {
            dx: dx.value(),
            dy: dy.value(),
        },
        TraceEvent::PointerMoved { x, y } => WindowEvent::PointerMoved {
            x: x.value(),
            y: y.value(),
        },
        TraceEvent::Wheel { dx, dy } => WindowEvent::Wheel {
            dx: dx.value(),
            dy: dy.value(),
        },
        TraceEvent::ScaleFactorChanged { scale } => WindowEvent::ScaleFactorChanged {
            scale: scale.value(),
        },
        TraceEvent::Touch {
            finger,
            phase,
            x,
            y,
        } => WindowEvent::Touch {
            finger,
            phase: phase_from_trace(phase),
            x: x.value(),
            y: y.value(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Unencodable, from_trace, to_trace};
    use renew_event::{EVERY_EVENT_SHAPE, PointerButton, WindowEvent, shape_index};

    /// **Raw pointer motion round-trips like everything else.** The
    /// format gained a token of its own for it rather than storing a
    /// delta under the position's: a recording that confused the two
    /// would replay as a cursor teleporting to the origin.
    #[test]
    fn raw_pointer_motion_survives_the_round_trip() {
        let motion = WindowEvent::PointerMotion { dx: 1.5, dy: -2.0 };
        let encoded = to_trace(motion).expect("motion encodes");
        assert_eq!(from_trace(encoded), motion);
    }

    /// A delta that is not a number is refused, like every other
    /// float-bearing shape: the format holds finite values only.
    #[test]
    fn a_motion_that_is_not_a_number_is_refused() {
        let refused = to_trace(WindowEvent::PointerMotion {
            dx: f64::NAN,
            dy: 0.0,
        });
        assert!(
            matches!(refused, Err(Unencodable { .. })),
            "got {refused:?}"
        );
    }

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
            assert!(to_trace(*event).is_ok(), "no encoding: {event:?}");
        }
    }

    /// Every shape survives the round trip through the trace vocabulary.
    ///
    /// The shape test above proves each one can be written down. This
    /// proves writing it down loses nothing — which is the property a
    /// replay depends on and the one a rename would quietly break, since
    /// two vocabularies that mirror each other are exactly the kind of
    /// thing that drifts one arm at a time.
    /// A value the window vocabulary admits and the trace format does
    /// not cannot be recorded.
    ///
    /// The event carries a `u32` so a driver forwards without
    /// converting, which means it can carry a surrogate. Recording one
    /// would put a value in a trace that no reader can turn back into a
    /// character.
    #[test]
    fn a_code_point_that_is_not_a_character_is_refused() {
        // The window vocabulary carries a `u32` so a driver forwards
        // without converting, which means it can carry a surrogate. A
        // trace holds scalars, so this is where that is caught — a
        // recording of a value no reader can turn back into a character
        // is worse than a refusal.
        for ch in [0xD800, 0xDFFF, 0x11_0000, u32::MAX] {
            assert!(
                to_trace(WindowEvent::TextEntered { ch }).is_err(),
                "{ch:#x} is not a scalar and must not record"
            );
        }
        assert!(to_trace(WindowEvent::TextEntered { ch: 0x1F642 }).is_ok());
    }

    /// Every shape in the vocabulary survives encode and decode.
    #[test]
    fn every_shape_survives_the_round_trip_unchanged() {
        for event in EVERY_EVENT_SHAPE {
            let encoded = to_trace(*event).expect("every shape encodes");
            assert_eq!(
                super::from_trace(encoded),
                *event,
                "changed in the round trip: {event:?}"
            );
        }
    }

    /// Every key and every button survives the round trip.
    ///
    /// The shape list carries one key and one button, which is all it
    /// needs to prove each *shape* is encodable — but it leaves every
    /// other key arm and button arm unexercised, and a rename in either
    /// of the two mirrored vocabularies would go unnoticed. Naming them
    /// here is the only way to cover the mapping rather than the shape.
    #[test]
    fn every_key_and_button_name_maps_back_to_itself() {
        const BUTTONS: [PointerButton; 6] = [
            PointerButton::Left,
            PointerButton::Right,
            PointerButton::Middle,
            PointerButton::Back,
            PointerButton::Forward,
            PointerButton::Other(9),
        ];

        // Walked from the trace side's own table, which names all 81:
        // key_from_trace then key_to_trace crosses both mirrored
        // vocabularies, so a transposition in either direction — the
        // widening's biggest risk — breaks the identity below. A local
        // list would be a third copy that could fall behind, which is
        // exactly what happened when this test still named 17 of 81.
        let keys = renew_trace::TraceKey::ALL
            .iter()
            .map(|key| super::key_from_trace(*key));
        for code in keys {
            let event = WindowEvent::Key {
                code,
                pressed: true,
                repeat: false,
            };
            let encoded = to_trace(event).expect("every named key encodes");
            assert_eq!(
                super::from_trace(encoded),
                event,
                "{code:?} did not survive"
            );
        }

        for button in BUTTONS {
            let event = WindowEvent::PointerButton {
                button,
                pressed: false,
            };
            let encoded = to_trace(event).expect("every named button encodes");
            assert_eq!(
                super::from_trace(encoded),
                event,
                "{button:?} did not survive"
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

        // The third float-bearing shape, so every one of them is pinned.
        let scale = WindowEvent::ScaleFactorChanged { scale: f64::NAN };
        assert_eq!(
            to_trace(scale),
            Err(Unencodable {
                shape: shape_index(&scale)
            })
        );

        // The fourth: a touch with a coordinate that is not a number.
        let touch = WindowEvent::Touch {
            finger: 1,
            phase: renew_event::TouchPhase::Moved,
            x: f64::NAN,
            y: 0.0,
        };
        assert_eq!(
            to_trace(touch),
            Err(Unencodable {
                shape: shape_index(&touch)
            })
        );
    }

    /// Every phase survives the round trip, and finger identity rides
    /// along untouched — the shape list carries one phase, which proves
    /// the shape and leaves three arms of the mirrored tables unread.
    #[test]
    fn every_touch_phase_maps_back_to_itself() {
        use renew_event::TouchPhase;
        for phase in [
            TouchPhase::Started,
            TouchPhase::Moved,
            TouchPhase::Ended,
            TouchPhase::Cancelled,
        ] {
            let event = WindowEvent::Touch {
                finger: u64::MAX,
                phase,
                x: 4.5,
                y: -0.0,
            };
            let encoded = to_trace(event).expect("every phase encodes");
            let decoded = from_trace(encoded);
            assert_eq!(decoded, event, "{phase:?} did not survive");
            // Float equality is sign-blind at zero, so the assertion
            // above would pass an implementation that lost the sign.
            // Crossing back into the trace vocabulary compares bit
            // patterns — its equality is on the bits by construction —
            // so a stripped sign on any field turns this red, with no
            // branch a test cannot take.
            let re_encoded = to_trace(decoded).expect("the decoded event re-encodes");
            assert_eq!(
                re_encoded, encoded,
                "{phase:?}: a bit changed crossing back"
            );
        }
    }

    /// The refusal says what a reader can act on.
    #[test]
    fn the_refusal_names_the_shape_in_its_message() {
        let shown = Unencodable { shape: 5 }.to_string();
        assert!(shown.contains('5'), "{shown}");
        assert!(shown.contains("drop"), "{shown}");
    }
}
