//! The one validating pass: bytes in, a flat node table out.
//!
//! **Nothing here recurses.** The nesting a document declares is held in
//! a vector of open container indices, and the depth limit is a check
//! against that vector's length. A recursive-descent reader would meet a
//! document of ten thousand open brackets with a stack overflow, and a
//! stack overflow is not a refusal — the process is gone, no error value
//! is returned, and nothing above can report what happened. The whole
//! contract of this crate is that untrusted bytes get an answer, so the
//! one construct that cannot give one is the one construct this file
//! does not use.
//!
//! The pass validates everything and interprets nothing. Escapes are
//! checked and left where they lie; numbers are checked against the
//! grammar and left as the characters they were written with. That
//! division is deliberate: what a number *is* depends on the type the
//! caller wanted, and a reader that decided at parse time — a `1` here,
//! a `1.0` there — would be making that decision by spelling, which is
//! the one thing the spelling does not tell you.

use crate::error::{JsonError, JsonErrorKind};
use crate::{Kind, MAX_DEPTH, MAX_NUMBER_LEN, Node};

/// The longest run of letters read before giving up on a bare word.
///
/// Bounded so that a megabyte of letters costs a refusal rather than a
/// megabyte-long error message. Long enough for every word this reader
/// names, `undefined` being the longest.
const MAX_WORD_LEN: usize = 24;

/// The words that are values in the languages people write exporters in
/// and values in no JSON document.
///
/// Compared without regard to case, so `NaN`, `nan` and `NAN` all land
/// here. The three JSON literals are matched exactly *before* this list
/// is consulted, which is what lets `True` and `None` be on it without
/// `true` and `null` being caught by them.
const NOT_JSON_WORDS: [&str; 8] = [
    "nan",
    "inf",
    "infinity",
    "undefined",
    "none",
    "true",
    "false",
    "null",
];

/// Check the whole slice as text, and refuse the two things that are
/// wrong before a single value is looked at.
///
/// The UTF-8 check comes first and covers everything, which buys a
/// property the rest of this file then never has to argue for: every
/// offset any refusal reports is a character boundary. A reader that
/// validated as it went would have to prove that for each scanner
/// separately, and a scanner that stopped mid-character would report an
/// offset no editor can point at.
pub(crate) fn validate_text(bytes: &[u8]) -> Result<&str, JsonError> {
    if bytes.is_empty() {
        return Err(JsonError::new(0, JsonErrorKind::Empty));
    }
    let source = core::str::from_utf8(bytes)
        .map_err(|error| JsonError::new(error.valid_up_to(), JsonErrorKind::NotUtf8))?;
    if source.starts_with('\u{feff}') {
        return Err(JsonError::new(0, JsonErrorKind::ByteOrderMark));
    }
    Ok(source)
}

/// Where the scan is, and the two questions it asks at every step.
struct Cursor<'a> {
    source: &'a str,
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            at: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn next_peek(&self) -> Option<u8> {
        self.bytes.get(self.at.saturating_add(1)).copied()
    }

    /// The character starting at the cursor, for a message about it.
    ///
    /// The replacement character stands for the impossible case — the
    /// cursor is only ever parked on a character boundary, because the
    /// whole slice was checked as text before this file saw it, and
    /// every byte this scanner stops on is ASCII. A reader that finds
    /// U+FFFD in a message has found a defect here, which is a better
    /// outcome than a panic in a crate that promises not to.
    fn here(&self) -> char {
        self.source
            .get(self.at..)
            .and_then(|rest| rest.chars().next())
            .unwrap_or(char::REPLACEMENT_CHARACTER)
    }

    fn error(&self, kind: JsonErrorKind) -> JsonError {
        JsonError::new(self.at, kind)
    }

    /// The four characters JSON calls whitespace, and no others.
    ///
    /// A vertical tab or a form feed is not whitespace here, which is
    /// how a document written by something that thought otherwise gets
    /// a refusal naming the character rather than a silent shrug.
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at = self.at.saturating_add(1);
        }
    }

    fn advance(&mut self) {
        self.at = self.at.saturating_add(1);
    }

    /// The run of ASCII letters at the cursor, consumed.
    fn word(&mut self) -> &'a str {
        let start = self.at;
        while self.peek().is_some_and(|byte| byte.is_ascii_alphabetic())
            && self.at.saturating_sub(start) < MAX_WORD_LEN
        {
            self.advance();
        }
        self.source.get(start..self.at).unwrap_or("")
    }
}

