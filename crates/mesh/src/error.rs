//! Every way a mesh file can fail to be one, by name.
//!
//! **One variant per way the input can be wrong, and every variant
//! carries the numbers.** That is the first two rules of this
//! repository's refusal vocabulary, and a mesh file is where they earn
//! their keep: the reader's caller is usually holding a file it did not
//! produce, exported by a tool it does not own, and "this STL is
//! malformed" sends that caller to a hex editor to recover what this
//! reader already knew.
//!
//! The ordering below is the order a parse can detect things in, which
//! is the order an implementer needs rather than an alphabet: too short
//! to have a header, then a header that does not describe this format,
//! then a count that does not match the bytes, then a word or a number
//! that is not one, then a value that is a number and not a usable one.
//!
//! **Nothing here is a variant no reader constructs**, and getting to
//! that took three deletions. Two variants were written from the shape
//! of the format rather than the shape of the code — a truncated-record
//! refusal and a trailing-bytes one — and neither was reachable once
//! the length arithmetic had already accounted for the file exactly.
//!
//! **The third is a fact about STL worth keeping.** There was a "these
//! bytes are not this format" refusal, on the reasoning that a caller
//! who fed a PNG to a mesh reader has a routing bug and a caller with a
//! truncated file has a download problem. Sound reasoning, and STL
//! gives no way to act on it: **the format has no magic number.** Its
//! binary encoding opens with eighty bytes of anything at all, so
//! "these are not STL bytes" and "these are STL bytes that were cut
//! short" are the same observation. The reader says the more useful of
//! the two — the count that was declared and the bytes that arrived —
//! and does not pretend to the distinction.
//!
//! An error vocabulary with unreachable variants in it is a claim that
//! cases exist which do not, and a caller matching exhaustively pays
//! for every one. All three were found by counting which variants the
//! committed corpus actually reaches, rather than by reading the code.

use core::fmt;

/// What was wrong with the bytes, and where.
///
/// **Closed on purpose** — no `#[non_exhaustive]`. A caller matching on
/// this exhaustively is the point: adding a refusal later should stop a
/// consumer compiling until somebody decides what it means, which is the
/// trade the JSON reader records and this crate reuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshError {
    /// Fewer bytes than the format's fixed header needs.
    ///
    /// The first thing any reader can say, and it is said before a
    /// single field is looked at.
    TooShortForHeader {
        /// What the format requires before anything else.
        needs: usize,
        /// What the caller handed over.
        len: usize,
    },

    /// The declared record count does not account for the bytes.
    ///
    /// Both numbers, because either can be the wrong one: a file
    /// truncated in transit has too few bytes for an honest count, and a
    /// file with a hostile count has an honest length.
    CountMismatch {
        /// Bytes the header's count implies, header included.
        declared: u64,
        /// Bytes actually present.
        actual: usize,
        /// Records the header claimed.
        count: u32,
    },

    /// A count that cannot be turned into a size on this target.
    ///
    /// The 32-bit case is not hypothetical: a `u32` count times a
    /// fifty-byte record overflows a 32-bit `usize` well before it
    /// overflows the count.
    TooLarge {
        /// Which header field held it.
        field: &'static str,
        /// The value, widened so the message is the same everywhere.
        value: u64,
    },

    /// A word the grammar requires is not there.
    ///
    /// Carries the line, because a text format's reader is read by a
    /// person looking at the text.
    ExpectedKeyword {
        /// The word the grammar wanted.
        expected: &'static str,
        /// What stood there instead, truncated to something printable.
        found: String,
        /// One-based, as an editor counts.
        line: u32,
    },

    /// A number the grammar requires is not one.
    NotANumber {
        /// The text that was supposed to be a number, truncated.
        found: String,
        /// One-based.
        line: u32,
    },

    /// A coordinate that is a float but not a usable one.
    ///
    /// **Refused at the boundary rather than carried inward**, because
    /// this is the last place that knows both the value and the field it
    /// came from. A NaN vertex draws nothing and collides with
    /// everything; an infinity makes every bounding box infinite.
    NotFinite {
        /// Which of position, normal or coordinate.
        field: &'static str,
        /// The record it was in, zero-based, as the file stores them.
        index: u32,
    },

    /// A file that declares no geometry at all.
    ///
    /// **A refusal rather than an empty mesh**, and the distinction is
    /// worth stating: an empty mesh is a legal thing to build in memory
    /// and an odd thing to find in a file. Every way a file ends up with
    /// zero triangles — an exporter with nothing selected, a filter that
    /// removed everything, a truncation that lost the body but kept the
    /// header — is a mistake upstream, and a caller that genuinely wants
    /// nothing did not need to read a file to get it.
    NoGeometry,
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShortForHeader { needs, len } => write!(
                f,
                "a {needs}-byte header is the least this format can be, and there are {len} bytes"
            ),
            Self::CountMismatch {
                declared,
                actual,
                count,
            } => write!(
                f,
                "the header's count of {count} accounts for {declared} bytes and the file holds \
                 {actual}"
            ),
            Self::TooLarge { field, value } => write!(
                f,
                "`{field}` is {value}, which is more than this target can address"
            ),
            Self::ExpectedKeyword {
                expected,
                found,
                line,
            } => write!(f, "line {line}: expected `{expected}`, found `{found}`"),
            Self::NotANumber { found, line } => {
                write!(f, "line {line}: `{found}` is not a number")
            }
            Self::NotFinite { field, index } => write!(
                f,
                "record {index}: the {field} is not a finite number, and a mesh carrying one is a \
                 mesh nothing downstream can bound"
            ),
            Self::NoGeometry => write!(
                f,
                "the file declares no geometry, which every way of arriving at is a mistake \
                 somewhere upstream"
            ),
        }
    }
}

