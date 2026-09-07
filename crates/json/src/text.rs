//! A string, still in the document it was read from.
//!
//! **Escapes are validated at parse time and decoded only when somebody
//! asks.** That split is the reason this type exists rather than a
//! `String` on the node. Almost every string a reader of asset metadata
//! touches, it touches as a comparison — is this member name
//! `POSITION`, is this one `indices` — and a comparison needs no buffer.
//! Decoding every string as it was read would allocate once per member
//! name in a file to serve the rare caller who wanted an owned copy.
//!
//! What a caller gets instead is [`Str::eq_str`], which compares without
//! allocating whether or not the text carries escapes;
//! [`Str::as_plain`], which hands back the source slice itself when
//! there is nothing to decode, which is the overwhelming majority; and
//! [`Str::decode_into`], which writes into a buffer the caller owns and
//! can reuse across a whole document.

use core::fmt;

/// A string value, or a member name, borrowed from the document.
///
/// The text inside is exactly what was between the quotes, escapes and
/// all. Every escape in it was checked when the document was parsed, so
/// nothing here can fail: there is no lone surrogate to trip over and no
/// truncated `\u` to run off the end of.
#[derive(Clone, Copy)]
pub struct Str<'a> {
    text: &'a str,
}

impl<'a> Str<'a> {
    pub(crate) const fn new(text: &'a str) -> Self {
        Self { text }
    }

    /// The characters between the quotes, exactly as they were written.
    ///
    /// Escapes are still escapes here. This is the string a caller wants
    /// to re-emit unchanged, or to hash without deciding what an escape
    /// means; [`decode_into`](Self::decode_into) is the one that reads
    /// them.
    #[must_use]
    pub const fn as_written(self) -> &'a str {
        self.text
    }

    /// The string itself, when it carries no escapes.
    ///
    /// `None` means there is at least one backslash and the text has to
    /// be decoded to be read. This is a fast path worth having because
    /// it is the usual case: a member name in a machine-written document
    /// almost never needs an escape.
    #[must_use]
    pub fn as_plain(self) -> Option<&'a str> {
        if self.text.contains('\\') {
            None
        } else {
            Some(self.text)
        }
    }

    /// The characters this string spells, escapes resolved.
    ///
    /// A surrogate pair comes back as the one character it spells, so
    /// `😀` is a single `char` rather than two halves of one.
    #[must_use]
    pub const fn chars(self) -> Chars<'a> {
        Chars { rest: self.text }
    }

    /// Does this string spell `other`?
    ///
    /// Allocation-free either way: identical when there is nothing to
    /// decode, and character by character when there is, so
    /// `"POSITION"` and `"POSITION"` compare equal without either
    /// of them being built.
    #[must_use]
    pub fn eq_str(self, other: &str) -> bool {
        self.as_plain()
            .map_or_else(|| self.chars().eq(other.chars()), |plain| plain == other)
    }

    /// Append the decoded characters to `out`.
    ///
    /// The caller owns the buffer, which is what lets one buffer serve a
    /// whole document instead of one allocation serving each string.
    pub fn decode_into(self, out: &mut String) {
        match self.as_plain() {
            Some(plain) => out.push_str(plain),
            None => out.extend(self.chars()),
        }
    }

    /// The decoded string, in a buffer of its own.
    ///
    /// The convenient spelling of [`decode_into`](Self::decode_into),
    /// for a caller that wants one string rather than a document's
    /// worth.
    #[must_use]
    pub fn decode(self) -> String {
        let mut out = String::new();
        self.decode_into(&mut out);
        out
    }
}

impl fmt::Display for Str<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for character in self.chars() {
            fmt::Write::write_char(f, character)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Str<'_> {
    /// The text as it was written, quoted — escapes shown as escapes.
    ///
    /// Deliberately not the decoded form: this is what a failing test
    /// prints, and the question a failing test is usually asking is what
    /// the document actually said.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", self.text)
    }
}

