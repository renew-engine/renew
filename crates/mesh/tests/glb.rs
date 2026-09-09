//! The binary glTF container, and every way a file can fail to be one.
//!
//! **Every fixture here is built by the code below**, byte by byte, from
//! a chunk list. Nothing was exported from a tool and nothing was
//! downloaded: a container is a wrapper around somebody's model, and a
//! model has an author.
//!
//! Two of these tests exist because the specification says the opposite
//! of what this crate's other readers do, and both are marked where they
//! sit: an unknown chunk type is **skipped** rather than refused, and an
//! empty binary chunk is **legal** rather than required to be omitted.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules — do
// not reach it. A fixture this file built and then could not read back
// is a broken test rather than a condition to recover from.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::format::{self, Format};
use renew_mesh::{GlbError, glb};

/// `JSON` and `BIN\0`, little-endian, as the file stores them.
const JSON: u32 = 0x4E4F_534A;
const BIN: u32 = 0x004E_4942;

/// A chunk type no version of the specification defines.
///
/// Spelled `XTRA` so that a failure message shows what it was meant to
/// stand for rather than a bare number.
const UNKNOWN: u32 = u32::from_le_bytes(*b"XTRA");

/// A glTF document small enough to read and padded to alignment.
///
/// 27 bytes of JSON and one trailing space, which is the padding byte
/// the specification names for this chunk.
const DOCUMENT: &[u8] = b"{\"asset\":{\"version\":\"2.0\"}} ";

