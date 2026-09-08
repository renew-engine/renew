//! Write the binary glTF container's seed corpus.
//!
//! **Every byte here is built by this program.** A container is a
//! wrapper around somebody's model, so a downloaded `.glb` would arrive
//! with a licence and an author; these carry a JSON chunk that is the
//! smallest legal glTF document and a binary chunk that is a counting
//! pattern.
//!
//! A fuzzer finds the four-byte magic quickly — fixed bytes at offset
//! zero are what a coverage-guided search is best at. What it does not
//! find on its own is a **coherent chain**: a header whose total length
//! is right, a first chunk whose length lands the second chunk's header
//! exactly on a boundary, and a second chunk that then fits. Every
//! interesting fault in this format is one number in that chain being
//! plausible and wrong, and a random walk almost never produces a chain
//! that survives far enough to be interesting. These seeds put a
//! mutation's starting point on both sides of every link.
//!
//! **Three seeds exist for rules that read backwards** from what the
//! other readers here do, and a corpus without them would let those
//! rules be deleted silently: an unknown chunk type must be *skipped*, a
//! zero-length binary chunk is *legal*, and the padding inside a chunk's
//! length is what makes the next chunk aligned.
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-mesh --example make_glb_corpus
//! ```
//!
//! Existing files are left alone. The fuzzer adds its own finds to this
//! directory over time, and this program must never delete them.

// The crate bans filesystem access because the library never touches a
// file -- a caller that reads one owns it, and owns the bound on reading
// it. This program is that caller: writing the corpus is its whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes, never a path.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;
use std::process::ExitCode;

/// `JSON` and `BIN\0`, little-endian, as the file stores them.
const JSON: u32 = 0x4E4F_534A;
const BIN: u32 = 0x004E_4942;

/// A chunk type no version of the specification defines.
const UNKNOWN: u32 = u32::from_le_bytes(*b"XTRA");

/// The smallest glTF document that is one, padded to alignment with the
/// space the specification names for this chunk.
const DOCUMENT: &[u8] = b"{\"asset\":{\"version\":\"2.0\"}} ";

/// A binary payload that is recognisable in a hex dump.
const PAYLOAD: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7];

