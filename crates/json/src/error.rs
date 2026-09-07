//! Why a document was refused: which byte, and what was wrong there.
//!
//! Every refusal here names one way a byte string can fail to be the
//! document a caller asked for, and carries the offset it happened at.
//! There is deliberately no `Other` and no string-typed catch-all: a
//! reader that can say "malformed" without saying how has stopped being
//! able to tell a truncated download from an attack, and a caller that
//! wants to report the difference to a person cannot.
//!
//! One error type serves both halves of the crate. Validation refusals
//! come out of [`Json::parse`](crate::Json::parse) and are about the
//! bytes; access refusals come out of the typed accessors on
//! [`Value`](crate::Value) and are about what a caller expected to find
//! where. They share a type because they share the thing a caller needs
//! most — the offset — so a reader of asset metadata can say "at line 12,
//! column 30" whichever half refused.

use crate::Kind;
use core::fmt;

/// A refusal, with the byte it happened at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    at: usize,
    kind: JsonErrorKind,
}

impl JsonError {
    pub(crate) const fn new(at: usize, kind: JsonErrorKind) -> Self {
        Self { at, kind }
    }

    /// The byte offset this refusal is about, counted from the start of
    /// the slice that was handed to the parser.
    ///
    /// A byte offset rather than a line and a column because that is what
    /// costs nothing to carry: a document arriving as one chunk of a
    /// larger file has no lines worth counting until somebody wants to
    /// print the refusal, and [`line_and_column`](Self::line_and_column)
    /// is there for when they do.
    #[must_use]
    pub const fn at(&self) -> usize {
        self.at
    }

    /// What went wrong, as a value a caller can match on rather than a
    /// string it would have to search.
    #[must_use]
    pub const fn kind(&self) -> &JsonErrorKind {
        &self.kind
    }

    /// Where [`at`](Self::at) falls in `source`, as a 1-based line and
    /// column.
    ///
    /// Computed on demand and never stored, because a caller that
    /// forwards a refusal to another layer wants the offset and a caller
    /// that prints one wants this, and only one of them is on a hot path.
    ///
    /// The column counts **characters** where the line up to the offset
    /// is valid text, and bytes where it is not — which is the case a
    /// [`NotUtf8`](JsonErrorKind::NotUtf8) refusal is precisely about, and
    /// the one place a character count has nothing to count.
    ///
    /// Passing a slice other than the one that was parsed gives a
    /// meaningless answer rather than a wrong one: the offset is clamped
    /// to the slice, so a short slice reports its own end.
    #[must_use]
    pub fn line_and_column(&self, source: &[u8]) -> (usize, usize) {
        let at = self.at.min(source.len());
        let before = source.get(..at).unwrap_or(source);
        // One pass for both numbers: which line the offset is on, and
        // where that line began.
        let mut line = 1usize;
        let mut line_start = 0usize;
        for (index, byte) in before.iter().enumerate() {
            if *byte == b'\n' {
                line = line.saturating_add(1);
                line_start = index.saturating_add(1);
            }
        }
        let prefix = before.get(line_start..).unwrap_or(&[]);
        let column = core::str::from_utf8(prefix).map_or(prefix.len(), |text| text.chars().count());
        (line, column.saturating_add(1))
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at byte {}: {}", self.at, self.kind)
    }
}

impl core::error::Error for JsonError {}

