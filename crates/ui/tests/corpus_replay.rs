//! The recorded fuzz corpus, replayed on the stable toolchain — the
//! document reader's twin of the pack and codec gates, same claim:
//! every committed input answers, Ok or a named refusal, in a
//! merge-gating run. And one claim more: what reads as a document
//! also instantiates, because validation promised instantiation
//! would never need to check again.

// The tripwire ban on filesystem access protects engine code; replaying
// committed corpus artifacts is this test's whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes and limits,
// never a path, but this harness's whole subject is a directory of
// committed files.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;

/// The committed corpus never shrinks below this without someone
/// noticing — the same floor as the pack reader's gate.
const LOW_WATER: usize = 10;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/ui_document")
}

#[test]
fn every_recorded_corpus_input_gets_an_answer() {
    let dir = corpus_dir();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("the committed corpus at {dir:?} must exist: {error}"));

    let mut replayed = 0usize;
    let mut documents = 0usize;
    for entry in entries {
        let entry = entry.expect("corpus entries are readable");
        let bytes = std::fs::read(entry.path()).expect("corpus files are readable");
        if let Ok(document) = renew_ui::Document::read(&bytes) {
            // The read proof's second half: a validated document
            // instantiates without re-checking, solves without
            // panicking on whatever styles the fuzzer minted, and —
            // because the accepted form is canonical — captures back
            // to the exact bytes it came from. The corpus holds all
            // three claims on every push, not only on the fuzz lane.
            let mut tree = document.tree();
            tree.solve(
                renew_ui::Fixed::from_int(320),
                renew_ui::Fixed::from_int(240),
            );
            assert_eq!(
                renew_ui::document::capture(&tree),
                bytes,
                "an accepted document must capture back to its own bytes"
            );
            documents += 1;
        }
        replayed += 1;
    }
    assert!(
        replayed >= LOW_WATER,
        "{replayed} corpus files replayed, below the committed floor of {LOW_WATER} — \
         the corpus has been gutted or the checkout is broken"
    );
    assert!(
        documents >= 1,
        "not one corpus file read as a document — the format has drifted away from \
         its own recorded inputs, and the seeds must be re-minted with it"
    );
}
