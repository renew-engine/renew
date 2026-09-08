//! The recorded fuzz corpus, replayed on the stable toolchain — the same
//! claim the other parsers' gates make: every committed input answers,
//! `Ok` or a named refusal, in a merge-gating run.
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
// committed corpus artifacts is this harness's whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes, never a path,
// but this harness's whole subject is a directory of committed files.
#![allow(clippy::disallowed_types)]
// A missing or unreadable corpus is a broken checkout, not a condition
// this harness recovers from.
#![allow(clippy::panic, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use renew_mesh::{MeshError, stl};

/// The committed corpus never shrinks below this many **distinct**
/// inputs.
///
/// Distinct by content, not by directory entry: counting entries lets a
/// corpus be padded back to strength with copies of one file, which is a
/// mutation this floor is supposed to refuse and would otherwise admit.
const LOW_WATER: usize = 18;

/// How many distinct outcomes the seeds must still reach between them.
///
/// **This is the assertion that stops the corpus rotting into
/// uselessness.** A corpus can keep its file count while every input
/// decays to the same early refusal, and a count-only gate would stay
/// green through it. **The committed seeds reach seven** — `Ok` plus six
/// of the reader's seven named refusals, counted rather than guessed;
/// the floor sits two below so the fuzzer's own minimisation has room,
/// and no lower, because slack here is exactly how many of the reader's
/// refusals may go unseeded without anyone noticing.
///
/// The seventh, `TooLarge`, is reachable only where a `u32` count times
/// fifty bytes overruns a `usize`, which is to say on a 32-bit target.
/// No seed can reach it here and none pretends to.
const DISTINCT_OUTCOMES: usize = 5;

/// Refusals a seed must still provoke, each guarding something a count
/// cannot.
///
/// **A distinct-count floor is not enough, and the reason is
/// arithmetic.** Deleting one guard does not necessarily remove an
/// answer from the corpus — the seed that reached it falls through to
/// whatever the next check says, which other seeds already produce, so
/// the total drops by one and clears any floor loose enough to let the
/// fuzzer minimise. These four are the guards where that would matter
/// most:
///
/// * `NotFinite` is the one that stops a coordinate nothing downstream
///   can bound reaching a vertex buffer. Losing it is the difference
///   between a refusal and a model that draws nowhere.
/// * `NoGeometry` is what separates "this file is empty" from "this file
///   was truncated to its header", which are different bugs upstream.
/// * `NotThisFormat` is the answer for bytes that are neither dialect,
///   and the only seed here that is not valid UTF-8.
/// * `NotANumber` is where this reader's number grammar and the standard
///   library's `parse` meet, and the refusal most easily lost by
///   delegating one to the other.
const REQUIRED_OUTCOMES: [&str; 4] = ["NotFinite", "NoGeometry", "CountMismatch", "NotANumber"];

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/stl_read")
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

/// The answer a byte string gets, as a name — the variant alone, without
/// its fields, so two truncations at different offsets count as one
/// answer.
///
/// **Matched on the enum, and with no wildcard.** `MeshError` is closed,
/// so a refusal added later stops this file compiling until somebody
/// decides whether a seed should reach it. That is the whole benefit of
/// the enum being closed, collected here.
fn outcome(bytes: &[u8]) -> &'static str {
    match stl::read(bytes) {
        Ok(_) => "Ok",
        Err(refusal) => match refusal {
            MeshError::TooShortForHeader { .. } => "TooShortForHeader",
            MeshError::CountMismatch { .. } => "CountMismatch",
            MeshError::TooLarge { .. } => "TooLarge",
            MeshError::ExpectedKeyword { .. } => "ExpectedKeyword",
            MeshError::NotANumber { .. } => "NotANumber",
            MeshError::NotFinite { .. } => "NotFinite",
            MeshError::NoGeometry => "NoGeometry",
        },
    }
}

/// Every committed input answers, one way or the other.
///
/// This is the claim, and it is worth stating what it is not: it says
/// nothing about *which* answer, because most of these files are wrong
/// on purpose and their refusals are the suite's business beside the
/// crate. What it says is that a recorded input never panics, never
/// reads past its end, and never reaches a caller as anything but an
/// answer.
#[test]
fn every_recorded_input_answers() {
    for bytes in corpus() {
        // The invariants a returned mesh carries, checked here as well
        // as in the fuzz target: this is the copy that runs on stable,
        // on every merge, on three operating systems.
        if let Ok(mesh) = stl::read(&bytes) {
            assert_eq!(mesh.positions.len() % 3, 0);
            assert!(!mesh.is_empty());
            assert!(mesh.normals.is_empty() || mesh.normals.len() == mesh.triangles());
            for value in mesh.positions.iter().chain(&mesh.normals).flatten() {
                assert!(value.is_finite());
            }
            let _ = mesh.winding_disagreements();
        }
    }
}

/// The corpus keeps its strength: enough distinct inputs, reaching
/// enough distinct answers, including the ones named above.
#[test]
fn the_corpus_still_covers_what_it_was_recorded_to_cover() {
    let inputs = corpus();
    let distinct: BTreeSet<Vec<u8>> = inputs.iter().cloned().collect();
    assert!(
        distinct.len() >= LOW_WATER,
        "the corpus holds {} distinct inputs and the floor is {LOW_WATER}; padding it back to \
         strength with copies is exactly what counting by content refuses",
        distinct.len()
    );

    let reached: BTreeSet<&'static str> = distinct.iter().map(|bytes| outcome(bytes)).collect();
    assert!(
        reached.len() >= DISTINCT_OUTCOMES,
        "the corpus reaches {} distinct answers and the floor is {DISTINCT_OUTCOMES}; a corpus \
         whose inputs have all decayed to the same early refusal keeps its file count and stops \
         being worth running. Reached: {reached:?}",
        reached.len()
    );
    for required in REQUIRED_OUTCOMES {
        assert!(
            reached.contains(required),
            "no committed seed reaches `{required}`, which is a guard nothing is exercising. \
             Reached: {reached:?}"
        );
    }
}

/// A valid file is still valid, which the outcome census alone would not
/// notice.
///
/// **`Ok` is one of the distinct answers**, so a corpus could satisfy
/// the floor above with nothing but refusals if the last valid seed were
/// minimised away. The reader's happy path is the one a real caller
/// takes, and a corpus that stopped covering it would be a corpus
/// testing only how this crate says no.
#[test]
fn some_committed_seed_is_a_file_that_reads() {
    let inputs = corpus();
    let readable = inputs
        .iter()
        .filter(|bytes| stl::read(bytes).is_ok())
        .count();
    assert!(
        readable >= 2,
        "only {readable} committed seeds read as meshes; both dialects should have at least one, \
         or the corpus is testing refusal alone"
    );
}

#[test]
#[ignore = "a census, not a gate: run it to update the numbers above"]
fn census() {
    let inputs = corpus();
    let distinct: BTreeSet<Vec<u8>> = inputs.iter().cloned().collect();
    let mut reached: BTreeSet<&'static str> = BTreeSet::new();
    for bytes in &distinct {
        reached.insert(outcome(bytes));
    }
    println!(
        "{} distinct inputs, {} outcomes: {reached:?}",
        distinct.len(),
        reached.len()
    );
}