impl PartialEq for Str<'_> {
    /// Compared by what the strings spell, not by how they were spelled.
    fn eq(&self, other: &Self) -> bool {
        self.chars().eq(other.chars())
    }
}

impl Eq for Str<'_> {}

impl PartialEq<str> for Str<'_> {
    fn eq(&self, other: &str) -> bool {
        self.eq_str(other)
    }
}

impl PartialEq<&str> for Str<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.eq_str(other)
    }
}

/// The characters of a [`Str`], escapes resolved as they are reached.
///
/// Deliberately not `Copy`, though every field in it would allow it: a
/// copy of a half-walked iterator silently restarts, and an iterator is
/// the one place that is a bug rather than a convenience.
#[derive(Clone, Debug)]
pub struct Chars<'a> {
    rest: &'a str,
}

impl Iterator for Chars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        let mut walk = self.rest.chars();
        let first = walk.next()?;
        if first != '\\' {
            self.rest = walk.as_str();
            return Some(first);
        }
        // The parse refused a backslash at the end of a string, so there
        // is always a character here. The fallback keeps that fact from
        // being a panic if it ever stops being true.
        let marker = walk.next().unwrap_or('\\');
        let decoded = match marker {
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'u' => return Some(self.escaped_code_point(walk.as_str())),
            // `"`, `\` and `/` each escape themselves, so the mapping is
            // the identity and needs no table. Anything else cannot
            // reach here — the parse named it `BadEscape` — and taking
            // it as itself is the answer that invents the least.
            other => other,
        };
        self.rest = walk.as_str();
        Some(decoded)
    }
}

impl<'a> Chars<'a> {
    /// The character a `\u` escape spells, the cursor left after it.
    ///
    /// A high surrogate is followed by its low half, because the parse
    /// refused one that was not. Both halves are consumed and the pair
    /// becomes the one character it means.
    fn escaped_code_point(&mut self, after_marker: &'a str) -> char {
        let (code, rest) = four_hex_digits(after_marker);
        if (0xD800..0xDC00).contains(&code) {
            let paired = rest.strip_prefix("\\u").unwrap_or(rest);
            let (low, tail) = four_hex_digits(paired);
            self.rest = tail;
            // The arithmetic the format defines for a surrogate pair.
            // Every input to it is in range because the parse checked
            // both halves, so the result is always a character.
            let combined = 0x1_0000 + ((code - 0xD800) << 10) + (low - 0xDC00);
            char::from_u32(combined).unwrap_or(char::REPLACEMENT_CHARACTER)
        } else {
            self.rest = rest;
            char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
        }
    }
}

/// Four hexadecimal digits as the number they spell, and what follows.
///
/// The digits are there because the parse checked them. A missing or
/// malformed one reads as zero rather than as a panic, which is the
/// quietest wrong answer available and the only one this crate's
/// contract allows.
fn four_hex_digits(text: &str) -> (u32, &str) {
    let mut code = 0u32;
    let mut walk = text.chars();
    for _ in 0..4 {
        let digit = walk.next().and_then(|c| c.to_digit(16)).unwrap_or(0);
        code = code * 16 + digit;
    }
    (code, walk.as_str())
}

#[cfg(test)]
mod tests {
    use crate::Json;

    /// The character a code point names, for an expectation.
    fn character(code: u32) -> String {
        char::from_u32(code)
            .unwrap_or_else(|| panic!("{code:#x} is a character"))
            .to_string()
    }

    /// A JSON string spelling `codes` as unicode escapes, wrapped in
    /// `before` and `after`.
    ///
    /// Assembled from its pieces rather than written as a literal. A
    /// literal would have to escape its own backslash, and this file's
    /// whole subject is what the *document* says — keeping every one of
    /// these out of the source means no reader has to work out which
    /// layer of escaping they are looking at.
    fn escaped(before: &str, codes: &[&str], after: &str) -> String {
        let mut out = String::from("\"");
        out.push_str(before);
        for code in codes {
            out.push('\\');
            out.push('u');
            out.push_str(code);
        }
        out.push_str(after);
        out.push('"');
        out
    }

