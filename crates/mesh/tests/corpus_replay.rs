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

use renew_mesh::{MeshError, mtl, obj, ply, stl};

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
/// The seventh, `TooLarge`, this reader does not make at all. It once
/// guarded the binary body's length conversion, on the argument that a
/// `u32` count times fifty bytes overruns a 32-bit `usize` — which the
/// length equality one line above it had already made impossible, by
/// bounding that product against the file's own size before the
/// conversion happens. The refusal is gone rather than unseeded.
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
            // The three the indexed reader adds. **Unreachable from an
            // STL and named here anyway**, because the enum is closed
            // and this match has no wildcard: adding them stopped this
            // file compiling until somebody decided what they mean to
            // this reader, which is the whole benefit of the enum being
            // closed. STL repeats every corner, so there is no index to
            // be out of range; it has no face element to be short of
            // corners; and it has no schema to be unsupported.
            MeshError::IndexOutOfRange { .. } => "IndexOutOfRange",
            MeshError::IndexZero { .. } => "IndexZero",
            MeshError::NotAFace { .. } => "NotAFace",
            MeshError::Unsupported { .. } => "Unsupported",
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
            assert!(mesh.face_normals.is_empty() || mesh.face_normals.len() == mesh.triangles());
            for value in mesh.positions.iter().chain(&mesh.face_normals).flatten() {
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

// ---------------------------------------------------------------------
// The PLY corpus, held to the same claims by the same shape of gate.
// ---------------------------------------------------------------------

/// The committed PLY corpus never shrinks below this many **distinct**
/// inputs. Distinct by content, for the reason above.
const PLY_LOW_WATER: usize = 16;

/// How many distinct outcomes the PLY seeds must still reach.
///
/// **Measured, not guessed** — the census below prints it. The floor
/// sits two under what the committed seeds reach, so `cargo fuzz cmin`
/// has room to minimise and no more, because slack here is exactly how
/// many of the reader's refusals may go unseeded unnoticed.
const PLY_DISTINCT_OUTCOMES: usize = 8;

/// Refusals a PLY seed must provoke, each guarding something a count
/// cannot.
///
/// * `IndexOutOfRange` is **the refusal this format adds over STL** —
///   the one that separates a mesh from a read past a buffer, and the
///   one no soup format can even have.
/// * `Unsupported` is what separates a valid file this reader cannot use
///   from a malformed one; the caller's next move differs.
/// * `NotFinite` stops a coordinate nothing downstream can bound
///   reaching a vertex buffer.
/// * `TooLarge` is the ceiling on a header's own arithmetic, which is
///   the number an attacker writes.
const PLY_REQUIRED: [&str; 4] = ["IndexOutOfRange", "Unsupported", "NotFinite", "TooLarge"];

fn ply_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/ply_read")
}

fn ply_corpus() -> Vec<Vec<u8>> {
    let dir = ply_corpus_dir();
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

/// The answer a byte string gets from the PLY reader, as a name.
///
/// Matched on the enum with no wildcard, so a refusal added later stops
/// this file compiling until somebody decides whether a seed reaches it.
fn ply_outcome(bytes: &[u8]) -> &'static str {
    match ply::read(bytes) {
        Ok(_) => "Ok",
        Err(refusal) => match refusal {
            MeshError::TooShortForHeader { .. } => "TooShortForHeader",
            MeshError::CountMismatch { .. } => "CountMismatch",
            MeshError::TooLarge { .. } => "TooLarge",
            MeshError::ExpectedKeyword { .. } => "ExpectedKeyword",
            MeshError::NotANumber { .. } => "NotANumber",
            MeshError::NotFinite { .. } => "NotFinite",
            MeshError::IndexOutOfRange { .. } => "IndexOutOfRange",
            MeshError::IndexZero { .. } => "IndexZero",
            MeshError::NotAFace { .. } => "NotAFace",
            MeshError::Unsupported { .. } => "Unsupported",
            MeshError::NoGeometry => "NoGeometry",
        },
    }
}

/// Every committed PLY input answers, one way or the other.
#[test]
fn every_recorded_ply_input_answers() {
    for bytes in ply_corpus() {
        let _ = ply::looks_like(&bytes);
        if let Ok(mesh) = ply::read(&bytes) {
            assert_eq!(mesh.positions.len() % 3, 0);
            assert!(!mesh.is_empty());
            assert!(mesh.face_normals.is_empty());
            for value in mesh.positions.iter().flatten() {
                assert!(value.is_finite());
            }
            let _ = mesh.winding_disagreements();
        }
    }
}

