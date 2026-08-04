//! The recorded fuzz corpus, replayed on the stable toolchain — the WAV
//! reader's twin of the pack reader's and the trace codec's gates, same
//! claim: every committed input answers, Ok or a named refusal, in a
//! merge-gating run.

// The tripwire ban on filesystem access protects engine code; replaying
// committed corpus artifacts is this test's whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes and never a path —
// which is what makes the reader fuzzable at all — but this harness's
// whole subject is a directory of committed files.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;

/// The committed corpus never shrinks below this without someone
/// noticing: a gate over a missing or gutted directory would be the
/// vacuous pass this tree keeps killing.
///
/// Nine inputs commit, and the floor sits at the same fraction of them as
/// the pack corpus's floor does of its eighteen. That leaves minimization
/// room to subsume a near-duplicate without reddening a merge gate, while
/// still failing loudly the moment the directory is emptied.
const LOW_WATER: usize = 5;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/wav")
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
        if let Ok(wav) = renew_audio::wav::parse(&bytes) {
            // The read surface's second half, cheap to include: decoding
            // is lazy, so nothing walks the samples until someone asks,
            // and that walk must answer for whatever parsed. Drained and
            // discarded — the values belong to the suites beside the
            // reader, and asserting on them here would fail the corpus
            // rather than the code.
            let _ = wav.samples_f32().count();
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
