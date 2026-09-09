//! JSON, read from bytes nobody here wrote.
//!
//! [`Json::parse`] takes a byte slice, validates every character of it,
//! and hands back a document that **borrows** those bytes. Nothing is
//! copied: a string stays where it lies until somebody asks for it
//! decoded, and a number stays as the characters it was written with
//! until somebody asks what type they wanted it as. What the parse
//! allocates is one flat table of fixed-width records, one per accepted
//! token, and nothing else.
//!
//! # Contract
//!
//! - **Every byte string gets an answer.** A document or a named
//!   refusal, never a panic and never a read past the end. The refusals
//!   are [`JsonErrorKind`], one variant per way the bytes can be wrong,
//!   each carrying the offset it happened at.
//! - **Nesting is bounded and the bound is refused, not crashed into.**
//!   The parser holds its stack on the heap and stops at [`MAX_DEPTH`].
//!   A recursive reader meets a deep enough document with a stack
//!   overflow, and a stack overflow is not an answer.
//! - **Nothing is allocated from a number the document declares.** The
//!   node table grows by pushing what has already been accepted, so a
//!   document refused at its second byte costs the reader almost
//!   nothing whatever its length.
//! - **The crate never touches the filesystem.** It takes bytes. That is
//!   what lets the same reader serve a whole file and a chunk carved out
//!   of the middle of a larger one, with no knowledge of the container
//!   that carved it.
//! - **Whitespace after the value is skipped before the end is
//!   demanded**, so a document padded out to an alignment boundary
//!   parses without the reader knowing why it was padded.
//!
//! # Example
//!
//! ```
//! use renew_json::Json;
//!
//! let doc = Json::parse(br#"{"mesh": 3, "name": "hull", "scale": [1, 2.5, 3e0]}"#)?;
//! let root = doc.root();
//!
//! assert_eq!(root.get("mesh").map(|v| v.as_u32()).transpose()?, Some(3));
//! assert!(root.get("name").is_some_and(|v| v.as_str().is_ok_and(|s| s == "hull")));
//!
//! let scale = root.get("scale").expect("the document has a scale");
//! assert_eq!(scale.len(), 3);
//! // A whole number spelled with an exponent is still whole.
//! assert_eq!(scale.index(2).map(|v| v.as_u32()).transpose()?, Some(3));
//! # Ok::<(), renew_json::JsonError>(())
//! ```
//!
//! # What this is not
//!
//! Not a writer. Not a tree a caller can build or edit. Not a mapping to
//! a caller's own types — there is no derive here, and adding one would
//! be a second crate rather than a wider surface on this one.
//!
//! Not a validator of anything above the grammar, either. Which member
//! names mean what, which numbers are indices into which array, whether
//! a required field is present: all of that belongs to whichever layer
//! knows the schema. A reader that knew one schema's member names would
//! be that schema's reader wearing a general name.
//!
//! # Two rules that look like bugs and are not
//!
//! **A duplicate member name is accepted, and the last one wins.** The
//! house habit is to refuse rather than repair, and this is the place it
//! is deliberately not followed: the asset formats this reader was sized
//! for say that member names *should* be unique and that a reader
//! *should* let a later value override an earlier one. Refusing would
//! reject files those formats bless. [`Value::get`] therefore answers
//! with the last match, and [`Value::get_all`] hands over every one, so
//! a layer that wants to be stricter than the format can be, with the
//! evidence in its hand.
//!
//! **A whole number may be written `3.0` or `3e2`.** Those formats
//! permit it explicitly, so [`Value::as_u32`] and its siblings accept
//! any spelling whose value is whole and refuse only a non-zero
//! fraction. The conversion is exact decimal arithmetic over the
//! characters, never a trip through a float, so a whole number near the
//! top of `u64` comes back as itself rather than as the nearest double.

// A reader hands back refusals as values; anything it printed would
// reach a stream it does not own, in a process that may have no console
// at all.
#![deny(clippy::print_stdout, clippy::print_stderr)]
// The parser does no float arithmetic anywhere: numbers are validated as
// characters and converted to integers by decimal arithmetic on those
// characters. The only float in the crate is the one a caller explicitly
// asks for, produced by the standard library and checked for finiteness.
#![deny(clippy::float_arithmetic)]

mod error;
mod number;
mod parse;
mod text;
mod value;

pub use error::{JsonError, JsonErrorKind};
pub use text::{Chars, Str};
pub use value::{Elements, Entries, Value};

use core::fmt;

/// The deepest nesting of objects and arrays this reader will follow.
///
/// Sixty-four, which is generous by roughly a factor of eight against
/// anything a schema produces on purpose: the deepest path through a
/// glTF scene description reaches six, and its ratified extensions add
/// two. The slack is for the parts of such a format that are explicitly
/// arbitrary application data, where the nesting is whatever somebody's
/// exporter felt like.
///
/// It is a *policy*, and it can be, because the parser's stack is a
/// vector rather than the call stack. A recursive reader's real limit is
/// whatever its frames happen to cost on whichever platform ran out
/// first, which is not a number anybody can write in a document.
pub const MAX_DEPTH: usize = 64;

