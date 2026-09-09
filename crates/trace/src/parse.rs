//! Reading a trace, and refusing everything else.
//!
//! The input is untrusted. Every rule here rejects rather than repairs,
//! and nothing is ever skipped: a line the reader does not understand is
//! the one thing a format must not shrug at, because a skipped line is how
//! two readers of the same file quietly stop agreeing about what it says.
//!
//! The reader takes text, never a path. That is what makes it testable
//! and fuzzable with no filesystem in sight, and it puts the bound on how
//! much untrusted data may be held where that bound can actually be
//! enforced: at the seam that does the reading, which can refuse an
//! oversized file before a single byte reaches a parser. It also means
//! invalid UTF-8 never arrives here — the caller's reader has to have
//! refused it already, and a reader that replaced bad bytes with
//! replacement characters has already lost the file. Choosing a strict
//! reader at that seam is a caller obligation this crate states and
//! cannot check.

use crate::error::{TraceError, TraceErrorKind};
use crate::event::{FiniteF32, FiniteF64, TraceButton, TraceEvent, TraceKey, TraceTouchPhase};
use crate::grammar::{
    ASSIGN, BUDGET, BUTTON, CLOSE, DOWN, EVENT, FIRST_EVENT_LINE, FOCUS, FOCUS_IN, FOCUS_OUT,
    FORMAT_VERSION, HEADER_LINE, HEX_PREFIX, KEY, MAGIC, MOTION, OTHER_BUTTON, POINTER, REDRAW,
    REPEAT, RESIZE, SAMPLE, SCALE, SEPARATOR, TEXT, TICKS, TIMESTEP_NS, TOUCH, TOUCH_VERSION, UP,
    WHEEL,
};
use crate::trace::{Trace, TraceHeader};

/// A byte order mark. Not part of the grammar and not tolerated: it is
/// invisible on screen, so a file carrying one looks exactly like a file
/// that does not, and every other refusal would blame the wrong thing.
const BYTE_ORDER_MARK: char = '\u{feff}';

/// Read a trace from text.
///
/// # Errors
///
/// On anything that is not exactly a trace this reader understands: an
/// empty file, a byte order mark, a missing or misplaced header, a version
/// newer than this reader's, a duplicate header key, a line keyword or
/// event kind it does not know, a number that is not plain ASCII digits or
/// does not fit its width, a float that is not a fixed-width lowercase
/// bit pattern or is not finite, trailing text, a tick that decreases, and
/// a tick past the end of the run the header describes. Every refusal
/// names the line it is about and what was expected there.
pub fn parse(text: &str) -> Result<Trace, TraceError> {
    if text.starts_with(BYTE_ORDER_MARK) {
        return Err(TraceError::new(HEADER_LINE, TraceErrorKind::ByteOrderMark));
    }
    if text.is_empty() {
        return Err(TraceError::new(HEADER_LINE, TraceErrorKind::Empty));
    }

    // One trailing newline is the file's terminator, not an empty line.
    // Any other empty line is a line, and is refused below like anything
    // else this reader cannot read.
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut segments = body.split('\n').map(without_carriage_return);
    // Splitting always yields at least one segment, so the header line is
    // always there to be read — whether it is a header is the next
    // question.
    let header_line = segments.next().unwrap_or_default();

    // Nothing is buffered and nothing is reserved. The input is
    // untrusted and may be enormous, so the reader holds only what it
    // has already accepted: a megabyte of garbage is refused on its
    // second line having allocated for one event, where collecting the
    // lines first would have held the whole file's worth of slices, and
    // a reservation taken from a line count would be a caller-controlled
    // allocation — one that aborts rather than returns on a 32-bit
    // target, in a crate that promises nothing here panics.
    let (version, header) = parse_header(header_line)?;
    let mut events = Vec::new();
    for (index, line) in segments.enumerate() {
        events.push(parse_event(index + FIRST_EVENT_LINE, line, version)?);
    }
    // The tick rules live in the constructor, so a trace built in memory
    // and a trace read from a file are held to one implementation of them
    // — and because every line after the header is exactly one event, the
    // line number it computes is the line number this file has.
    Trace::new(header, events)
}