/// Build a container around a chunk list, with a truthful total length.
///
/// Payloads are written exactly as given, padding included, so a seed
/// that wants an unaligned chunk passes one.
fn container(chunks: &[(u32, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for (kind, data) in chunks {
        let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(data);
    }
    let total = u32::try_from(out.len()).unwrap_or(u32::MAX);
    out[8..12].copy_from_slice(&total.to_le_bytes());
    out
}

/// Rewrite the header's total length, for the seeds whose whole point is
/// that it disagrees with the file.
fn with_declared_length(mut bytes: Vec<u8>, declared: u32) -> Vec<u8> {
    bytes[8..12].copy_from_slice(&declared.to_le_bytes());
    bytes
}

/// Containers that read, so the fuzzer has somewhere to mutate *from*.
fn readable_seeds() -> Vec<(String, Vec<u8>)> {
    vec![
        ("json-only".to_owned(), container(&[(JSON, DOCUMENT)])),
        (
            "json-and-binary".to_owned(),
            container(&[(JSON, DOCUMENT), (BIN, PAYLOAD)]),
        ),
        // **Legal, and the specification is explicit that it is.** The
        // binary chunk *should* be omitted when the buffer is empty --
        // should, not must -- so a reader that refused this would reject
        // conformant files.
        (
            "empty-binary-chunk".to_owned(),
            container(&[(JSON, DOCUMENT), (BIN, b"")]),
        ),
        // **Skipped rather than refused**, which is the one rule here
        // that contradicts what every other reader in this crate does.
        (
            "unknown-chunk-third".to_owned(),
            container(&[(JSON, DOCUMENT), (BIN, PAYLOAD), (UNKNOWN, b"anything")]),
        ),
        (
            "unknown-chunk-second".to_owned(),
            container(&[(JSON, DOCUMENT), (UNKNOWN, b"pad!")]),
        ),
        // A JSON chunk long enough that its own padding is more than one
        // byte, so a mutation of the length lands inside the padding
        // rather than inside the document.
        (
            "padded-json".to_owned(),
            container(&[(JSON, b"{\"asset\":{\"version\":\"2.0\"},\"scenes\":[]}   ")]),
        ),
    ]
}

/// One container per refusal, so a mutation of each starts from a file
/// that is wrong in exactly one way.
fn refused_seeds() -> Vec<(String, Vec<u8>)> {
    let whole = container(&[(JSON, DOCUMENT)]);
    let mut wrong_magic = whole.clone();
    wrong_magic[0] = b'X';
    let mut wrong_version = whole.clone();
    wrong_version[4..8].copy_from_slice(&9u32.to_le_bytes());
    let mut overrun = whole.clone();
    overrun[12..16].copy_from_slice(&0xFFFF_FFF0_u32.to_le_bytes());

    let mut partial_chunk_header = whole.clone();
    partial_chunk_header.extend_from_slice(&[0, 0, 0, 0]);
    let total = u32::try_from(partial_chunk_header.len()).unwrap_or(u32::MAX);
    partial_chunk_header = with_declared_length(partial_chunk_header, total);

    let mut header_only = b"glTF".to_vec();
    header_only.extend_from_slice(&2u32.to_le_bytes());
    header_only.extend_from_slice(&12u32.to_le_bytes());

    vec![
        ("short-header".to_owned(), b"glT".to_vec()),
        (
            "not-a-container".to_owned(),
            b"solid teapot\nfacet\n".to_vec(),
        ),
        ("wrong-magic".to_owned(), wrong_magic),
        ("wrong-version".to_owned(), wrong_version),
        (
            "declared-too-long".to_owned(),
            with_declared_length(whole.clone(), 4096),
        ),
        ("trailing-bytes".to_owned(), {
            let mut appended = whole.clone();
            appended.extend_from_slice(b"more");
            appended
        }),
        ("partial-chunk-header".to_owned(), partial_chunk_header),
        ("chunk-overruns".to_owned(), overrun),
        (
            "unaligned-chunk".to_owned(),
            container(&[(JSON, &DOCUMENT[..DOCUMENT.len() - 1])]),
        ),
        ("no-chunks".to_owned(), header_only),
        (
            "binary-chunk-first".to_owned(),
            container(&[(BIN, PAYLOAD), (JSON, DOCUMENT)]),
        ),
        (
            "two-json-chunks".to_owned(),
            container(&[(JSON, DOCUMENT), (JSON, DOCUMENT)]),
        ),
        (
            "two-binary-chunks".to_owned(),
            container(&[(JSON, DOCUMENT), (BIN, PAYLOAD), (BIN, PAYLOAD)]),
        ),
        (
            "binary-chunk-third".to_owned(),
            container(&[(JSON, DOCUMENT), (UNKNOWN, b"pad!"), (BIN, PAYLOAD)]),
        ),
    ]
}

fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/glb_read");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {error}", dir.display());
        return ExitCode::FAILURE;
    }

    let mut written = 0usize;
    let mut kept = 0usize;
    for (name, bytes) in readable_seeds().into_iter().chain(refused_seeds()) {
        let path = dir.join(format!("{name}.glb"));
        if path.exists() {
            kept += 1;
            continue;
        }
        // **A generator that swallows a write failure and then reports
        // success is worse than one that crashes**: the caller sees a
        // count and believes the corpus is whole.
        if let Err(error) = std::fs::write(&path, &bytes) {
            eprintln!("cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        written += 1;
    }

    println!(
        "{written} written, {kept} already present, in {}",
        dir.display()
    );
    ExitCode::SUCCESS
}