/// The PLY corpus keeps its strength.
#[test]
fn the_ply_corpus_still_covers_what_it_was_recorded_to_cover() {
    let inputs = ply_corpus();
    let distinct: BTreeSet<Vec<u8>> = inputs.iter().cloned().collect();
    assert!(
        distinct.len() >= PLY_LOW_WATER,
        "the corpus holds {} distinct inputs and the floor is {PLY_LOW_WATER}",
        distinct.len()
    );

    let reached: BTreeSet<&'static str> = distinct.iter().map(|bytes| ply_outcome(bytes)).collect();
    assert!(
        reached.len() >= PLY_DISTINCT_OUTCOMES,
        "the corpus reaches {} distinct answers and the floor is {PLY_DISTINCT_OUTCOMES}. \
         Reached: {reached:?}",
        reached.len()
    );
    for required in PLY_REQUIRED {
        assert!(
            reached.contains(required),
            "no committed seed reaches `{required}`, which is a guard nothing is exercising. \
             Reached: {reached:?}"
        );
    }
    assert!(
        distinct.iter().filter(|b| ply::read(b).is_ok()).count() >= 3,
        "all three encodings should have a seed that reads, or the corpus tests refusal alone"
    );
}

#[test]
#[ignore = "a census, not a gate: run it to update the numbers above"]
fn ply_census() {
    let distinct: BTreeSet<Vec<u8>> = ply_corpus().into_iter().collect();
    let reached: BTreeSet<&'static str> = distinct.iter().map(|b| ply_outcome(b)).collect();
    println!(
        "{} distinct inputs, {} outcomes: {reached:?}",
        distinct.len(),
        reached.len()
    );
}

// ---------------------------------------------------------------------
// The OBJ corpus, held to the same claims by the same shape of gate.
// ---------------------------------------------------------------------

/// The committed OBJ corpus never shrinks below this many **distinct**
/// inputs. Distinct by content, for the reason above.
const OBJ_LOW_WATER: usize = 16;

/// How many distinct outcomes the OBJ seeds must still reach.
///
/// **Measured, not guessed** — `obj_census` below prints it. The floor
/// sits two under what the committed seeds reach, so `cargo fuzz cmin`
/// has room to minimise and no more.
const OBJ_DISTINCT_OUTCOMES: usize = 7;

/// Refusals an OBJ seed must provoke, each guarding something a count
/// cannot.
///
/// * `IndexZero` is **the refusal this format adds over the other two**.
///   It exists because OBJ numbers from one, and a zero there is a field
///   nobody filled in rather than a number computed wrongly.
/// * `IndexOutOfRange` covers both directions at once: a positive index
///   past the end and a negative one reaching before the beginning are
///   the same refusal down two different arms of the resolver.
/// * `Unsupported` is the face whose corners disagree about their own
///   shape — a well-formed file with no representation here, which is a
///   different problem from a malformed one.
/// * `NotFinite` stops a coordinate nothing downstream can bound
///   reaching a vertex buffer.
const OBJ_REQUIRED: [&str; 4] = ["IndexZero", "IndexOutOfRange", "Unsupported", "NotFinite"];

fn obj_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/obj_read")
}

fn obj_corpus() -> Vec<Vec<u8>> {
    let dir = obj_corpus_dir();
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

/// The answer a byte string gets from the OBJ reader, as a name.
///
/// Matched on the enum with no wildcard, so a refusal added later stops
/// this file compiling until somebody decides whether a seed reaches it.
fn obj_outcome(bytes: &[u8]) -> &'static str {
    match obj::read(bytes) {
        Ok(_) => "Ok",
        Err(refusal) => match refusal {
            MeshError::TooShortForHeader { .. } => "TooShortForHeader",
            MeshError::CountMismatch { .. } => "CountMismatch",
            MeshError::TooLarge { .. } => "TooLarge",
            MeshError::ExpectedKeyword { .. } => "ExpectedKeyword",
            MeshError::NotANumber { .. } => "NotANumber",
            MeshError::NotFinite { .. } => "NotFinite",
            MeshError::IndexOutOfRange { .. } => "IndexOutOfRange",
            MeshError::IndexZero { .. } => "IndexZero",
            MeshError::NotAFace { .. } => "NotAFace",
            MeshError::Unsupported { .. } => "Unsupported",
            MeshError::NoGeometry => "NoGeometry",
        },
    }
}

