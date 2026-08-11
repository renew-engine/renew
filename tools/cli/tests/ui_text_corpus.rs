//! The grammar's recorded fuzz corpus, replayed on the stable
//! toolchain — the third such gate, same claim as its siblings: every
//! committed input answers, compiled or refused with a place, in a
//! merge-gating run. An accepted compile is also read back and
//! instantiated, holding the compiler's only-mint-what-read-accepts
//! promise on every push, not only on the fuzz lane.

use std::path::PathBuf;

/// The committed corpus never shrinks below this without someone
/// noticing — the same floor as the sibling gates.
const LOW_WATER: usize = 10;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/ui_text")
}

#[test]
fn every_recorded_corpus_input_gets_an_answer() {
    let dir = corpus_dir();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("the committed corpus at {dir:?} must exist: {error}"));

    let mut replayed = 0usize;
    let mut compiled_count = 0usize;
    for entry in entries {
        let entry = entry.expect("corpus entries are readable");
        let bytes = std::fs::read(entry.path()).expect("corpus files are readable");
        let Ok(text) = core::str::from_utf8(&bytes) else {
            // The fuzzer mutates bytes; non-UTF-8 inputs are the
            // target's early return, and the same non-answer here.
            replayed += 1;
            continue;
        };
        if let Ok(compiled) = renew_cli::ui_compile::compile(text) {
            let document = renew_ui::Document::read(&compiled.bytes)
                .expect("the compiler must only mint what the reader accepts");
            let _ = document.tree();
            compiled_count += 1;
        }
        replayed += 1;
    }
    assert!(
        replayed >= LOW_WATER,
        "{replayed} corpus files replayed, below the committed floor of {LOW_WATER} — \
         the corpus has been gutted or the checkout is broken"
    );
    assert!(
        compiled_count >= 1,
        "not one corpus file compiled — the grammar has drifted away from its own \
         recorded inputs, and the seeds must be re-minted with it"
    );
}