/// Is this one of the words that means a value in some other language?
fn not_json_word(word: &str) -> bool {
    NOT_JSON_WORDS
        .iter()
        .any(|known| word.eq_ignore_ascii_case(known))
}

/// Read a whole document into a node table, or refuse it.
///
/// The shape of the loop is worth stating, because it is what replaces
/// the recursion. The outer pass always reads *one value*. A scalar
/// finishes immediately; a container pushes itself onto the stack and
/// either sends the pass back round for its first element, or — when it
/// is empty — falls straight into the ascent, which sees the closing
/// bracket and closes it. The ascent then walks back out through every
/// container that the same bracket run finishes, and stops either at a
/// comma, which sends the pass round again, or at an empty stack, which
/// is the end of the document.
pub(crate) fn document(source: &str) -> Result<Vec<Node>, JsonError> {
    let mut cursor = Cursor::new(source);
    // No capacity hint, and that is the point: a reservation taken from
    // the length of an untrusted document is how a reader ends up
    // holding megabytes to say no. The table grows by pushing what has
    // already been accepted, so the cost is proportional to what was
    // read rather than to what was claimed.
    let mut nodes: Vec<Node> = Vec::new();
    let mut open: Vec<usize> = Vec::new();

    'value: loop {
        cursor.skip_whitespace();
        let start = cursor.at;
        let Some(byte) = cursor.peek() else {
            return Err(cursor.error(JsonErrorKind::EndOfDocument {
                expected: "a value",
            }));
        };

        match byte {
            b'{' | b'[' => {
                if open.len() >= MAX_DEPTH {
                    return Err(cursor.error(JsonErrorKind::DepthLimit { limit: MAX_DEPTH }));
                }
                let object = byte == b'{';
                open.push(nodes.len());
                nodes.push(Node {
                    kind: if object { Kind::Object } else { Kind::Array },
                    start,
                    end: start,
                    subtree: 0,
                });
                cursor.advance();
                cursor.skip_whitespace();
                if cursor.peek() != Some(if object { b'}' } else { b']' }) {
                    if object {
                        read_member_name(&mut cursor, &mut nodes)?;
                    }
                    continue 'value;
                }
                // An empty container: fall through with the closing
                // bracket still unread, so the ascent below closes it
                // through the same path every other container takes.
            }
            _ => read_scalar(&mut cursor, &mut nodes, byte)?,
        }

        loop {
            let Some(&parent) = open.last() else {
                break 'value;
            };
            let object = nodes
                .get(parent)
                .is_some_and(|node| node.kind == Kind::Object);
            let close = if object { b'}' } else { b']' };
            cursor.skip_whitespace();
            match cursor.peek() {
                Some(b',') => {
                    cursor.advance();
                    cursor.skip_whitespace();
                    if cursor.peek() == Some(close) {
                        return Err(cursor.error(JsonErrorKind::TrailingComma));
                    }
                    if object {
                        read_member_name(&mut cursor, &mut nodes)?;
                    }
                    continue 'value;
                }
                Some(found) if found == close => {
                    cursor.advance();
                    let total = nodes.len();
                    if let Some(node) = nodes.get_mut(parent) {
                        node.end = cursor.at;
                        node.subtree = total.saturating_sub(parent);
                    }
                    open.pop();
                }
                Some(_) => {
                    let found = cursor.here();
                    return Err(cursor.error(if object {
                        JsonErrorKind::ExpectedCommaOrBraceClose { found }
                    } else {
                        JsonErrorKind::ExpectedCommaOrBracketClose { found }
                    }));
                }
                None => {
                    return Err(cursor.error(JsonErrorKind::EndOfDocument {
                        expected: if object {
                            "`,` or `}` after a member"
                        } else {
                            "`,` or `]` after an element"
                        },
                    }));
                }
            }
        }
    }

    cursor.skip_whitespace();
    if cursor.peek().is_some() {
        return Err(cursor.error(JsonErrorKind::TrailingText {
            found: cursor.here(),
        }));
    }
    Ok(nodes)
}