impl core::error::Error for MeshError {}

/// The longest a quoted run may be **once escaped**, in characters.
///
/// **A bound on the message a malformed input can make this reader
/// build**, which is this repository's rule that a refused input never
/// costs more than its own length buys. Without it a line of a hundred
/// thousand letters becomes a hundred-thousand-letter error string.
///
/// **Counted after escaping, not before, and that distinction was a
/// defect for about an hour.** This bounded the *input* characters at
/// thirty-two while escaping expanded each unprintable one to eight, so
/// a run of thirty-two control bytes produced a two-hundred-and-sixty
/// character message — the bound held on the wrong side of the
/// multiplication. Found by a test that fed the reader eighty-two zero
/// bytes and printed what came back.
pub(crate) const MAX_QUOTED: usize = 32;

/// `text` as a refusal will quote it: bounded, printable, and with the
/// fact that it was cut visible rather than silent.
///
/// **Non-printing characters are escaped rather than passed through.**
/// The text being quoted came out of a file nobody here wrote, so it can
/// hold anything — and a refusal is printed to a terminal. Control bytes
/// travelling from a hostile file into a log through an error message
/// are how a file that could not be parsed still gets to move a cursor,
/// clear a screen, or hide the rest of the line it was reported on.
/// Found by a test feeding the reader three arbitrary bytes and reading
/// what came back.
pub(crate) fn quoted(text: &str) -> String {
    let mut kept = String::new();
    let mut shown = 0usize;
    let mut cut = false;
    for character in text.chars() {
        // The escape rather than the character, so what is printed is
        // what the file held and not what it would have done.
        let piece: String = if character.is_control() {
            format!("<U+{:04X}>", u32::from(character))
        } else {
            character.to_string()
        };
        if shown + piece.chars().count() > MAX_QUOTED {
            cut = true;
            break;
        }
        shown += piece.chars().count();
        kept.push_str(&piece);
    }
    if cut || kept.chars().count() < text.chars().count() {
        kept.push('…');
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::{MAX_QUOTED, MeshError, quoted};

    /// Every variant says something, and says the numbers it carries.
    ///
    /// **Matched exhaustively with no wildcard**, which is the whole
    /// benefit of the enum being closed: a refusal added later stops
    /// this file compiling until somebody writes what it says.
    #[test]
    fn every_refusal_names_its_numbers() {
        let all = [
            MeshError::TooShortForHeader { needs: 84, len: 3 },
            MeshError::CountMismatch {
                declared: 134,
                actual: 90,
                count: 1,
            },
            MeshError::TooLarge {
                field: "triangle count",
                value: 4_000_000_000,
            },
            MeshError::ExpectedKeyword {
                expected: "facet",
                found: "wombat".to_owned(),
                line: 7,
            },
            MeshError::NotANumber {
                found: "1.0.0".to_owned(),
                line: 9,
            },
            MeshError::NotFinite {
                field: "position",
                index: 12,
            },
            MeshError::NoGeometry,
        ];

        for refusal in &all {
            let shown = refusal.to_string();
            assert!(!shown.is_empty(), "{refusal:?} says nothing");
            // Rule two: the numbers a caller would otherwise go to a hex
            // editor for are in the message.
            let numbers: Vec<&str> = match refusal {
                MeshError::TooShortForHeader { .. } => vec!["84", "3"],
                MeshError::CountMismatch { .. } => vec!["134", "90", "1"],
                MeshError::TooLarge { .. } => vec!["triangle count", "4000000000"],
                MeshError::ExpectedKeyword { .. } => vec!["facet", "wombat", "7"],
                MeshError::NotANumber { .. } => vec!["1.0.0", "9"],
                MeshError::NotFinite { .. } => vec!["position", "12"],
                MeshError::NoGeometry => vec!["no geometry"],
            };
            for number in numbers {
                assert!(
                    shown.contains(number),
                    "`{shown}` omits `{number}`, which is the number a reader is holding the file \
                     to look for"
                );
            }
        }
    }

    /// A quoted run is bounded, and says so when it is cut.
    ///
    /// Probed by returning `text.to_owned()`: red, the refusal quotes
    /// all ten thousand characters.
    #[test]
    fn a_quote_is_bounded_and_marks_where_it_stopped() {
        let short = "facet";
        assert_eq!(quoted(short), short, "nothing to cut, nothing added");

        let long = "x".repeat(10_000);
        let cut = quoted(&long);
        assert!(
            cut.chars().count() == MAX_QUOTED + 1,
            "a quote is {MAX_QUOTED} characters and the mark that it was cut, got {}",
            cut.chars().count()
        );
        assert!(cut.ends_with('…'), "a cut quote says it was cut: `{cut}`");
    }

    /// The cut counts characters, not bytes, so a quote is always text.
    ///
    /// **A byte-counting cut can split a multi-byte character**, and the
    /// half it leaves is not a `String` — in Rust that is a panic in the
    /// reader, which is the one thing a reader of hostile input may
    /// never do. The file's own text is exactly where such characters
    /// come from: an exporter writing a UTF-8 object name.
    ///
    /// Probed by cutting with `&text[..MAX_QUOTED]`: red, the slice
    /// panics on a character boundary.
    #[test]
    fn a_quote_of_wide_characters_is_still_text() {
        // Two-byte characters, so a byte-counting cut of 32 lands
        // between the halves of the seventeenth.
        let wide = "é".repeat(100);
        let cut = quoted(&wide);
        assert_eq!(cut.chars().count(), MAX_QUOTED + 1);
        assert!(cut.starts_with('é'));
    }
}
