//! Why a trace was refused: which line, and what was expected there.
//!
//! Every rule in this crate rejects rather than repairs, and every refusal
//! names a line. Nothing is skipped: a line a reader does not understand
//! is the one thing a format must never shrug at, because skipping is how
//! two readers of the same file quietly stop agreeing about what it says.
//!
//! One error type serves both directions. The codec numbers a trace by its
//! text lines — the header is line 1, and because every line after it is
//! exactly one event, the event at index *k* is line *k* + 2. A trace
//! assembled in memory is numbered the same way, so a refusal from
//! [`Trace::new`](crate::Trace::new) names the line the offending event
//! *would* occupy: the same number the reader would report reading it
//! back.

use crate::event::{TraceButton, TraceKey};
use crate::grammar::{MAGIC, OTHER_BUTTON};
use core::fmt;

/// A refusal, with the line it happened on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceError {
    line: usize,
    kind: TraceErrorKind,
}

impl TraceError {
    pub(crate) const fn new(line: usize, kind: TraceErrorKind) -> Self {
        Self { line, kind }
    }

    /// The 1-based line this refusal is about.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// What went wrong, as a value a caller can match on rather than a
    /// string it would have to search.
    #[must_use]
    pub const fn kind(&self) -> &TraceErrorKind {
        &self.kind
    }
}

impl fmt::Display for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.kind)
    }
}

impl std::error::Error for TraceError {}