/// A member name and the colon after it.
///
/// The name is pushed as an ordinary string node. Nothing marks it as a
/// name, because nothing needs to: an object's children are written name,
/// value, name, value, in the order they were read, and the only walk
/// that reaches them knows which is which by its position. A separate
/// kind would be a second thing to keep true.
fn read_member_name(cursor: &mut Cursor<'_>, nodes: &mut Vec<Node>) -> Result<(), JsonError> {
    cursor.skip_whitespace();
    match cursor.peek() {
        Some(b'"') => read_string(cursor, nodes)?,
        Some(_) => {
            return Err(cursor.error(JsonErrorKind::ExpectedKey {
                found: cursor.here(),
            }));
        }
        None => {
            return Err(cursor.error(JsonErrorKind::EndOfDocument {
                expected: "a member name",
            }));
        }
    }
    cursor.skip_whitespace();
    match cursor.peek() {
        Some(b':') => {
            cursor.advance();
            Ok(())
        }
        Some(_) => Err(cursor.error(JsonErrorKind::ExpectedColon {
            found: cursor.here(),
        })),
        None => Err(cursor.error(JsonErrorKind::EndOfDocument {
            expected: "the `:` after a member name",
        })),
    }
}

/// Everything that is not a bracket: a string, a word, or a number.
fn read_scalar(cursor: &mut Cursor<'_>, nodes: &mut Vec<Node>, byte: u8) -> Result<(), JsonError> {
    match byte {
        b'"' => read_string(cursor, nodes),
        // `.` joins the number path rather than falling to "no value
        // begins here": `.5` is a number somebody wrote wrong, and
        // saying so is more use than saying a full stop begins nothing.
        b'-' | b'.' | b'0'..=b'9' => read_number(cursor, nodes),
        b'+' => Err(cursor.error(JsonErrorKind::LeadingPlus)),
        _ if byte.is_ascii_alphabetic() => read_word(cursor, nodes),
        _ => Err(cursor.error(JsonErrorKind::NoValue {
            found: cursor.here(),
        })),
    }
}

/// `true`, `false`, `null` — or a word from a language that is not JSON.
fn read_word(cursor: &mut Cursor<'_>, nodes: &mut Vec<Node>) -> Result<(), JsonError> {
    let start = cursor.at;
    let word = cursor.word();
    let kind = match word {
        "true" | "false" => Kind::Bool,
        "null" => Kind::Null,
        _ => {
            let kind = if not_json_word(word) {
                JsonErrorKind::NonJsonLiteral {
                    word: word.to_owned(),
                }
            } else {
                JsonErrorKind::BadLiteral {
                    found: word.to_owned(),
                }
            };
            return Err(JsonError::new(start, kind));
        }
    };
    nodes.push(Node {
        kind,
        start,
        end: cursor.at,
        subtree: 1,
    });
    Ok(())
}

/// A string, checked to its closing quote and left where it lies.
///
/// Every escape is *validated* here and none is decoded. That is the
/// split the whole crate turns on: a member name is compared far more
/// often than it is read, and comparing needs no buffer, so decoding at
/// parse time would allocate for every name in a document to serve the
/// rare caller who wanted an owned string.
fn read_string(cursor: &mut Cursor<'_>, nodes: &mut Vec<Node>) -> Result<(), JsonError> {
    let start = cursor.at;
    cursor.advance();
    loop {
        let Some(byte) = cursor.peek() else {
            return Err(JsonError::new(
                start,
                JsonErrorKind::EndOfDocument {
                    expected: "the closing quote of a string",
                },
            ));
        };
        match byte {
            b'"' => {
                cursor.advance();
                break;
            }
            b'\\' => read_escape(cursor)?,
            // The control characters, which the format spells with
            // escapes and never raw. A literal newline between quotes is
            // a writer that forgot, and the message should say that
            // rather than "a string character was expected here".
            0x00..=0x1F => {
                return Err(cursor.error(JsonErrorKind::ControlCharacterInString { byte }));
            }
            // Every other byte, continuation bytes included. They can be
            // walked one at a time because none of the cases above is a
            // continuation byte, so the cursor only ever stops on a
            // character boundary.
            _ => cursor.advance(),
        }
    }
    nodes.push(Node {
        kind: Kind::String,
        start,
        end: cursor.at,
        subtree: 1,
    });
    Ok(())
}

