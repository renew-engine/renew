//! `data:` URIs, and the base64 their payloads are written in.
//!
//! A document that wants to be one file has to carry its binary
//! somewhere, and the way it does that is RFC 2397: a URI whose payload
//! *is* the resource, rather than a name for one somewhere else. **This
//! module turns such a URI back into bytes and does nothing else.** It
//! does not know what the bytes are for, does not know which media types
//! a caller will accept, and never touches the filesystem — a URI that
//! names a second file is not this module's to refuse, because it is not
//! this module's to fetch.
//!
//! # Strict about the payload, forgiving about its spelling
//!
//! The two are not in tension, they are the same rule applied twice:
//! **a difference that cannot change a single output byte is tolerated,
//! and a difference that can is refused.**
//!
//! Forgiven: the `;base64` marker in any letter case, because
//! `;BASE64` and `;base64` are the same instruction and neither one
//! decodes differently.
//!
//! Refused: an encoding that is not a whole number of four-character
//! groups; a character outside the alphabet, including whitespace, which
//! some decoders skip; padding anywhere but the end; and — the one that
//! is easy to miss — **a final group whose unused bits are not zero.**
//!
//! # Why the unused bits matter
//!
//! `QQ==` and `QR==` would both decode to the single byte `A`, and this
//! reader accepts only the first. The last four
//! bits of the second character are not part of any output byte, so an
//! encoder writes them as zero and a lenient decoder ignores whatever is
//! there. That makes the encoding **many-to-one: two documents that
//! differ in their bytes produce byte-identical resources.** A reader
//! that accepts both has quietly agreed that two different files are the
//! same file, which is the same trade the container refuses when it
//! insists its declared length is exactly the file's length rather than
//! at most it. Refusing the non-canonical spelling keeps text and bytes
//! one to one.
//!
//! # What the caller still has to decide
//!
//! [`DataUri::media_type`] is reported, never judged. Which types are
//! acceptable is a fact about what the caller is reading — a buffer, an
//! image, a font — and belongs where that is known.

use core::fmt;

/// The scheme, with its colon.
const SCHEME: &str = "data:";

/// The parameter that says the payload is base64 rather than
/// percent-encoded text.
const MARKER: &str = ";base64";

/// The padding character, which carries no bits.
const PAD: u8 = b'=';

/// Characters per encoded group, and bytes per decoded one.
const GROUP: usize = 4;
const DECODED: usize = 3;

/// Every way a `data:` URI can fail to yield bytes.
///
/// **Closed on purpose**, as this crate's other refusals are: a caller
/// matching exhaustively should stop compiling when a refusal is added
/// rather than route a new one through a wildcard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataUriError {
    /// The text does not begin with `data:`.
    ///
    /// Usually not a fault at all — it is what a relative path to a
    /// second file looks like from here. The caller knows whether it can
    /// go and get that file; this module knows only that it cannot.
    NotADataUri,

    /// No comma, so nothing marks where the payload begins.
    NoPayload,

    /// The `;base64` marker is absent.
    ///
    /// **This reader's limit rather than the format's.** RFC 2397 also
    /// permits a percent-encoded payload, and that spelling is legal in
    /// every document that may use a `data:` URI at all. It is not
    /// implemented here, and saying so is more useful than a complaint
    /// that blames the document for a choice this reader made.
    ///
    /// **Named for the reader's gap rather than the payload's shape**,
    /// which is why it is not called `NotBase64`: a percent-encoded
    /// payload genuinely is not base64, and a program keying on that
    /// name would read a conformant document as a malformed one. The
    /// document reader one file over spells the same idea the same way.
    Unsupported,

    /// A character that is not in the base64 alphabet.
    ///
    /// Whitespace lands here too. Some decoders skip it; skipping it
    /// would mean two spellings of one payload, which is the thing this
    /// module exists to avoid.
    BadDigit {
        /// The byte found.
        byte: u8,
        /// Its offset in the whole URI, so the caller can point at it.
        at: usize,
    },

    /// The payload is not a whole number of four-character groups.
    NotWholeGroups {
        /// How many bytes the payload has.
        ///
        /// **Bytes, not characters.** This check runs before any
        /// character is looked at, so a payload carrying anything wider
        /// than ASCII is measured in bytes -- and in a module whose
        /// whole argument turns on that distinction, the field had
        /// better not blur it.
        len: usize,
    },

    /// Padding somewhere padding cannot be.
    ///
    /// It is legal only as the last one or two characters of the last
    /// group. Anywhere else it either pads a group that is not the end
    /// or has a digit after it, and both mean the payload was assembled
    /// from pieces rather than encoded.
    BadPadding {
        /// Where the `=` is, in the whole URI.
        at: usize,
    },

    /// A padded final group whose unused bits are not zero.
    ///
    /// See this module's own documentation: the alternative is accepting
    /// two different texts as one resource.
    NonCanonical {
        /// The character carrying the stray bits, in the whole URI.
        at: usize,
        /// What they were, so the message can show what was expected.
        bits: u8,
    },
}

