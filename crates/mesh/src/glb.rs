//! The binary glTF container: a header, a chunk table, and nothing read.
//!
//! **This module does framing and no semantics.** It hands back the
//! bytes of the JSON chunk and the bytes of the binary chunk, borrowed
//! from the caller's buffer, and parses neither. Everything that gives
//! those bytes meaning — accessors, buffer views, geometry — is a layer
//! above, and keeping the split here is what lets the framing be fuzzed
//! on its own, where every length in the file is hostile and none of
//! them has been read yet.
//!
//! # The refusal that is not here, and why
//!
//! Every other reader in this crate refuses a construct it does not
//! recognise. The blob refuses a presence bit outside its vocabulary,
//! on the argument that a header using a construct the reader does not
//! know is a file a later build wrote, and guessing at it means reading
//! arrays at the wrong offsets.
//!
//! **The container specification requires the opposite for chunks, in
//! as many words:** *"Client implementations MUST ignore chunks with
//! unknown types to enable glTF extensions to reference additional
//! chunks with new types following the first two chunks."* An unknown
//! chunk type is a designed extension point, not a hole, and refusing
//! one rejects conformant files.
//!
//! So this reader **skips a chunk whose type it does not know and
//! refuses a version it does not know**, four lines apart, and the two
//! are not inconsistent: the version is a claim about the whole file,
//! and a chunk type is a claim the format has promised is skippable.
//! Skipping is safe only because the chunk's own length is validated
//! first — an unknown chunk is stepped over, never read.
//!
//! # Its own refusals, and not the geometry crate's
//!
//! [`GlbError`] is separate from [`MeshError`](crate::MeshError) because
//! a container fault and a geometry fault send a caller to different
//! places, and because folding eleven container-shaped variants into the
//! geometry enum would put an arm in every reader's refusal census
//! saying "not a container" five times over. The two meet when a glTF
//! reader exists; at this layer there is nothing to convert.

use core::fmt;

/// The four bytes every binary glTF opens with.
///
/// `0x46546C67` little-endian, which is `glTF` in ASCII — spelled here
/// as the bytes rather than the integer because that is the order they
/// appear in the file, and a reader comparing a slice should read like
/// one.
pub(crate) const MAGIC: [u8; 4] = *b"glTF";

/// The container version this reader implements.
///
/// **The container's version, not the asset's.** The specification is
/// explicit that a client must also check the asset version inside the
/// JSON chunk, which is a different number in a different place and
/// belongs to whatever reads that JSON.
const VERSION: u32 = 2;

/// The fixed preamble: magic, version, total length.
const HEADER: usize = 12;

/// Where the version sits, and where the length does.
const OFF_VERSION: usize = 4;
const OFF_LENGTH: usize = 8;

/// A chunk's own preamble: its length and its type.
const CHUNK_HEADER: usize = 8;

/// `JSON` in ASCII, little-endian.
const CHUNK_JSON: u32 = 0x4E4F_534A;

/// `BIN\0` in ASCII, little-endian.
const CHUNK_BIN: u32 = 0x004E_4942;

/// The alignment every chunk starts and ends on.
const ALIGN: u32 = 4;