/// The most characters this reader will read a single number as.
///
/// A million-digit number is legal JSON, and every consumer of one pays
/// for it: the standard library's float parser walks the digits, and so
/// does the exact integer conversion here. The shortest decimal form
/// that round-trips a double needs twenty-odd characters, so a hundred
/// and twenty-eight leaves room for padding nobody needed and still puts
/// a wall in front of a generated document.
pub const MAX_NUMBER_LEN: usize = 128;

/// What a value is.
///
/// Six kinds, which is all of them: JSON has no others, and this reader
/// adds none. `true` and `false` are one kind here because a caller that
/// wants to know which asks [`Value::as_bool`], and a caller that wants
/// to know the shape of a document does not care.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool,
    /// A number, as it was written.
    Number,
    /// A string, escapes and all.
    String,
    /// An array.
    Array,
    /// An object.
    Object,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Null => "null",
            Self::Bool => "a boolean",
            Self::Number => "a number",
            Self::String => "a string",
            Self::Array => "an array",
            Self::Object => "an object",
        })
    }
}

/// One accepted token, as the document remembers it.
///
/// Fixed width and `Copy`, which is the point of it. The obvious shape
/// for a parsed document is a tree of enums holding owned strings and
/// nested vectors, and it is the wrong one here for three separate
/// reasons: it allocates once per member and once per string, all of it
/// sized by an untrusted file and all of it before any layer above has
/// looked at a single field; it drops **recursively**, so a deep
/// document unwinds through as many frames on the way out as the parser
/// refused to use on the way in; and it decides what a number is by how
/// it was spelled, which is exactly the decision that has to wait for a
/// caller who knows the schema.
///
/// A flat table has none of those. It is one allocation that grows by
/// pushing, it frees in one call because nothing in it owns anything,
/// and it keeps every scalar as the span of source it came from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Node {
    /// Which of the six kinds — plus the member names, which are stored
    /// as strings because that is what they are.
    kind: Kind,
    /// The first byte of the token, in the source.
    start: usize,
    /// One past the last byte of the token, in the source. For a
    /// container that is one past its closing bracket, so a container's
    /// span is the text a caller would cut out to quote it.
    end: usize,
    /// How many nodes this one's subtree occupies, itself included.
    ///
    /// This is what replaces a child pointer and a sibling pointer.
    /// Nodes are written in the order they were read, so a container's
    /// children begin at the very next index, and the node after the
    /// whole subtree is at `index + subtree`. Two numbers become one,
    /// and the one that remains is the one a walk actually needs.
    subtree: usize,
}

impl Node {
    /// The node a cursor falls back on when a document hands it nothing.
    ///
    /// Unreachable from a document this crate parsed — every accepted
    /// document has at least one node, because a document is one value.
    /// It exists so that "unreachable" is spelled as a null rather than
    /// as a panic, in a crate whose contract is that nothing here
    /// panics.
    const VOID: Self = Self {
        kind: Kind::Null,
        start: 0,
        end: 0,
        subtree: 1,
    };
}

/// A validated document, borrowing the bytes it was read from.
///
/// The lifetime is the bytes'. The cursors handed out by [`root`] and
/// everything reachable from it borrow *this* value, so a document may
/// be dropped while the bytes live on, and nothing handed out survives
/// the document — which is what makes the node table safe to hold as a
/// plain vector of indices.
///
/// [`root`]: Json::root
#[derive(Debug)]
pub struct Json<'a> {
    source: &'a str,
    nodes: Vec<Node>,
}

impl<'a> Json<'a> {
    /// Validate `bytes` and borrow them as a document.
    ///
    /// The whole slice is checked as text first, so every offset any
    /// refusal reports is a character boundary by construction rather
    /// than by an argument somebody has to keep re-checking. Then the
    /// grammar is checked in one pass: every string's escapes, every
    /// number's spelling, every bracket's partner, and the nesting depth.
    /// A document that comes back has nothing deferred in it.
    ///
    /// Bytes rather than text on purpose. A caller reading a file has
    /// bytes; a caller handed one chunk of a container format has bytes;
    /// and taking text would push the "is this even UTF-8" question onto
    /// every one of them, which is the question this reader is best
    /// placed to answer and name.
    ///
    /// # Errors
    ///
    /// A [`JsonError`] naming what was wrong and the offset it was wrong
    /// at. Every one of them is a [`JsonErrorKind`] variant; there is no
    /// bucket.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, JsonError> {
        let source = parse::validate_text(bytes)?;
        let nodes = parse::document(source)?;
        Ok(Self { source, nodes })
    }

    /// The document's one value.
    ///
    /// The cursor borrows this document, not the bytes: a document may
    /// be dropped while its bytes live on, and nothing it handed out
    /// outlives it. That is what makes a table of plain indices safe to
    /// walk with no checks a reader has to trust.
    #[must_use]
    pub fn root(&self) -> Value<'_> {
        Value::new(self.source, &self.nodes)
    }
}