/// A trailing carriage return is stripped rather than refused: these are
/// text files in a repository whose builds run on Windows, and a line
/// ending is not a change to what the line says.
fn without_carriage_return(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// The header, plus the version the file claims — which the event
/// lines are then held to, because a word can be newer than the claim.
fn parse_header(line: &str) -> Result<(u64, TraceHeader), TraceError> {
    let mut cursor = Cursor::new(HEADER_LINE, line);
    let word = cursor.optional().unwrap_or_default();
    if word != MAGIC {
        return Err(cursor.error(TraceErrorKind::NotATrace {
            found: word.to_string(),
        }));
    }

    // The version is positional and first, because a reader has to know
    // how to read the rest of the line before it reads it. It is read as
    // a full 64-bit number so that any version a file might plausibly
    // claim comes back as a version this reader cannot read, which is
    // what it is, rather than as an overflowing field. Past sixty-four
    // bits it is an overflowing field, and by then that is the honest
    // description.
    let version = cursor.decimal_u64("the format version")?;
    if version > u64::from(FORMAT_VERSION) {
        return Err(cursor.error(TraceErrorKind::UnsupportedVersion {
            found: version,
            supported: FORMAT_VERSION,
        }));
    }

    let sample = cursor.header_value(SAMPLE, "`sample=<name>`")?;
    let ticks = cursor.header_number(TICKS, "`ticks=<u64>`")?;
    let timestep_ns = cursor.header_number(TIMESTEP_NS, "`timestep_ns=<u64>`")?;
    let budget = cursor.header_number_u32(BUDGET, "`budget=<u32>`")?;

    // Building through the same constructor a caller uses keeps one
    // implementation of what a header may say: the reader cannot accept a
    // header that could not have been built, and the writer cannot be
    // handed one it could not write back.
    let mut header = TraceHeader::new(sample, ticks, timestep_ns, budget)?;
    while let Some(field) = cursor.optional() {
        if field.is_empty() {
            return Err(cursor.error(TraceErrorKind::BlankField));
        }
        let Some((key, value)) = field.split_once(ASSIGN) else {
            return Err(cursor.error(TraceErrorKind::NotAKeyValuePair {
                field: field.to_string(),
            }));
        };
        header = header.with_key(key, value)?;
    }
    Ok((version, header))
}

fn parse_event(number: usize, line: &str, version: u64) -> Result<(u64, TraceEvent), TraceError> {
    let mut cursor = Cursor::new(number, line);
    let keyword = cursor.optional().unwrap_or_default();
    if keyword == MAGIC {
        return Err(cursor.error(TraceErrorKind::HeaderAfterEvents));
    }
    if keyword != EVENT {
        return Err(cursor.error(TraceErrorKind::UnknownKeyword {
            keyword: keyword.to_string(),
        }));
    }

    let tick = cursor.decimal_u64("the tick")?;
    let kind = cursor.expect("the kind of event")?;
    let event = match kind {
        KEY => {
            let name = cursor.expect("the key name")?;
            let code = TraceKey::from_name(name).ok_or_else(|| {
                cursor.error(TraceErrorKind::UnknownKey {
                    name: name.to_string(),
                })
            })?;
            // A name newer than the file's own claim: the file is lying
            // to every reader of the version it names — the same refusal
            // the touch words established, applied per name.
            if version < u64::from(code.introduced()) {
                return Err(cursor.error(TraceErrorKind::EventFromANewerFormat {
                    kind: format!("key name `{name}`"),
                    introduced: code.introduced(),
                    declared: version,
                }));
            }
            let pressed = cursor.pressed()?;
            // The repeat flag is present or absent, never written as a
            // word meaning "no": an absent flag and a flag that says
            // nothing happened are two spellings of one fact.
            let repeat = match cursor.optional() {
                None => false,
                Some(REPEAT) => true,
                Some("") => return Err(cursor.error(TraceErrorKind::BlankField)),
                Some(other) => {
                    return Err(cursor.error(TraceErrorKind::NotTheRepeatFlag {
                        text: other.to_string(),
                    }));
                }
            };
            TraceEvent::Key {
                code,
                pressed,
                repeat,
            }
        }
        POINTER => TraceEvent::PointerMoved {
            x: cursor.hex_f64("the pointer's x coordinate")?,
            y: cursor.hex_f64("the pointer's y coordinate")?,
        },
        MOTION => TraceEvent::PointerMotion {
            dx: cursor.hex_f64("the pointer's rightward movement")?,
            dy: cursor.hex_f64("the pointer's downward movement")?,
        },
        BUTTON => {
            let name = cursor.expect("the pointer button")?;
            let button = cursor.button(name)?;
            TraceEvent::PointerButton {
                button,
                pressed: cursor.pressed()?,
            }
        }
        WHEEL => TraceEvent::Wheel {
            dx: cursor.hex_f32("the wheel's horizontal delta")?,
            dy: cursor.hex_f32("the wheel's vertical delta")?,
        },
        FOCUS => TraceEvent::Focused(cursor.focused()?),
        TEXT => TraceEvent::TextEntered {
            ch: cursor.typed_character("the typed character")?,
        },
        RESIZE => TraceEvent::Resized {
            width: cursor.decimal_u32("the width (u32)")?,
            height: cursor.decimal_u32("the height (u32)")?,
        },
        SCALE => TraceEvent::ScaleFactorChanged {
            scale: cursor.hex_f64("the scale factor")?,
        },
        REDRAW => TraceEvent::RedrawRequested,
        CLOSE => TraceEvent::CloseRequested,
        TOUCH => {
            // A word newer than the file's own claim: the file is lying
            // to every reader of the version it names, so the one reader
            // able to notice refuses rather than laundering the header
            // to a version the producer did not write.
            if version < u64::from(TOUCH_VERSION) {
                return Err(cursor.error(TraceErrorKind::EventFromANewerFormat {
                    kind: TOUCH.to_string(),
                    introduced: TOUCH_VERSION,
                    declared: version,
                }));
            }
            TraceEvent::Touch {
                finger: cursor.decimal_u64("the finger id")?,
                phase: cursor.touch_phase()?,
                x: cursor.hex_f64("the touch's x coordinate")?,
                y: cursor.hex_f64("the touch's y coordinate")?,
            }
        }
        other => {
            return Err(cursor.error(TraceErrorKind::UnknownEventKind {
                kind: other.to_string(),
            }));
        }
    };
    cursor.end()?;
    Ok((tick, event))
}

/// A position in one line: the fields still to be read, and the line
/// number every refusal from here will carry.
struct Cursor<'a> {
    number: usize,
    fields: core::str::Split<'a, char>,
}