/// One escape, from its backslash to its last character.
fn read_escape(cursor: &mut Cursor<'_>) -> Result<(), JsonError> {
    let start = cursor.at;
    cursor.advance();
    let Some(byte) = cursor.peek() else {
        return Err(JsonError::new(
            start,
            JsonErrorKind::EndOfDocument {
                expected: "the character after a `\\`",
            },
        ));
    };
    match byte {
        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
            cursor.advance();
            Ok(())
        }
        b'u' => {
            cursor.advance();
            let code = read_four_hex_digits(cursor)?;
            if is_high_surrogate(code) {
                // A character outside the basic plane is written as two
                // escapes and means nothing as one. The second must
                // follow immediately: a high surrogate then a plain
                // character is a name that would silently change if this
                // reader substituted anything for the missing half.
                if cursor.peek() != Some(b'\\') || cursor.next_peek() != Some(b'u') {
                    return Err(JsonError::new(
                        start,
                        JsonErrorKind::LoneHighSurrogate { code },
                    ));
                }
                cursor.advance();
                cursor.advance();
                let low = read_four_hex_digits(cursor)?;
                if !is_low_surrogate(low) {
                    return Err(JsonError::new(
                        start,
                        JsonErrorKind::LoneHighSurrogate { code },
                    ));
                }
                Ok(())
            } else if is_low_surrogate(code) {
                Err(JsonError::new(
                    start,
                    JsonErrorKind::LoneLowSurrogate { code },
                ))
            } else {
                Ok(())
            }
        }
        _ => Err(cursor.error(JsonErrorKind::BadEscape {
            found: cursor.here(),
        })),
    }
}

const fn is_high_surrogate(code: u32) -> bool {
    code >= 0xD800 && code < 0xDC00
}

const fn is_low_surrogate(code: u32) -> bool {
    code >= 0xDC00 && code < 0xE000
}

/// The four digits of a `\u` escape, as the number they spell.
fn read_four_hex_digits(cursor: &mut Cursor<'_>) -> Result<u32, JsonError> {
    let mut code = 0u32;
    for _ in 0..4 {
        let Some(byte) = cursor.peek() else {
            return Err(cursor.error(JsonErrorKind::EndOfDocument {
                expected: "four hexadecimal digits after `\\u`",
            }));
        };
        let Some(digit) = char::from(byte).to_digit(16) else {
            return Err(cursor.error(JsonErrorKind::BadHexEscape {
                found: cursor.here(),
            }));
        };
        // Four hexadecimal digits reach 0xFFFF and no further, so this
        // cannot leave a `u32` however hostile the digits are.
        code = code * 16 + digit;
        cursor.advance();
    }
    Ok(code)
}

