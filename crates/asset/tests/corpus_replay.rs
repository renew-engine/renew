//! The recorded fuzz corpus, replayed on the stable toolchain — the
//! pack reader's twin of the trace codec's gate, same claim: every
//! committed input answers, Ok or a named refusal, in a merge-gating
//! run.

// The tripwire ban on filesystem access protects engine code; replaying
// committed corpus artifacts is this test's whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes and names, never
// a path, but this harness's whole subject is a directory of committed
// files.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;

/// The committed corpus never shrinks below this without someone
/// noticing.
const LOW_WATER: usize = 10;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/asset_pack")
}

#[test]
fn every_recorded_corpus_input_gets_an_answer() {
    let dir = corpus_dir();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("the committed corpus at {dir:?} must exist: {error}"));

    let mut replayed = 0usize;
    for entry in entries {
        let entry = entry.expect("corpus entries are readable");
        let bytes = std::fs::read(entry.path()).expect("corpus files are readable");
        if let Ok(pack) = renew_asset::Pack::read(&bytes) {
            // The read surface's second half, cheap to include: the
            // integrity walk must also answer for whatever parsed.
            let _ = pack.mismatched();
        }
        replayed += 1;
    }
    assert!(
        replayed >= LOW_WATER,
        "{replayed} corpus files replayed, below the committed floor of {LOW_WATER} — \
         the corpus has been gutted or the checkout is broken"
    );
    eprintln!("{replayed} recorded corpus inputs replayed without a crash");
}