/// Build a container around a chunk list, with a truthful total length.
///
/// The chunk payloads are written exactly as given, **padding
/// included**, so a test that wants an unaligned chunk simply passes one
/// and this helper does not quietly correct it.
fn container(chunks: &[(u32, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for (kind, data) in chunks {
        let length = u32::try_from(data.len()).expect("a fixture is small");
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(data);
    }
    let total = u32::try_from(out.len()).expect("a fixture is small");
    out[8..12].copy_from_slice(&total.to_le_bytes());
    out
}

/// The smallest container this reader accepts.
fn minimal() -> Vec<u8> {
    container(&[(JSON, DOCUMENT)])
}

/// The refusal a byte string gets, or a panic naming what it read
/// instead.
fn refusal(bytes: &[u8]) -> GlbError {
    match glb::read(bytes) {
        Ok(container) => panic!("these bytes read as a container: {container:?}"),
        Err(refused) => refused,
    }
}

/// A container of one JSON chunk reads, and the bytes come back
/// untouched.
#[test]
fn a_json_chunk_alone_is_a_container() {
    let bytes = minimal();
    let read = glb::read(&bytes).expect("a minimal container reads");
    assert_eq!(read.json, DOCUMENT, "the chunk's bytes, padding and all");
    assert_eq!(read.binary, None, "no binary chunk was written");
}

/// **The padding is returned rather than trimmed.**
///
/// The specification pads this chunk with spaces, and trimming them here
/// would be the framing layer making a claim about content it has
/// deliberately not read. A JSON reader tolerates trailing whitespace,
/// which is what the padding is.
#[test]
fn the_json_chunk_keeps_its_padding() {
    let bytes = minimal();
    let read = glb::read(&bytes).expect("a minimal container reads");
    assert!(read.json.ends_with(b" "), "the pad byte is still there");
    assert_eq!(read.json.len() % 4, 0, "and it is what makes it aligned");
}

/// A binary chunk comes back beside the JSON one.
#[test]
fn a_binary_chunk_is_handed_back_beside_the_json() {
    let payload: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8];
    let bytes = container(&[(JSON, DOCUMENT), (BIN, payload)]);
    let read = glb::read(&bytes).expect("two chunks read");
    assert_eq!(read.json, DOCUMENT);
    assert_eq!(read.binary, Some(payload));
}

/// **An empty binary chunk is legal, and `Some(&[])` is not `None`.**
///
/// The specification says a container *should* omit this chunk when the
/// buffer is empty — should, not must — so a conforming writer may emit
/// an empty one. A reader that folded the two answers together would
/// report a chunk that exists as a chunk that does not, and the JSON
/// beside it can legally point at a zero-length view of it.
///
/// Probed by returning `None` for a zero-length chunk: red here, green
/// everywhere else in this file.
#[test]
fn an_empty_binary_chunk_is_present_and_not_absent() {
    let bytes = container(&[(JSON, DOCUMENT), (BIN, b"")]);
    let read = glb::read(&bytes).expect("an empty binary chunk is legal");
    assert_eq!(
        read.binary,
        Some(&[][..]),
        "the chunk is there and it is empty"
    );
    assert_ne!(read.binary, None, "which is a different answer from absent");
}

/// **A chunk type this reader does not know is skipped, not refused.**
///
/// This is the one place the container specification contradicts what
/// every other reader in this crate does. The blob refuses a presence
/// bit outside its vocabulary; here, *"Client implementations MUST
/// ignore chunks with unknown types to enable glTF extensions to
/// reference additional chunks with new types following the first two
/// chunks."* Refusing one rejects conformant files.
///
/// Skipping is only safe because the chunk's length is validated first,
/// which the next tests pin.
#[test]
fn an_unknown_chunk_type_is_skipped() {
    let bytes = container(&[(JSON, DOCUMENT), (BIN, b"\0\0\0\0"), (UNKNOWN, b"anything")]);
    let read = glb::read(&bytes).expect("an unknown chunk is not a refusal");
    assert_eq!(read.json, DOCUMENT);
    assert_eq!(read.binary, Some(&b"\0\0\0\0"[..]));
}

/// An unknown chunk between the JSON and the binary one is stepped over,
/// and the binary chunk after it is out of place.
///
/// Both halves matter: the skip must not swallow the ordering rule.
#[test]
fn a_binary_chunk_after_an_unknown_one_is_out_of_place() {
    let bytes = container(&[(JSON, DOCUMENT), (UNKNOWN, b"pad!"), (BIN, b"\0\0\0\0")]);
    assert_eq!(
        refusal(&bytes),
        GlbError::BinaryChunkOutOfPlace { chunk: 2 }
    );
}

/// Fewer bytes than the preamble, before anything else is asked.
#[test]
fn bytes_shorter_than_the_header_are_refused_first() {
    assert_eq!(
        refusal(b"glT"),
        GlbError::TooShortForHeader { needs: 12, len: 3 }
    );
    // Even bytes that could never be a container get this answer, because
    // the length is what is checked first.
    assert_eq!(
        refusal(b""),
        GlbError::TooShortForHeader { needs: 12, len: 0 }
    );
}

/// A file of another format is refused as that.
#[test]
fn bytes_that_are_not_a_container_are_refused_by_name() {
    let mut bytes = minimal();
    bytes[0] = b'X';
    assert_eq!(
        refusal(&bytes),
        GlbError::NotThisFormat { expected: "glTF" }
    );

    // A whole file of another kind, long enough to have a header.
    let elsewhere = b"solid a-perfectly-good-stl\nfacet normal 0 0 1\n";
    assert_eq!(
        refusal(elsewhere),
        GlbError::NotThisFormat { expected: "glTF" }
    );
}

/// A container version this build does not read.
#[test]
fn a_version_this_build_does_not_read_is_refused() {
    let mut bytes = minimal();
    bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(refusal(&bytes), GlbError::UnknownVersion { found: 1 });

    bytes[4..8].copy_from_slice(&3u32.to_le_bytes());
    assert_eq!(refusal(&bytes), GlbError::UnknownVersion { found: 3 });
}

/// **The declared length must be the length present, in both
/// directions.**
///
/// A truncated transfer and a file with something appended are equally
/// refused: accepting trailing bytes accepts a second payload hidden
/// after the first.
#[test]
fn a_declared_length_that_is_not_the_length_present_is_refused() {
    let whole = minimal();

    let short = &whole[..whole.len() - 4];
    assert_eq!(
        refusal(short),
        GlbError::SizeMismatch {
            declared: whole.len() as u64,
            actual: whole.len() - 4,
        }
    );

    let mut appended = whole.clone();
    appended.extend_from_slice(b"more");
    assert_eq!(
        refusal(&appended),
        GlbError::SizeMismatch {
            declared: whole.len() as u64,
            actual: whole.len() + 4,
        }
    );
}

/// A container whose header is honest and whose body is not there.
#[test]
fn a_container_with_no_chunks_is_refused() {
    let mut bytes = b"glTF".to_vec();
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&12u32.to_le_bytes());
    assert_eq!(refusal(&bytes), GlbError::NoChunks);
}