impl<'a> Cursor<'a> {
    fn new(number: usize, line: &'a str) -> Self {
        Self {
            number,
            fields: line.split(SEPARATOR),
        }
    }

    fn error(&self, kind: TraceErrorKind) -> TraceError {
        TraceError::new(self.number, kind)
    }

    /// The next field, whatever it is or is not.
    fn optional(&mut self) -> Option<&'a str> {
        self.fields.next()
    }

    /// The next field, which the grammar requires. `expected` describes
    /// what belongs here and is reused verbatim in whatever the field
    /// turns out to be wrong about.
    fn expect(&mut self, expected: &'static str) -> Result<&'a str, TraceError> {
        match self.fields.next() {
            Some("") => Err(self.error(TraceErrorKind::BlankField)),
            Some(field) => Ok(field),
            None => Err(self.error(TraceErrorKind::LineEndsEarly { expected })),
        }
    }

    /// Nothing may follow.
    fn end(&mut self) -> Result<(), TraceError> {
        match self.fields.next() {
            None => Ok(()),
            Some("") => Err(self.error(TraceErrorKind::BlankField)),
            Some(text) => Err(self.error(TraceErrorKind::TrailingText {
                text: text.to_string(),
            })),
        }
    }

    /// One of the header's own `key=value` fields, by the key that must be
    /// there. They are positional so that the reader never has to guess
    /// which field it is holding.
    fn header_value(&mut self, key: &str, expected: &'static str) -> Result<&'a str, TraceError> {
        let field = self.expect(expected)?;
        let Some((found, value)) = field.split_once(ASSIGN) else {
            return Err(self.error(TraceErrorKind::NotAKeyValuePair {
                field: field.to_string(),
            }));
        };
        if found != key {
            return Err(self.error(TraceErrorKind::HeaderFieldOutOfOrder {
                expected,
                found: field.to_string(),
            }));
        }
        Ok(value)
    }

    fn header_number(&mut self, key: &str, expected: &'static str) -> Result<u64, TraceError> {
        let text = self.header_value(key, expected)?;
        digits(text, expected, self.number)
    }

    fn header_number_u32(&mut self, key: &str, expected: &'static str) -> Result<u32, TraceError> {
        let text = self.header_value(key, expected)?;
        let value = digits(text, expected, self.number)?;
        u32::try_from(value).map_err(|_| {
            self.error(TraceErrorKind::IntegerTooLarge {
                field: expected,
                text: text.to_string(),
            })
        })
    }

    fn decimal_u64(&mut self, field: &'static str) -> Result<u64, TraceError> {
        let text = self.expect(field)?;
        digits(text, field, self.number)
    }

    fn decimal_u32(&mut self, field: &'static str) -> Result<u32, TraceError> {
        let text = self.expect(field)?;
        let value = digits(text, field, self.number)?;
        u32::try_from(value).map_err(|_| {
            self.error(TraceErrorKind::IntegerTooLarge {
                field,
                text: text.to_string(),
            })
        })
    }

    /// A typed character, in decimal.
    ///
    /// **Validated to the same rule the live seam applies**, not merely
    /// to `u32`. A trace is external data, and the event type promises
    /// its consumers that a control character never arrives — a promise
    /// a recording could otherwise break by carrying `13`. So this
    /// refuses anything that is not a scalar and anything that is a
    /// control character, and the two directions cannot disagree about
    /// what text is.
    fn typed_character(&mut self, field: &'static str) -> Result<u32, TraceError> {
        let value = self.decimal_u32(field)?;
        if char::from_u32(value).is_some_and(|ch| !ch.is_control()) {
            return Ok(value);
        }
        Err(self.error(TraceErrorKind::NotTypedText {
            field,
            text: value.to_string(),
        }))
    }

    fn hex_f64(&mut self, field: &'static str) -> Result<FiniteF64, TraceError> {
        let text = self.expect(field)?;
        let body = hex_body(text, FiniteF64::HEX_DIGITS).ok_or_else(|| {
            self.error(TraceErrorKind::NotAHexPattern {
                field,
                text: text.to_string(),
                digits: FiniteF64::HEX_DIGITS,
            })
        })?;
        let bits = body
            .bytes()
            .fold(0_u64, |bits, byte| (bits << 4) | u64::from(hex_digit(byte)));
        FiniteF64::from_bits(bits).ok_or_else(|| {
            self.error(TraceErrorKind::NonFinite {
                field,
                text: text.to_string(),
            })
        })
    }

    fn hex_f32(&mut self, field: &'static str) -> Result<FiniteF32, TraceError> {
        let text = self.expect(field)?;
        let body = hex_body(text, FiniteF32::HEX_DIGITS).ok_or_else(|| {
            self.error(TraceErrorKind::NotAHexPattern {
                field,
                text: text.to_string(),
                digits: FiniteF32::HEX_DIGITS,
            })
        })?;
        let bits = body
            .bytes()
            .fold(0_u32, |bits, byte| (bits << 4) | u32::from(hex_digit(byte)));
        FiniteF32::from_bits(bits).ok_or_else(|| {
            self.error(TraceErrorKind::NonFinite {
                field,
                text: text.to_string(),
            })
        })
    }

    /// A named button, or a native one by its index.
    fn button(&self, name: &str) -> Result<TraceButton, TraceError> {
        if let Some(text) = name.strip_prefix(OTHER_BUTTON) {
            const FIELD: &str = "the native button index (u16)";
            let value = digits(text, FIELD, self.number)?;
            let index = u16::try_from(value).map_err(|_| {
                self.error(TraceErrorKind::IntegerTooLarge {
                    field: FIELD,
                    text: text.to_string(),
                })
            })?;
            return Ok(TraceButton::Other(index));
        }
        TraceButton::from_name(name).ok_or_else(|| {
            self.error(TraceErrorKind::UnknownButton {
                name: name.to_string(),
            })
        })
    }

    fn pressed(&mut self) -> Result<bool, TraceError> {
        let text = self.expect("`down` or `up`")?;
        match text {
            DOWN => Ok(true),
            UP => Ok(false),
            _ => Err(self.error(TraceErrorKind::NotAPressedState {
                text: text.to_string(),
            })),
        }
    }

    fn touch_phase(&mut self) -> Result<TraceTouchPhase, TraceError> {
        let text = self.expect("`start`, `move`, `end` or `cancel`")?;
        TraceTouchPhase::from_name(text).ok_or_else(|| {
            self.error(TraceErrorKind::NotATouchPhase {
                text: text.to_string(),
            })
        })
    }

    fn focused(&mut self) -> Result<bool, TraceError> {
        let text = self.expect("`in` or `out`")?;
        match text {
            FOCUS_IN => Ok(true),
            FOCUS_OUT => Ok(false),
            _ => Err(self.error(TraceErrorKind::NotAFocusState {
                text: text.to_string(),
            })),
        }
    }
}

/// A whole number, in ASCII digits and nothing else.
///
/// Hand-read rather than handed to the standard library's parser, because
/// "whatever that accepts" is not a specification for a byte-exact format:
/// it takes a leading `+`, and what it takes is free to grow. Here the
/// answer is fixed — digits, no sign, no underscores, no other base.
fn digits(text: &str, field: &'static str, line: usize) -> Result<u64, TraceError> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TraceError::new(
            line,
            TraceErrorKind::NotADecimalInteger {
                field,
                text: text.to_string(),
            },
        ));
    }
    let mut value: u64 = 0;
    for byte in text.bytes() {
        value = value
            .checked_mul(10)
            .and_then(|shifted| shifted.checked_add(u64::from(byte.wrapping_sub(b'0'))))
            .ok_or_else(|| {
                TraceError::new(
                    line,
                    TraceErrorKind::IntegerTooLarge {
                        field,
                        text: text.to_string(),
                    },
                )
            })?;
    }
    Ok(value)
}