/// Every way a binary glTF container can fail to be one.
///
/// **Closed on purpose**, as this crate's geometry refusals are: a
/// caller matching exhaustively should stop compiling when a refusal is
/// added, rather than silently routing it through a wildcard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlbError {
    /// Fewer bytes than the twelve-byte preamble needs.
    TooShortForHeader {
        /// What the container requires before anything else.
        needs: usize,
        /// What the caller handed over.
        len: usize,
    },

    /// These bytes do not open with `glTF`.
    NotThisFormat {
        /// What the opening bytes would have had to say.
        expected: &'static str,
    },

    /// A container version this build does not read.
    UnknownVersion {
        /// What the header declared.
        found: u32,
    },

    /// The header's total length is not the length of the file.
    ///
    /// **Equality, never "at least."** Something appended to a container
    /// is as much a sign of trouble as something cut off it: a reader
    /// that accepts trailing bytes accepts a file with a second payload
    /// hidden after the first, and accepts two different files as the
    /// same file.
    SizeMismatch {
        /// Bytes the header said the file has.
        declared: u64,
        /// Bytes it actually has.
        actual: usize,
    },

    /// A chunk's own eight-byte preamble runs off the end.
    ChunkHeaderTruncated {
        /// Which chunk, zero-based, counting every chunk including
        /// skipped ones.
        chunk: u32,
        /// Bytes left where eight were needed.
        available: usize,
    },

    /// A chunk claims more bytes than remain after it.
    ChunkOverruns {
        /// Which chunk, zero-based.
        chunk: u32,
        /// What its header claimed.
        declared: u64,
        /// What was left.
        available: usize,
    },

    /// A chunk length that would leave the next chunk unaligned.
    ///
    /// The specification requires each chunk to start and end on a
    /// four-byte boundary. Since the preamble and every chunk header are
    /// themselves multiples of four, that reduces to this: a chunk's
    /// length is a multiple of four. **The padding is inside the length**
    /// — a JSON chunk is padded with spaces and a binary chunk with
    /// zeros, and both are counted — so a length that is not a multiple
    /// of four is a writer that forgot to pad, not a reader that should.
    ChunkNotAligned {
        /// Which chunk, zero-based.
        chunk: u32,
        /// The length it declared.
        length: u32,
    },

    /// A container with no chunks at all.
    ///
    /// Separate from a first chunk of the wrong kind, because it is a
    /// different fault: this file was cut down to its header, and there
    /// is no chunk to have been wrong.
    NoChunks,

    /// The first chunk is not the JSON one.
    ///
    /// The specification requires the JSON chunk to be the very first,
    /// so that a reader can retrieve the rest progressively. A container
    /// that puts anything else first cannot be read that way, and a
    /// reader that hunted for the JSON chunk instead would be accepting
    /// a file no conforming writer produces.
    FirstChunkNotJson {
        /// The type the first chunk declared, as the file spelled it.
        found: u32,
    },

    /// A second chunk of a kind the container permits only once.
    RepeatedChunk {
        /// `JSON` or `BIN`.
        kind: &'static str,
        /// Which chunk repeated it, zero-based.
        chunk: u32,
    },

    /// A binary chunk somewhere other than second.
    BinaryChunkOutOfPlace {
        /// Where it was, zero-based.
        chunk: u32,
    },
}

impl GlbError {
    /// The variant's own name, for a caller acting on which refusal this
    /// is rather than reading it.
    ///
    /// **A message is for a person and a name is for a program**, which
    /// is the same trade this crate's geometry refusals make. The
    /// sentences below are meant to improve; these names are part of the
    /// surface.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TooShortForHeader { .. } => "TooShortForHeader",
            Self::NotThisFormat { .. } => "NotThisFormat",
            Self::UnknownVersion { .. } => "UnknownVersion",
            Self::SizeMismatch { .. } => "SizeMismatch",
            Self::ChunkHeaderTruncated { .. } => "ChunkHeaderTruncated",
            Self::ChunkOverruns { .. } => "ChunkOverruns",
            Self::ChunkNotAligned { .. } => "ChunkNotAligned",
            Self::NoChunks => "NoChunks",
            Self::FirstChunkNotJson { .. } => "FirstChunkNotJson",
            Self::RepeatedChunk { .. } => "RepeatedChunk",
            Self::BinaryChunkOutOfPlace { .. } => "BinaryChunkOutOfPlace",
        }
    }
}

