//! The event vocabulary: what a trace can say happened, spelled in this
//! crate's own words.
//!
//! The vocabulary is owned here rather than borrowed from the windowing
//! layer, and that is the whole reason this crate depends on nothing. A
//! codec that named the windowing crate's event enum would pull that
//! crate — and everything it links, an entire windowing stack — into
//! every build that merely wants to read a file, headless ones included.
//! It would also make the meaning of a recorded file hostage to that
//! enum's growth: a variant added upstream would quietly change what an
//! already-written trace means. Owning the words means a new upstream
//! variant changes only what the *conversion* in the application must
//! handle, which is one compile error in one place.
//!
//! Floats are carried as their IEEE-754 bit patterns, never as decimal
//! text. A float value has two zeros and no equality for `NaN`, and
//! decimal text does not survive a round trip through a parser without
//! care; a bit pattern is an integer, compares exactly, and is what a
//! determinism digest absorbs anyway.

use crate::grammar::OTHER_BUTTON;
use core::fmt;

/// A finite `f64`, carried as its IEEE-754 bit pattern.
///
/// The type exists so that a trace cannot be built which the reader would
/// then refuse: an infinity or a `NaN` is rejected here, at construction,
/// instead of at the moment someone reads the file back. That is what lets
/// the writer be infallible and makes writing and reading exact inverses.
///
/// Equality is on the bit pattern, which is the comparison a byte-exact
/// format needs: positive and negative zero are different traces, because
/// they are different bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FiniteF64(u64);

/// A finite `f32`, carried as its IEEE-754 bit pattern. The `f64` type's
/// reasoning applies unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FiniteF32(u32);

/// Every bit pattern whose exponent field is all ones is an infinity or a
/// `NaN`; everything else is finite. The test is written on the integer so
/// that no float arithmetic — and so no rounding, no signalling `NaN`, and
/// nothing a compiler flag could relax — takes part in deciding what a
/// file is allowed to contain.
const F64_EXPONENT: u64 = 0x7ff0_0000_0000_0000;
const F32_EXPONENT: u32 = 0x7f80_0000;

impl FiniteF64 {
    /// How many hexadecimal digits the text form carries, after `0x`.
    /// Exactly the width of the type: a shorter or longer field is a
    /// different number silently, so the width is fixed rather than
    /// inferred.
    pub const HEX_DIGITS: usize = 16;

    /// The value's bit pattern, or `None` if it is not finite.
    #[must_use]
    pub const fn new(value: f64) -> Option<Self> {
        Self::from_bits(value.to_bits())
    }

    /// The pattern itself, or `None` if it names an infinity or a `NaN`.
    #[must_use]
    pub const fn from_bits(bits: u64) -> Option<Self> {
        if bits & F64_EXPONENT == F64_EXPONENT {
            None
        } else {
            Some(Self(bits))
        }
    }

    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn value(self) -> f64 {
        f64::from_bits(self.0)
    }
}

impl FiniteF32 {
    /// How many hexadecimal digits the text form carries, after `0x`.
    pub const HEX_DIGITS: usize = 8;

    /// The value's bit pattern, or `None` if it is not finite.
    #[must_use]
    pub const fn new(value: f32) -> Option<Self> {
        Self::from_bits(value.to_bits())
    }

    /// The pattern itself, or `None` if it names an infinity or a `NaN`.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Option<Self> {
        if bits & F32_EXPONENT == F32_EXPONENT {
            None
        } else {
            Some(Self(bits))
        }
    }

    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn value(self) -> f32 {
        f32::from_bits(self.0)
    }
}

/// A physical key, by the name the format writes it under.
///
/// The set is closed on purpose. It is deliberately *not* marked as open
/// to extension: a downstream conversion should stop compiling the day a
/// key is added, which is the only moment anyone can still decide what
/// the new key is called in a file.
///
/// Adding a name here bumps [`FORMAT_VERSION`](crate::FORMAT_VERSION).
/// A name is a word, this format refuses words it does not know rather
/// than skipping them, and so a file using a new one is a file no reader
/// already in the world can read — which is what a version number is
/// for.
///
/// [`TraceKey::Unidentified`] is a first-class name, not a refusal. It is
/// what the windowing seam produces for every physical key outside the
/// mapped set, so treating it as unencodable would abort a recording the
/// first time someone pressed Shift — during exactly the session the
/// recording exists to capture. What is genuinely unrecoverable is *which*
/// physical key it was; the event itself records and replays fine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceKey {
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
    /// A physical key outside the mapped set. Encodable, replayable, and
    /// honest about what it does not know.
    Unidentified,
}