/// What was wrong with the document, or with what a caller asked of it.
///
/// Matching is exhaustive on purpose while the crate is young: a caller
/// that handles every refusal today should stop compiling the day a new
/// one is added, rather than silently routing it to a catch-all arm. It
/// is also what lets the corpus replay beside this crate name its
/// outcomes without a wildcard — from outside a closed enum, a new
/// refusal is a compile error rather than a line in a bucket nobody
/// reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonErrorKind {
    // -- Framing ------------------------------------------------------
    /// No bytes at all. A JSON document is one value, and there is no
    /// value spelled with nothing.
    Empty,
    /// The bytes stop being valid text at this offset.
    ///
    /// Checked over the whole slice before anything is scanned, so every
    /// offset any other refusal reports is a character boundary by
    /// construction rather than by an argument somebody has to re-check.
    NotUtf8,
    /// A byte order mark in front of the document.
    ///
    /// Named specifically because it is invisible: a file carrying one
    /// looks exactly like a file that does not, and every other refusal
    /// would blame the wrong thing. Stripping it is a policy a caller can
    /// hold; guessing at it here is not.
    ByteOrderMark,
    /// A character that begins no JSON value.
    NoValue {
        /// The character that was found there.
        found: char,
    },
    /// The document ran out where the grammar still wanted something.
    ///
    /// The field says what: a value, a closing quote, the digits of an
    /// escape. One variant rather than a dozen because the defect is
    /// always the same one — the bytes are short — and what a reader
    /// needs is which piece went missing.
    EndOfDocument {
        /// What the grammar was waiting for.
        expected: &'static str,
    },
    /// A word that is not JSON, and is what a broken writer emits.
    ///
    /// `NaN`, `Infinity`, `-Infinity`, `undefined`, and the capitalised
    /// `True`, `False` and `None` a Python writer reaches for: values in
    /// the languages people write exporters in and values in no JSON
    /// document. Told apart from [`NoValue`](Self::NoValue) on purpose —
    /// "no value begins with `N`" sends a reader hunting a stray
    /// character, and the actual fault is upstream in whatever wrote the
    /// file.
    NonJsonLiteral {
        /// The word as it was written. A bounded run of ASCII letters,
        /// with the sign in front of it where there was one.
        word: String,
    },
    /// A word beginning like `true`, `false` or `null` and not finishing
    /// as one.
    BadLiteral {
        /// The word as it was written.
        found: String,
    },
    /// Something other than whitespace after the end of the document.
    ///
    /// Whitespace is skipped first, which is what lets a JSON chunk
    /// padded out to an alignment boundary parse with no knowledge of the
    /// container that padded it.
    TrailingText {
        /// The first character past the value.
        found: char,
    },
    /// Nesting deeper than this reader will follow.
    ///
    /// **The bound is the point.** A reader that recursed would meet a
    /// deep enough document with a stack overflow, which is not a refusal
    /// — it is a crash, in a crate whose whole contract is that untrusted
    /// bytes get an answer. This parser holds its own stack on the heap
    /// and refuses past a stated depth, so the limit is a policy rather
    /// than a bet about how big a stack frame turned out to be.
    DepthLimit {
        /// The deepest nesting this reader follows.
        limit: usize,
    },

    // -- Objects ------------------------------------------------------
    /// Something other than a quoted string where a member name goes.
    ExpectedKey {
        /// The character that was found there.
        found: char,
    },
    /// Something other than `:` between a member name and its value.
    ExpectedColon {
        /// The character that was found there.
        found: char,
    },
    /// Something other than `,` or `}` after a member.
    ExpectedCommaOrBraceClose {
        /// The character that was found there.
        found: char,
    },
    /// A comma with the closing bracket straight after it.
    ///
    /// Separate from [`NoValue`](Self::NoValue), which is what the
    /// grammar alone would report, because this is the commonest mistake
    /// a person editing a document by hand makes and "no value begins
    /// with `}`" sends them looking for a missing value instead of a
    /// stray comma.
    TrailingComma,

    // -- Arrays -------------------------------------------------------
    /// Something other than `,` or `]` after an element.
    ExpectedCommaOrBracketClose {
        /// The character that was found there.
        found: char,
    },

    // -- Strings ------------------------------------------------------
    /// A raw control character inside a string.
    ///
    /// A literal newline or tab between quotes is not a string character;
    /// the escapes exist for them. Named rather than folded into "a
    /// string character was expected", because what happened is that a
    /// writer forgot to escape, and the message should say so.
    ControlCharacterInString {
        /// The byte that was found there.
        byte: u8,
    },
    /// A backslash followed by something that escapes nothing.
    BadEscape {
        /// The character after the backslash.
        found: char,
    },
    /// A `\u` escape with something other than four hexadecimal digits.
    BadHexEscape {
        /// The first character that was not a hexadecimal digit.
        found: char,
    },
    /// The first half of a surrogate pair with no second half after it.
    ///
    /// A character outside the basic plane — an emoji in an exported
    /// object name — is written as two escapes, and half of one is not a
    /// character. Refused rather than replaced, because a replacement
    /// character silently changes a name a caller is about to compare.
    LoneHighSurrogate {
        /// The code unit that was escaped.
        code: u32,
    },
    /// The second half of a surrogate pair with no first half before it.
    LoneLowSurrogate {
        /// The code unit that was escaped.
        code: u32,
    },

    // -- Numbers, as they are written ---------------------------------
    /// A leading `+`. JSON writes a positive number without a sign.
    LeadingPlus,
    /// A leading zero on a multi-digit integer part.
    ///
    /// Checked explicitly because it is exactly where the standard
    /// library's own float grammar is wider than JSON's: `01` parses
    /// happily as one, and a reader that leaned on it would accept a
    /// document no other reader does.
    LeadingZero,
    /// A sign or a decimal point with no digits before the point.
    ///
    /// `-`, `.5` and `-.5` are all this. JSON has no bare fraction.
    NoIntegerDigits,
    /// A decimal point with no digits after it: `1.`.
    NoFractionDigits,
    /// An exponent marker with no digits after it: `1e`, `1e+`.
    NoExponentDigits,
    /// A number written with more characters than this reader will read.
    ///
    /// A million-digit number is legal JSON and costs whoever consumes it
    /// real time; the shortest form that round-trips a double needs
    /// twenty-odd characters, so the limit is generous by a factor a
    /// fuzzer cannot walk through.
    NumberTooLong {
        /// How many characters the number was written with.
        len: usize,
        /// The most this reader will read.
        limit: usize,
    },

    // -- Numbers and values, as a caller asks for them ----------------
    /// A value of one kind where the caller wanted another.
    NotThisKind {
        /// What the caller asked for.
        wanted: Kind,
        /// What is actually there.
        found: Kind,
    },
    /// A number with a non-zero fraction where a whole number was wanted.
    ///
    /// `3.0` and `3e2` are **not** this: a writer is allowed to spell a
    /// whole number with a fractional part or an exponent, and a reader
    /// that refused those would reject documents that are entirely legal.
    /// Only a non-zero fraction is a refusal.
    FractionalInteger,
    /// A whole number that does not fit the type the caller asked for.
    IntegerOutOfRange {
        /// The type that was asked for.
        target: &'static str,
    },
    /// A number that is finite as written and not finite once read.
    ///
    /// `1e999` is a legal JSON number and rounds to an infinity. Refused
    /// rather than handed over, because an infinity poisons every sum it
    /// reaches — a bounding box built from one is silently wrong
    /// everywhere instead of loudly wrong here.
    NumberNotFinite,
}