/// A number, checked against JSON's grammar rather than the standard
/// library's.
///
/// **The two are not the same, and the difference is the reason this
/// function exists.** Rust's float parser accepts `01`, `1.`, `.5` and
/// `+1`; JSON accepts none of them. A reader that scanned a plausible
/// run of characters and handed it to `from_str` would take documents
/// no other reader takes, which is the quiet kind of wrong: the file
/// works here and fails everywhere else, and the report comes back as
/// "your exporter is broken" against a reader that was.
fn read_number(cursor: &mut Cursor<'_>, nodes: &mut Vec<Node>) -> Result<(), JsonError> {
    let start = cursor.at;
    if cursor.peek() == Some(b'-') {
        cursor.advance();
        // `-Infinity` is the spelling a float writer reaches for, and it
        // arrives here rather than at the word path because of its sign.
        if cursor.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
            let word = cursor.word();
            let kind = if not_json_word(word) {
                JsonErrorKind::NonJsonLiteral {
                    word: format!("-{word}"),
                }
            } else {
                JsonErrorKind::NoIntegerDigits
            };
            return Err(JsonError::new(start, kind));
        }
    }

    match cursor.peek() {
        Some(b'0') => {
            cursor.advance();
            if cursor.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                return Err(JsonError::new(start, JsonErrorKind::LeadingZero));
            }
        }
        Some(b'1'..=b'9') => skip_digits(cursor),
        _ => return Err(JsonError::new(start, JsonErrorKind::NoIntegerDigits)),
    }

    if cursor.peek() == Some(b'.') {
        cursor.advance();
        if !cursor.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            return Err(cursor.error(JsonErrorKind::NoFractionDigits));
        }
        skip_digits(cursor);
    }

    if matches!(cursor.peek(), Some(b'e' | b'E')) {
        cursor.advance();
        if matches!(cursor.peek(), Some(b'+' | b'-')) {
            cursor.advance();
        }
        if !cursor.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            return Err(cursor.error(JsonErrorKind::NoExponentDigits));
        }
        skip_digits(cursor);
    }

    let len = cursor.at.saturating_sub(start);
    if len > MAX_NUMBER_LEN {
        return Err(JsonError::new(
            start,
            JsonErrorKind::NumberTooLong {
                len,
                limit: MAX_NUMBER_LEN,
            },
        ));
    }
    nodes.push(Node {
        kind: Kind::Number,
        start,
        end: cursor.at,
        subtree: 1,
    });
    Ok(())
}

fn skip_digits(cursor: &mut Cursor<'_>) {
    while cursor.peek().is_some_and(|byte| byte.is_ascii_digit()) {
        cursor.advance();
    }
}

#[cfg(test)]
mod tests {
    use crate::{Json, JsonErrorKind, Kind, MAX_DEPTH, MAX_NUMBER_LEN};

    /// The refusal a document gets, or a panic naming the document that
    /// was supposed to be refused.
    fn refusal(text: &str) -> JsonErrorKind {
        Json::parse(text.as_bytes())
            .err()
            .unwrap_or_else(|| panic!("`{text}` was accepted and should not have been"))
            .kind()
            .clone()
    }

    fn accepts(text: &str) -> Kind {
        Json::parse(text.as_bytes())
            .unwrap_or_else(|error| panic!("`{text}` was refused: {error}"))
            .root()
            .kind()
    }

    /// **A document cut short says which piece went missing**, at every
    /// place the grammar can be waiting for one.
    ///
    /// Eight places, eight prefixes. This is the test that stops
    /// `EndOfDocument` from becoming one message doing seven jobs badly:
    /// each `expected` is asserted by its own words, so a call site that
    /// starts passing another's string is a failure here rather than a
    /// diagnostic nobody can act on.
    ///
    /// Probed by making the escape site say "a value" like the first
    /// one: the `\` case fails, reporting that a string cut after its
    /// backslash claimed to be waiting for a value.
    #[test]
    fn every_place_the_document_can_end_early_says_what_it_wanted() {
        let cases = [
            ("[", "a value"),
            ("[1,", "a value"),
            ("[1", "`,` or `]` after an element"),
            ("{\"a\":1", "`,` or `}` after a member"),
            ("{", "a member name"),
            ("{\"a\"", "the `:` after a member name"),
            ("\"abc", "the closing quote of a string"),
            ("\"\\", "the character after a `\\`"),
            ("\"\\u12", "four hexadecimal digits after `\\u`"),
        ];
        let mut said = std::collections::BTreeSet::new();
        for (text, expected) in cases {
            // Compared as a whole refusal rather than matched and
            // unwrapped: a prefix that answers something else entirely
            // prints both sides here, where a match arm would have to
            // carry a message of its own that nothing ever reads.
            assert_eq!(
                refusal(text),
                JsonErrorKind::EndOfDocument { expected },
                "`{text}` did not end waiting for {expected}"
            );
            said.insert(expected);
        }
        assert_eq!(
            said.len(),
            8,
            "two of these places say the same thing, so one of them cannot be acted on: {said:?}"
        );
    }