impl DataUriError {
    /// The variant's own name, for a caller acting on which refusal this
    /// is rather than reading it.
    ///
    /// **A message is for a person and a name is for a program**, the
    /// same trade this crate's other refusals make.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::NotADataUri => "NotADataUri",
            Self::NoPayload => "NoPayload",
            Self::Unsupported => "Unsupported",
            Self::BadDigit { .. } => "BadDigit",
            Self::NotWholeGroups { .. } => "NotWholeGroups",
            Self::BadPadding { .. } => "BadPadding",
            Self::NonCanonical { .. } => "NonCanonical",
        }
    }
}

impl fmt::Display for DataUriError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotADataUri => write!(out, "this does not begin with `{SCHEME}`"),
            Self::NoPayload => write!(out, "there is no comma, so no payload begins"),
            Self::Unsupported => write!(
                out,
                "the payload is not marked `{MARKER}`, and this reader decodes no other spelling"
            ),
            Self::BadDigit { byte, at } => write!(
                out,
                "byte {byte:#04x} at offset {at} is not a base64 character"
            ),
            Self::NotWholeGroups { len } => write!(
                out,
                "the payload is {len} bytes, which is not a whole number of {GROUP}"
            ),
            Self::BadPadding { at } => {
                write!(out, "the padding at offset {at} is not at the end")
            }
            Self::NonCanonical { at, bits } => write!(
                out,
                "the character at offset {at} carries {bits:#04x} in bits no output byte uses, \
                 and an encoder writes those as zero"
            ),
        }
    }
}

impl core::error::Error for DataUriError {}

/// A decoded `data:` URI.
///
/// The text fields borrow from the URI; only the payload is owned,
/// because decoding it is the one thing here that cannot be a view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataUri<'a> {
    /// The media type as written, or `""` when the URI omitted it.
    ///
    /// **Reported, never judged.** RFC 2397 says an omitted type means
    /// `text/plain`, but saying so here would be inventing a claim the
    /// document did not make; a caller that cares can apply the default
    /// and a caller that requires an explicit type can see that there
    /// was none.
    pub media_type: &'a str,

    /// Whatever followed the media type, without the `;` that separated
    /// it and without the `;base64` marker. `""` when there were none,
    /// and separators *between* parameters are kept, so
    /// `;charset=x;name=y` is reported as `charset=x;name=y`.
    pub parameters: &'a str,

    /// The payload.
    pub bytes: Vec<u8>,
}

/// Whether this text is a `data:` URI at all.
///
/// Cheap, and answers the question a caller actually has: whether the
/// resource is here or somewhere this crate will not go.
///
/// **Compares bytes rather than characters**, which is not a
/// micro-optimisation: a URI beginning `data\u{e9}` has no character
/// boundary five bytes in -- the accented letter spans bytes four and
/// five -- and slicing a string there is a panic rather than a
/// mismatch.
#[must_use]
pub fn looks_like(uri: &str) -> bool {
    uri.as_bytes()
        .get(..SCHEME.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(SCHEME.as_bytes()))
}

