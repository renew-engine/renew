//! Minimal JSON emission and parsing.
//!
//! Emission serves the `--json` output mode; parsing serves reading
//! `cargo metadata` output for the structure check. Both are hand-rolled:
//! the surfaces are small and this keeps the binary dependency-free.

use core::fmt;

/// A JSON value. Object fields keep insertion order. `Number` covers the
/// integral values this crate emits; `Float` exists for parsed input whose
/// numbers are not exactly representable as `i64`.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    String(String),
    Number(i64),
    Float(f64),
    Bool(bool),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// Render the value as compact JSON.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    /// Object field lookup by key; `None` on non-objects. With duplicate
    /// keys (which the parser retains), the first match wins.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Object(fields) => fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Self::Object(fields) => Some(fields),
            _ => None,
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::String(text) => {
                out.push('"');
                escape_into(text, out);
                out.push('"');
            }
            Self::Null => out.push_str("null"),
            Self::Number(number) => out.push_str(&number.to_string()),
            Self::Float(number) => out.push_str(&number.to_string()),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Self::Object(fields) => {
                out.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    escape_into(key, out);
                    out.push_str("\":");
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn escape_into(text: &str, out: &mut String) {
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                use core::fmt::Write as _;

                let code = control as u32;
                let _ = write!(out, "\\u{code:04x}");
            }
            other => out.push(other),
        }
    }
}

/// Build the standard result document every subcommand emits in `--json`
/// mode. `schema_version` is always the first field.
#[must_use]
pub fn result_envelope(
    command: &str,
    status: &str,
    exit_code: i64,
    duration_ms: i64,
    stdout: &str,
    stderr: &str,
) -> Value {
    Value::Object(vec![
        ("schema_version".to_string(), Value::Number(1)),
        ("command".to_string(), Value::String(command.to_string())),
        ("status".to_string(), Value::String(status.to_string())),
        ("exit_code".to_string(), Value::Number(exit_code)),
        ("duration_ms".to_string(), Value::Number(duration_ms)),
        ("stdout".to_string(), Value::String(stdout.to_string())),
        ("stderr".to_string(), Value::String(stderr.to_string())),
    ])
}

/// Why parsing failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonError {
    Syntax { at: usize, expected: &'static str },
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { at, expected } => {
                write!(f, "malformed JSON at byte {at}: expected {expected}")
            }
        }
    }
}

impl std::error::Error for JsonError {}

/// Maximum container nesting the parser accepts. The parser is recursive,
/// so unbounded nesting would exhaust the stack; real `cargo metadata`
/// documents are a handful of levels deep.
const MAX_DEPTH: usize = 128;

/// Parse one JSON document.
///
/// # Errors
///
/// Returns [`JsonError`] on any syntax violation, including trailing data
/// after the document and container nesting deeper than 128 levels (the
/// recursion bound that keeps hostile input from exhausting the stack).
pub fn parse(text: &str) -> Result<Value, JsonError> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        at: 0,
        depth: 0,
    };
    parser.skip_ws();
    let value = parser.value()?;
    parser.skip_ws();
    if parser.at == parser.bytes.len() {
        Ok(value)
    } else {
        Err(JsonError::Syntax {
            at: parser.at,
            expected: "end of document",
        })
    }
}

