//! The engine's event vocabulary: what happened, as plain data.
//!
//! The event enums, the list of every event shape, and the index
//! function over it — the enums counted by their declarations rather
//! than by a number here, which went stale the day a fourth arrived.
//! No dependencies, no operating system, nothing that can make
//! anything happen — a set of types describing an event, and only that.
//!
//! # Contract
//!
//! **This crate must never acquire a way to observe the outside world**,
//! and that obligation is the reason it exists as a crate rather than a
//! module. A crate promising its output depends only on build, seed and
//! input may depend on this one; adding a dependency here would defeat
//! that for every such crate at once, which is why the manifest declares
//! none and must keep declaring none.
//!
//! **How much of this is enforced, stated exactly.** The reverse edge —
//! this crate depending on the platform crate — is impossible: the
//! platform crate depends on this one, and a dependency cycle is
//! rejected. That is a real check. **The forward direction is checked
//! too**: the workspace structure check walks the dependency graph and
//! refuses any crate that promises determinism a path back to the
//! platform crate, at any depth, dev-dependencies included. Keeping this
//! crate dependency-free is what keeps that walk meaningful — every
//! crate reachable from a deterministic one inherits the prohibition.
//!
//! **The vocabulary used to live inside the platform crate**, beside the
//! clock, the filesystem and thread spawning, marked "deliberately not
//! behind the `window` feature". A feature gate is a convention the
//! dependency graph cannot see; a crate boundary is one it can.
//!
//! `renew-platform` re-exports this crate as its `event` module, so
//! every path a consumer already uses keeps working.

// Diagnostics are not this crate's job; the standard output macros are
// banned by construction, not convention.
// Simulation crates deny float arithmetic, and the rule is only worth
// anything if it holds across a simulation's whole dependency closure --
// a crate that computes floats is no less able to do so one edge away.
// This crate is in that closure (the input crate's vocabulary), so it
// carries the deny too, and does so without changing a line: nothing
// here is a float.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::float_arithmetic)]

/// The engine's event vocabulary, translated from the OS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowEvent {
    /// The user asked to close the window; the app decides what happens
    /// (typically by requesting exit through the loop's control handle).
    CloseRequested,
    Resized {
        width: u32,
        height: u32,
    },
    ScaleFactorChanged {
        scale: f64,
    },
    /// The OS wants the window drawn now. Render here, never elsewhere.
    RedrawRequested,
    Key {
        code: KeyCode,
        pressed: bool,
        repeat: bool,
    },
    /// The pointer moved by this much, in whatever units the platform
    /// reports.
    ///
    /// **Raw movement, not a position**, and the difference is what a
    /// first-person view needs. [`Self::PointerMoved`] carries where the
    /// cursor is inside the window: it stops at the edge, and stops
    /// entirely when the cursor leaves, so a view driven by it stops
    /// turning halfway through a turn. This carries how far the device
    /// moved and is unaffected by where any cursor is or whether one
    /// exists.
    ///
    /// A device-scoped event in a window-scoped enum, deliberately. This
    /// enum is an application's single input channel — [`Self::Key`] is
    /// already device-scoped — and a second enum would buy accuracy of
    /// naming at the cost of a second seam in every application.
    ///
    /// Not comparable with [`Self::PointerMoved`]'s coordinates: the
    /// platform may report these in a different scale entirely, and the
    /// only sound use is as a delta multiplied by a sensitivity.
    PointerMotion {
        /// Rightward movement.
        dx: f64,
        /// Downward movement.
        dy: f64,
    },
    PointerMoved {
        x: f64,
        y: f64,
    },
    PointerButton {
        button: PointerButton,
        pressed: bool,
    },
    Wheel {
        dx: f32,
        dy: f32,
    },
    Focused(bool),
    /// One character the window system says was typed.
    ///
    /// **A character, not a key**, and a separate event rather than a
    /// field on [`Self::Key`], because the two do not correspond. An
    /// input method commits text with no key behind it; a dead key
    /// produces a press with no text and then text on the next press; a
    /// held key repeats text. Which key produced a character — and
    /// whether shift, a layout or a composition was involved — is the
    /// window system's answer, and deriving it from a key code is wrong
    /// differently in every locale.
    ///
    /// **Control characters never arrive here.** Enter, Tab and
    /// Backspace are keys; the window system reports text for them too,
    /// and a field that inserted `\r` would hold bytes no reader can
    /// see. Editing intent travels as a key and is mapped by the driver.
    ///
    /// A `u32` rather than a `char` because a recording read back from
    /// disk has to be validated regardless, and the type that admits the
    /// invalid value is the honest one to carry across a seam that
    /// external data reaches.
    TextEntered {
        /// The Unicode scalar value typed.
        ch: u32,
    },
    /// A finger touched the screen, moved on it, or left it.
    ///
    /// **The finger is the identity.** Its id is unique for as long as
    /// that finger stays in contact and may be recycled afterwards —
    /// the platform's contract, restated here because two fingers with
    /// one id would make multi-touch unexpressible. Which finger is
    /// "the pointer" is a driver's decision, not this vocabulary's:
    /// the seam reports what happened and nothing is synthesized.
    ///
    /// [`TouchPhase::Cancelled`] is deliberately not `Ended`: the
    /// operating system stole the gesture (an edge swipe, a palm
    /// rejection), and a consumer that treats an interrupted press as
    /// a completed one must write that decision where it can be read.
    ///
    /// Coordinates follow [`Self::PointerMoved`]'s convention —
    /// physical position within the window. Pressure is deliberately
    /// absent: it is platform-variant payload with no consumer yet,
    /// and adding it later is additive where dropping it silently
    /// would not have been.
    Touch {
        /// Which finger, unique while it stays in contact.
        finger: u64,
        phase: TouchPhase,
        x: f64,
        y: f64,
    },
}