impl TraceKey {
    /// Every key, in the order the format documents them.
    ///
    /// The parser reads this table rather than carrying a second one, so
    /// the name a key is written under and the name it is read back from
    /// are the same string in the same place. Adding a variant is a
    /// compile error in [`TraceKey::name`]; adding it here as well is the
    /// author's job, and the count test in this module is what notices.
    pub const ALL: &'static [Self] = &[
        Self::Escape,
        Self::Space,
        Self::Enter,
        Self::Tab,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::ArrowLeft,
        Self::ArrowRight,
        Self::KeyW,
        Self::KeyA,
        Self::KeyS,
        Self::KeyD,
        Self::Unidentified,
    ];

    /// The text this key is written as. Lowercase, hyphenated, and never
    /// a bare `up` or `down`, which are the words a key line already uses
    /// for its state.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Escape => "escape",
            Self::Space => "space",
            Self::Enter => "enter",
            Self::Tab => "tab",
            Self::ArrowUp => "arrow-up",
            Self::ArrowDown => "arrow-down",
            Self::ArrowLeft => "arrow-left",
            Self::ArrowRight => "arrow-right",
            Self::KeyW => "key-w",
            Self::KeyA => "key-a",
            Self::KeyS => "key-s",
            Self::KeyD => "key-d",
            Self::Unidentified => "unidentified",
        }
    }

    /// The key written under this name, or `None` if no key is.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|key| key.name() == name)
    }
}

/// A pointer button: the five named ones, or a native button by its index.
///
/// Closed for the same reason as [`TraceKey`], and adding a name here
/// bumps [`FORMAT_VERSION`](crate::FORMAT_VERSION) for the same reason
/// too. A native button needs no new name — it is written by its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    /// A native button by its operating-system index — distinct from the
    /// named variants above, and never aliased onto one.
    Other(u16),
}

impl TraceButton {
    /// The five named buttons, in the order the format documents them.
    pub const NAMED: &'static [Self] = &[
        Self::Left,
        Self::Right,
        Self::Middle,
        Self::Back,
        Self::Forward,
    ];

    /// The button written under this name, or `None` if no *named* button
    /// is. A native index is spelled `other:<index>` and is read by the
    /// parser, which owns the number.
    ///
    /// The comparison goes through the writer's own text rather than a
    /// second table. The allocation is deliberate and paid on a handful of
    /// lines per file: it buys the guarantee that a button reads back
    /// exactly as it was written, because there is only one place where
    /// the two could disagree and it is the same place.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::NAMED
            .iter()
            .copied()
            .find(|button| button.to_string() == name)
    }
}

impl fmt::Display for TraceButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Left => f.write_str("left"),
            Self::Right => f.write_str("right"),
            Self::Middle => f.write_str("middle"),
            Self::Back => f.write_str("back"),
            Self::Forward => f.write_str("forward"),
            Self::Other(index) => write!(f, "{OTHER_BUTTON}{index}"),
        }
    }
}

/// One thing that happened, in the codec's own vocabulary.
///
/// Nine variants for the nine lines the format defines, and the names
/// match the windowing layer's event names so that the conversion in an
/// application reads as a rename rather than as a translation. Plain data
/// throughout: no method here interprets an event, because what an event
/// *means* belongs to the application replaying it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    Key {
        code: TraceKey,
        pressed: bool,
        /// The operating system's auto-repeat, preserved rather than
        /// filtered: whether a repeat counts is the application's rule.
        repeat: bool,
    },
    PointerMoved {
        x: FiniteF64,
        y: FiniteF64,
    },
    PointerButton {
        button: TraceButton,
        pressed: bool,
    },
    Wheel {
        dx: FiniteF32,
        dy: FiniteF32,
    },
    Focused(bool),
    Resized {
        width: u32,
        height: u32,
    },
    ScaleFactorChanged {
        scale: FiniteF64,
    },
    RedrawRequested,
    CloseRequested,
}

#[cfg(test)]
mod tests {
    use super::{FiniteF32, FiniteF64, TraceButton, TraceEvent, TraceKey};

    /// Values are compared as bit patterns here, never with `==`: this
    /// crate's whole position on floats is that the pattern is the value,
    /// and a test that used float equality would be asserting something
    /// weaker than what it means to assert.
    #[test]
    fn a_finite_double_round_trips_through_its_bit_pattern() {
        let one_and_a_half = FiniteF64::new(1.5).unwrap();
        assert_eq!(one_and_a_half.bits(), 0x3ff8_0000_0000_0000);
        assert_eq!(one_and_a_half.value().to_bits(), 1.5_f64.to_bits());
        assert_eq!(
            FiniteF64::from_bits(0x3ff8_0000_0000_0000),
            Some(one_and_a_half)
        );
    }

    #[test]
    fn a_finite_single_round_trips_through_its_bit_pattern() {
        let one = FiniteF32::new(1.0).unwrap();
        assert_eq!(one.bits(), 0x3f80_0000);
        assert_eq!(one.value().to_bits(), 1.0_f32.to_bits());
        assert_eq!(FiniteF32::from_bits(0x3f80_0000), Some(one));
    }