impl fmt::Display for GlbError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShortForHeader { needs, len } => write!(
                out,
                "a binary glTF needs {needs} bytes of header and this is {len}"
            ),
            Self::NotThisFormat { expected } => {
                write!(out, "these bytes do not open with `{expected}`")
            }
            Self::UnknownVersion { found } => write!(
                out,
                "this is container version {found} and this build reads version {VERSION}"
            ),
            Self::SizeMismatch { declared, actual } => write!(
                out,
                "the header declares {declared} bytes and {actual} are present"
            ),
            Self::ChunkHeaderTruncated { chunk, available } => write!(
                out,
                "chunk {chunk} needs {CHUNK_HEADER} bytes of header and {available} remain"
            ),
            Self::ChunkOverruns {
                chunk,
                declared,
                available,
            } => write!(
                out,
                "chunk {chunk} declares {declared} bytes and {available} remain"
            ),
            Self::ChunkNotAligned { chunk, length } => write!(
                out,
                "chunk {chunk} is {length} bytes, which is not a multiple of {ALIGN}"
            ),
            Self::NoChunks => write!(out, "this container holds no chunks at all"),
            Self::FirstChunkNotJson { found } => write!(
                out,
                "the first chunk is type {found:#010x} and a binary glTF opens with JSON \
                 ({CHUNK_JSON:#010x})"
            ),
            Self::RepeatedChunk { kind, chunk } => {
                write!(out, "chunk {chunk} is a second {kind} chunk")
            }
            Self::BinaryChunkOutOfPlace { chunk } => write!(
                out,
                "the binary chunk is chunk {chunk} and it belongs second"
            ),
        }
    }
}

impl core::error::Error for GlbError {}

/// A validated container, borrowing the bytes it was read from.
///
/// Nothing here is copied. A malformed container costs no allocation,
/// and a caller that has already bounded its read has bounded this too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Container<'a> {
    /// The JSON chunk's bytes, **including its padding**.
    ///
    /// The padding is spaces, by specification, so a JSON reader that
    /// tolerates trailing whitespace — which every JSON reader must —
    /// needs nothing done to these bytes. Trimming them here would be
    /// this layer making a claim about content it has deliberately not
    /// read.
    pub json: &'a [u8],

    /// The binary chunk's bytes, if the container carries one.
    ///
    /// **`Some(&[])` and `None` are different answers.** The
    /// specification says a container *should* omit the chunk when the
    /// buffer is empty — should, not must — so a conforming writer may
    /// emit an empty one, and a reader that folded the two together
    /// would be reporting a chunk that exists as a chunk that does not.
    pub binary: Option<&'a [u8]>,
}

/// Whether these bytes open as a binary glTF.
///
/// A whole four-byte magic at offset zero, so unlike a text format's
/// keyword there is no prefix question to get wrong here.
#[must_use]
pub fn looks_like(bytes: &[u8]) -> bool {
    bytes.get(..MAGIC.len()) == Some(&MAGIC[..])
}

/// Read a little-endian `u32` at `offset`, or `None` past the end.
///
/// Total by construction: the addition is checked and the slice is taken
/// with `get`, so no caller has to have bounded the offset first.
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    let array: [u8; 4] = slice.try_into().ok()?;
    Some(u32::from_le_bytes(array))
}