/// The digits of a bit pattern, if the field is one: the prefix, exactly
/// the width of the type, lowercase. A shorter field is a different number
/// silently, and an uppercase one is a second spelling of a file that is
/// meant to have exactly one.
fn hex_body(text: &str, digits: usize) -> Option<&str> {
    let body = text.strip_prefix(HEX_PREFIX)?;
    (body.len() == digits && body.bytes().all(is_lowercase_hex)).then_some(body)
}

fn is_lowercase_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}

/// The value of one hexadecimal digit. Only ever called on a byte
/// [`is_lowercase_hex`] has already accepted; the wrapping arithmetic is
/// there so that a future caller who forgets cannot make it panic.
fn hex_digit(byte: u8) -> u8 {
    if byte.is_ascii_digit() {
        byte.wrapping_sub(b'0')
    } else {
        byte.wrapping_sub(b'a').wrapping_add(10)
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::event::{FiniteF32, FiniteF64, TraceButton, TraceEvent, TraceKey, TraceTouchPhase};

    const HEADER: &str = "renew-trace 3 sample=input_echo ticks=30 timestep_ns=16666667 budget=5";

    /// A one-event trace, so a test can name the shape it cares about and
    /// nothing else.
    fn one(line: &str) -> TraceEvent {
        let text = format!("{HEADER}\n{line}\n");
        let trace = parse(&text).unwrap();
        assert_eq!(trace.events().len(), 1);
        trace.events()[0].1
    }

    #[test]
    fn a_header_alone_is_a_trace_with_no_events() {
        let trace = parse(&format!("{HEADER}\n")).unwrap();
        assert_eq!(trace.header().sample(), "input_echo");
        assert_eq!(trace.header().ticks(), 30);
        assert_eq!(trace.header().timestep_ns(), 16_666_667);
        assert_eq!(trace.header().budget(), 5);
        assert!(trace.events().is_empty());
    }

    /// A file need not end with a newline, and a file that does not is the
    /// same trace as one that does.
    #[test]
    fn the_final_newline_is_optional() {
        assert_eq!(parse(HEADER), parse(&format!("{HEADER}\n")));
    }

    #[test]
    fn caller_keys_are_kept_verbatim_and_in_order() {
        let trace = parse(&format!("{HEADER} seed=3 extent=640x480\n")).unwrap();
        assert_eq!(
            trace.header().keys(),
            [
                ("seed".to_string(), "3".to_string()),
                ("extent".to_string(), "640x480".to_string()),
            ]
        );
    }

    /// A field is split at its **first** separator, so a value may carry
    /// as many more as it likes. The rule matters in both directions: a
    /// reader that split at the last one would silently rename the key
    /// and shorten the value, and the writer would then be emitting
    /// files that read as something else.
    #[test]
    fn a_caller_field_is_split_at_its_first_separator() {
        let trace = parse(&format!("{HEADER} extent=640=480\n")).unwrap();
        assert_eq!(
            trace.header().keys(),
            [("extent".to_string(), "640=480".to_string())]
        );
        assert_eq!(trace.header().value("extent"), Some("640=480"));
    }

    #[test]
    fn every_line_shape_reads_back_as_the_event_it_names() {
        assert_eq!(
            one("e 0 key arrow-right down"),
            TraceEvent::Key {
                code: TraceKey::ArrowRight,
                pressed: true,
                repeat: false,
            }
        );
        assert_eq!(
            one("e 0 key space up repeat"),
            TraceEvent::Key {
                code: TraceKey::Space,
                pressed: false,
                repeat: true,
            }
        );
        assert_eq!(
            one("e 0 pointer 0x3ff8000000000000 0xc000000000000000"),
            TraceEvent::PointerMoved {
                x: FiniteF64::new(1.5).unwrap(),
                y: FiniteF64::new(-2.0).unwrap(),
            }
        );
        // The delta's own token, read back as a delta.
        assert_eq!(
            one("e 0 motion 0x3ff8000000000000 0xc000000000000000"),
            TraceEvent::PointerMotion {
                dx: FiniteF64::new(1.5).unwrap(),
                dy: FiniteF64::new(-2.0).unwrap(),
            }
        );
        assert_eq!(
            one("e 0 button middle down"),
            TraceEvent::PointerButton {
                button: TraceButton::Middle,
                pressed: true,
            }
        );
        assert_eq!(
            one("e 0 button other:9 up"),
            TraceEvent::PointerButton {
                button: TraceButton::Other(9),
                pressed: false,
            }
        );
        assert_eq!(
            one("e 0 wheel 0x00000000 0xbf000000"),
            TraceEvent::Wheel {
                dx: FiniteF32::new(0.0).unwrap(),
                dy: FiniteF32::new(-0.5).unwrap(),
            }
        );
        assert_eq!(one("e 0 focus in"), TraceEvent::Focused(true));
        assert_eq!(one("e 0 text 97"), TraceEvent::TextEntered { ch: 0x61 });
        assert_eq!(one("e 0 focus out"), TraceEvent::Focused(false));
        assert_eq!(
            one("e 0 resize 1280 720"),
            TraceEvent::Resized {
                width: 1280,
                height: 720,
            }
        );
        assert_eq!(
            one("e 0 scale 0x4000000000000000"),
            TraceEvent::ScaleFactorChanged {
                scale: FiniteF64::new(2.0).unwrap(),
            }
        );
        assert_eq!(one("e 0 redraw"), TraceEvent::RedrawRequested);
        assert_eq!(one("e 0 close"), TraceEvent::CloseRequested);
        assert_eq!(
            one("e 0 touch 7 start 0x3ff8000000000000 0xc000000000000000"),
            TraceEvent::Touch {
                finger: 7,
                phase: TraceTouchPhase::Started,
                x: FiniteF64::new(1.5).unwrap(),
                y: FiniteF64::new(-2.0).unwrap(),
            }
        );
        // The other three phases, so the word table is read end to end.
        for (word, phase) in [
            ("move", TraceTouchPhase::Moved),
            ("end", TraceTouchPhase::Ended),
            ("cancel", TraceTouchPhase::Cancelled),
        ] {
            assert_eq!(
                one(&format!(
                    "e 0 touch 0 {word} 0x0000000000000000 0x0000000000000000"
                )),
                TraceEvent::Touch {
                    finger: 0,
                    phase,
                    x: FiniteF64::new(0.0).unwrap(),
                    y: FiniteF64::new(0.0).unwrap(),
                }
            );
        }
    }

    /// A phase word outside the four is refused by name, and the refusal
    /// says what would have been legal there.
    #[test]
    fn a_touch_phase_outside_the_four_is_refused() {
        use crate::error::TraceErrorKind;
        let text = format!("{HEADER}\ne 0 touch 1 hover 0x0000000000000000 0x0000000000000000\n");
        let error = parse(&text).unwrap_err();
        assert_eq!(
            *error.kind(),
            TraceErrorKind::NotATouchPhase {
                text: "hover".to_string(),
            }
        );
    }

    /// A reader accepts every older version — the format's own stated
    /// rule, pinned here so a version bump cannot silently orphan every
    /// trace already recorded.
    #[test]
    fn an_older_version_is_still_read() {
        let old = "renew-trace 1 sample=input_echo ticks=30 timestep_ns=16666667 budget=5\n\
                   e 0 close\n";
        let trace = parse(old).unwrap();
        assert_eq!(trace.events(), [(0, TraceEvent::CloseRequested)]);
    }

    /// The deliberate carve-out under the version gate, pinned rather
    /// than only stated: `motion` and `text` entered the format while
    /// its version number lagged, so genuinely mislabeled files from
    /// those eras exist and stay readable. A future generalization of
    /// the gate that swept them in would orphan recordings this format
    /// already blessed — and would turn this red first.
    #[test]
    fn the_pre_gate_words_stay_readable_under_the_versions_that_shipped_them() {
        for (declared, line) in [
            (0, "e 0 motion 0x3ff8000000000000 0xc000000000000000"),
            (0, "e 0 text 97"),
            (1, "e 0 motion 0x3ff8000000000000 0xc000000000000000"),
            (1, "e 0 text 97"),
        ] {
            let text = format!(
                "renew-trace {declared} sample=input_echo ticks=30 timestep_ns=16666667 budget=5
{line}
"
            );
            assert!(
                parse(&text).is_ok(),
                "version {declared} must keep reading `{line}`"
            );
        }
    }

    /// A file whose header claims a version older than a word it uses is
    /// lying to every reader of that version, and the one reader able to
    /// notice must not launder it: before this rule, such a file parsed
    /// here and rewrote as canonical version 2 — a header the producer
    /// never wrote.
    #[test]
    fn a_touch_line_under_an_older_claimed_version_is_refused() {
        use crate::error::TraceErrorKind;
        for declared in [0, 1] {
            let text = format!(
                "renew-trace {declared} sample=input_echo ticks=30 timestep_ns=16666667 budget=5\n\
                 e 0 touch 7 start 0x0000000000000000 0x0000000000000000\n"
            );
            let error = parse(&text).unwrap_err();
            assert_eq!(
                *error.kind(),
                TraceErrorKind::EventFromANewerFormat {
                    kind: "touch".to_string(),
                    introduced: 2,
                    declared,
                },
                "version {declared} must not admit a touch line"
            );
            assert_eq!(error.line(), 2, "the refusal names the touch line");
        }
    }

    /// **A widened key under an older claimed version is refused.** The
    /// 64 names introduced at version 3 must not parse out of a file
    /// claiming 2: such a file lies to every version-2 reader it names,
    /// and the one reader able to notice refuses instead of laundering
    /// it — the touch words' own rule, applied per name. The original
    /// names still parse under the old claim, or the gate would orphan
    /// every version-2 file for no reason.
    ///
    /// Probed red by deleting the `introduced()` check in the key arm.
    #[test]
    fn a_widened_key_under_an_older_claimed_version_is_refused() {
        use crate::error::TraceErrorKind;
        let text = "renew-trace 2 sample=input_echo ticks=30 timestep_ns=16666667 budget=5\n\
                    e 0 key key-z down\n";
        let error = parse(text).unwrap_err();
        assert_eq!(
            *error.kind(),
            TraceErrorKind::EventFromANewerFormat {
                kind: "key name `key-z`".to_string(),
                introduced: 3,
                declared: 2,
            },
            "version 2 must not admit a version-3 key name"
        );
        assert_eq!(error.line(), 2, "the refusal names the key line");

        let old = "renew-trace 2 sample=input_echo ticks=30 timestep_ns=16666667 budget=5\n\
                   e 0 key key-w down\n";
        assert!(
            parse(old).is_ok(),
            "an original name under version 2 was refused; the gate orphans old files"
        );
    }

    /// Every lowercase hexadecimal digit, so the digit-to-value step is
    /// exercised across both of its halves rather than on one letter.
    #[test]
    fn every_hexadecimal_digit_reads_back_as_its_value() {
        assert_eq!(
            one("e 0 pointer 0x0123456789abcdef 0x0000000000000000"),
            TraceEvent::PointerMoved {
                x: FiniteF64::from_bits(0x0123_4567_89ab_cdef).unwrap(),
                y: FiniteF64::from_bits(0).unwrap(),
            }
        );
        assert_eq!(
            one("e 0 wheel 0x89abcdef 0x01234567"),
            TraceEvent::Wheel {
                dx: FiniteF32::from_bits(0x89ab_cdef).unwrap(),
                dy: FiniteF32::from_bits(0x0123_4567).unwrap(),
            }
        );
    }

    /// The two zeros are different bit patterns and stay different.
    #[test]
    fn negative_zero_survives_the_read() {
        assert_eq!(
            one("e 0 scale 0x8000000000000000"),
            TraceEvent::ScaleFactorChanged {
                scale: FiniteF64::new(-0.0).unwrap(),
            }
        );
    }

    #[test]
    fn a_carriage_return_before_a_newline_is_not_part_of_the_line() {
        let text = format!("{HEADER}\r\ne 0 close\r\n");
        assert_eq!(parse(&text), parse(&format!("{HEADER}\ne 0 close\n")));
    }

    /// Equal ticks are legal and keep the order they were written in:
    /// which key went down first is part of what was recorded.
    #[test]
    fn two_events_on_one_tick_keep_the_order_they_were_written_in() {
        let text = format!("{HEADER}\ne 4 key key-a down\ne 4 key key-d down\n");
        let trace = parse(&text).unwrap();
        assert_eq!(
            trace.events(),
            [
                (
                    4,
                    TraceEvent::Key {
                        code: TraceKey::KeyA,
                        pressed: true,
                        repeat: false,
                    }
                ),
                (
                    4,
                    TraceEvent::Key {
                        code: TraceKey::KeyD,
                        pressed: true,
                        repeat: false,
                    }
                ),
            ]
        );
    }

    /// The trailing bucket: a tick equal to the run's length means after
    /// the final step, and it is the common case rather than an edge one.
    #[test]
    fn a_tick_equal_to_the_run_length_is_read() {
        let trace = parse(&format!("{HEADER}\ne 30 close\n")).unwrap();
        assert_eq!(trace.events()[0].0, 30);
    }

    /// Leading zeros are digits, so they are read as the number they
    /// spell. Writing back produces the canonical spelling — which is why
    /// the round-trip claim runs from a written file, not from an
    /// arbitrary hand-typed one.
    #[test]
    fn leading_zeros_are_read_as_the_number_they_spell() {
        let trace = parse(&format!("{HEADER}\ne 007 close\n")).unwrap();
        assert_eq!(trace.events()[0].0, 7);
    }
}