    /// **Nesting is bounded, and the bound is the stated one.**
    ///
    /// The pair matters more than either half. A parser that refused at
    /// some depth would pass a test that only checked the deep case, and
    /// a parser with no bound at all would pass a test that only checked
    /// the shallow one. What is asserted here is that the wall is
    /// exactly where the constant says.
    ///
    /// Probed by making the check `>` rather than `>=`: the sixty-five
    /// deep case is accepted, and the failure prints the whole document
    /// that got through.
    #[test]
    fn nesting_stops_exactly_at_the_stated_depth() {
        let nest = |depth: usize| {
            let mut text = "[".repeat(depth);
            text.push_str(&"]".repeat(depth));
            text
        };
        assert_eq!(accepts(&nest(MAX_DEPTH)), Kind::Array);
        assert_eq!(
            refusal(&nest(MAX_DEPTH + 1)),
            JsonErrorKind::DepthLimit { limit: MAX_DEPTH }
        );
        // The depth is nesting, not length: a flat document of the same
        // size is nowhere near it.
        let flat = format!("[{}]", vec!["1"; MAX_DEPTH * 4].join(","));
        assert_eq!(accepts(&flat), Kind::Array);
    }

    /// **A number's spelling is checked against JSON's grammar, not the
    /// standard library's.**
    ///
    /// The four in the second half are the ones `f64::from_str` accepts
    /// and JSON does not, which is exactly why the scanner is written by
    /// hand rather than delegating. A reader that took them would accept
    /// documents no other reader accepts.
    ///
    /// Probed by deleting the fraction-digit check: "`1.` was accepted
    /// and should not have been".
    #[test]
    fn a_number_is_spelled_the_way_json_spells_one() {
        for text in [
            "0",
            "-0",
            "1",
            "-1",
            "12",
            "1.5",
            "0.5",
            "1e5",
            "1E5",
            "1e+5",
            "1e-5",
            "1.5e-5",
            "-12.75e+3",
        ] {
            assert_eq!(accepts(text), Kind::Number, "`{text}` is a JSON number");
        }
        for (text, expected) in [
            ("+1", JsonErrorKind::LeadingPlus),
            ("01", JsonErrorKind::LeadingZero),
            ("-01", JsonErrorKind::LeadingZero),
            (".5", JsonErrorKind::NoIntegerDigits),
            ("-.5", JsonErrorKind::NoIntegerDigits),
            ("-", JsonErrorKind::NoIntegerDigits),
            ("-x", JsonErrorKind::NoIntegerDigits),
            ("1.", JsonErrorKind::NoFractionDigits),
            ("1.e5", JsonErrorKind::NoFractionDigits),
            ("1e", JsonErrorKind::NoExponentDigits),
            ("1e+", JsonErrorKind::NoExponentDigits),
        ] {
            assert_eq!(refusal(text), expected, "`{text}` is not a JSON number");
        }
    }

    /// **A number this long is a generator gone wrong**, and the wall is
    /// where the constant says it is.
    ///
    /// Probed by making the check `>=`: the number of exactly the limit
    /// is refused instead, and the failure quotes the refusal back — "a
    /// number written with 128 characters, and this reader reads 128".
    #[test]
    fn a_number_stops_being_read_at_the_stated_length() {
        let digits = |count: usize| "9".repeat(count);
        assert_eq!(accepts(&digits(MAX_NUMBER_LEN)), Kind::Number);
        assert_eq!(
            refusal(&digits(MAX_NUMBER_LEN + 1)),
            JsonErrorKind::NumberTooLong {
                len: MAX_NUMBER_LEN + 1,
                limit: MAX_NUMBER_LEN,
            }
        );
    }