impl fmt::Display for JsonErrorKind {
    // Long, and deliberately one piece. This is a table of messages, not
    // an algorithm: every arm is one refusal and its words, and the match
    // is exhaustive, which is what stops a new refusal from reaching a
    // reader with no words at all. Splitting it into groups would need
    // either a second match to route between them or a catch-all arm in
    // each, and a catch-all is exactly the thing whose absence is doing
    // the work here.
    //
    // `expect` rather than `allow`: if this table ever shrinks back under
    // the limit, the compiler says so and the exemption goes, instead of
    // sitting here outliving its reason.
    #[expect(
        clippy::too_many_lines,
        reason = "a table of messages, kept in one exhaustive match on purpose"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str(
                "the document is empty; a JSON document is exactly one value, and no value is spelled with nothing",
            ),
            Self::NotUtf8 => f.write_str(
                "the bytes stop being valid UTF-8 here; JSON text is UTF-8, and a reader that guessed at the rest would be inventing characters nobody wrote",
            ),
            Self::ByteOrderMark => f.write_str(
                "the document begins with a byte order mark (U+FEFF), which is not part of any JSON value; it is invisible on screen, so it is named here rather than reported as a stray character",
            ),
            Self::NoValue { found } => write!(
                f,
                "no JSON value begins with `{found}`; a value is an object, an array, a string, a number, `true`, `false` or `null`",
                found = shown_char(*found),
            ),
            Self::EndOfDocument { expected } => {
                write!(f, "the document ends before {expected}")
            }
            Self::NonJsonLiteral { word } => write!(
                f,
                "`{word}` is a value in the language that wrote this file and in no JSON document; JSON has no not-a-number, no infinity and no undefined, so the number that produced this has to be fixed where it was written",
            ),
            Self::BadLiteral { found } => write!(
                f,
                "expected `true`, `false` or `null`, found `{found}`",
            ),
            Self::TrailingText { found } => write!(
                f,
                "`{found}` follows the end of the document; a JSON document is one value, and whitespace is all that may come after it",
                found = shown_char(*found),
            ),
            Self::DepthLimit { limit } => write!(
                f,
                "nesting deeper than {limit} objects and arrays; the limit is this reader's, not the format's, and it is here so that a document cannot choose how much stack to spend",
            ),
            Self::ExpectedKey { found } => write!(
                f,
                "expected a quoted member name, found `{found}`; the names in a JSON object are strings, quotes included",
                found = shown_char(*found),
            ),
            Self::ExpectedColon { found } => write!(
                f,
                "expected `:` after a member name, found `{found}`",
                found = shown_char(*found),
            ),
            Self::ExpectedCommaOrBraceClose { found } => write!(
                f,
                "expected `,` or `}}` after a member, found `{found}`",
                found = shown_char(*found),
            ),
            Self::TrailingComma => f.write_str(
                "a comma with nothing after it; the bracket that follows closes the value, so the comma is one too many",
            ),
            Self::ExpectedCommaOrBracketClose { found } => write!(
                f,
                "expected `,` or `]` after an element, found `{found}`",
                found = shown_char(*found),
            ),
            Self::ControlCharacterInString { byte } => write!(
                f,
                "a raw control byte {byte:#04x} inside a string; a literal newline or tab is not a string character, and the escapes exist for them",
            ),
            Self::BadEscape { found } => write!(
                f,
                "`\\{found}` escapes nothing; the escapes are \\\" \\\\ \\/ \\b \\f \\n \\r \\t and \\u",
                found = shown_char(*found),
            ),
            Self::BadHexEscape { found } => write!(
                f,
                "`\\u` takes exactly four hexadecimal digits, and `{found}` is not one",
                found = shown_char(*found),
            ),
            Self::LoneHighSurrogate { code } => write!(
                f,
                "the escape `\\u{code:04X}` is the first half of a surrogate pair and nothing pairs with it; a character outside the basic plane is written as two escapes, and half of one is not a character",
            ),
            Self::LoneLowSurrogate { code } => write!(
                f,
                "the escape `\\u{code:04X}` is the second half of a surrogate pair and nothing precedes it",
            ),
            Self::LeadingPlus => f.write_str(
                "a leading `+`; JSON writes a positive number with no sign at all",
            ),
            Self::LeadingZero => f.write_str(
                "a leading zero; JSON writes an integer part as a single `0` or as digits that do not begin with one",
            ),
            Self::NoIntegerDigits => f.write_str(
                "a number with no digits before its decimal point; JSON has no bare fraction, so `.5` is written `0.5`",
            ),
            Self::NoFractionDigits => f.write_str(
                "a decimal point with no digits after it; JSON has no trailing point, so `1.` is written `1`",
            ),
            Self::NoExponentDigits => {
                f.write_str("an exponent marker with no digits after it")
            }
            Self::NumberTooLong { len, limit } => write!(
                f,
                "a number written with {len} characters, and this reader reads {limit}; the shortest form that round-trips a double needs twenty-odd, so a number this long is a generator gone wrong rather than a value",
            ),
            Self::NotThisKind { wanted, found } => {
                write!(f, "expected {wanted}, found {found}")
            }
            Self::FractionalInteger => f.write_str(
                "a whole number was wanted and this one has a fraction; `3.0` and `3e2` are whole and would have been accepted, so what is refused here is the fraction rather than the spelling",
            ),
            Self::IntegerOutOfRange { target } => {
                write!(f, "a whole number outside the range of {target}")
            }
            Self::NumberNotFinite => f.write_str(
                "the number is finite as written and rounds to an infinity when read; an infinity poisons every sum it reaches, so it is refused here rather than handed on",
            ),
        }
    }
}