struct Parser<'a> {
    /// The document, kept beside its bytes. Scanning happens over the
    /// bytes; slicing happens over the text, which is why no scanned run
    /// ever needs UTF-8 revalidation.
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    /// The document text from `start` to the cursor.
    ///
    /// Every position the scanner can stop at is a character boundary: it
    /// starts on one and only ever steps over ASCII bytes (`"`, `\`,
    /// controls, digits, exponent punctuation), never into the middle of a
    /// multi-byte sequence. The fallback therefore cannot be reached, and
    /// yields the empty string rather than inventing an error case no
    /// `&str` input can produce.
    fn slice(&self, start: usize) -> &'a str {
        // The empty fallback is only correct while both ends really are
        // boundaries. This is a parser of untrusted input: if a future
        // scanner change stops mid-codepoint, a silently truncated value
        // is far worse than a loud failure, so dev builds say so.
        debug_assert!(
            self.text.is_char_boundary(start) && self.text.is_char_boundary(self.at),
            "the scanner stopped inside a multi-byte sequence"
        );
        self.text.get(start..self.at).unwrap_or_default()
    }

    fn err(&self, expected: &'static str) -> JsonError {
        JsonError::Syntax {
            at: self.at,
            expected,
        }
    }

    fn enter(&mut self) -> Result<(), JsonError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            Err(self.err("nesting no deeper than 128 levels"))
        } else {
            Ok(())
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8, expected: &'static str) -> Result<(), JsonError> {
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.err(expected))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, JsonError> {
        if self
            .bytes
            .get(self.at..)
            .is_some_and(|rest| rest.starts_with(word.as_bytes()))
        {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.err("a literal"))
        }
    }

    fn value(&mut self) -> Result<Value, JsonError> {
        self.skip_ws();
        match self.bytes.get(self.at) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(_) => self.number(),
            None => Err(self.err("a value")),
        }
    }

    fn object(&mut self) -> Result<Value, JsonError> {
        self.enter()?;
        let result = self.object_body();
        self.depth -= 1;
        result
    }

    fn object_body(&mut self) -> Result<Value, JsonError> {
        self.eat(b'{', "`{`")?;
        let mut fields = Vec::new();
        self.skip_ws();
        if self.bytes.get(self.at) == Some(&b'}') {
            self.at += 1;
            return Ok(Value::Object(fields));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.eat(b':', "`:`")?;
            let value = self.value()?;
            fields.push((key, value));
            self.skip_ws();
            match self.bytes.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(fields));
                }
                _ => return Err(self.err("`,` or `}`")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, JsonError> {
        self.enter()?;
        let result = self.array_body();
        self.depth -= 1;
        result
    }

    fn array_body(&mut self) -> Result<Value, JsonError> {
        self.eat(b'[', "`[`")?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.bytes.get(self.at) == Some(&b']') {
            self.at += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.bytes.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.err("`,` or `]`")),
            }
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.eat(b'"', "`\"`")?;
        let mut out = String::new();
        loop {
            let start = self.at;
            // Take unescaped runs in one slice for correct UTF-8 handling.
            while self
                .bytes
                .get(self.at)
                .is_some_and(|byte| *byte != b'"' && *byte != b'\\' && *byte >= 0x20)
            {
                self.at += 1;
            }
            if self.at > start {
                out.push_str(self.slice(start));
            }
            match self.bytes.get(self.at) {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    self.escape_into(&mut out)?;
                }
                _ => return Err(self.err("a string character")),
            }
        }
    }

    fn escape_into(&mut self, out: &mut String) -> Result<(), JsonError> {
        let escape = self
            .bytes
            .get(self.at)
            .copied()
            .ok_or_else(|| self.err("an escape"))?;
        self.at += 1;
        match escape {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{8}'),
            b'f' => out.push('\u{c}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let mut scalar = self.hex4()?;
                // Surrogate pairs: cargo metadata output is plain, but the
                // grammar allows them; decode or reject cleanly.
                if (0xD800..=0xDBFF).contains(&scalar) {
                    self.eat(b'\\', "a low surrogate")?;
                    self.eat(b'u', "a low surrogate")?;
                    let low = self.hex4()?;
                    if !(0xDC00..=0xDFFF).contains(&low) {
                        return Err(self.err("a low surrogate"));
                    }
                    scalar = 0x10000 + ((scalar - 0xD800) << 10) + (low - 0xDC00);
                }
                // One conversion for both shapes. A decoded pair is always
                // a scalar value; the check that bites is the lone
                // surrogate arriving unpaired.
                out.push(char::from_u32(scalar).ok_or_else(|| self.err("a valid code point"))?);
            }
            _ => return Err(self.err("a valid escape")),
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let mut code: u32 = 0;
        for _ in 0..4 {
            let digit = self
                .bytes
                .get(self.at)
                .and_then(|byte| char::from(*byte).to_digit(16))
                .ok_or_else(|| self.err("four hex digits"))?;
            code = code * 16 + digit;
            self.at += 1;
        }
        Ok(code)
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.at;
        if self.bytes.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        while self.bytes.get(self.at).is_some_and(|byte| {
            byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-')
        }) {
            self.at += 1;
        }
        let lexeme = self.slice(start);
        if !lexeme.contains(['.', 'e', 'E'])
            && let Ok(integer) = lexeme.parse::<i64>()
        {
            return Ok(Value::Number(integer));
        }
        match lexeme.parse::<f64>() {
            // JSON has no representation for non-finite numbers; accepting
            // them would break the render round-trip.
            Ok(float) if float.is_finite() => Ok(Value::Float(float)),
            _ => Err(JsonError::Syntax {
                at: start,
                expected: "a finite number",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_escape_quotes_backslashes_and_control_characters() {
        let value = Value::String("a\"b\\c\nd\re\tf\u{1}g".to_string());
        assert_eq!(value.render(), "\"a\\\"b\\\\c\\nd\\re\\tf\\u0001g\"");
    }

    #[test]
    fn non_ascii_text_passes_through_unescaped() {
        let value = Value::String("çağdaş ✔".to_string());
        assert_eq!(value.render(), "\"çağdaş ✔\"");
    }

    #[test]
    fn objects_preserve_field_order() {
        let value = Value::Object(vec![
            ("b".to_string(), Value::Number(2)),
            ("a".to_string(), Value::Number(1)),
        ]);
        assert_eq!(value.render(), "{\"b\":2,\"a\":1}");
    }

    #[test]
    fn arrays_and_booleans_render_compactly() {
        let value = Value::Array(vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Number(-3),
        ]);
        assert_eq!(value.render(), "[true,false,-3]");
    }

    #[test]
    fn parse_round_trips_what_the_emitter_writes() {
        let original = result_envelope("check", "ok", 0, 7, "a\"b\\c\nd", "çağdaş ✔");
        let parsed = parse(&original.render()).expect("emitter output parses");
        assert_eq!(parsed, original);
    }

    #[test]
    fn parse_handles_nesting_numbers_and_literals() {
        let document = r#"{"a":[1,-2,3.5,1e2,true,false,null],"b":{"c":"d"},"e":9007199254740993}"#;
        let parsed = parse(document).expect("valid document parses");
        assert_eq!(
            parsed
                .get("a")
                .and_then(Value::as_array)
                .map(<[Value]>::len),
            Some(7)
        );
        assert_eq!(
            parsed
                .get("a")
                .and_then(|a| a.as_array())
                .and_then(|a| a.first()),
            Some(&Value::Number(1))
        );
        assert_eq!(
            parsed
                .get("b")
                .and_then(|b| b.get("c"))
                .and_then(Value::as_str),
            Some("d")
        );
        // Large but exact i64 stays integral.
        assert_eq!(parsed.get("e"), Some(&Value::Number(9_007_199_254_740_993)));
    }

    #[test]
    fn parse_decodes_unicode_escapes_and_surrogate_pairs() {
        assert_eq!(
            parse(r#""\u00e7a\ud83d\ude00""#).expect("escapes parse"),
            Value::String("ça😀".to_string())
        );
    }

    #[test]
    fn parse_rejects_malformed_documents() {
        for bad in [
            "",
            "{",
            "[1,",
            "{\"a\":}",
            "{\"a\":1,}",
            "tru",
            "\"unterminated",
            "\"bad \\q escape\"",
            "1 2",
            "{\"a\":1}garbage",
            "\"\\ud800\"",
            "-",
            "\"\\udc00\"",
            "\"\\uZZZZ\"",
            "\"\\ud800\\u0041\"",
            "\"\\ud800\\ue000\"",
            "{1:2}",
            "1e999",
            // Containers that end where a separator or a closer is due:
            // truncated, and — for the object — run together.
            "{\"a\":1",
            "{\"a\":1 \"b\":2}",
            "[1",
            "[1 2]",
            // A backslash with nothing after it: the escape itself runs
            // off the end of the document.
            "\"abc\\",
        ] {
            assert!(parse(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn parse_decodes_the_control_solidus_and_hex_escapes() {
        assert_eq!(
            parse(r#""\b\f\/\u0041\t""#).expect("escapes parse"),
            Value::String("\u{8}\u{c}/A\t".to_string())
        );
    }

    #[test]
    fn parse_decodes_the_carriage_return_and_self_referential_escapes() {
        // The rest of the escape table, which the test above omits: `\r`,
        // and the two escapes for the delimiters themselves.
        assert_eq!(
            parse(r#""a\rb\"c\\d""#).expect("escapes parse"),
            Value::String("a\rb\"c\\d".to_string())
        );
    }

    #[test]
    fn multi_byte_text_survives_the_run_slicing() {
        // Unescaped text is taken in runs and pushed as one slice; a run
        // bounded by an escape on either side must not split a code point.
        assert_eq!(
            parse("\"çağdaş\\tçağdaş!ç\"").expect("multi-byte text parses"),
            Value::String("çağdaş\tçağdaş!ç".to_string())
        );
    }

    #[test]
    fn null_and_float_values_render() {
        assert_eq!(Value::Null.render(), "null");
        assert_eq!(Value::Float(1.5).render(), "1.5");
        assert_eq!(
            Value::Array(vec![Value::Null, Value::Float(-0.25)]).render(),
            "[null,-0.25]"
        );
    }

    #[test]
    fn accessors_return_none_on_the_wrong_shape() {
        let value = Value::Null;
        assert_eq!(value.get("key"), None);
        assert_eq!(value.as_str(), None);
        assert_eq!(value.as_bool(), None);
        assert!(value.as_array().is_none());
        assert!(value.as_object().is_none());
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
    }

    #[test]
    fn errors_carry_their_byte_position() {
        let error = parse("[1,]").expect_err("must fail");
        assert!(error.to_string().contains("byte"), "{error}");
    }

    #[test]
    fn nesting_beyond_the_depth_bound_errors_instead_of_aborting() {
        let fine = format!("{}{}", "[".repeat(100), "]".repeat(100));
        assert!(parse(&fine).is_ok(), "100 levels must parse");

        let too_deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let error = parse(&too_deep).expect_err("200 levels must be rejected");
        assert!(error.to_string().contains("nesting"), "{error}");

        // Depth is per-branch, not cumulative: many shallow siblings pass.
        let siblings = format!("[{}]", vec!["[[]]"; 200].join(","));
        assert!(
            parse(&siblings).is_ok(),
            "sibling containers must not count"
        );
    }

    #[test]
    fn envelope_leads_with_schema_version_one() {
        let document = result_envelope("test", "ok", 0, 42, "out", "err").render();
        assert!(
            document.starts_with("{\"schema_version\":1,\"command\":\"test\""),
            "unexpected prefix: {document}"
        );
        assert!(document.contains("\"status\":\"ok\""));
        assert!(document.contains("\"exit_code\":0"));
        assert!(document.contains("\"duration_ms\":42"));
    }
}