    /// **The words other languages write where JSON has no value are
    /// named as what they are.**
    ///
    /// Every one of these comes from a writer that had a value JSON
    /// cannot carry and wrote it anyway. Saying "no value begins with
    /// `N`" would send a reader looking at the document; the fault is in
    /// whatever produced it.
    ///
    /// Probed by replacing `infinity` in the list with a word nothing
    /// writes: `Infinity` answers `BadLiteral` instead, which tells a
    /// reader to go looking for a misspelt `true`.
    #[test]
    fn a_word_from_another_language_is_named_as_one() {
        for text in [
            "NaN",
            "nan",
            "Infinity",
            "inf",
            "undefined",
            "None",
            "True",
            "False",
            "Null",
        ] {
            assert_eq!(
                refusal(text),
                JsonErrorKind::NonJsonLiteral {
                    word: text.to_owned()
                },
                "`{text}` is a value in some language and in no JSON document"
            );
        }
        assert_eq!(
            refusal("-Infinity"),
            JsonErrorKind::NonJsonLiteral {
                word: "-Infinity".to_owned()
            }
        );
        // And the three that are JSON keep working, which is what stops
        // the list above from swallowing them.
        assert_eq!(accepts("true"), Kind::Bool);
        assert_eq!(accepts("false"), Kind::Bool);
        assert_eq!(accepts("null"), Kind::Null);
        assert_eq!(
            refusal("tru"),
            JsonErrorKind::BadLiteral {
                found: "tru".to_owned()
            }
        );
        assert_eq!(
            refusal("truthy"),
            JsonErrorKind::BadLiteral {
                found: "truthy".to_owned()
            }
        );
    }

    /// **Whitespace after the value is skipped before the end is
    /// demanded.**
    ///
    /// This is the accommodation that lets a JSON chunk padded out to an
    /// alignment boundary by a container format parse here with no
    /// knowledge of the container. It costs one call and it is the only
    /// thing this reader does on any container's behalf.
    ///
    /// Probed by demanding the end before skipping: the padded case is
    /// refused — "at byte 7: ` ` follows the end of the document".
    #[test]
    fn padding_after_the_document_is_skipped_and_anything_else_is_not() {
        assert_eq!(accepts("{\"a\":1}   "), Kind::Object);
        assert_eq!(accepts(" \t\r\n[1]\t\r\n "), Kind::Array);
        assert_eq!(refusal("{} x"), JsonErrorKind::TrailingText { found: 'x' });
        assert_eq!(refusal("1 2"), JsonErrorKind::TrailingText { found: '2' });
        // A vertical tab is not one of JSON's four, so it is named
        // rather than shrugged at.
        assert_eq!(refusal("\u{b}1"), JsonErrorKind::NoValue { found: '\u{b}' });
    }