/// Validate `bytes` as a binary glTF container and borrow its chunks.
///
/// # Errors
///
/// A [`GlbError`] naming exactly what was wrong, with the numbers. Every
/// length in the file is checked against the bytes actually present
/// before it is used to slice anything.
pub fn read(bytes: &[u8]) -> Result<Container<'_>, GlbError> {
    if bytes.len() < HEADER {
        return Err(GlbError::TooShortForHeader {
            needs: HEADER,
            len: bytes.len(),
        });
    }
    if !looks_like(bytes) {
        return Err(GlbError::NotThisFormat { expected: "glTF" });
    }

    // Total reads: the length check above has already made all three
    // fields present, and `u32_at` would answer `None` rather than panic
    // if that reasoning were ever wrong.
    let version = u32_at(bytes, OFF_VERSION).unwrap_or_default();
    if version != VERSION {
        return Err(GlbError::UnknownVersion { found: version });
    }

    // **Equality against the file, before any chunk is looked at.** This
    // is what bounds everything below: once the header's total is the
    // file's own length, no chunk length can imply a read the file
    // cannot satisfy without this reader noticing.
    let declared = u64::from(u32_at(bytes, OFF_LENGTH).unwrap_or_default());
    if declared != bytes.len() as u64 {
        return Err(GlbError::SizeMismatch {
            declared,
            actual: bytes.len(),
        });
    }

    let mut json: Option<&[u8]> = None;
    let mut binary: Option<&[u8]> = None;
    let mut at = HEADER;
    let mut index: u32 = 0;

    while at < bytes.len() {
        let available = bytes.len().saturating_sub(at);
        if available < CHUNK_HEADER {
            return Err(GlbError::ChunkHeaderTruncated {
                chunk: index,
                available,
            });
        }
        let length = u32_at(bytes, at).unwrap_or_default();
        let kind = u32_at(bytes, at.saturating_add(4)).unwrap_or_default();

        if !length.is_multiple_of(ALIGN) {
            return Err(GlbError::ChunkNotAligned {
                chunk: index,
                length,
            });
        }

        let start = at.saturating_add(CHUNK_HEADER);
        let rest = bytes.len().saturating_sub(start);
        if u64::from(length) > rest as u64 {
            return Err(GlbError::ChunkOverruns {
                chunk: index,
                declared: u64::from(length),
                available: rest,
            });
        }
        // In range: `length` was just compared against what remains.
        let end = start.saturating_add(length as usize);
        let data = bytes.get(start..end).unwrap_or_default();

        // **Asked before the kinds are sorted out, so that a first chunk
        // of any wrong kind gets the same answer.** Routing it through
        // the arms below instead would tell a file whose first chunk is
        // binary that its binary chunk is out of place — true, and not
        // the useful half: what is wrong is that the JSON chunk, which
        // the container requires first so that the rest can be read
        // progressively, is not there.
        if index == 0 && kind != CHUNK_JSON {
            return Err(GlbError::FirstChunkNotJson { found: kind });
        }

        match kind {
            CHUNK_JSON => {
                // Chunk zero is JSON or the file was already refused, so
                // a JSON chunk anywhere else is a second one.
                if index != 0 {
                    return Err(GlbError::RepeatedChunk {
                        kind: "JSON",
                        chunk: index,
                    });
                }
                json = Some(data);
            }
            CHUNK_BIN => {
                if binary.is_some() {
                    return Err(GlbError::RepeatedChunk {
                        kind: "BIN",
                        chunk: index,
                    });
                }
                if index != 1 {
                    return Err(GlbError::BinaryChunkOutOfPlace { chunk: index });
                }
                binary = Some(data);
            }
            // **Skipped, and this is the specification's requirement
            // rather than this reader's leniency.** Stepping over it is
            // safe because its length was validated above; nothing in it
            // is read.
            _ => {}
        }

        at = end;
        index = index.saturating_add(1);
    }

    let json = json.ok_or(GlbError::NoChunks)?;
    Ok(Container { json, binary })
}

#[cfg(test)]
mod tests {
    use super::{
        ALIGN, CHUNK_BIN, CHUNK_HEADER, CHUNK_JSON, GlbError, HEADER, MAGIC, OFF_LENGTH,
        OFF_VERSION, VERSION, u32_at,
    };

    /// The field reader is total, and nothing above it has to have
    /// bounded the offset first.
    ///
    /// **Exercised here because `read` cannot reach these answers.**
    /// Every call site there sits behind a length check, so the `None`
    /// arms are unreachable from the outside — which is the property
    /// worth having and also a pair of paths nothing would execute. A
    /// helper whose failure arms are never run is a helper nobody has
    /// checked is total.
    #[test]
    fn the_field_reader_answers_rather_than_panicking_past_the_end() {
        assert_eq!(u32_at(&[1, 0, 0, 0], 0), Some(1));
        assert_eq!(u32_at(&[1, 0, 0], 0), None, "three bytes are not four");
        assert_eq!(u32_at(&[], 0), None);
        assert_eq!(u32_at(&[1, 0, 0, 0], 1), None, "and the offset counts");
        assert_eq!(
            u32_at(&[1, 0, 0, 0], usize::MAX),
            None,
            "an offset that would overflow the addition is an answer, not a panic"
        );
    }

    /// The offsets are a running sum of the field widths above them.
    #[test]
    fn the_header_offsets_are_where_the_fields_are() {
        assert_eq!(OFF_VERSION, MAGIC.len());
        assert_eq!(OFF_LENGTH, OFF_VERSION + 4);
        assert_eq!(HEADER, OFF_LENGTH + 4);
        assert_eq!(CHUNK_HEADER, 8);
        assert_eq!(VERSION, 2);
        assert_eq!(ALIGN, 4);
    }