/// Bytes left over that cannot hold a chunk's own preamble.
#[test]
fn a_partial_chunk_header_is_refused() {
    let mut bytes = minimal();
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    let total = u32::try_from(bytes.len()).expect("a fixture is small");
    bytes[8..12].copy_from_slice(&total.to_le_bytes());
    assert_eq!(
        refusal(&bytes),
        GlbError::ChunkHeaderTruncated {
            chunk: 1,
            available: 4,
        }
    );
}

/// A chunk claiming more bytes than remain after it.
#[test]
fn a_chunk_that_runs_past_the_end_is_refused() {
    let mut bytes = minimal();
    // The chunk header sits right after the twelve-byte preamble.
    bytes[12..16].copy_from_slice(&900u32.to_le_bytes());
    assert_eq!(
        refusal(&bytes),
        GlbError::ChunkOverruns {
            chunk: 0,
            declared: 900,
            available: DOCUMENT.len(),
        }
    );
}

/// A chunk length that would leave the next chunk unaligned.
///
/// The padding is inside the length, so this is a writer that forgot to
/// pad rather than a reader that should.
#[test]
fn a_chunk_length_that_is_not_a_multiple_of_four_is_refused() {
    let unpadded = &DOCUMENT[..DOCUMENT.len() - 1];
    let bytes = container(&[(JSON, unpadded)]);
    assert_eq!(
        refusal(&bytes),
        GlbError::ChunkNotAligned {
            chunk: 0,
            length: u32::try_from(unpadded.len()).expect("a fixture is small"),
        }
    );
}

/// **A first chunk of any wrong kind gets one answer, and it names the
/// missing JSON rather than the chunk that was there.**
///
/// A binary chunk first is out of place *and* the JSON chunk is missing;
/// the second is the useful half, because the container requires JSON
/// first so the rest can be read progressively.
#[test]
fn a_first_chunk_that_is_not_json_is_refused_for_that() {
    let binary_first = container(&[(BIN, b"\0\0\0\0"), (JSON, DOCUMENT)]);
    assert_eq!(
        refusal(&binary_first),
        GlbError::FirstChunkNotJson { found: BIN }
    );

    let unknown_first = container(&[(UNKNOWN, b"pad!"), (JSON, DOCUMENT)]);
    assert_eq!(
        refusal(&unknown_first),
        GlbError::FirstChunkNotJson { found: UNKNOWN }
    );
}

/// A kind the container permits once, appearing twice.
#[test]
fn a_repeated_chunk_is_refused() {
    let two_json = container(&[(JSON, DOCUMENT), (JSON, DOCUMENT)]);
    assert_eq!(
        refusal(&two_json),
        GlbError::RepeatedChunk {
            kind: "JSON",
            chunk: 1,
        }
    );

    let two_binary = container(&[(JSON, DOCUMENT), (BIN, b"\0\0\0\0"), (BIN, b"\0\0\0\0")]);
    assert_eq!(
        refusal(&two_binary),
        GlbError::RepeatedChunk {
            kind: "BIN",
            chunk: 2,
        }
    );
}

/// The magic is asked as a whole four bytes, in both directions.
#[test]
fn looks_like_answers_about_the_whole_magic() {
    assert!(glb::looks_like(&minimal()));
    assert!(glb::looks_like(b"glTF"), "four bytes are enough to say");
    assert!(!glb::looks_like(b"glT"), "three are not");
    assert!(!glb::looks_like(b"gltf"), "the case is part of it");
    assert!(!glb::looks_like(b""));
    assert!(
        !glb::looks_like(b"a glTF file"),
        "the magic opens the file, it does not merely appear in it"
    );
}

/// **A binary glTF is detected as one, and it used to be detected as an
/// STL.**
///
/// The fallback arm answers for everything unrecognised, so before this
/// format was known a `.glb` reached the STL reader and was refused with
/// a complaint about a truncated STL — a confident answer about the
/// wrong format.
#[test]
fn a_container_is_detected_as_one() {
    assert_eq!(format::detect(&minimal()), Format::Glb);
    assert_eq!(Format::Glb.name(), "glb");
    assert!(Format::Glb.carries_geometry());
}

/// **Detection finds the format and reading reads it, and a refusal
/// names the layer that made it.**
///
/// This test asserted the opposite until there was a reader: that the
/// geometry inside a container had none, which was honest then and false
/// now. The minimal container here carries a document with no scenes —
/// a library of entities rather than a model — so the refusal comes from
/// the *document*, and says so.
#[test]
fn a_detected_container_is_read_and_refuses_by_layer() {
    let bytes = minimal();
    let answer = Format::Glb
        .read(&bytes)
        .expect("this format carries geometry");
    let refused = answer.expect_err("this document places nothing");
    assert_eq!(
        refused.name(),
        "Gltf",
        "the outer name says a document-shaped format refused"
    );
    assert!(
        refused.to_string().contains("scenes"),
        "and the inner refusal says which member: `{refused}`"
    );
}