/// Where in its life a touch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TouchPhase {
    /// The finger arrived.
    Started,
    /// It moved while in contact.
    Moved,
    /// It left the screen; the gesture completed.
    Ended,
    /// The system took the gesture away; nothing completed.
    Cancelled,
}

/// Physical keys: the standard board a binding screen could name.
///
/// The vocabulary was consumer-driven — four letters and a handful of
/// editing keys — until recording fidelity argued for breadth: an
/// unmapped key arrives as [`KeyCode::Unidentified`], which replays
/// fine but has forgotten *which* key it was, so any key a binding
/// could plausibly claim belongs here before somebody records with it.
/// The numpad is deliberately still out; adding it later is one more
/// trace-format version. Unmapped keys lose nothing silently and
/// nothing panics.
// `Ord` so a consumer can keep these in a sorted table and binary
// search it. The order is declaration order, which is arbitrary but
// stable within a build — which is all a lookup key needs, and is
// what a hash map cannot offer, since its iteration order varies
// between runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyCode {
    Escape,
    Space,
    Enter,
    Tab,
    /// Remove the character before the cursor.
    Backspace,
    /// Remove the character at the cursor.
    Delete,
    /// Go to the start of the line.
    Home,
    /// Go to the end of the line.
    End,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    KeyW,
    KeyA,
    KeyS,
    KeyD,
    KeyB,
    KeyC,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyT,
    KeyU,
    KeyV,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    PageUp,
    PageDown,
    Insert,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    Backslash,
    Backquote,
    Unidentified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    /// A native button by its OS index — distinct from the named
    /// variants above; nothing is aliased.
    Other(u16),
}