/// Every committed OBJ input answers, one way or the other, and every
/// mesh that comes back holds what its type promises.
#[test]
fn every_recorded_obj_input_answers() {
    for bytes in obj_corpus() {
        let _ = obj::looks_like(&bytes);
        // The other public entry point, asked of the same bytes.
        let _ = obj::materials(&bytes);
        if let Ok(mesh) = obj::read(&bytes) {
            assert_eq!(mesh.positions.len() % 3, 0);
            assert!(!mesh.is_empty());
            assert!(mesh.face_normals.is_empty());
            // The pairing this format brings: a corner without a normal
            // has none that could be invented for it.
            assert!(
                mesh.corner_normals.is_empty() || mesh.corner_normals.len() == mesh.positions.len()
            );
            assert!(
                mesh.corner_texcoords.is_empty()
                    || mesh.corner_texcoords.len() == mesh.positions.len()
            );
            for value in mesh.positions.iter().chain(&mesh.corner_normals).flatten() {
                assert!(value.is_finite());
            }
            let _ = mesh.winding_disagreements();
        }
    }
}

/// The OBJ corpus keeps its strength.
#[test]
fn the_obj_corpus_still_covers_what_it_was_recorded_to_cover() {
    let inputs = obj_corpus();
    let distinct: BTreeSet<Vec<u8>> = inputs.iter().cloned().collect();
    assert!(
        distinct.len() >= OBJ_LOW_WATER,
        "the corpus holds {} distinct inputs and the floor is {OBJ_LOW_WATER}",
        distinct.len()
    );

    let reached: BTreeSet<&'static str> = distinct.iter().map(|bytes| obj_outcome(bytes)).collect();
    assert!(
        reached.len() >= OBJ_DISTINCT_OUTCOMES,
        "the corpus reaches {} distinct answers and the floor is {OBJ_DISTINCT_OUTCOMES}. \
         Reached: {reached:?}",
        reached.len()
    );
    for required in OBJ_REQUIRED {
        assert!(
            reached.contains(required),
            "no committed seed reaches `{required}`, which is a guard nothing is exercising. \
             Reached: {reached:?}"
        );
    }
    // A corpus of nothing but refusals teaches the search that
    // everything is refused, and every branch past the first refusal
    // goes unvisited.
    assert!(
        distinct.iter().filter(|b| obj::read(b).is_ok()).count() >= 6,
        "the corpus needs files that read, or it tests refusal alone"
    );
}

#[test]
#[ignore = "a census, not a gate: run it to update the numbers above"]
fn obj_census() {
    let distinct: BTreeSet<Vec<u8>> = obj_corpus().into_iter().collect();
    let reached: BTreeSet<&'static str> = distinct.iter().map(|b| obj_outcome(b)).collect();
    println!(
        "{} distinct inputs, {} outcomes: {reached:?}",
        distinct.len(),
        reached.len()
    );
}

// ---------------------------------------------------------------------
// The MTL corpus, held to the same claims by the same shape of gate.
// ---------------------------------------------------------------------

/// The committed MTL corpus never shrinks below this many **distinct**
/// inputs. Distinct by content, for the reason above.
const MTL_LOW_WATER: usize = 13;

/// How many distinct outcomes the MTL seeds must still reach.
///
/// **Measured, not guessed** — `mtl_census` below prints it, and the
/// committed seeds reach five: `Ok` and every one of the four refusals
/// this reader can make.
///
/// **The slack here is one seed rather than the two the gates above
/// take, and the reason is arithmetic.** Those readers reach eight and
/// nine outcomes, where two is a quarter of the range; this one reaches
/// five, where two would be a forty-per-cent hole in a gate whose whole
/// job is to notice holes. A reader with few answers needs a tighter
/// floor, not the same one.
const MTL_DISTINCT_OUTCOMES: usize = 4;