/// One character of the document, made safe to read where these
/// messages are actually read.
///
/// The character came out of an untrusted file, and printing one
/// verbatim hands its author the terminal: an escape sequence can
/// recolour a build log, erase the line that reported the problem, or
/// scroll it away. The error value keeps the character as it was — a
/// caller matching on the refusal sees the document — and only the
/// printed form is tamed.
///
/// The `word` fields need no such treatment and get none: the scanner
/// that produces them takes ASCII letters and stops, at a length it
/// bounds itself, so there is nothing in one to tame.
fn shown_char(found: char) -> String {
    found.escape_debug().collect()
}

/// **One byte string per refusal, and a compile-time guard that the list
/// is whole.**
///
/// The tests beside the scanner provoke individual refusals and document
/// how each one is reached; this module asks a different question — *is
/// every named refusal still reachable at all?* A validation check that
/// becomes dead code stops being tested by the suite that names it, and
/// nothing else here notices: the other tests keep passing, because each
/// one asserts about the refusal it was written for and no test owns the
/// list.
///
/// The guard is [`name`], which matches [`JsonErrorKind`] exhaustively
/// and **deliberately without a wildcard**. The enum is closed, so this
/// would compile from outside the crate too — but it lives here because
/// this is where a refusal is added, and a file that stops compiling in
/// the same crate as the change is the one whoever made the change is
/// already looking at.
///
/// It also formats every refusal it reaches, which is how the message
/// table stays exercised: a variant whose words nobody has ever read is
/// a variant whose words are wrong.
#[cfg(test)]
mod refusals {
    use super::{JsonError, JsonErrorKind};
    use crate::{Json, Value};

