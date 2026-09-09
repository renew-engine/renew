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
//! that took two deletions. Both were written from the shape of the
//! format rather than the shape of the code — a truncated-record
//! refusal and a trailing-bytes one — and neither was reachable once
//! the length arithmetic had already accounted for the file exactly.
//!
//! **A third was deleted and has since come back, which is the more
//! useful story.** `NotThisFormat` was removed on the reasoning that a
//! caller who fed a PNG to a mesh reader has a routing bug and a caller
//! with a truncated file has a download problem — sound — and that STL
//! gives no way to act on the distinction, which is also sound: **that
//! format has no magic number.** Its binary encoding opens with eighty
//! bytes of anything at all, so "these are not STL bytes" and "these are
//! STL bytes that were cut short" are the same observation. STL says the
//! more useful of the two, the count declared and the bytes that
//! arrived, and does not pretend to a distinction it cannot make.
//!
//! **The error was generalising one format's limitation to the enum.**
//! PLY opens with the word `ply` and the blob with an eight-byte magic,
//! and both could tell the two faults apart the whole time. PLY instead
//! answered a foreign file with `ExpectedKeyword` naming `end_header` —
//! a claim about a truncated PLY, made about a file that had never been
//! one, and byte-identical to what a genuinely truncated PLY got. A
//! refusal reachable by two unrelated faults says nothing about either.
//! The variant is back, the readers that can reach it do, and the three
//! that cannot say so in their own censuses.
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

    /// A number past a ceiling this reader sets.
    ///
    /// **A policy ceiling, not a representation limit**, and the
    /// distinction is the reason this exists at all: a representation
    /// limit is reached only after the allocation has been attempted,
    /// and a policy ceiling is a refusal that costs nothing. The
    /// ceilings are on a schema's element and property counts, on how
    /// many corners one face may name, and on the total geometry a file
    /// may build — that last one because the first three bound
    /// factors and none of them bounds the product.
    ///
    /// An earlier version of this doc said the variant was about a
    /// count "that cannot be turned into a size on this target", and
    /// gave the 32-bit overflow as the case. No reader here can reach
    /// that: the pack-style length equality bounds the product by the
    /// file before the conversion happens.
    TooLarge {
        /// Which header field held it.
        field: &'static str,
        /// The value, widened so the message is the same everywhere.
        value: u64,
    },

    /// These bytes are not this format at all.
    ///
    /// **The difference from every refusal below is what it tells the
    /// caller to go and look at.** A malformed file of the right kind
    /// sends a person to the file; a file of the wrong kind sends them
    /// to the wiring — the wrong path, the wrong entry, a mislabelled
    /// asset. Folding the two together sends somebody hunting for
    /// corruption in a perfectly good file of another format.
    ///
    /// **Every other parser in this repository has this refusal** — the
    /// PNG decoder, the pack reader, the WAV reader, the UI document,
    /// the wire codec and the trace codec each name it. The mesh readers
    /// did not, and answered instead with a keyword and a line number
    /// they had invented, which was worse than a generic failure because
    /// it was specific and wrong.
    NotThisFormat {
        /// What the opening bytes would have had to say.
        expected: &'static str,
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

    /// A face naming a vertex the file does not contain.
    ///
    /// **The refusal an indexed format needs and a soup format cannot
    /// have.** STL repeats every corner, so there is no index to be
    /// wrong; PLY numbers its vertices and its faces point at them, and
    /// a number one past the end is the difference between a mesh and a
    /// read past the end of a buffer. Carries the face as well as the
    /// index, because a file with one bad face and a file whose whole
    /// index base is off by one are different problems.
    IndexOutOfRange {
        /// The vertex number the face asked for, as the file spelled it.
        ///
        /// **Signed, because one of these formats has signed indices.**
        /// OBJ lets a face count backwards from what has been declared
        /// so far, so `-5` in a file with three vertices is a real thing
        /// a writer emits and a caller needs to see spelled the way the
        /// file spelled it. Reporting its magnitude would send someone
        /// looking for a fifth vertex that was never the subject.
        index: i64,
        /// How many vertices the file actually declared.
        count: usize,
        /// Which face asked, zero-based, as the file stores them.
        face: u32,
    },

    /// An index of zero where the format numbers from one.
    ///
    /// **Separate from an index out of range, because it is usually a
    /// different bug.** Out of range is a writer that computed the wrong
    /// number; zero is very often a field nobody filled in, a default
    /// that escaped, or a base that was never added. Telling them apart
    /// points at different code.
    IndexZero {
        /// Which line asked, one-based, as an editor counts.
        line: u32,
    },

    /// A face with too few corners to be a surface.
    ///
    /// Two corners is a line and one is a point; neither covers any
    /// area, and a reader that silently dropped them would be deciding
    /// that a file which says it has forty faces really has thirty-nine.
    NotAFace {
        /// Which face, zero-based.
        face: u32,
        /// How many corners it named.
        corners: usize,
    },

    /// The file does not describe something this reader can turn into
    /// geometry, though it is well-formed.
    ///
    /// **Separate from a malformed file, because the caller's next move
    /// differs.** A PLY holding only a point cloud, or vertices with no
    /// `x` property, is a valid PLY that this reader cannot use; the
    /// answer is to convert it or to read it with something else, not to
    /// re-export it. Names what was looked for so the caller can tell
    /// which.
    Unsupported {
        /// What the reader needed and did not find, in the file's own
        /// vocabulary, so it can be searched for.
        wanted: &'static str,
    },

    /// Two streams describing the same vertices, and different numbers
    /// of them.
    ///
    /// **The refusal a format with one payload never needs.** Every
    /// other reader in this crate has a single stream whose length must
    /// divide; a model has several, and the constraint between them is
    /// equality rather than divisibility. Nothing downstream can catch
    /// it either: whatever interleaves these arrays is the last thing
    /// that knows they were separate, so reading past the end of the
    /// short one is that code's own bug rather than the file's.
    ///
    /// Carries the stream's name as the file spells it, so a caller can
    /// search the file for the one that disagreed.
    StreamLengthMismatch {
        /// The attribute, in the file's own vocabulary.
        stream: &'static str,
        /// How many the authoritative stream held.
        expected: usize,
        /// How many this one held.
        found: usize,
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

impl MeshError {
    /// The variant's own name, for a caller that has to act on which
    /// refusal this is rather than read it.
    ///
    /// **A message is for a person and a name is for a program.** The
    /// sentences below are meant to improve, and a script keying on one
    /// breaks when it does; these names are part of the surface and do
    /// not change without the variant changing.
    ///
    /// No wildcard arm, so a variant added later stops this compiling
    /// until it is named.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::TooShortForHeader { .. } => "TooShortForHeader",
            Self::CountMismatch { .. } => "CountMismatch",
            Self::TooLarge { .. } => "TooLarge",
            Self::NotThisFormat { .. } => "NotThisFormat",
            Self::ExpectedKeyword { .. } => "ExpectedKeyword",
            Self::NotANumber { .. } => "NotANumber",
            Self::NotFinite { .. } => "NotFinite",
            Self::IndexOutOfRange { .. } => "IndexOutOfRange",
            Self::IndexZero { .. } => "IndexZero",
            Self::NotAFace { .. } => "NotAFace",
            Self::Unsupported { .. } => "Unsupported",
            Self::StreamLengthMismatch { .. } => "StreamLengthMismatch",
            Self::NoGeometry => "NoGeometry",
        }
    }
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Says what the file is not, and does not guess at what it
            // is: the caller knows what it meant to open, and naming
            // another format here would be a second guess on top of a
            // first.
            Self::NotThisFormat { expected } => write!(
                f,
                "these bytes do not open with {expected}, so they are not this format at all"
            ),
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
                "`{field}` is {value}, past the ceiling this reader sets for it"
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
            Self::IndexOutOfRange { index, count, face } => write!(
                f,
                "face {face} names vertex {index} and the file declares {count}"
            ),
            Self::IndexZero { line } => write!(
                f,
                "line {line} names vertex 0, and this format numbers its vertices from one"
            ),
            Self::NotAFace { face, corners } => write!(
                f,
                "face {face} names {corners} corners, and a surface needs three"
            ),
            Self::Unsupported { wanted } => write!(
                f,
                "this file is well-formed and this reader cannot use it: no `{wanted}`"
            ),
            Self::StreamLengthMismatch {
                stream,
                expected,
                found,
            } => write!(
                f,
                "`{stream}` holds {found} and the positions hold {expected}"
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
/// Whether a character must be escaped before it is printed back.
///
/// **`char::is_control` was not enough, and the gap was the exact threat
/// this escaping was written against.** That predicate is Unicode
/// category Cc alone, so `U+202E` RIGHT-TO-LEFT OVERRIDE passed through
/// untouched — and a bidi override is precisely the thing that "hides
/// the rest of the line it was reported on", which is the sentence the
/// original comment used to justify escaping at all. `U+2028` LINE
/// SEPARATOR, `U+200F`, `U+00AD` and `U+061C` went through with it.
///
/// So the question is not "is this a control character" but "can this
/// change what a reader of the message sees". Cc for the terminal
/// escapes, Cf for the bidi and formatting controls, and the two
/// separators that end a line without being `\n`.
fn is_dangerous(character: char) -> bool {
    character.is_control()
        || matches!(character,
            // Cf: bidi overrides and embeddings, the soft hyphen, the
            // Arabic letter mark, the zero-width joiners.
            '\u{00AD}' | '\u{061C}' | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
            // Zl and Zp: a line and a paragraph separator, which end a
            // line in a renderer without being a newline here.
            | '\u{2028}' | '\u{2029}')
}

pub(crate) fn quoted(text: &str) -> String {
    let mut kept = String::new();
    let mut shown = 0usize;
    let mut cut = false;
    for character in text.chars() {
        // The escape rather than the character, so what is printed is
        // what the file held and not what it would have done.
        let piece: String = if is_dangerous(character) {
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
    ///
    /// **The match is checked by the compiler and the list below is
    /// not, and that gap has already been fallen into.** Adding
    /// `NotThisFormat` forced a new arm — the enum is closed, so it had
    /// to — and the arm was written while the list was not touched, so
    /// the new variant had a rule about its message and no instance to
    /// apply it to. The test went on passing and measured one variant
    /// fewer. The count below is the only part of this that fails when
    /// that happens again; it is a hand-maintained number, and it is
    /// here because the compiler cannot be made to hold this half.
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
                field: "element count",
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
            MeshError::IndexOutOfRange {
                index: 7,
                count: 4,
                face: 1,
            },
            MeshError::IndexZero { line: 11 },
            MeshError::NotAFace {
                face: 2,
                corners: 2,
            },
            MeshError::Unsupported { wanted: "vertex" },
            MeshError::StreamLengthMismatch {
                stream: "NORMAL",
                expected: 512,
                found: 511,
            },
            MeshError::NoGeometry,
            MeshError::NotThisFormat { expected: "ply" },
        ];

        // One per variant. Raise it when the enum grows, in the same
        // change that writes the new arm below.
        assert_eq!(all.len(), 13, "a variant is missing an instance here");

        for refusal in &all {
            let shown = refusal.to_string();
            assert!(!shown.is_empty(), "{refusal:?} says nothing");
            // Rule two: the numbers a caller would otherwise go to a hex
            // editor for are in the message.
            let numbers: Vec<&str> = match refusal {
                MeshError::NotThisFormat { .. } => vec!["ply"],
                MeshError::StreamLengthMismatch { .. } => vec!["NORMAL", "512", "511"],
                MeshError::TooShortForHeader { .. } => vec!["84", "3"],
                MeshError::CountMismatch { .. } => vec!["134", "90", "1"],
                MeshError::TooLarge { .. } => vec!["element count", "4000000000"],
                MeshError::ExpectedKeyword { .. } => vec!["facet", "wombat", "7"],
                MeshError::NotANumber { .. } => vec!["1.0.0", "9"],
                MeshError::NotFinite { .. } => vec!["position", "12"],
                MeshError::IndexOutOfRange { .. } => vec!["7", "4", "1"],
                MeshError::IndexZero { .. } => vec!["11", "0"],
                MeshError::NotAFace { .. } => vec!["2", "2"],
                MeshError::Unsupported { .. } => vec!["vertex"],
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

    /// **Every range `is_dangerous` names, exercised.**
    ///
    /// The predicate lists five Cf ranges and two separators, and the
    /// suite reached only some of them: `U+2060`–`U+2064` and
    /// `U+2066`–`U+2069` were named by the code and by nothing else, so
    /// deleting either arm changed no test. They are the invisible
    /// joiners and the isolate controls — the ones that reorder a line
    /// without putting a mark on it, which is the whole reason the
    /// predicate is wider than `char::is_control`.
    ///
    /// Probed by deleting each range in turn: red, the character comes
    /// back unescaped.
    #[test]
    fn every_character_the_predicate_names_is_escaped() {
        // One from each arm, so no arm stands for another.
        for character in [
            '\u{0007}', '\u{00AD}', '\u{061C}', '\u{180E}', '\u{200B}', '\u{200F}', '\u{202A}',
            '\u{202E}', '\u{2060}', '\u{2064}', '\u{2066}', '\u{2069}', '\u{FEFF}', '\u{2028}',
            '\u{2029}',
        ] {
            let text = format!("a{character}b");
            let shown = quoted(&text);
            // The code point is bound here rather than formatted
            // inside the message: an argument evaluated only when
            // the assertion fails is a line no passing run reaches.
            let code = u32::from(character);
            assert!(
                !shown.contains(character),
                "U+{code:04X} reached a message unescaped: `{shown}`"
            );
            assert!(
                shown.starts_with('a') && shown.ends_with('b'),
                "the text around it survived: `{shown}`"
            );
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
        // `assert_eq!` rather than `assert!` with the count repeated in
        // the message: the repeat is only evaluated when the assertion
        // fails, so it is a line no passing run can reach.
        assert_eq!(
            cut.chars().count(),
            MAX_QUOTED + 1,
            "a quote is {MAX_QUOTED} characters and the mark that it was cut"
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