/// A container is copied, compared and printed, because a caller holds
/// one and this crate derives all three.
#[test]
fn a_container_can_be_copied_compared_and_printed() {
    let bytes = container(&[(JSON, DOCUMENT), (BIN, &[0, 0, 0, 0])]);
    let read = glb::read(&bytes).expect("two chunks read");
    let copy = read;
    assert_eq!(copy, read, "a container is compared by what it borrows");

    let shown = format!("{read:?}");
    assert!(shown.contains("json"), "the debug output names the chunks");
    assert!(shown.contains("binary"), "both of them: `{shown}`");
}

/// **Every refusal this reader can make is reachable, and every one it
/// cannot make says why.**
///
/// No wildcard arm, so a variant added later stops this file compiling
/// until somebody decides which it is.
fn glb_cannot_reach(refusal: &GlbError) -> Option<&'static str> {
    match refusal {
        // Reachable, and each is provoked by a byte string in this file.
        GlbError::TooShortForHeader { .. }
        | GlbError::NotThisFormat { .. }
        | GlbError::UnknownVersion { .. }
        | GlbError::SizeMismatch { .. }
        | GlbError::ChunkHeaderTruncated { .. }
        | GlbError::ChunkOverruns { .. }
        | GlbError::ChunkNotAligned { .. }
        | GlbError::NoChunks
        | GlbError::FirstChunkNotJson { .. }
        | GlbError::RepeatedChunk { .. }
        | GlbError::BinaryChunkOutOfPlace { .. } => None,
    }
}

/// The census and the byte strings agree, in both directions.
#[test]
fn the_census_and_the_bytes_agree() {
    let whole = minimal();

    let mut wrong_magic = whole.clone();
    wrong_magic[0] = b'X';
    let mut wrong_version = whole.clone();
    wrong_version[4..8].copy_from_slice(&9u32.to_le_bytes());
    let mut overrun = whole.clone();
    overrun[12..16].copy_from_slice(&900u32.to_le_bytes());
    let mut trailing = whole.clone();
    trailing.extend_from_slice(&[0, 0, 0, 0]);
    let total = u32::try_from(trailing.len()).expect("a fixture is small");
    trailing[8..12].copy_from_slice(&total.to_le_bytes());

    let mut headerless = b"glTF".to_vec();
    headerless.extend_from_slice(&2u32.to_le_bytes());
    headerless.extend_from_slice(&12u32.to_le_bytes());

    let provocations: [(&str, Vec<u8>); 11] = [
        ("TooShortForHeader", b"glT".to_vec()),
        ("NotThisFormat", wrong_magic),
        ("UnknownVersion", wrong_version),
        ("SizeMismatch", whole[..whole.len() - 4].to_vec()),
        ("ChunkHeaderTruncated", trailing),
        ("ChunkOverruns", overrun),
        (
            "ChunkNotAligned",
            container(&[(JSON, &DOCUMENT[..DOCUMENT.len() - 1])]),
        ),
        ("NoChunks", headerless),
        ("FirstChunkNotJson", container(&[(BIN, b"\0\0\0\0")])),
        (
            "RepeatedChunk",
            container(&[(JSON, DOCUMENT), (JSON, DOCUMENT)]),
        ),
        (
            "BinaryChunkOutOfPlace",
            container(&[(JSON, DOCUMENT), (UNKNOWN, b"pad!"), (BIN, b"\0\0\0\0")]),
        ),
    ];

    for (name, bytes) in &provocations {
        let got = refusal(bytes);
        assert!(
            glb_cannot_reach(&got).is_none(),
            "`{name}` is provoked by bytes here, and the census calls it unreachable"
        );
        assert_eq!(
            got.name(),
            *name,
            "the bytes meant to provoke `{name}` provoked `{got}`"
        );
    }

    // Every variant the census calls reachable is one of the eleven
    // above: the list is exhaustive by count, and the names are distinct
    // by the unit test beside the reader.
    assert_eq!(provocations.len(), 11, "one provocation per refusal");
}