    /// The chunk types are the ASCII the specification names, in the
    /// byte order a little-endian file stores them.
    #[test]
    fn the_chunk_types_spell_what_they_are() {
        assert_eq!(CHUNK_JSON.to_le_bytes(), *b"JSON");
        assert_eq!(CHUNK_BIN.to_le_bytes(), *b"BIN\0");
        assert_eq!(MAGIC, *b"glTF");
    }

    /// Every refusal says something, and says the numbers it carries.
    ///
    /// **Matched exhaustively with no wildcard**, and the list below is
    /// pinned by a count — the match is checked by the compiler and the
    /// list is not, which is a gap this repository has already fallen
    /// into once on an enum shaped exactly like this one.
    #[test]
    fn every_refusal_names_its_numbers() {
        let all = [
            GlbError::TooShortForHeader { needs: 12, len: 3 },
            GlbError::NotThisFormat { expected: "glTF" },
            GlbError::UnknownVersion { found: 7 },
            GlbError::SizeMismatch {
                declared: 40,
                actual: 36,
            },
            GlbError::ChunkHeaderTruncated {
                chunk: 1,
                available: 5,
            },
            GlbError::ChunkOverruns {
                chunk: 0,
                declared: 900,
                available: 16,
            },
            GlbError::ChunkNotAligned {
                chunk: 0,
                length: 13,
            },
            GlbError::NoChunks,
            GlbError::FirstChunkNotJson { found: CHUNK_BIN },
            GlbError::RepeatedChunk {
                kind: "JSON",
                chunk: 1,
            },
            GlbError::BinaryChunkOutOfPlace { chunk: 2 },
        ];

        // One per variant. Raise it when the enum grows, in the same
        // change that writes the new arm below.
        assert_eq!(all.len(), 11, "a variant is missing an instance here");

        for refusal in all {
            let shown = refusal.to_string();
            assert!(!shown.is_empty(), "{refusal:?} says nothing");
            let numbers: Vec<&str> = match refusal {
                GlbError::TooShortForHeader { .. } => vec!["12", "3"],
                GlbError::NotThisFormat { .. } => vec!["glTF"],
                GlbError::UnknownVersion { .. } => vec!["7", "2"],
                GlbError::SizeMismatch { .. } => vec!["40", "36"],
                GlbError::ChunkHeaderTruncated { .. } => vec!["1", "8", "5"],
                GlbError::ChunkOverruns { .. } => vec!["0", "900", "16"],
                GlbError::ChunkNotAligned { .. } => vec!["0", "13", "4"],
                GlbError::NoChunks => vec!["no chunks"],
                GlbError::FirstChunkNotJson { .. } => vec!["0x004e4942", "0x4e4f534a"],
                GlbError::RepeatedChunk { .. } => vec!["JSON", "1"],
                GlbError::BinaryChunkOutOfPlace { .. } => vec!["2"],
            };
            for number in numbers {
                assert!(
                    shown.contains(number),
                    "`{shown}` does not carry `{number}`"
                );
            }
        }
    }

    /// The names are distinct, because a caller keys on them.
    #[test]
    fn no_two_refusals_share_a_name() {
        let names = [
            GlbError::TooShortForHeader { needs: 0, len: 0 }.name(),
            GlbError::NotThisFormat { expected: "" }.name(),
            GlbError::UnknownVersion { found: 0 }.name(),
            GlbError::SizeMismatch {
                declared: 0,
                actual: 0,
            }
            .name(),
            GlbError::ChunkHeaderTruncated {
                chunk: 0,
                available: 0,
            }
            .name(),
            GlbError::ChunkOverruns {
                chunk: 0,
                declared: 0,
                available: 0,
            }
            .name(),
            GlbError::ChunkNotAligned {
                chunk: 0,
                length: 0,
            }
            .name(),
            GlbError::NoChunks.name(),
            GlbError::FirstChunkNotJson { found: 0 }.name(),
            GlbError::RepeatedChunk { kind: "", chunk: 0 }.name(),
            GlbError::BinaryChunkOutOfPlace { chunk: 0 }.name(),
        ];
        let mut seen: Vec<&str> = Vec::new();
        for name in names {
            assert!(!seen.contains(&name), "`{name}` is used twice");
            seen.push(name);
        }
        assert_eq!(seen.len(), 11);
    }
}