/// Read a `data:` URI into its media type and its bytes.
///
/// # Errors
///
/// Every way the text can fail to be a base64 `data:` URI — see
/// [`DataUriError`]. The three that are about one character
/// ([`BadDigit`](DataUriError::BadDigit),
/// [`BadPadding`](DataUriError::BadPadding) and
/// [`NonCanonical`](DataUriError::NonCanonical)) carry its offset in the
/// whole URI; the rest are about the text as a whole and have no place
/// to point at.
pub fn read(uri: &str) -> Result<DataUri<'_>, DataUriError> {
    if !looks_like(uri) {
        return Err(DataUriError::NotADataUri);
    }
    let comma = uri.find(',').ok_or(DataUriError::NoPayload)?;

    // Everything between the scheme and the comma describes the payload;
    // everything after it is the payload. Both ends of this slice are
    // character boundaries: the scheme was matched byte for byte above,
    // and the comma came from a search of this very string.
    let described = &uri[SCHEME.len()..comma];

    // The marker is compared as bytes, and only then sliced off. Doing
    // it the other way round would slice a string at whatever offset
    // seven bytes from its end happens to be, which is a panic on any
    // media type ending in a character wider than one byte.
    let described = match described.len().checked_sub(MARKER.len()) {
        Some(cut) if described.as_bytes()[cut..].eq_ignore_ascii_case(MARKER.as_bytes()) => {
            &described[..cut]
        }
        _ => return Err(DataUriError::Unsupported),
    };

    // A type may be absent and parameters present -- `data:;charset=x`
    // is a URI RFC 2397 spells out -- so the split is at the first `;`
    // and either side may be empty.
    let (media_type, parameters) = match described.find(';') {
        Some(at) => (&described[..at], &described[at + 1..]),
        // **The empty tail of this string, not an empty literal.** A
        // `""` literal is a dangling `'static` pointer that borrows
        // nothing, so a caller checking provenance -- and the fuzz
        // target does, on every input -- would find a field the type
        // says is a view into the URI and that is not one. Slicing at
        // the string's own end is a character boundary by construction.
        None => (described, &described[described.len()..]),
    };

    Ok(DataUri {
        media_type,
        parameters,
        bytes: decode(uri.as_bytes(), comma + 1)?,
    })
}

/// What [`TABLE`] holds for a byte that is not a base64 character.
///
/// Outside the six-bit range every real entry occupies, so it cannot be
/// confused with a value.
const INVALID: u8 = 0xFF;

/// The value of one base64 character, or [`INVALID`].
const fn digit(byte: u8) -> u8 {
    match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => INVALID,
    }
}

/// Every byte's value, built at compile time from [`digit`].
///
/// **The alphabet is still written exactly once**, in the match above,
/// which is the reason to generate the table rather than type it out: a
/// hand-written 256-entry array is 256 chances to disagree with the
/// ranges it is meant to encode.
///
/// The reason to have it at all is that the ranges cost a chain of
/// comparisons per character, and a payload is a whole mesh buffer --
/// megabytes of text is ordinary here. A lookup is the textbook shape
/// for this, and measurement is what says so rather than instinct: the
/// match ran at roughly a quarter of the table's throughput on an eight
/// megabyte payload, with the error path ruled out as the cause by
/// measuring a sentinel-returning match at the same speed as the
/// `Option` one.
static TABLE: [u8; 256] = {
    let mut table = [INVALID; 256];
    let mut byte = 0_usize;
    while byte < 256 {
        // `byte` is bounded by the loop, so the cast is exact.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the loop bound is 256, which is every value of the type"
        )]
        {
            table[byte] = digit(byte as u8);
        }
        byte += 1;
    }
    table
};