/// One value of every shape [`WindowEvent`] can take.
///
/// **The vocabulary is exhaustive, so the compiler is the primary guard
/// and this list is the backstop.** It used to be the other way round.
/// These enums carried `#[non_exhaustive]`, which forced every downstream
/// match to write a wildcard — so a consumer translating events into a
/// file format or a replay log would have started silently dropping a new
/// variant, and nothing would have failed until someone noticed the
/// output was wrong. This slice and its test existed to move that failure
/// somewhere a compiler could still speak.
///
/// The attribute is gone (2026-08-04). Adding a variant now breaks the
/// build at every match that must handle it, in every crate, which is
/// what the list was approximating by hand. It was bought to protect
/// consumers outside this workspace; there are none, and the protection
/// cost the compiler's own report.
///
/// The list stays because it is still useful and no longer load-bearing:
/// a consumer iterating it gets a table-driven test of its own coverage,
/// and `shape_index` still names a shape in a refusal message. Neither
/// is now the only thing standing between a new variant and a silent
/// drop.
///
/// The values are arbitrary. Only the shapes matter.
pub const EVERY_EVENT_SHAPE: &[WindowEvent] = &[
    WindowEvent::CloseRequested,
    WindowEvent::Resized {
        width: 640,
        height: 360,
    },
    WindowEvent::ScaleFactorChanged { scale: 2.0 },
    WindowEvent::RedrawRequested,
    WindowEvent::Key {
        code: KeyCode::KeyW,
        pressed: true,
        repeat: false,
    },
    WindowEvent::PointerMoved { x: 12.5, y: 34.5 },
    WindowEvent::PointerMotion { dx: -3.5, dy: 1.25 },
    WindowEvent::PointerButton {
        button: PointerButton::Left,
        pressed: true,
    },
    WindowEvent::Wheel { dx: 0.0, dy: 1.0 },
    WindowEvent::Focused(true),
    WindowEvent::TextEntered { ch: 0x61 },
    WindowEvent::Touch {
        finger: 1,
        phase: TouchPhase::Started,
        x: 120.0,
        y: 96.0,
    },
];

/// Where a shape sits in [`EVERY_EVENT_SHAPE`].
///
/// This is the forcing function. The match is exhaustive and carries no
/// wildcard, so a new variant fails to compile until it is given an
/// index — and the test below fails until it is also added to the list.
/// Public because the consumer that iterates the list wants it: an index
/// is how a translation layer keys a coverage table, and how it names the
/// shape it is refusing.
///
/// One limit, stated rather than implied. The compile error forces the
/// *match* to be updated; the list beside it is still maintained by hand,
/// so a variant given an index and then left out of the list would pass
/// everything here. What that costs is bounded — a consumer's own test
/// iterates the list — and it is the reason this is a guard rather than
/// a proof.
#[must_use]
pub const fn shape_index(event: &WindowEvent) -> usize {
    match event {
        WindowEvent::CloseRequested => 0,
        WindowEvent::Resized { .. } => 1,
        WindowEvent::ScaleFactorChanged { .. } => 2,
        WindowEvent::RedrawRequested => 3,
        WindowEvent::Key { .. } => 4,
        WindowEvent::PointerMoved { .. } => 5,
        WindowEvent::PointerMotion { .. } => 6,
        WindowEvent::PointerButton { .. } => 7,
        WindowEvent::Wheel { .. } => 8,
        WindowEvent::Focused(_) => 9,
        WindowEvent::TextEntered { .. } => 10,
        WindowEvent::Touch { .. } => 11,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The list and the exhaustive match must agree, in both directions.
    ///
    /// Adding a variant already breaks the build at `shape_index`, which
    /// is what makes the match a forcing function. This closes the other
    /// half: without it, someone could give the new variant an index and
    /// never add it to the list, and every consumer iterating the list
    /// would keep silently missing it — the exact failure the list
    /// exists to prevent, reintroduced one step further along.
    #[test]
    fn every_shape_is_listed_exactly_once_and_in_index_order() {
        for (position, event) in EVERY_EVENT_SHAPE.iter().enumerate() {
            // No extra argument in the message: `assert_eq!` already
            // prints both sides, and an expression evaluated only on
            // failure is a region no passing run can cover.
            assert_eq!(shape_index(event), position, "misplaced: {event:?}");
        }
        // Every index below the length is covered, so no gap can hide a
        // variant that was indexed and then left out of the list.
        let mut seen = vec![false; EVERY_EVENT_SHAPE.len()];
        for event in EVERY_EVENT_SHAPE {
            seen[shape_index(event)] = true;
        }
        assert!(seen.iter().all(|covered| *covered), "{seen:?}");
    }
}