    /// The two zeros are different bytes and therefore different traces.
    /// A codec that compared values rather than patterns would lose the
    /// distinction silently.
    #[test]
    fn the_two_zeros_are_different_patterns() {
        assert_ne!(FiniteF64::new(0.0), FiniteF64::new(-0.0));
        assert_ne!(FiniteF32::new(0.0), FiniteF32::new(-0.0));
    }

    #[test]
    fn no_infinity_and_no_not_a_number_can_be_constructed() {
        assert_eq!(FiniteF64::new(f64::INFINITY), None);
        assert_eq!(FiniteF64::new(f64::NEG_INFINITY), None);
        assert_eq!(FiniteF64::new(f64::NAN), None);
        assert_eq!(FiniteF32::new(f32::INFINITY), None);
        assert_eq!(FiniteF32::new(f32::NEG_INFINITY), None);
        assert_eq!(FiniteF32::new(f32::NAN), None);
        // The largest finite values sit immediately below the rejected
        // exponent, so the boundary is tested from the legal side too.
        assert!(FiniteF64::new(f64::MAX).is_some());
        assert!(FiniteF32::new(f32::MAX).is_some());
    }

    /// A signalling `NaN` — one whose payload has the quiet bit clear —
    /// is rejected by the same exponent test, with no float touched.
    #[test]
    fn a_signalling_pattern_is_rejected_too() {
        assert_eq!(FiniteF64::from_bits(0x7ff0_0000_0000_0001), None);
        assert_eq!(FiniteF32::from_bits(0x7f80_0001), None);
    }

    #[test]
    fn the_hex_width_is_the_width_of_the_type() {
        assert_eq!(FiniteF64::HEX_DIGITS, 16);
        assert_eq!(FiniteF32::HEX_DIGITS, 8);
    }

    /// The count is the tripwire behind the table: adding a key without
    /// adding it here leaves the parser unable to read what the writer
    /// emits, and this is what says so.
    #[test]
    fn every_key_has_a_unique_name_that_reads_back() {
        assert_eq!(TraceKey::ALL.len(), 13);
        for key in TraceKey::ALL {
            assert_eq!(TraceKey::from_name(key.name()), Some(*key));
        }
        // Two keys sharing a name would make the lookup above find the
        // first of them for both, so uniqueness is checked directly
        // rather than left to a round trip that would hide it.
        let mut names: Vec<&str> = TraceKey::ALL.iter().map(|key| key.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TraceKey::ALL.len(), "two keys share a name");
    }

    #[test]
    fn an_unknown_key_name_names_nothing() {
        assert_eq!(TraceKey::from_name("meta"), None);
        assert_eq!(TraceKey::from_name(""), None);
        // The state words a key line already uses are not key names.
        assert_eq!(TraceKey::from_name("down"), None);
    }

    #[test]
    fn every_named_button_writes_and_reads_back_as_itself() {
        assert_eq!(TraceButton::NAMED.len(), 5);
        for button in TraceButton::NAMED {
            assert_eq!(TraceButton::from_name(&button.to_string()), Some(*button));
        }
        assert_eq!(TraceButton::Left.to_string(), "left");
        assert_eq!(TraceButton::Right.to_string(), "right");
        assert_eq!(TraceButton::Middle.to_string(), "middle");
        assert_eq!(TraceButton::Back.to_string(), "back");
        assert_eq!(TraceButton::Forward.to_string(), "forward");
    }

    /// A native index is written by the same `Display` and is *not* a
    /// name: the parser owns the number, so looking it up as a name finds
    /// nothing.
    #[test]
    fn a_native_button_is_written_behind_its_prefix() {
        assert_eq!(TraceButton::Other(7).to_string(), "other:7");
        assert_eq!(TraceButton::Other(u16::MAX).to_string(), "other:65535");
        assert_eq!(TraceButton::from_name("other:7"), None);
        assert_eq!(TraceButton::from_name("other"), None);
    }

    /// The vocabulary is plain data: two events built the same way are
    /// the same event, and every variant says what it is when printed.
    #[test]
    fn events_are_comparable_and_printable_plain_data() {
        let event = TraceEvent::Key {
            code: TraceKey::Space,
            pressed: true,
            repeat: false,
        };
        assert_eq!(event, event);
        assert_ne!(event, TraceEvent::CloseRequested);
        assert!(format!("{event:?}").contains("Space"));
        assert!(format!("{:?}", TraceKey::KeyW).contains("KeyW"));
        assert!(format!("{:?}", TraceButton::Other(3)).contains('3'));
        assert!(format!("{:?}", FiniteF64::new(2.0)).contains("FiniteF64"));
        assert!(format!("{:?}", FiniteF32::new(2.0)).contains("FiniteF32"));
    }
}