/// What was wrong with the line.
///
/// Matching is exhaustive on purpose while the crate is young: a caller
/// that handles every refusal today should stop compiling the day a new
/// one is added, rather than silently routing it to a catch-all arm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceErrorKind {
    /// Nothing at all. A trace is at minimum its header line.
    Empty,
    /// A byte order mark in front of the header. Named specifically
    /// because it is invisible: the file looks correct on screen, and
    /// every other message would blame the wrong thing.
    ByteOrderMark,
    /// The first line does not begin with the format's own name.
    NotATrace { found: String },
    /// A version this reader does not know how to read. A reader accepts
    /// its own version and every older one, never a newer one.
    UnsupportedVersion { found: u64, supported: u32 },
    /// The line stopped before something the grammar requires.
    LineEndsEarly { expected: &'static str },
    /// A required header field, but not the one that belongs here. The
    /// four the codec owns are positional so that a reader knows what it
    /// is holding before it interprets it.
    HeaderFieldOutOfOrder {
        expected: &'static str,
        found: String,
    },
    /// A header field after the version that is not `key=value`.
    NotAKeyValuePair { field: String },
    /// The same header key twice. Which one wins is not a question a
    /// format should leave open.
    DuplicateHeaderKey { key: String },
    /// Two field separators in a row.
    BlankField,
    /// A header key or value that could not be written back out as
    /// itself.
    UnwritableText { text: String, reason: &'static str },
    /// A second header line. A trace has exactly one, and it is first.
    HeaderAfterEvents,
    /// A line whose first word is not one this reader knows.
    UnknownKeyword { keyword: String },
    /// An event line naming a kind of event this reader does not know.
    UnknownEventKind { kind: String },
    /// An event kind newer than the version the file itself claims. The
    /// reader knows the word; the file's header disclaims it, and a
    /// header that lies about its vocabulary sends every older reader
    /// into the wrong refusal.
    EventFromANewerFormat {
        kind: String,
        introduced: u32,
        declared: u64,
    },
    /// Text after the end of a complete line.
    TrailingText { text: String },
    /// A number written in something other than plain ASCII digits.
    NotADecimalInteger { field: &'static str, text: String },
    /// A number too large for the field it was written in.
    IntegerTooLarge { field: &'static str, text: String },
    /// A character field holding something that is not typed text: a
    /// surrogate, a code point past the last one, or a control
    /// character. Distinct from a number that does not fit, because
    /// every one of these fits a `u32` and none of them is text.
    NotTypedText { field: &'static str, text: String },
    /// A float field that is not a fixed-width lowercase hexadecimal bit
    /// pattern.
    NotAHexPattern {
        field: &'static str,
        text: String,
        digits: usize,
    },
    /// A bit pattern naming an infinity or a `NaN`.
    NonFinite { field: &'static str, text: String },
    /// A key name outside the encodable set.
    UnknownKey { name: String },
    /// A pointer button outside the encodable set.
    UnknownButton { name: String },
    /// A key or button state that is neither `down` nor `up`.
    NotAPressedState { text: String },
    /// A focus state that is neither `in` nor `out`.
    NotAFocusState { text: String },
    /// A touch phase outside the four the format names.
    NotATouchPhase { text: String },
    /// Something other than the literal `repeat` where only that may go.
    NotTheRepeatFlag { text: String },
    /// A tick earlier than the one before it.
    TickWentBackwards { tick: u64, previous: u64 },
    /// A tick past the end of the run the header describes.
    TickBeyondHeader { tick: u64, ticks: u64 },
}

impl fmt::Display for TraceErrorKind {
    // Long, and deliberately one piece. This is a table of messages, not
    // an algorithm: every arm is one refusal and its words, and the
    // match is exhaustive, which is what stops a new refusal from
    // reaching a user with no words at all. Splitting it into groups
    // would need either a second match to route between them or a
    // catch-all arm in each, and a catch-all is exactly the thing whose
    // absence is doing the work here.
    //
    // `expect` rather than `allow`: if this table ever shrinks back
    // under the limit, the compiler says so and the exemption goes,
    // instead of sitting here outliving its reason.
    #[expect(
        clippy::too_many_lines,
        reason = "a table of messages, kept in one exhaustive match on purpose"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str(
                "the file is empty; a trace is a header line followed by one line per event",
            ),
            Self::ByteOrderMark => f.write_str(
                "the file begins with a byte order mark (U+FEFF), which is not part of the header; it is invisible on screen, so it is named here rather than reported as a malformed first word",
            ),
            Self::NotATrace { found } => write!(
                f,
                "expected a header beginning with `{MAGIC}`, found `{found}`",
                found = shown(found),
            ),
            Self::UnsupportedVersion { found, supported } => write!(
                f,
                "this trace is version {found} and this reader understands version {supported} and older",
            ),
            Self::LineEndsEarly { expected } => write!(f, "the line ends before {expected}"),
            Self::HeaderFieldOutOfOrder { expected, found } => write!(
                f,
                "expected {expected} here, found `{found}`; the header's own fields are positional",
                found = shown(found),
            ),
            Self::NotAKeyValuePair { field } => write!(
                f,
                "header fields after the version are written `key=value`, found `{field}`",
                field = shown(field),
            ),
            Self::DuplicateHeaderKey { key } => write!(
                f,
                "the header sets `{key}` twice; keys are unique",
                key = shown(key),
            ),
            Self::BlankField => f.write_str(
                "an empty field; fields are separated by exactly one space, so neither a doubled space nor a trailing one belongs here",
            ),
            Self::UnwritableText { text, reason } => write!(
                f,
                "the header field `{text}` cannot be written: {reason}",
                text = shown(text),
            ),
            Self::HeaderAfterEvents => f.write_str(
                "a second header; a trace has exactly one and it is the first line",
            ),
            Self::UnknownKeyword { keyword } => write!(
                f,
                "unknown line keyword `{keyword}`; every line after the header begins with `e`. The line is refused rather than skipped: if this file really is a version this reader accepts, then this reader's table is the thing that is incomplete",
                keyword = shown(keyword),
            ),
            Self::UnknownEventKind { kind } => write!(
                f,
                "unknown event kind `{kind}`; this reader knows key, pointer, motion, button, wheel, focus, text, resize, scale, redraw, close and touch. The line is refused rather than skipped: if this file really is a version this reader accepts, then this reader's table is the thing that is incomplete",
                kind = shown(kind),
            ),
            Self::EventFromANewerFormat {
                kind,
                introduced,
                declared,
            } => write!(
                f,
                "the event kind `{kind}` was introduced in format version {introduced}, and this file claims version {declared}; a header that disclaims its own vocabulary sends every older reader into the wrong refusal, so the claim is the thing to fix",
                kind = shown(kind),
            ),
            Self::TrailingText { text } => write!(
                f,
                "unexpected `{text}` after the end of the line",
                text = shown(text),
            ),
            Self::NotADecimalInteger { field, text } => write!(
                f,
                "{field} is written in ASCII digits only — no sign, no underscores, no other base — found `{text}`",
                text = shown(text),
            ),
            Self::IntegerTooLarge { field, text } => write!(
                f,
                "{field} does not fit its width, found `{text}`",
                text = shown(text),
            ),
            Self::NotTypedText { field, text } => write!(
                f,
                "{field} is not typed text: `{text}` is a surrogate, past the last code point, or a control character",
                text = shown(text),
            ),
            Self::NotAHexPattern {
                field,
                text,
                digits,
            } => write!(
                f,
                "{field} is written as `0x` and exactly {digits} lowercase hexadecimal digits, found `{text}`",
                text = shown(text),
            ),
            Self::NonFinite { field, text } => write!(
                f,
                "{field} is `{text}`, which is an infinity or a not-a-number; a trace carries finite values only",
                text = shown(text),
            ),
            Self::UnknownKey { name } => write!(
                f,
                "unknown key name `{name}`; this reader knows {names}",
                name = shown(name),
                names = joined(TraceKey::ALL.iter().map(|key| key.name().to_string())),
            ),
            Self::UnknownButton { name } => write!(
                f,
                "unknown pointer button `{name}`; this reader knows {names}, and `{OTHER_BUTTON}<index>` for a native button by its number",
                name = shown(name),
                names = joined(TraceButton::NAMED.iter().map(TraceButton::to_string)),
            ),
            Self::NotAPressedState { text } => write!(
                f,
                "a key or a button is `down` or `up`, found `{text}`",
                text = shown(text),
            ),
            Self::NotAFocusState { text } => write!(
                f,
                "focus is `in` or `out`, found `{text}`",
                text = shown(text),
            ),
            Self::NotATouchPhase { text } => write!(
                f,
                "a touch phase is `start`, `move`, `end` or `cancel`, found `{text}`",
                text = shown(text),
            ),
            Self::NotTheRepeatFlag { text } => write!(
                f,
                "the only thing a key line may carry after its state is the literal `repeat`, found `{text}`",
                text = shown(text),
            ),
            Self::TickWentBackwards { tick, previous } => write!(
                f,
                "tick {tick} follows tick {previous}; ticks never decrease. Equal ticks are allowed and their recorded order is part of the trace, so nothing here is sorted",
            ),
            Self::TickBeyondHeader { tick, ticks } => write!(
                f,
                "tick {tick} is past the end of a run of {ticks} ticks; the last legal tick is {ticks} itself, which means after the final step",
            ),
        }
    }
}

/// The known names of something, as one comma-separated list.
fn joined(names: impl Iterator<Item = String>) -> String {
    names.collect::<Vec<_>>().join(", ")
}

/// How much of an offending field a message quotes back.
const SHOWN_CHARS: usize = 64;

/// A piece of the file, made safe to read in the place these messages are
/// actually read.
///
/// Every quoted fragment here came out of an untrusted file, and printing
/// one verbatim hands its author the terminal: an escape sequence can
/// recolour a build log, erase the line that reported the problem, or
/// scroll it away, and a field a megabyte long can bury it just as well.
/// The header rules refuse control characters in a *written* trace for
/// exactly this reason; a message about a file that broke those rules
/// cannot then print the thing they were protecting against. So the text
/// is escaped and capped for display, while the error value keeps the
/// bytes as they were — a caller matching on the refusal sees the file,
/// and only the printed form is tamed.
fn shown(text: &str) -> String {
    let mut escaped = text.escape_debug();
    let mut safe: String = escaped.by_ref().take(SHOWN_CHARS).collect();
    if escaped.next().is_some() {
        safe.push('…');
    }
    safe
}

#[cfg(test)]
mod tests {
    use super::{TraceError, TraceErrorKind};

    /// Assert a table of refusals against the exact words each produces,
    /// and against how many of them there are.
    ///
    /// The three tables below are one list split in three, because a
    /// single one outgrew what a person reads in one sitting. Together
    /// they hold every refusal this crate can produce — 11 + 8 + 9 — and
    /// each says so, so a refusal added without a message here fails a
    /// count rather than going out into the world unread.
    fn all_of(count: usize, cases: Vec<(TraceErrorKind, &str)>) {
        assert_eq!(cases.len(), count, "a refusal is missing from this table");
        for (kind, expected) in cases {
            assert_eq!(kind.to_string(), expected);
        }
    }

    /// What the reader says about a file, and about the one header line
    /// every file begins with.
    #[test]
    fn every_refusal_about_a_header_says_what_was_expected() {
        all_of(
            11,
            vec![
                (
                    TraceErrorKind::Empty,
                    "the file is empty; a trace is a header line followed by one line per event",
                ),
                (
                    TraceErrorKind::ByteOrderMark,
                    "the file begins with a byte order mark (U+FEFF), which is not part of the header; it is invisible on screen, so it is named here rather than reported as a malformed first word",
                ),
                (
                    TraceErrorKind::NotATrace {
                        found: "hello".to_string(),
                    },
                    "expected a header beginning with `renew-trace`, found `hello`",
                ),
                (
                    TraceErrorKind::UnsupportedVersion {
                        found: 9,
                        supported: 0,
                    },
                    "this trace is version 9 and this reader understands version 0 and older",
                ),
                (
                    TraceErrorKind::LineEndsEarly {
                        expected: "the tick",
                    },
                    "the line ends before the tick",
                ),
                (
                    TraceErrorKind::HeaderFieldOutOfOrder {
                        expected: "`ticks=<u64>`",
                        found: "budget=5".to_string(),
                    },
                    "expected `ticks=<u64>` here, found `budget=5`; the header's own fields are positional",
                ),
                (
                    TraceErrorKind::NotAKeyValuePair {
                        field: "seed".to_string(),
                    },
                    "header fields after the version are written `key=value`, found `seed`",
                ),
                (
                    TraceErrorKind::DuplicateHeaderKey {
                        key: "seed".to_string(),
                    },
                    "the header sets `seed` twice; keys are unique",
                ),
                (
                    TraceErrorKind::BlankField,
                    "an empty field; fields are separated by exactly one space, so neither a doubled space nor a trailing one belongs here",
                ),
                (
                    TraceErrorKind::UnwritableText {
                        text: "seed=".to_string(),
                        reason: "a header key and a header value are each at least one character",
                    },
                    "the header field `seed=` cannot be written: a header key and a header value are each at least one character",
                ),
                (
                    TraceErrorKind::HeaderAfterEvents,
                    "a second header; a trace has exactly one and it is the first line",
                ),
            ],
        );
    }

    /// What the reader says about the shape of an event line: the words
    /// it is made of, and where they stop.
    #[test]
    fn every_refusal_about_a_line_shape_says_what_was_expected() {
        all_of(
            8,
            vec![
                (
                    TraceErrorKind::UnknownKeyword {
                        keyword: "x".to_string(),
                    },
                    "unknown line keyword `x`; every line after the header begins with `e`. The line is refused rather than skipped: if this file really is a version this reader accepts, then this reader's table is the thing that is incomplete",
                ),
                (
                    TraceErrorKind::UnknownEventKind {
                        kind: "gamepad".to_string(),
                    },
                    "unknown event kind `gamepad`; this reader knows key, pointer, motion, button, wheel, focus, text, resize, scale, redraw, close and touch. The line is refused rather than skipped: if this file really is a version this reader accepts, then this reader's table is the thing that is incomplete",
                ),
                (
                    TraceErrorKind::EventFromANewerFormat {
                        kind: "touch".to_string(),
                        introduced: 2,
                        declared: 1,
                    },
                    "the event kind `touch` was introduced in format version 2, and this file claims version 1; a header that disclaims its own vocabulary sends every older reader into the wrong refusal, so the claim is the thing to fix",
                ),
                (
                    TraceErrorKind::TrailingText {
                        text: "extra".to_string(),
                    },
                    "unexpected `extra` after the end of the line",
                ),
                (
                    TraceErrorKind::NotAPressedState {
                        text: "pressed".to_string(),
                    },
                    "a key or a button is `down` or `up`, found `pressed`",
                ),
                (
                    TraceErrorKind::NotAFocusState {
                        text: "yes".to_string(),
                    },
                    "focus is `in` or `out`, found `yes`",
                ),
                (
                    TraceErrorKind::NotATouchPhase {
                        text: "hover".to_string(),
                    },
                    "a touch phase is `start`, `move`, `end` or `cancel`, found `hover`",
                ),
                (
                    TraceErrorKind::NotTheRepeatFlag {
                        text: "again".to_string(),
                    },
                    "the only thing a key line may carry after its state is the literal `repeat`, found `again`",
                ),
            ],
        );
    }

    /// What the reader says about a value it cannot accept: a number, a
    /// bit pattern, a name, or a tick out of order.
    #[test]
    fn every_refusal_about_a_value_says_what_was_expected() {
        all_of(
            9,
            vec![
                (
                    TraceErrorKind::NotTypedText {
                        field: "the typed character",
                        text: "13".to_string(),
                    },
                    "the typed character is not typed text: `13` is a surrogate, past the last code point, or a control character",
                ),
                (
                    TraceErrorKind::NotADecimalInteger {
                        field: "the tick",
                        text: "+4".to_string(),
                    },
                    "the tick is written in ASCII digits only — no sign, no underscores, no other base — found `+4`",
                ),
                (
                    TraceErrorKind::IntegerTooLarge {
                        field: "`budget` (u32)",
                        text: "4294967296".to_string(),
                    },
                    "`budget` (u32) does not fit its width, found `4294967296`",
                ),
                (
                    TraceErrorKind::NotAHexPattern {
                        field: "the pointer's x coordinate",
                        text: "0xFF".to_string(),
                        digits: 16,
                    },
                    "the pointer's x coordinate is written as `0x` and exactly 16 lowercase hexadecimal digits, found `0xFF`",
                ),
                (
                    TraceErrorKind::NonFinite {
                        field: "the scale factor",
                        text: "0x7ff0000000000000".to_string(),
                    },
                    "the scale factor is `0x7ff0000000000000`, which is an infinity or a not-a-number; a trace carries finite values only",
                ),
                (
                    TraceErrorKind::UnknownKey {
                        name: "meta".to_string(),
                    },
                    "unknown key name `meta`; this reader knows escape, space, enter, tab, backspace, delete, home, end, arrow-up, arrow-down, arrow-left, arrow-right, key-w, key-a, key-s, key-d, key-b, key-c, key-e, key-f, key-g, key-h, key-i, key-j, key-k, key-l, key-m, key-n, key-o, key-p, key-q, key-r, key-t, key-u, key-v, key-x, key-y, key-z, digit-0, digit-1, digit-2, digit-3, digit-4, digit-5, digit-6, digit-7, digit-8, digit-9, f1, f2, f3, f4, f5, f6, f7, f8, f9, f10, f11, f12, shift-left, shift-right, control-left, control-right, alt-left, alt-right, page-up, page-down, insert, minus, equal, bracket-left, bracket-right, semicolon, quote, comma, period, slash, backslash, backquote, unidentified",
                ),
                (
                    TraceErrorKind::UnknownButton {
                        name: "thumb".to_string(),
                    },
                    "unknown pointer button `thumb`; this reader knows left, right, middle, back, forward, and `other:<index>` for a native button by its number",
                ),
                (
                    TraceErrorKind::TickWentBackwards {
                        tick: 3,
                        previous: 7,
                    },
                    "tick 3 follows tick 7; ticks never decrease. Equal ticks are allowed and their recorded order is part of the trace, so nothing here is sorted",
                ),
                (
                    TraceErrorKind::TickBeyondHeader {
                        tick: 31,
                        ticks: 30,
                    },
                    "tick 31 is past the end of a run of 30 ticks; the last legal tick is 30 itself, which means after the final step",
                ),
            ],
        );
    }

    /// A refusal is read in a build log, and the thing it quotes came
    /// out of an untrusted file. An escape sequence quoted verbatim can
    /// recolour that log or erase the line that reported the problem, so
    /// the printed form is escaped — while the error value itself keeps
    /// the bytes, because a caller matching on it is entitled to the
    /// file as it was.
    #[test]
    fn a_refusal_never_prints_an_escape_sequence_from_the_file() {
        let hostile = "\u{1b}[2K\u{1b}[31mgone";
        let kind = TraceErrorKind::TrailingText {
            text: hostile.to_string(),
        };
        let printed = kind.to_string();
        assert!(!printed.contains('\u{1b}'), "{printed}");
        assert!(printed.contains("\\u{1b}[2K"), "{printed}");
        // The value is untouched: only the printing is tamed.
        assert_eq!(
            kind,
            TraceErrorKind::TrailingText {
                text: hostile.to_string()
            }
        );
    }

    /// Every refusal that quotes the file, checked one by one.
    ///
    /// Two of these were pinned before and fifteen were not, which meant
    /// fifteen sites where the escaping could be dropped and nothing
    /// would notice. The table is counted for the same reason the
    /// message tables above are: a new refusal that carries text from
    /// the file has to be added here, or the count says so.
    #[test]
    fn every_refusal_that_quotes_the_file_escapes_what_it_quotes() {
        let hostile = "\u{1b}[2Kgone\u{7}";
        let quoting: Vec<TraceErrorKind> = vec![
            TraceErrorKind::NotATrace {
                found: hostile.to_string(),
            },
            TraceErrorKind::HeaderFieldOutOfOrder {
                expected: "`ticks=<u64>`",
                found: hostile.to_string(),
            },
            TraceErrorKind::NotAKeyValuePair {
                field: hostile.to_string(),
            },
            TraceErrorKind::DuplicateHeaderKey {
                key: hostile.to_string(),
            },
            TraceErrorKind::UnwritableText {
                text: hostile.to_string(),
                reason: "a control character is invisible in a diff and in a log",
            },
            TraceErrorKind::UnknownKeyword {
                keyword: hostile.to_string(),
            },
            TraceErrorKind::UnknownEventKind {
                kind: hostile.to_string(),
            },
            TraceErrorKind::EventFromANewerFormat {
                kind: hostile.to_string(),
                introduced: 2,
                declared: 1,
            },
            TraceErrorKind::TrailingText {
                text: hostile.to_string(),
            },
            TraceErrorKind::NotADecimalInteger {
                field: "the tick",
                text: hostile.to_string(),
            },
            TraceErrorKind::IntegerTooLarge {
                field: "the tick",
                text: hostile.to_string(),
            },
            TraceErrorKind::NotAHexPattern {
                field: "the scale factor",
                text: hostile.to_string(),
                digits: 16,
            },
            TraceErrorKind::NonFinite {
                field: "the scale factor",
                text: hostile.to_string(),
            },
            TraceErrorKind::UnknownKey {
                name: hostile.to_string(),
            },
            TraceErrorKind::UnknownButton {
                name: hostile.to_string(),
            },
            TraceErrorKind::NotTypedText {
                field: "the typed character",
                text: hostile.to_string(),
            },
            TraceErrorKind::NotAPressedState {
                text: hostile.to_string(),
            },
            TraceErrorKind::NotAFocusState {
                text: hostile.to_string(),
            },
            TraceErrorKind::NotATouchPhase {
                text: hostile.to_string(),
            },
            TraceErrorKind::NotTheRepeatFlag {
                text: hostile.to_string(),
            },
        ];
        assert_eq!(
            quoting.len(),
            20,
            "every refusal that quotes the file belongs in this table"
        );
        for kind in quoting {
            let printed = kind.to_string();
            assert!(
                !printed.contains('\u{1b}') && !printed.contains('\u{7}'),
                "raw control characters reached the message: {printed:?}"
            );
            assert!(
                printed.contains("\\u{1b}[2Kgone\\u{7}"),
                "the quoted text is missing or unescaped: {printed:?}"
            );
        }
    }

    /// And a field long enough to bury the message is cut short with a
    /// mark saying so, rather than scrolling the reason off the screen.
    #[test]
    fn a_refusal_quotes_only_the_front_of_an_enormous_field() {
        let long = "x".repeat(4096);
        let printed = TraceErrorKind::UnknownKeyword { keyword: long }.to_string();
        assert!(printed.contains(&format!("`{}…`", "x".repeat(super::SHOWN_CHARS))));
        // Exactly the cap is quoted whole, with no mark: a field that
        // fits is never made to look truncated.
        let exact = "y".repeat(super::SHOWN_CHARS);
        let printed = TraceErrorKind::UnknownKeyword {
            keyword: exact.clone(),
        }
        .to_string();
        assert!(printed.contains(&format!("`{exact}`")), "{printed}");
    }

    #[test]
    fn a_refusal_carries_its_line_in_front_of_its_reason() {
        let error = TraceError::new(7, TraceErrorKind::HeaderAfterEvents);
        assert_eq!(error.line(), 7);
        assert_eq!(error.kind(), &TraceErrorKind::HeaderAfterEvents);
        assert_eq!(
            error.to_string(),
            "line 7: a second header; a trace has exactly one and it is the first line"
        );
        // It is an error in the ordinary sense, so it composes with
        // whatever the caller already uses to report failures.
        let boxed: Box<dyn std::error::Error> = Box::new(error.clone());
        assert_eq!(boxed.to_string(), error.to_string());
        assert!(format!("{error:?}").contains("HeaderAfterEvents"));
    }
}