    /// The one string in a document of one string.
    fn only(text: &str) -> String {
        let document = Json::parse(text.as_bytes())
            .unwrap_or_else(|error| panic!("`{text}` was refused: {error}"));
        document
            .root()
            .as_str()
            .unwrap_or_else(|error| panic!("`{text}` is not a string: {error}"))
            .decode()
    }

    /// **Every escape decodes to the character it names.**
    ///
    /// The table is the format's own, and two entries in it are the ones
    /// a table nobody checked gets wrong: the backspace and the form
    /// feed, neither of which has a Rust escape of its own, so both are
    /// easy to write as something else and never notice.
    ///
    /// Probed by decoding the backspace escape as a form feed: that case
    /// fails, printing both by their code points — `\u{c}` against
    /// `\u{8}`.
    #[test]
    fn every_escape_decodes_to_the_character_it_names() {
        assert_eq!(only(r#""\"""#), "\"");
        assert_eq!(only(r#""\\""#), "\\");
        assert_eq!(only(r#""\/""#), "/");
        assert_eq!(only(r#""\b""#), character(0x08));
        assert_eq!(only(r#""\f""#), character(0x0c));
        assert_eq!(only(r#""\n""#), "\n");
        assert_eq!(only(r#""\r""#), "\r");
        assert_eq!(only(r#""\t""#), "\t");
        assert_eq!(only(r#""A""#), "A");
        assert_eq!(only(r#""a\tb""#), "a\tb");
        assert_eq!(only(&escaped("", &["0041"], "")), "A");
        assert_eq!(only(&escaped("", &["0000"], "")), character(0x00));
        // Two escapes running together, and a non-character, which is
        // legal text and not this reader's business to object to.
        assert_eq!(
            only(&escaped("", &["00e9", "FFFF"], "")),
            format!("{}{}", character(0xE9), character(0xFFFF))
        );
        // And plain characters either side of an escape, so the walk has
        // to come back out of one.
        assert_eq!(only(&escaped("a", &["0042"], "c")), "aBc");
    }

    /// **A surrogate pair decodes to the one character it spells.**
    ///
    /// This is what a writer that escapes every non-ASCII character
    /// emits for anything outside the basic plane — which is what
    /// Python's own JSON writer does by default — so an exported object
    /// name with an emoji in it arrives here as two escapes and has to
    /// come back as one character.
    ///
    /// Probed by shifting the high half nine bits instead of ten: the
    /// pair decodes to a different character in a different script, and
    /// the first case fails printing both.
    #[test]
    fn a_surrogate_pair_decodes_to_one_character() {
        let grin = character(0x1_F600);
        assert_eq!(only(&escaped("", &["D83D", "DE00"], "")), grin);
        assert_eq!(
            only(&escaped("a", &["D83D", "DE00"], "b")),
            format!("a{grin}b")
        );

        // One character, not two halves of one.
        let source = escaped("", &["D83D", "DE00"], "");
        let document = Json::parse(source.as_bytes()).expect("a paired surrogate");
        let text = document.root().as_str().expect("a string");
        assert_eq!(text.chars().count(), 1);
    }

    /// **A string with nothing to decode is handed back where it lies.**
    ///
    /// The fast path is the usual path — a member name in a
    /// machine-written document almost never carries an escape — and it
    /// is what makes comparing a name free. The assertion is on the
    /// address, not the contents: equal contents would pass with a copy,
    /// which is exactly the thing this is here to forbid.
    ///
    /// Probed by making `as_plain` always answer `None`: the test stops
    /// before the address assertion, on "no escapes to decode" — which
    /// is the fast path being gone rather than being wrong.
    #[test]
    fn a_string_without_escapes_is_borrowed_rather_than_built() {
        let source = br#"{"POSITION": "hull"}"#;
        let document = Json::parse(source).expect("a small object");
        let value = document.root().get("POSITION").expect("the member");
        let text = value.as_str().expect("a string");
        let plain = text.as_plain().expect("no escapes to decode");
        assert_eq!(plain, "hull");
        // Inside the very bytes that were handed to the parser.
        let base = source.as_ptr() as usize;
        let borrowed = plain.as_ptr() as usize;
        assert!(
            borrowed >= base && borrowed < base.saturating_add(source.len()),
            "the string was copied out of the document rather than borrowed from it"
        );

        let escaped = Json::parse(br#""a\tb""#).expect("an escaped string");
        assert!(
            escaped
                .root()
                .as_str()
                .expect("a string")
                .as_plain()
                .is_none(),
            "a string with an escape in it cannot be handed back unread"
        );
    }

    /// **Comparing a name costs no allocation, escaped or not.**
    ///
    /// Both paths answer the same question the same way, which is the
    /// property worth pinning: a document that spells a member name with
    /// escapes names the same member as one that does not, and a reader
    /// that only compared the raw text would disagree.
    ///
    /// Probed by comparing the raw text in every case: the assertion
    /// `escaped.eq_str("A")` fails, because the escaped `A` stops
    /// being an `A`.
    #[test]
    fn a_string_compares_by_what_it_spells() {
        let source = format!("[\"A\", {}, \"B\"]", escaped("", &["0041"], ""));
        let document = Json::parse(source.as_bytes()).expect("three strings");
        let mut values = document.root().elements().expect("an array");
        let plain = values
            .next()
            .expect("the first")
            .as_str()
            .expect("a string");
        let escaped = values
            .next()
            .expect("the second")
            .as_str()
            .expect("a string");
        let other = values
            .next()
            .expect("the third")
            .as_str()
            .expect("a string");

        assert!(plain.eq_str("A"));
        assert!(escaped.eq_str("A"));
        assert!(!escaped.eq_str("B"));
        assert_eq!(plain, escaped);
        assert_ne!(plain, other);
        assert_eq!(plain, *"A");
        assert_eq!(escaped, "A");
    }

    /// **A string can be read into a buffer the caller owns**, and the
    /// two spellings of that agree.
    ///
    /// Probed by making `decode_into` clear the buffer first: the buffer
    /// comes back as `plain` rather than `prefix:a\tbplain` — both the
    /// prefix and the first string gone.
    #[test]
    fn decoding_appends_to_the_caller_s_buffer() {
        let document = Json::parse(br#"["a\tb", "plain"]"#).expect("two strings");
        let mut walk = document.root().elements().expect("an array");
        let escaped = walk.next().expect("the first").as_str().expect("a string");
        let plain = walk.next().expect("the second").as_str().expect("a string");

        let mut buffer = String::from("prefix:");
        escaped.decode_into(&mut buffer);
        plain.decode_into(&mut buffer);
        assert_eq!(buffer, "prefix:a\tbplain");
        assert_eq!(escaped.decode(), "a\tb");
    }

    /// **The two written forms say what they are for.**
    ///
    /// `Display` is the decoded string, because that is what a message
    /// about a name should carry. `Debug` is the source text, because
    /// what a failing test is usually asking is what the document
    /// actually said — and `as_written` is that same text as a value.
    ///
    /// Probed by making `Debug` print the decoded form: the tab in the
    /// document prints as a real tab, and the case fails against the
    /// two-character escape it was written as — `"a\tb"` either way on
    /// the page, and not the same string.
    #[test]
    fn a_string_prints_decoded_and_debugs_as_written() {
        let document = Json::parse(br#""a\tb""#).expect("an escaped string");
        let text = document.root().as_str().expect("a string");
        assert_eq!(text.to_string(), "a\tb");
        assert_eq!(format!("{text:?}"), r#""a\tb""#);
        assert_eq!(text.as_written(), r"a\tb");
    }
}