    /// Every refusal this crate can return, by name — the variant alone,
    /// without its fields.
    ///
    /// No wildcard. See this module's own documentation for why that is
    /// the point of the function rather than an accident of style.
    fn name(kind: &JsonErrorKind) -> &'static str {
        match kind {
            JsonErrorKind::Empty => "Empty",
            JsonErrorKind::NotUtf8 => "NotUtf8",
            JsonErrorKind::ByteOrderMark => "ByteOrderMark",
            JsonErrorKind::NoValue { .. } => "NoValue",
            JsonErrorKind::EndOfDocument { .. } => "EndOfDocument",
            JsonErrorKind::NonJsonLiteral { .. } => "NonJsonLiteral",
            JsonErrorKind::BadLiteral { .. } => "BadLiteral",
            JsonErrorKind::TrailingText { .. } => "TrailingText",
            JsonErrorKind::DepthLimit { .. } => "DepthLimit",
            JsonErrorKind::ExpectedKey { .. } => "ExpectedKey",
            JsonErrorKind::ExpectedColon { .. } => "ExpectedColon",
            JsonErrorKind::ExpectedCommaOrBraceClose { .. } => "ExpectedCommaOrBraceClose",
            JsonErrorKind::TrailingComma => "TrailingComma",
            JsonErrorKind::ExpectedCommaOrBracketClose { .. } => "ExpectedCommaOrBracketClose",
            JsonErrorKind::ControlCharacterInString { .. } => "ControlCharacterInString",
            JsonErrorKind::BadEscape { .. } => "BadEscape",
            JsonErrorKind::BadHexEscape { .. } => "BadHexEscape",
            JsonErrorKind::LoneHighSurrogate { .. } => "LoneHighSurrogate",
            JsonErrorKind::LoneLowSurrogate { .. } => "LoneLowSurrogate",
            JsonErrorKind::LeadingPlus => "LeadingPlus",
            JsonErrorKind::LeadingZero => "LeadingZero",
            JsonErrorKind::NoIntegerDigits => "NoIntegerDigits",
            JsonErrorKind::NoFractionDigits => "NoFractionDigits",
            JsonErrorKind::NoExponentDigits => "NoExponentDigits",
            JsonErrorKind::NumberTooLong { .. } => "NumberTooLong",
            JsonErrorKind::NotThisKind { .. } => "NotThisKind",
            JsonErrorKind::FractionalInteger => "FractionalInteger",
            JsonErrorKind::IntegerOutOfRange { .. } => "IntegerOutOfRange",
            JsonErrorKind::NumberNotFinite => "NumberNotFinite",
        }
    }

    /// What a caller asks of a document that parsed.
    ///
    /// Four of these refusals are not about the bytes at all: they are
    /// about a caller wanting a number where a string is, or a whole
    /// number where a fraction is. Those need a question as well as a
    /// document, so every case carries the question it is asked.
    /// The other twenty-five carry [`None`]: the parse refuses them, so
    /// no question is ever put to them. Written that way rather than as
    /// a do-nothing function so that the table holds no entry whose only
    /// job is to be unreachable.
    type Probe = fn(Value<'_>) -> Result<(), JsonError>;

    /// The refusal a byte string and a question produce between them.
    fn refusal(bytes: &[u8], probe: Option<Probe>) -> Option<JsonError> {
        match Json::parse(bytes) {
            Err(error) => Some(error),
            Ok(document) => probe.and_then(|ask| ask(document.root()).err()),
        }
    }

    /// **Every named refusal has a byte string that still provokes it.**
    ///
    /// Twenty-nine entries for twenty-nine variants, and the assertion is
    /// on the *set* as well as on each entry: if a validation check is
    /// deleted, its input falls through to whatever the next check says,
    /// two entries collapse onto one answer, and this names which one
    /// went missing. That is the failure a per-refusal test cannot
    /// produce, because a per-refusal test only ever knows about the
    /// refusal it was written for.
    ///
    /// Every refusal is also formatted here, so the message table is
    /// exercised rather than merely written.
    ///
    /// Probed by deleting the leading-zero check in `read_number`: `01`
    /// scans as `0` followed by a stray `1`, and the failure named the
    /// refusal that stopped being reachable — "the document written to
    /// provoke `LeadingZero` now answers `TrailingText`". Probed a
    /// second time by deleting the trailing-comma check, which reported
    /// "the document written to provoke `TrailingComma` now answers
    /// `NoValue`" — an answer another entry already produces, which is
    /// exactly the collapse the set assertion behind it exists for.
    #[test]
    fn every_named_refusal_has_a_document_that_provokes_it() {
        let deep = {
            let mut bytes = vec![b'['; crate::MAX_DEPTH + 1];
            bytes.extend(std::iter::repeat_n(b']', crate::MAX_DEPTH + 1));
            bytes
        };
        let long_number = {
            let mut bytes = vec![b'1'];
            bytes.extend(std::iter::repeat_n(b'0', crate::MAX_NUMBER_LEN + 8));
            bytes
        };

        let cases: [(&str, Vec<u8>, Option<Probe>); 29] = [
            ("Empty", Vec::new(), None),
            // A lone 0xFF is a byte no character begins with.
            ("NotUtf8", vec![b'"', 0xFF, b'"'], None),
            ("ByteOrderMark", "\u{feff}{}".as_bytes().to_vec(), None),
            ("NoValue", b"@".to_vec(), None),
            ("EndOfDocument", b"[".to_vec(), None),
            ("NonJsonLiteral", b"NaN".to_vec(), None),
            ("BadLiteral", b"tru".to_vec(), None),
            ("TrailingText", b"1 2".to_vec(), None),
            ("DepthLimit", deep, None),
            ("ExpectedKey", b"{1:2}".to_vec(), None),
            ("ExpectedColon", br#"{"a" 1}"#.to_vec(), None),
            (
                "ExpectedCommaOrBraceClose",
                br#"{"a":1 "b":2}"#.to_vec(),
                None,
            ),
            ("TrailingComma", b"[1,]".to_vec(), None),
            ("ExpectedCommaOrBracketClose", b"[1 2]".to_vec(), None),
            ("ControlCharacterInString", b"\"\n\"".to_vec(), None),
            ("BadEscape", br#""\q""#.to_vec(), None),
            ("BadHexEscape", br#""\uZZZZ""#.to_vec(), None),
            ("LoneHighSurrogate", br#""\uD800""#.to_vec(), None),
            ("LoneLowSurrogate", br#""\uDC00""#.to_vec(), None),
            ("LeadingPlus", b"+1".to_vec(), None),
            ("LeadingZero", b"01".to_vec(), None),
            ("NoIntegerDigits", b".5".to_vec(), None),
            ("NoFractionDigits", b"1.".to_vec(), None),
            ("NoExponentDigits", b"1e".to_vec(), None),
            ("NumberTooLong", long_number, None),
            // The four a caller reaches, on documents that are perfectly
            // well formed and simply do not hold what was asked for.
            (
                "NotThisKind",
                b"[]".to_vec(),
                Some(|value: Value<'_>| value.as_u32().map(drop)),
            ),
            (
                "FractionalInteger",
                b"3.5".to_vec(),
                Some(|value: Value<'_>| value.as_u32().map(drop)),
            ),
            (
                "IntegerOutOfRange",
                b"99999999999".to_vec(),
                Some(|value: Value<'_>| value.as_u32().map(drop)),
            ),
            (
                "NumberNotFinite",
                b"1e999".to_vec(),
                Some(|value: Value<'_>| value.as_f64().map(drop)),
            ),
        ];

        let mut reached = std::collections::BTreeSet::new();
        for (expected, bytes, probe) in &cases {
            // A document that is accepted answers "Ok", which no entry
            // claims, so the same assertion catches a refusal that
            // became another refusal and one that stopped happening.
            let outcome = refusal(bytes, *probe);
            let got = outcome.as_ref().map_or("Ok", |error| name(error.kind()));
            let message = outcome
                .as_ref()
                .map_or_else(String::new, JsonError::to_string);
            assert_eq!(
                got, *expected,
                "the document written to provoke {expected} now answers {got}"
            );
            assert!(
                !message.is_empty(),
                "{expected} formats to nothing, so a reader of the message learns nothing"
            );
            reached.insert(got);
        }

        assert_eq!(
            reached.len(),
            cases.len(),
            "two entries provoke the same refusal, so one of them is not testing what it names: \
             {reached:?}"
        );
    }

    /// **Every kind has words**, which the mismatch message needs.
    ///
    /// `NotThisKind` prints two kinds, so the six-arm table under
    /// [`Kind`](crate::Kind) is reachable only through it. One case per
    /// kind, each asking for something the document is not.
    ///
    /// Probed by making the `Kind::Array` arm print the same words as
    /// `Kind::Object`: "a document of [91, 93] did not describe itself
    /// as an array: at byte 0: expected a number, found an object".
    #[test]
    fn every_kind_has_words_of_its_own() {
        let cases: [(&str, &[u8], Probe); 6] = [
            ("null", b"null", |value| value.as_u32().map(drop)),
            ("a boolean", b"true", |value| value.as_u32().map(drop)),
            ("a number", b"1", |value| value.as_str().map(drop)),
            ("a string", br#""x""#, |value| value.as_u32().map(drop)),
            ("an array", b"[]", |value| value.as_u32().map(drop)),
            ("an object", b"{}", |value| value.as_u32().map(drop)),
        ];
        let mut said = std::collections::BTreeSet::new();
        for (words, bytes, probe) in cases {
            // Empty when the document was accepted, which no case here
            // expects, so the assertion below covers both that and a
            // mismatch that named the wrong kind.
            let message = refusal(bytes, Some(probe))
                .map(|error| error.to_string())
                .unwrap_or_default();
            assert!(
                message.contains(&format!("found {words}")),
                "a document of {bytes:?} did not describe itself as {words}: {message:?}"
            );
            said.insert(words);
        }
        assert_eq!(
            said.len(),
            6,
            "two kinds print the same words, so a mismatch message cannot tell them apart"
        );
    }

    /// **A refusal can be pointed at, in a document with lines in it.**
    ///
    /// The offset is what the crate carries; a line and a column is what
    /// a person reads. Both branches of the column are here: one over
    /// text with a multi-byte character in the line before the fault,
    /// where characters and bytes disagree, and one over bytes that stop
    /// being text at all, where there are no characters to count.
    ///
    /// Probed by measuring the prefix one byte long: the first case
    /// comes back as column 8 rather than column 7, which is the
    /// off-by-one a reader would chase in their own file rather than in
    /// this one.
    #[test]
    fn a_refusal_names_the_line_and_column_it_happened_on() {
        let source = "[\n  1,\n  \"é\" x\n]".as_bytes();
        let error = Json::parse(source).expect_err("`x` follows a complete element");
        // Three characters of indent and quoting, the accented one, the
        // closing quote and a space: the column counts characters, so
        // the two bytes of `é` count once.
        assert_eq!(error.line_and_column(source), (3, 7));

        let broken = b"[\n\xFF]";
        let error = Json::parse(broken).expect_err("0xFF begins no character");
        assert_eq!(error.line_and_column(broken), (2, 1));
    }
}
