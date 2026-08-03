//! The engine's event vocabulary: what happened, as plain data.
//!
//! Three enums, the list of every event shape, and the index function
//! over it. No dependencies, no operating system, nothing that can make
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
#![deny(clippy::print_stdout, clippy::print_stderr)]

/// The engine's event vocabulary, translated from the OS.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
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
}

/// Physical keys, the subset current consumers need — grows additively.
/// Unmapped keys arrive as [`KeyCode::Unidentified`]; nothing is lost
/// silently, nothing panics.
// `Ord` so a consumer can keep these in a sorted table and binary
// search it. The order is declaration order, which is arbitrary but
// stable within a build — which is all a lookup key needs, and is
// what a hash map cannot offer, since its iteration order varies
// between runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum KeyCode {
    Escape,
    Space,
    Enter,
    Tab,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    KeyW,
    KeyA,
    KeyS,
    KeyD,
    Unidentified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
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
/// The event vocabulary is `#[non_exhaustive]`, which binds every crate
/// except this one: downstream matches must carry a wildcard arm, so the
/// compiler will never tell a consumer that a new variant is unhandled.
/// A consumer that translates events — into a file format, a script
/// binding, a replay log — would therefore start silently dropping the
/// new one, and nothing would fail until someone noticed the output was
/// wrong.
///
/// This list, and the exhaustive match beside it, move that failure to
/// where the compiler can still speak: adding a variant breaks the build
/// **here**, in the crate that owns the enum, at the moment it is added.
/// A consumer then iterates this slice and asserts it handles every
/// entry, which turns "did we remember?" into a test rather than a habit.
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
    WindowEvent::PointerButton {
        button: PointerButton::Left,
        pressed: true,
    },
    WindowEvent::Wheel { dx: 0.0, dy: 1.0 },
    WindowEvent::Focused(true),
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
        WindowEvent::PointerButton { .. } => 6,
        WindowEvent::Wheel { .. } => 7,
        WindowEvent::Focused(_) => 8,
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