    /// **Every escape the format defines is accepted, and nothing else
    /// is.**
    ///
    /// The surrogate pair is the one worth its own case: a character
    /// outside the basic plane is written as two escapes by every writer
    /// that escapes non-ASCII at all, which is what a Python exporter
    /// does by default.
    ///
    /// Probed by accepting a high surrogate with no partner: "`"\uD800"`
    /// was accepted and should not have been".
    #[test]
    fn a_string_carries_every_escape_the_format_defines() {
        for text in [
            r#""\"""#,
            r#""\\""#,
            r#""\/""#,
            r#""\b""#,
            r#""\f""#,
            r#""\n""#,
            r#""\r""#,
            r#""\t""#,
            r#""A""#,
            "\" \"",
            // A non-character and a character outside the basic
            // plane, both written directly rather than escaped,
            // which is what a writer that does not escape emits.
            "\"\u{ffff}\"",
            "\"\u{1f600}\"",
            "\"\u{e9}\"",
            "\"\u{7f}\"",
        ] {
            assert_eq!(accepts(text), Kind::String, "`{text}` is a JSON string");
        }
        for (text, expected) in [
            (r#""\q""#, JsonErrorKind::BadEscape { found: 'q' }),
            (r#""\U0041""#, JsonErrorKind::BadEscape { found: 'U' }),
            (r#""\uZZZZ""#, JsonErrorKind::BadHexEscape { found: 'Z' }),
            (r#""\u00 1""#, JsonErrorKind::BadHexEscape { found: ' ' }),
            (
                r#""\uD800""#,
                JsonErrorKind::LoneHighSurrogate { code: 0xD800 },
            ),
            (
                r#""\uD800A""#,
                JsonErrorKind::LoneHighSurrogate { code: 0xD800 },
            ),
            (
                r#""\uD800A""#,
                JsonErrorKind::LoneHighSurrogate { code: 0xD800 },
            ),
            (
                r#""\uD800\\""#,
                JsonErrorKind::LoneHighSurrogate { code: 0xD800 },
            ),
            (
                r#""\uDC00""#,
                JsonErrorKind::LoneLowSurrogate { code: 0xDC00 },
            ),
            (
                "\"\t\"",
                JsonErrorKind::ControlCharacterInString { byte: b'\t' },
            ),
            (
                "\"\u{0}\"",
                JsonErrorKind::ControlCharacterInString { byte: 0 },
            ),
        ] {
            assert_eq!(refusal(text), expected, "`{text}` is not a JSON string");
        }
    }

    /// **A surrogate pair is refused unless both halves are the halves
    /// they claim to be.**
    ///
    /// Three ways a pair can be wrong, and the third is the one a
    /// scanner that merely looked for `\u` after `\u` would take: two
    /// high halves in a row, which is two escapes and no character.
    ///
    /// The documents are assembled rather than written, because a
    /// literal would have to escape its own backslash and what is under
    /// test is what the document says.
    ///
    /// Probed by accepting whatever the second escape decodes to:
    /// "`"\uD800\uD800"` was accepted and should not have been".
    #[test]
    fn a_surrogate_pair_needs_both_of_its_halves() {
        let pair = |first: &str, second: &str| {
            let mut text = String::from("\"");
            for code in [first, second] {
                text.push('\\');
                text.push('u');
                text.push_str(code);
            }
            text.push('"');
            text
        };
        assert_eq!(accepts(&pair("D83D", "DE00")), Kind::String);
        assert_eq!(accepts(&pair("0041", "0042")), Kind::String);
        assert_eq!(
            refusal(&pair("D800", "D800")),
            JsonErrorKind::LoneHighSurrogate { code: 0xD800 }
        );
        assert_eq!(
            refusal(&pair("D800", "0041")),
            JsonErrorKind::LoneHighSurrogate { code: 0xD800 }
        );
        assert_eq!(
            refusal(&pair("DC00", "DC00")),
            JsonErrorKind::LoneLowSurrogate { code: 0xDC00 }
        );
        // The second half is read with the same scanner as the first, so
        // a malformed one is refused as a malformed escape rather than
        // as a broken pair — which is the more specific answer of the
        // two, and the one that points at the characters actually wrong.
        assert_eq!(
            refusal(&pair("D800", "ZZZZ")),
            JsonErrorKind::BadHexEscape { found: 'Z' }
        );
    }

    /// **A container's own punctuation is checked**, including the two
    /// mistakes a person makes editing one by hand.
    ///
    /// Probed by deleting the trailing-comma check: `[1,]` answers
    /// `NoValue { found: ']' }`, which sends a reader looking for a
    /// missing value rather than at the comma they left behind.
    #[test]
    fn a_container_is_punctuated_the_way_the_format_says() {
        for text in [
            "{}",
            "[]",
            "[[]]",
            "{\"a\":{}}",
            "[{},[],1,\"x\",true,null]",
            "{ \"a\" : 1 , \"b\" : 2 }",
        ] {
            let kind = accepts(text);
            assert!(matches!(kind, Kind::Object | Kind::Array));
        }
        for (text, expected) in [
            ("[1,]", JsonErrorKind::TrailingComma),
            ("{\"a\":1,}", JsonErrorKind::TrailingComma),
            ("[,]", JsonErrorKind::NoValue { found: ',' }),
            ("{1:2}", JsonErrorKind::ExpectedKey { found: '1' }),
            ("{,}", JsonErrorKind::ExpectedKey { found: ',' }),
            ("{\"a\" 1}", JsonErrorKind::ExpectedColon { found: '1' }),
            (
                "{\"a\":1 \"b\":2}",
                JsonErrorKind::ExpectedCommaOrBraceClose { found: '"' },
            ),
            (
                "[1 2]",
                JsonErrorKind::ExpectedCommaOrBracketClose { found: '2' },
            ),
            (
                "[1}",
                JsonErrorKind::ExpectedCommaOrBracketClose { found: '}' },
            ),
            (
                "{\"a\":1]",
                JsonErrorKind::ExpectedCommaOrBraceClose { found: ']' },
            ),
        ] {
            assert_eq!(refusal(text), expected, "`{text}`");
        }
    }
}