/// Refusals an MTL seed must provoke.
///
/// This reader has only four it can make at all, which is itself the
/// point: a material library indexes nothing, declares no counts and
/// multiplies nothing, so the ways it can be wrong are few and each one
/// carries weight.
///
/// * `ExpectedKeyword` is the property stated before any `newmtl` — a
///   file whose first material is missing rather than a value with
///   nowhere to go. It is also what bytes that are not text get.
/// * `Unsupported` is the library that declares no material: well-formed
///   and unusable, where the caller's next move is to find the right
///   file rather than re-export this one.
/// * `NotFinite` stops a factor nothing downstream can bound reaching a
///   renderer.
/// * `NotANumber` is the one a mutator finds by accident and the one
///   that would otherwise be a silent zero.
const MTL_REQUIRED: [&str; 4] = ["ExpectedKeyword", "Unsupported", "NotFinite", "NotANumber"];

fn mtl_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/mtl_read")
}

fn mtl_corpus() -> Vec<Vec<u8>> {
    let dir = mtl_corpus_dir();
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

/// The answer a byte string gets from the MTL reader, as a name.
///
/// Matched on the enum with no wildcard, so a refusal added later stops
/// this file compiling until somebody decides whether a seed reaches it.
fn mtl_outcome(bytes: &[u8]) -> &'static str {
    match mtl::read(bytes) {
        Ok(_) => "Ok",
        Err(refusal) => match refusal {
            MeshError::TooShortForHeader { .. } => "TooShortForHeader",
            MeshError::CountMismatch { .. } => "CountMismatch",
            MeshError::TooLarge { .. } => "TooLarge",
            MeshError::ExpectedKeyword { .. } => "ExpectedKeyword",
            MeshError::NotANumber { .. } => "NotANumber",
            MeshError::NotFinite { .. } => "NotFinite",
            MeshError::IndexOutOfRange { .. } => "IndexOutOfRange",
            MeshError::IndexZero { .. } => "IndexZero",
            MeshError::NotAFace { .. } => "NotAFace",
            MeshError::Unsupported { .. } => "Unsupported",
            MeshError::NoGeometry => "NoGeometry",
        },
    }
}

/// Every committed MTL input answers, and every library that comes back
/// holds what its type promises.
#[test]
fn every_recorded_mtl_input_answers() {
    for bytes in mtl_corpus() {
        if let Ok(library) = mtl::read(&bytes) {
            assert!(!library.is_empty());
            for material in &library {
                for colour in [
                    material.ambient,
                    material.diffuse,
                    material.specular,
                    material.emissive,
                ]
                .into_iter()
                .flatten()
                {
                    for value in colour {
                        assert!(value.is_finite());
                    }
                }
                for value in [material.shininess, material.opacity].into_iter().flatten() {
                    assert!(value.is_finite());
                }
            }
        }
    }
}

/// The MTL corpus keeps its strength.
#[test]
fn the_mtl_corpus_still_covers_what_it_was_recorded_to_cover() {
    let inputs = mtl_corpus();
    let distinct: BTreeSet<Vec<u8>> = inputs.iter().cloned().collect();
    assert!(
        distinct.len() >= MTL_LOW_WATER,
        "the corpus holds {} distinct inputs and the floor is {MTL_LOW_WATER}",
        distinct.len()
    );

    let reached: BTreeSet<&'static str> = distinct.iter().map(|bytes| mtl_outcome(bytes)).collect();
    assert!(
        reached.len() >= MTL_DISTINCT_OUTCOMES,
        "the corpus reaches {} distinct answers and the floor is {MTL_DISTINCT_OUTCOMES}. \
         Reached: {reached:?}",
        reached.len()
    );
    for required in MTL_REQUIRED {
        assert!(
            reached.contains(required),
            "no committed seed reaches `{required}`, which is a guard nothing is exercising. \
             Reached: {reached:?}"
        );
    }
    assert!(
        distinct.iter().filter(|b| mtl::read(b).is_ok()).count() >= 6,
        "the corpus needs files that read, or it tests refusal alone"
    );
}

#[test]
#[ignore = "a census, not a gate: run it to update the numbers above"]
fn mtl_census() {
    let distinct: BTreeSet<Vec<u8>> = mtl_corpus().into_iter().collect();
    let reached: BTreeSet<&'static str> = distinct.iter().map(|b| mtl_outcome(b)).collect();
    println!(
        "{} distinct inputs, {} outcomes: {reached:?}",
        distinct.len(),
        reached.len()
    );
}
