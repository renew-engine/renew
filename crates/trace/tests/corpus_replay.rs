//! The recorded fuzz corpus, replayed on the stable toolchain: every
//! committed input the coverage-guided search has found, fed to the
//! parser in a merge-gating run — so "zero known crashes over the
//! recorded corpus" is a claim this workspace makes, not a scheduled
//! job's mood.
//!
//! The assertion is the harness's own and nothing more: the parser
//! answers, Ok or a named refusal, for every byte string — content
//! correctness belongs to the round-trip suites beside the parser.

// The tripwire ban on filesystem access protects engine code; replaying
// committed corpus artifacts is this test's whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the codec takes text, never a path,
// but this harness's whole subject is a directory of committed files.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;

/// The committed corpus never shrinks below this without someone
/// noticing: a gate over a missing or gutted directory would be the
/// vacuous pass this tree keeps killing.
const LOW_WATER: usize = 100;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/trace_parse")
}

#[test]
fn every_recorded_corpus_input_gets_an_answer() {
    let dir = corpus_dir();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("the committed corpus at {dir:?} must exist: {error}"));

    let mut fed = 0usize;
    let mut skipped = 0usize;
    for entry in entries {
        let entry = entry.expect("corpus entries are readable");
        let bytes = std::fs::read(entry.path()).expect("corpus files are readable");
        // The harness's exact gate: the contract starts at &str, so
        // non-UTF-8 bytes exercise nothing this parser owns. Counted
        // apart so the floor guards inputs the parser actually saw.
        if let Ok(text) = core::str::from_utf8(&bytes) {
            let _ = renew_trace::parse(text);
            fed += 1;
        } else {
            skipped += 1;
        }
    }
    let replayed = fed;
    assert!(
        replayed >= LOW_WATER,
        "{replayed} corpus files replayed, below the committed floor of {LOW_WATER} — \
         the corpus has been gutted or the checkout is broken"
    );
    eprintln!(
        "{fed} recorded corpus inputs fed to the parser without a crash ({skipped} non-UTF-8 skipped)"
    );
}