/// Decode the payload beginning at `start`.
///
/// Offsets in every refusal are into the whole URI rather than into the
/// payload, because that is what a caller has in its hand.
fn decode(uri: &[u8], start: usize) -> Result<Vec<u8>, DataUriError> {
    let payload = &uri[start..];
    if !payload.len().is_multiple_of(GROUP) {
        return Err(DataUriError::NotWholeGroups { len: payload.len() });
    }

    let groups = payload.len() / GROUP;
    let mut out = Vec::with_capacity(groups * DECODED);
    for (group, chunk) in payload.as_chunks::<GROUP>().0.iter().enumerate() {
        let last = group + 1 == groups;
        let mut sextets = [0_u8; GROUP];
        let mut padding = 0_usize;

        for (offset, &byte) in chunk.iter().enumerate() {
            let at = start + group * GROUP + offset;
            if byte == PAD {
                // Only the final group's last two places, and once a
                // group is padding it stays padding to the end.
                if !last || offset < GROUP - 2 {
                    return Err(DataUriError::BadPadding { at });
                }
                padding += 1;
                continue;
            }
            if padding != 0 {
                return Err(DataUriError::BadPadding { at: at - padding });
            }
            let value = TABLE[byte as usize];
            if value == INVALID {
                return Err(DataUriError::BadDigit { byte, at });
            }
            sextets[offset] = value;
        }

        // Twenty-four bits in, three bytes out, and one fewer byte for
        // each padding character. The bits the missing bytes would have
        // used must be zero, which is what makes the encoding
        // reversible.
        let carrier = GROUP - 1 - padding;
        let unused = match padding {
            1 => sextets[carrier] & 0b11,
            2 => sextets[carrier] & 0b1111,
            _ => 0,
        };
        if unused != 0 {
            return Err(DataUriError::NonCanonical {
                at: start + group * GROUP + carrier,
                bits: unused,
            });
        }

        let bits = (u32::from(sextets[0]) << 18)
            | (u32::from(sextets[1]) << 12)
            | (u32::from(sextets[2]) << 6)
            | u32::from(sextets[3]);
        for shift in 0..DECODED - padding {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the shift leaves eight bits, which is what the cast keeps"
            )]
            out.push((bits >> (16 - shift * 8)) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{INVALID, TABLE, digit};

    /// **Every entry of the table is the match it was generated from.**
    ///
    /// The table exists so the decoder does one lookup per character
    /// rather than a chain of comparisons, and it is built from `digit`
    /// so the alphabet is written exactly once. This is what says the
    /// generation actually worked -- and it is also the only thing that
    /// runs `digit` outside const evaluation, which is worth knowing:
    /// without it the function is compiled, used, and invisible to
    /// anything that measures what ran.
    #[test]
    fn every_entry_agrees_with_the_match_it_came_from() {
        for byte in 0..=u8::MAX {
            assert_eq!(
                TABLE[byte as usize],
                digit(byte),
                "the table and the match disagree about {byte:#04x}"
            );
        }
    }

    /// **The alphabet is a bijection onto the six-bit values.**
    ///
    /// Sixty-four characters, each with its own value, and every value
    /// spoken for. That is what makes the encoding reversible, and it is
    /// the property a typo in one of the five ranges would break --
    /// quietly, because a duplicated value still decodes and a missing
    /// one is only reached by the payloads that happen to need it.
    #[test]
    fn sixty_four_characters_cover_every_six_bit_value_once() {
        let mut seen = [false; 64];
        let mut characters = 0_usize;
        for value in TABLE {
            if value == INVALID {
                continue;
            }
            characters += 1;
            let index = value as usize;
            assert!(index < 64, "{value} is not a six-bit value");
            assert!(!seen[index], "two characters both mean {value}");
            seen[index] = true;
        }
        assert_eq!(characters, 64, "base64 has sixty-four characters");
        assert!(
            seen.iter().all(|&spoken_for| spoken_for),
            "some six-bit value has no character to spell it"
        );
    }
}
