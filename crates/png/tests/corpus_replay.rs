//! The recorded fuzz corpus, replayed on the stable toolchain — the
//! decoder's twin of the pack reader's gate, same claim: every committed
//! input answers, `Ok` or a named refusal, in a merge-gating run.
//!
//! The fuzz workspace needs nightly and runs on its own schedule. This
//! test is what puts the corpus in front of every merge: if a change
//! makes a recorded input panic or read past its end, it fails here
//! rather than on a nightly job nobody is watching.
//!
//! **It cannot catch a hang.** The harness has no per-test deadline and
//! no workflow here sets one, so an input that loops forever wedges the
//! job rather than failing it. Said plainly because the obvious reading
//! of "every input answers" is that non-answers are caught, and one kind
//! is not.

// The tripwire ban on filesystem access protects engine code; replaying
// committed corpus artifacts is this test's whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes, never a path,
// but this harness's whole subject is a directory of committed files.
#![allow(clippy::disallowed_types)]
// A missing or unreadable corpus is a broken checkout, not a condition
// this harness recovers from -- and these live in a helper rather than in
// the test bodies, where the lints would allow them, because reading the
// corpus by content is what stops the tests naming files the corpus
// procedure is required to rename.
#![allow(clippy::panic, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

/// The committed corpus never shrinks below this many **distinct** inputs.
///
/// Distinct by content, not by directory entry: counting entries lets a
/// corpus be padded back to strength with copies of one file, which is a
/// mutation this floor is supposed to refuse and would otherwise admit.
const LOW_WATER: usize = 16;

/// How many distinct outcomes the seeds must still reach between them.
///
/// **This is the assertion that stops the corpus rotting into
/// uselessness.** A corpus can keep its file count while every input
/// decays to the same early refusal, and a count-only gate would stay
/// green through it. The committed seeds reach twelve; the floor sits two
/// below so the fuzzer's own minimisation has room, and no lower —
/// slack here is exactly how many of the decoder's refusals may go
/// unseeded without anyone noticing.
const DISTINCT_OUTCOMES: usize = 10;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/png_decode")
}

/// Every committed input, as bytes.
///
/// Read by content and never by name. `cargo fuzz cmin` renames what it
/// keeps to a content hash, so a test that names a file forbids the
/// minimisation the corpus's own procedure requires.
fn corpus() -> Vec<Vec<u8>> {
    let dir = corpus_dir();
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|error| {
        panic!(
            "the committed corpus at {} must exist: {error}",
            dir.display()
        )
    });
    entries
        .map(|entry| {
            let entry = entry.expect("corpus entries are readable");
            std::fs::read(entry.path()).expect("corpus files are readable")
        })
        .collect()
}

/// The answer a file gets, as a name — the variant alone, without its
/// fields, so two truncations at different offsets count as one answer.
///
/// **Matched on the enum, not parsed out of `Debug`.** `DecodeError` is
/// `non_exhaustive`, so the wildcard is required and a new variant lands
/// in it silently; that is the cost of the enum being open, and it is
/// still cheaper than depending on a `Debug` rendering no document treats
/// as a contract.
fn outcome_name(bytes: &[u8]) -> &'static str {
    use renew_png::decode::DecodeError as E;
    match renew_png::decode::decode(bytes) {
        Ok(_) => "Ok",
        Err(E::NotAPng) => "NotAPng",
        Err(E::ChunkOverruns { .. }) => "ChunkOverruns",
        Err(E::BadChecksum { .. }) => "BadChecksum",
        Err(E::BadHeader) => "BadHeader",
        Err(E::ZeroExtent { .. }) => "ZeroExtent",
        Err(E::BadColourType { .. }) => "BadColourType",
        Err(E::UnsupportedDepth { .. }) => "UnsupportedDepth",
        Err(E::Interlaced) => "Interlaced",
        Err(E::BadMethod { .. }) => "BadMethod",
        Err(E::BadPalette { .. }) => "BadPalette",
        Err(E::NoImageData) => "NoImageData",
        Err(E::MissingEnd) => "MissingEnd",
        Err(E::BadZlibHeader { .. }) => "BadZlibHeader",
        Err(E::Deflate { .. }) => "Deflate",
        Err(E::BadImageLength { .. }) => "BadImageLength",
        Err(E::BadFilter { .. }) => "BadFilter",
        Err(E::TooLarge { .. }) => "TooLarge",
        Err(_) => "unnamed",
    }
}

/// Refusals a seed must still provoke, each guarding something a count
/// cannot.
///
/// **A distinct-count floor is not enough, and this was measured rather
/// than assumed.** Deleting the decoder's allocation ceiling does not
/// remove an answer from the corpus — the seed that reached `TooLarge`
/// simply falls through to `MissingEnd`, which other seeds already
/// produce. The total drops by one and clears any floor loose enough to
/// let the fuzzer minimise. So the guards that have exactly one seed each
/// are named here: losing one is a specific defence going unseeded, and
/// that is what this catches.
const REQUIRED_OUTCOMES: [&str; 4] = ["TooLarge", "BadChecksum", "BadZlibHeader", "MissingEnd"];

#[test]
fn every_recorded_corpus_input_gets_an_answer() {
    let corpus = corpus();

    let mut distinct_inputs: BTreeSet<&[u8]> = BTreeSet::new();
    let mut outcomes: BTreeSet<&str> = BTreeSet::new();
    for bytes in &corpus {
        distinct_inputs.insert(bytes.as_slice());
        outcomes.insert(outcome_name(bytes));
    }

    assert!(
        distinct_inputs.len() >= LOW_WATER,
        "{} distinct inputs of {} files, below the committed floor of {LOW_WATER} — the corpus \
         has been gutted, padded with duplicates, or the checkout is broken",
        distinct_inputs.len(),
        corpus.len()
    );
    for required in REQUIRED_OUTCOMES {
        assert!(
            outcomes.contains(required),
            "no committed input provokes {required} any more — the guard it seeds is unreachable \
             from this corpus, so a change that removed that guard would not be caught here. \
             Reached: {outcomes:?}"
        );
    }
    assert!(
        outcomes.len() >= DISTINCT_OUTCOMES,
        "the corpus reached only {} distinct outcomes ({outcomes:?}), below the floor of \
         {DISTINCT_OUTCOMES} — the seeds have collapsed onto fewer refusals and are no longer \
         seeding what they were written to seed",
        outcomes.len()
    );
}

/// Some committed input still decodes, and its pixels agree with its own
/// header.
///
/// The replay above deliberately does not care what an input answers, so
/// on its own it would stay green with a decoder that refused every file
/// in the directory. This is the other half.
///
/// **Selected by content, not by name.** An earlier form of this test
/// named three files and panicked if any was absent, which forbade the
/// `cargo fuzz cmin` minimisation the corpus's own procedure requires —
/// the two halves of this file contradicted each other.
#[test]
fn some_committed_input_still_decodes_to_its_own_extent() {
    let decoded: Vec<_> = corpus()
        .iter()
        .filter_map(|bytes| renew_png::decode::decode(bytes).ok())
        .collect();

    assert!(
        !decoded.is_empty(),
        "no committed input decodes at all — the corpus has lost every valid image, or the \
         decoder refuses everything"
    );
    for image in &decoded {
        assert_eq!(
            image.pixels.len(),
            (image.width as usize) * (image.height as usize) * 4,
            "a decoded image's pixel buffer disagrees with the extent it reported"
        );
        assert!(
            image.width > 0 && image.height > 0,
            "a decoded image reported a zero extent, which the decoder refuses by name"
        );
    }
}
