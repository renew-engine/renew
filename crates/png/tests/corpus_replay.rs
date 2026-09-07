//! The recorded fuzz corpus, replayed on the stable toolchain — the
//! decoder's twin of the pack reader's gate, same claim: every committed
//! input answers, `Ok` or a named refusal, in a merge-gating run.
//!
//! The fuzz workspace needs nightly and runs on its own schedule. This
//! test is what puts the corpus in front of every merge: if a change
//! makes a recorded input panic, hang past the harness's patience, or
//! read past its end, it fails here rather than on a nightly job nobody
//! is watching.

// The tripwire ban on filesystem access protects engine code; replaying
// committed corpus artifacts is this test's whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes, never a path,
// but this harness's whole subject is a directory of committed files.
#![allow(clippy::disallowed_types)]

use std::collections::BTreeSet;
use std::path::PathBuf;

/// The committed corpus never shrinks below this without someone
/// noticing. Fourteen seeds are committed; the floor sits below that so
/// the fuzzer's own minimisation can prune a duplicate without failing
/// the build, and far enough above zero that a lost directory is caught.
const LOW_WATER: usize = 10;

/// How many distinct outcomes the seeds must still reach between them.
///
/// **This is the assertion that stops the corpus rotting into
/// uselessness.** A corpus can keep its file count while every input
/// decays to the same early refusal — one edit to the signature check
/// would send all fourteen to `NotAPng` and a count-only gate would stay
/// green. The seeds were written to land on ten different answers, and
/// the floor is set below that so ordinary churn does not trip it.
const DISTINCT_OUTCOMES: usize = 6;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/png_decode")
}

/// The variant name alone, without the fields — two truncations at
/// different offsets are the same answer for this test's purpose.
fn outcome_name(bytes: &[u8]) -> String {
    match renew_png::decode::decode(bytes) {
        Ok(_) => "Ok".to_string(),
        Err(error) => {
            let rendered = format!("{error:?}");
            rendered
                .split_once([' ', '{', '('])
                .map_or(rendered.clone(), |(head, _)| head.to_string())
        }
    }
}

#[test]
fn every_recorded_corpus_input_gets_an_answer() {
    let dir = corpus_dir();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("the committed corpus at {dir:?} must exist: {error}"));

    let mut replayed = 0usize;
    let mut outcomes: BTreeSet<String> = BTreeSet::new();
    for entry in entries {
        let entry = entry.expect("corpus entries are readable");
        let bytes = std::fs::read(entry.path()).expect("corpus files are readable");
        outcomes.insert(outcome_name(&bytes));
        replayed += 1;
    }

    assert!(
        replayed >= LOW_WATER,
        "{replayed} corpus files replayed, below the committed floor of {LOW_WATER} — \
         the corpus has been gutted or the checkout is broken"
    );
    assert!(
        outcomes.len() >= DISTINCT_OUTCOMES,
        "the corpus reached only {} distinct outcomes ({outcomes:?}), below the floor of \
         {DISTINCT_OUTCOMES} — the seeds have collapsed onto one early refusal and are no \
         longer seeding anything",
        outcomes.len()
    );
}

/// A valid image in the corpus still decodes to the size it was written
/// at.
///
/// The replay above deliberately does not care what an input answers, so
/// on its own it would stay green if `decode` refused every file in the
/// directory. This is the other half: at least one committed input must
/// still come back as a picture, with the extent it was authored with.
#[test]
fn the_valid_seeds_still_decode_to_their_own_extents() {
    let dir = corpus_dir();
    let expected: [(&str, u32, u32); 3] = [
        ("valid-1x1.png", 1, 1),
        ("valid-4x4.png", 4, 4),
        ("valid-16x9.png", 16, 9),
    ];

    for (name, width, height) in expected {
        let path = dir.join(name);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("{name} must be in the corpus: {error}"));
        let image = renew_png::decode::decode(&bytes)
            .unwrap_or_else(|error| panic!("{name} must still decode: {error:?}"));
        assert_eq!(
            (image.width, image.height),
            (width, height),
            "{name} decoded to a different extent than it was written at"
        );
        assert_eq!(
            image.pixels.len(),
            (width as usize) * (height as usize) * 4,
            "{name} decoded to a pixel buffer that disagrees with its own extent"
        );
    }
}
