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

use renew_mesh::accessor::Shape;
use renew_mesh::{blob, data_uri, glb, gltf, mtl, obj, ply, stl};

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
/// * `CountMismatch` is the answer for bytes that are neither dialect,
///   and the only seed here that is not valid UTF-8. **This bullet named
///   `NotThisFormat` until somebody checked**, which is a refusal this
///   reader cannot make: STL has no magic, so "not this format" and "cut
///   short" are one observation, and the count is what it can honestly
///   report. The array below always said `CountMismatch`; only the prose
///   was wrong, which is the kind of drift that survives a green suite.
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
/// **The names come from `MeshError::name`, which is where the closed
/// enum is spent now.** This function used to spell all eleven out, with
/// a note saying the wildcard-free match was what forced somebody to
/// decide about a new variant. That forcing did not go away when the
/// list moved: `name` has no wildcard either, so a variant added later
/// still stops the build — and the per-reader question, "what does this
/// one mean to STL", is asked by `stl_cannot_reach` in the suite beside
/// the reader, which is a better place for it than a corpus gate. What
/// went away is five copies of one list.
fn outcome(bytes: &[u8]) -> &'static str {
    match stl::read(bytes) {
        Ok(_) => "Ok",
        Err(refusal) => refusal.name(),
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
        Err(refusal) => refusal.name(),
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
        Err(refusal) => refusal.name(),
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
/// take, and the reason is arithmetic.** Those readers reach between
/// seven and ten outcomes, where two is under a third; this one reaches
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
        Err(refusal) => refusal.name(),
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

// ---------------------------------------------------------------------
// The blob corpus, held to the same claims by the same shape of gate.
// ---------------------------------------------------------------------

/// The committed blob corpus never shrinks below this many **distinct**
/// inputs. Distinct by content, for the reason above.
const BLOB_LOW_WATER: usize = 16;

/// How many distinct outcomes the blob seeds must still reach.
///
/// **Measured, not guessed** — `blob_census` below prints it.
const BLOB_DISTINCT_OUTCOMES: usize = 7;

/// Refusals a blob seed must provoke.
///
/// * `TooShortForHeader` and `CountMismatch` are the two a transfer that
///   went wrong produces, and they are different problems: one is bytes
///   that never held a header, the other is a header describing a body
///   that is not there.
/// * `Unsupported` is a blob from a later build — well-formed and
///   unusable here, where the caller's next move is a newer build.
/// * `TooLarge` is the four-byte corner count that sizes an allocation,
///   which is the one number in this format an attacker would reach for.
const BLOB_REQUIRED: [&str; 6] = [
    "TooShortForHeader",
    "CountMismatch",
    "Unsupported",
    "TooLarge",
    // **Added because they were missing and it showed.** With the two
    // seeds that reach these gone, the floor still passed: deleting
    // `nan-position`, `infinite-position` and `ragged-corners` left this
    // gate green, and those are every seed exercising the finiteness
    // check and every seed exercising the whole-triangles check. Every
    // other corpus in this file names `NotFinite`; this one did not.
    "NotFinite",
    "NotAFace",
];

fn blob_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/blob_read")
}

fn blob_corpus() -> Vec<Vec<u8>> {
    let dir = blob_corpus_dir();
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

/// The answer a byte string gets from the blob reader, as a name.
///
/// Matched on the enum with no wildcard, so a refusal added later stops
/// this file compiling until somebody decides whether a seed reaches it.
fn blob_outcome(bytes: &[u8]) -> &'static str {
    match blob::read(bytes) {
        Ok(_) => "Ok",
        Err(refusal) => refusal.name(),
    }
}

/// Every committed blob answers, and every mesh that comes back writes
/// itself again unchanged.
///
/// **The canonical claim, replayed on stable at every merge.** The other
/// corpora can only check that a reader answered; this one can check
/// that the format is a bijection on what it accepts, because the writer
/// is here too.
#[test]
fn every_recorded_blob_answers_and_rewrites_itself() {
    for bytes in blob_corpus() {
        let Ok(mesh) = blob::read(&bytes) else {
            continue;
        };
        assert_eq!(mesh.positions.len() % 3, 0);
        assert!(!mesh.is_empty());
        assert!(mesh.face_normals.is_empty() || mesh.face_normals.len() == mesh.triangles());
        assert!(
            mesh.corner_normals.is_empty() || mesh.corner_normals.len() == mesh.positions.len()
        );
        assert!(
            mesh.corner_texcoords.is_empty() || mesh.corner_texcoords.len() == mesh.positions.len()
        );
        for value in mesh.positions.iter().flatten() {
            assert!(value.is_finite());
        }

        let again = blob::write(&mesh);
        let twice = blob::read(&again).expect("what this crate wrote, this crate reads");
        assert_eq!(twice, mesh, "a blob read and rewritten is the same mesh");
        assert_eq!(blob::write(&twice), again, "and the same bytes");
    }
}

/// The blob corpus keeps its strength.
#[test]
fn the_blob_corpus_still_covers_what_it_was_recorded_to_cover() {
    let inputs = blob_corpus();
    let distinct: BTreeSet<Vec<u8>> = inputs.iter().cloned().collect();
    assert!(
        distinct.len() >= BLOB_LOW_WATER,
        "the corpus holds {} distinct inputs and the floor is {BLOB_LOW_WATER}",
        distinct.len()
    );

    let reached: BTreeSet<&'static str> =
        distinct.iter().map(|bytes| blob_outcome(bytes)).collect();
    assert!(
        reached.len() >= BLOB_DISTINCT_OUTCOMES,
        "the corpus reaches {} distinct answers and the floor is {BLOB_DISTINCT_OUTCOMES}. \
         Reached: {reached:?}",
        reached.len()
    );
    for required in BLOB_REQUIRED {
        assert!(
            reached.contains(required),
            "no committed seed reaches `{required}`, which is a guard nothing is exercising. \
             Reached: {reached:?}"
        );
    }
    // The presence bits are independent, so a corpus that only ever held
    // one shape would teach the search nothing about the other seven.
    assert!(
        distinct.iter().filter(|b| blob::read(b).is_ok()).count() >= 8,
        "the corpus needs every presence shape that reads, or it tests one arm of three"
    );
}

#[test]
#[ignore = "a census, not a gate: run it to update the numbers above"]
fn blob_census() {
    let distinct: BTreeSet<Vec<u8>> = blob_corpus().into_iter().collect();
    let reached: BTreeSet<&'static str> = distinct.iter().map(|b| blob_outcome(b)).collect();
    println!(
        "{} distinct inputs, {} outcomes: {reached:?}",
        distinct.len(),
        reached.len()
    );
}
// ---------------------------------------------------------------------
// The binary glTF container.
//
// **The only corpus here whose reader parses nothing.** These seeds
// exercise arithmetic: a total length, then a chain of chunk lengths
// each of which decides where the next chunk header is read from. A
// single wrong number does not produce one bad slice, it moves the
// cursor, so the seeds that matter are the ones where the chain is
// plausible for one more link than it should be.
// ---------------------------------------------------------------------

/// The committed container corpus never shrinks below this many
/// **distinct** inputs.
const GLB_LOW_WATER: usize = 18;

/// How many distinct outcomes the container seeds must still reach.
///
/// **Measured, not guessed** — `glb_census` below prints it. The floor
/// sits below the measured number so the fuzzer's own minimisation has
/// room, and the specific guards that must survive are named separately
/// underneath, because a count alone cannot notice *which* one went.
const GLB_DISTINCT_OUTCOMES: usize = 10;

/// Refusals a container seed must provoke, each guarding something a
/// count cannot.
///
/// * `SizeMismatch` is the equality that bounds everything after it. It
///   is what makes every chunk length below safe to trust against the
///   file, and a reader that relaxed it to "at least" would accept a
///   second payload hidden after the first.
/// * `ChunkOverruns` and `ChunkHeaderTruncated` are the two halves of
///   the chain going wrong — a chunk claiming more than remains, and a
///   chunk header that does not fit in what is left. Different faults,
///   and a corpus that reached only one would leave the other unseeded.
/// * `ChunkNotAligned` guards the rule that keeps the chain walkable at
///   all: the padding is inside the length, so a length that is not a
///   multiple of four is a writer that skipped it.
/// * `FirstChunkNotJson` is the ordering rule the format is built on,
///   and the one whose deletion would be least visible — a reader that
///   hunted for the JSON chunk anywhere would pass every other seed
///   here.
const GLB_REQUIRED: [&str; 5] = [
    "SizeMismatch",
    "ChunkOverruns",
    "ChunkHeaderTruncated",
    "ChunkNotAligned",
    "FirstChunkNotJson",
];

fn glb_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/glb_read")
}

fn glb_corpus() -> Vec<Vec<u8>> {
    let dir = glb_corpus_dir();
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

/// The answer a byte string gets from the container reader, as a name.
fn glb_outcome(bytes: &[u8]) -> &'static str {
    match glb::read(bytes) {
        Ok(_) => "Ok",
        Err(refusal) => refusal.name(),
    }
}

/// Every committed container answers, and what comes back borrows the
/// bytes that went in.
///
/// **The borrow is the claim worth replaying**, because it is the one a
/// caller relies on and the one no refusal would reveal: a container
/// that read but handed back a slice reaching past what it validated
/// would look exactly like a container that read.
#[test]
fn every_recorded_container_answers_and_borrows_its_input() {
    for bytes in glb_corpus() {
        let Ok(container) = glb::read(&bytes) else {
            continue;
        };
        let base = bytes.as_ptr() as usize;
        let inside = |part: &[u8]| {
            let start = part.as_ptr() as usize;
            start >= base && start.saturating_add(part.len()) <= base.saturating_add(bytes.len())
        };
        assert!(inside(container.json), "the JSON chunk borrows the input");
        assert_eq!(container.json.len() % 4, 0, "a chunk is whole words");
        if let Some(binary) = container.binary {
            assert!(inside(binary), "the binary chunk borrows the input");
            assert_eq!(binary.len() % 4, 0, "a chunk is whole words");
        }
    }
}

/// The container corpus keeps its strength.
#[test]
fn the_glb_corpus_still_covers_what_it_was_recorded_to_cover() {
    let inputs = glb_corpus();
    let distinct: BTreeSet<Vec<u8>> = inputs.iter().cloned().collect();
    assert!(
        distinct.len() >= GLB_LOW_WATER,
        "the corpus holds {} distinct inputs and the floor is {GLB_LOW_WATER}",
        distinct.len()
    );

    let reached: BTreeSet<&'static str> = distinct.iter().map(|bytes| glb_outcome(bytes)).collect();
    assert!(
        reached.len() >= GLB_DISTINCT_OUTCOMES,
        "the corpus reaches {} distinct answers and the floor is {GLB_DISTINCT_OUTCOMES}. \
         Reached: {reached:?}",
        reached.len()
    );
    for required in GLB_REQUIRED {
        assert!(
            reached.contains(required),
            "no committed seed reaches `{required}`, which is a guard nothing is exercising. \
             Reached: {reached:?}"
        );
    }

    // **A seed for each of the two rules that read backwards from every
    // other reader here.** Both are `Ok`, so the distinct-outcome floor
    // cannot notice either going missing: deleting the seed that carries
    // an unknown chunk type would leave this gate green while the skip
    // it exists to protect went unexercised.
    let readable: Vec<&Vec<u8>> = distinct
        .iter()
        .filter(|bytes| glb::read(bytes).is_ok())
        .collect();
    assert!(
        readable.len() >= 5,
        "the corpus needs containers that read, or the search starts nowhere"
    );
    assert!(
        readable
            .iter()
            .any(|bytes| glb::read(bytes).is_ok_and(|c| c.binary == Some(&[][..]))),
        "no seed carries an empty binary chunk, which the format permits and this reader \
         must not fold into `None`"
    );
    assert!(
        readable.iter().any(|bytes| bytes.len() > 60),
        "no seed carries a chunk past the second, which is where an unknown type is skipped"
    );
}

#[test]
#[ignore = "a census, not a gate: run it to update the numbers above"]
fn glb_census() {
    let distinct: BTreeSet<Vec<u8>> = glb_corpus().into_iter().collect();
    let reached: BTreeSet<&'static str> = distinct.iter().map(|b| glb_outcome(b)).collect();
    println!(
        "{} distinct inputs, {} outcomes: {reached:?}",
        distinct.len(),
        reached.len()
    );
}
// ---------------------------------------------------------------------
// The accessor layer.
//
// **The only corpus here whose input is not a file.** An accessor is a
// byte region plus six parameters, so the seeds carry the parameters in
// a nine-byte head; the encoding is
// `tests/shared/accessor_seed.rs`, included below and by two other
// targets, because three copies of one encoding is a defect waiting to
// happen.
// ---------------------------------------------------------------------

#[path = "shared/accessor_seed.rs"]
mod accessor_seed;

/// The committed accessor corpus never shrinks below this many
/// **distinct** inputs.
const ACCESSOR_LOW_WATER: usize = 26;

/// How many distinct outcomes the accessor seeds must still reach.
///
/// **Measured, not guessed** — `accessor_census` below prints it.
const ACCESSOR_DISTINCT_OUTCOMES: usize = 14;

/// Refusals a seed must provoke, each guarding something a count cannot.
///
/// * `OutOfRange` is the refusal this layer exists for: the count, the
///   stride and the region are three claims, and it is the only one that
///   compares them.
/// * `StrideSmallerThanElement` is the disagreement that is not a bad
///   value — a legal stride, too small for these elements — and the one
///   most easily lost by checking the format's range and stopping.
/// * `OffsetNotAligned` and `StrideNotAligned` are the two the format
///   states outright, and the two a reader is most tempted to let pass
///   because nothing downstream would notice.
/// * `NormalizedIsMeaningless` is the flag with no reading, which a
///   reader that simply divided would answer with a number instead.
/// * `NotAnIndexType` and `NormalizedIndices` are the two the index
///   entry point refuses and the attribute one does not. **They were
///   unreachable until a flag bit was added to the head**: every seed
///   called `view`, so half this layer's public surface had no seed at
///   all and neither the corpus nor the fuzzer would ever have said so.
/// * `ViewOutOfRange` is the claim an accessor cannot make on its own —
///   whether the region it was handed was really inside its buffer — and
///   `StrideExceedsView` is the one stride rule that is about the view
///   rather than the value. A corpus reaching neither would leave the
///   layer in front of the accessor entirely unexercised.
/// * `IndexOutOfRange` and `NotAFace` come from the layer above both:
///   an index addressing a vertex that is not there, and a corner count
///   that does not divide into faces. The first is the only fault at
///   that layer a wrong comparison turns into an out-of-bounds read,
///   which is why the corpus assembles at all.
const ACCESSOR_REQUIRED: [&str; 11] = [
    "OutOfRange",
    "StrideSmallerThanElement",
    "OffsetNotAligned",
    "StrideNotAligned",
    "NormalizedIsMeaningless",
    "NotAnIndexType",
    "NormalizedIndices",
    "ViewOutOfRange",
    "StrideExceedsView",
    "IndexOutOfRange",
    "NotAFace",
];

fn accessor_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/accessor_view")
}

fn accessor_corpus() -> Vec<Vec<u8>> {
    let dir = accessor_corpus_dir();
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

/// Every committed seed answers, and every view borrows its own region.
#[test]
fn every_recorded_accessor_answers_and_stays_inside_its_region() {
    for bytes in accessor_corpus() {
        let Some(seed) = accessor_seed::decode(&bytes) else {
            continue;
        };
        let Ok(accessor) = seed.accessor() else {
            continue;
        };
        let Ok(view) = accessor.view(seed.region) else {
            continue;
        };
        assert_eq!(view.len(), accessor.count);
        // Recomputed from the accessor's own numbers rather than trusted
        // from the reader, which is the point of asserting it here.
        let last = accessor.byte_offset
            + (accessor.count - 1) * accessor.stride()
            + accessor.element_size();
        assert!(
            last <= seed.region.len(),
            "a view that read reaches byte {last} of a {}-byte region",
            seed.region.len()
        );
        for component in 0..accessor.shape.components() {
            assert!(view.float(view.len() - 1, component).is_some());
        }
        assert!(view.float(view.len(), 0).is_none());
    }
}

/// The accessor corpus keeps its strength.
#[test]
fn the_accessor_corpus_still_covers_what_it_was_recorded_to_cover() {
    let inputs = accessor_corpus();
    let distinct: BTreeSet<Vec<u8>> = inputs.iter().cloned().collect();
    assert!(
        distinct.len() >= ACCESSOR_LOW_WATER,
        "the corpus holds {} distinct inputs and the floor is {ACCESSOR_LOW_WATER}",
        distinct.len()
    );

    let reached: BTreeSet<&'static str> = distinct
        .iter()
        .map(|bytes| accessor_seed::outcome(bytes))
        .collect();
    assert!(
        reached.len() >= ACCESSOR_DISTINCT_OUTCOMES,
        "the corpus reaches {} distinct answers and the floor is {ACCESSOR_DISTINCT_OUTCOMES}. \
         Reached: {reached:?}",
        reached.len()
    );
    for required in ACCESSOR_REQUIRED {
        assert!(
            reached.contains(required),
            "no committed seed reaches `{required}`, which is a guard nothing is exercising. \
             Reached: {reached:?}"
        );
    }

    // **The pair that makes the bound testable at all.** One seed fits
    // exactly by the expression that counts the last element's size, and
    // one is a byte short. A corpus with only tightly packed seeds
    // cannot tell that expression from `count * stride`, because for
    // those two they are the same number.
    let interleaved: Vec<&Vec<u8>> = distinct
        .iter()
        .filter(|bytes| {
            accessor_seed::decode(bytes).is_some_and(|seed| {
                seed.accessor()
                    .is_ok_and(|accessor| accessor.stride() > accessor.element_size())
            })
        })
        .collect();
    assert!(
        interleaved.len() >= 2,
        "the corpus needs interleaved seeds on both sides of the bound, and holds {}",
        interleaved.len()
    );
    assert!(
        interleaved
            .iter()
            .any(|bytes| accessor_seed::outcome(bytes) == "Ok"),
        "one of them must fit"
    );
    assert!(
        interleaved
            .iter()
            .any(|bytes| accessor_seed::outcome(bytes) == "OutOfRange"),
        "and one of them must not"
    );
}

/// **The two halves of the shared encoding agree.**
///
/// `encode` is used only by the generator and `decode` by everything
/// else, so nothing else in the tree would notice them drifting apart —
/// the corpus would simply start meaning something other than what it
/// was written to mean, and every gate above would stay green.
#[test]
fn the_seed_encoding_round_trips() {
    let region = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let cases = [
        (5126u32, Shape::Vec3, 3usize, 0usize, None, false),
        (5121, Shape::Scalar, 1, 4, Some(16), true),
        (5124, Shape::Vec4, 65535, 65535, Some(65535), true),
        (5120, Shape::Vec2, 0, 0, Some(0), false),
    ];
    for (code, shape, count, byte_offset, byte_stride, normalized) in cases {
        let seed = accessor_seed::Seed {
            code,
            shape,
            count,
            byte_offset,
            byte_stride,
            assemble: byte_stride.map(|_| 6),
            index_offset: 12,
            view: byte_stride.map(|stride| accessor_seed::BufferView {
                byte_offset: 4,
                byte_length: 8,
                byte_stride: Some(stride),
            }),
            normalized,
            as_indices: normalized,
            region: &region,
        };
        let bytes = accessor_seed::encode(&seed);
        let back = accessor_seed::decode(&bytes).expect("what encode writes, decode reads");
        assert_eq!(back.code, code);
        assert_eq!(back.shape, shape);
        assert_eq!(back.count, count);
        assert_eq!(back.byte_offset, byte_offset);
        assert_eq!(back.byte_stride, byte_stride);
        assert_eq!(back.normalized, normalized);
        assert_eq!(
            back.as_indices, normalized,
            "the third flag bit survives too"
        );
        assert_eq!(
            back.view.map(|view| (view.byte_offset, view.byte_length)),
            byte_stride.map(|_| (4, 8)),
            "and the view the fourth bit carries"
        );
        assert_eq!(
            back.assemble,
            byte_stride.map(|_| 6),
            "and the index stream the fifth bit carries"
        );
        assert_eq!(back.index_offset, 12);
        assert_eq!(back.region, &region[..]);
    }
}

#[test]
#[ignore = "a census, not a gate: run it to update the numbers above"]
fn accessor_census() {
    let distinct: BTreeSet<Vec<u8>> = accessor_corpus().into_iter().collect();
    let reached: BTreeSet<&'static str> =
        distinct.iter().map(|b| accessor_seed::outcome(b)).collect();
    println!(
        "{} distinct inputs, {} outcomes: {reached:?}",
        distinct.len(),
        reached.len()
    );
}

// ---------------------------------------------------------------------
// The binary glTF reader, whole.
//
// **The one corpus here whose seeds are files a tool could have
// written.** Every other corpus in this file is either a format's own
// bytes or, for the accessor, an encoding invented so a fuzzer could
// reach parameters that are not in a file at all. A container is a file,
// so the generator writes containers and this reads them.
//
// **No seed carries a node cycle**, and that is deliberate rather than
// an oversight: the reader refuses one by entering each node at most
// once, and if that guard were removed a cycle seed would make this gate
// stop making progress rather than fail. A stall is the one outcome a
// merge gate cannot report. The cycle is pinned by a deterministic test
// beside the crate, where a wedged run is a failed test.
// ---------------------------------------------------------------------

const GLTF_LOW_WATER: usize = 26;

/// How many distinct outcomes the glTF seeds must still reach.
///
/// **Measured, not guessed** — `gltf_census` below prints it. The outer
/// vocabulary is what is counted: a caller keys on which *layer* refused,
/// and the inner refusal's own name is one call away through the value.
const GLTF_DISTINCT_OUTCOMES: usize = 13;

/// Refusals a glTF seed must provoke.
///
/// * `Container` and `Document` are the two framing layers, and a corpus
///   reaching neither would be exercising the reader with nothing but
///   well-formed wrappers.
/// * `Accessor` is where the arithmetic lives — a count past its chunk,
///   a view past its buffer — and the layer a wrong bound would show in.
/// * `Geometry` is everything the assembly and placement layers refuse,
///   reached here through six layers of document rather than directly.
/// * **All six the buffer layer can produce are here.** A document
///   carrying its own geometry rather than sitting beside a chunk is
///   half of this format, and a corpus of containers alone never enters
///   it -- so the list names every refusal that half can reach rather
///   than the three that happened to have seeds. `ExternalResource` is
///   not new: it moved down a layer, from the view table to the buffer
///   that view points at.
const GLTF_REQUIRED: [&str; 10] = [
    "Container",
    "Document",
    "Accessor",
    "Geometry",
    "NoBinaryChunk",
    "ExternalResource",
    "WrongMediaType",
    "Payload",
    "BufferTooShort",
    "BufferWithoutSource",
];

fn gltf_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/gltf_read")
}

fn gltf_corpus() -> Vec<Vec<u8>> {
    let dir = gltf_corpus_dir();
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

/// The answer a container gets, as the name of the layer that refused.
fn gltf_outcome(bytes: &[u8]) -> &'static str {
    match gltf::read(bytes) {
        Ok(_) => "Ok",
        Err(refusal) => refusal.name(),
    }
}

/// Every committed container answers, and what it accepts is geometry.
#[test]
fn every_recorded_container_answers_and_is_whole() {
    for bytes in gltf_corpus() {
        let Ok(mesh) = gltf::read(&bytes) else {
            continue;
        };
        assert_eq!(mesh.positions.len() % 3, 0, "whole triangles");
        assert!(!mesh.is_empty());
        assert!(mesh.face_normals.is_empty() || mesh.face_normals.len() == mesh.triangles());
        assert!(
            mesh.corner_normals.is_empty() || mesh.corner_normals.len() == mesh.positions.len()
        );
        assert!(
            mesh.corner_texcoords.is_empty() || mesh.corner_texcoords.len() == mesh.positions.len()
        );
        for value in mesh
            .positions
            .iter()
            .chain(&mesh.face_normals)
            .chain(&mesh.corner_normals)
            .flatten()
        {
            assert!(value.is_finite(), "a coordinate nothing can bound");
        }
    }
}

/// The container corpus keeps its strength.
#[test]
fn the_gltf_corpus_still_covers_what_it_was_recorded_to_cover() {
    let inputs = gltf_corpus();
    let distinct: BTreeSet<Vec<u8>> = inputs.iter().cloned().collect();
    assert!(
        distinct.len() >= GLTF_LOW_WATER,
        "the corpus holds {} distinct inputs and the floor is {GLTF_LOW_WATER}",
        distinct.len()
    );

    let reached: BTreeSet<&'static str> =
        distinct.iter().map(|bytes| gltf_outcome(bytes)).collect();
    assert!(
        reached.len() >= GLTF_DISTINCT_OUTCOMES,
        "the corpus reaches {} distinct answers and the floor is {GLTF_DISTINCT_OUTCOMES}. \
         Reached: {reached:?}",
        reached.len()
    );
    for required in GLTF_REQUIRED {
        assert!(
            reached.contains(required),
            "no committed seed reaches `{required}`, which is a layer nothing is exercising. \
             Reached: {reached:?}"
        );
    }

    // **Seeds that read, and read something worth mutating from.** Six
    // layers have to agree before the reader reaches the arithmetic, so
    // a corpus whose only successes were the smallest possible document
    // would leave the interesting paths — a transform, a hierarchy, an
    // index stream — reachable only by luck.
    let readable: Vec<&Vec<u8>> = distinct
        .iter()
        .filter(|bytes| gltf::read(bytes).is_ok())
        .collect();
    assert!(
        readable.len() >= 5,
        "the corpus needs containers that read, and holds {}",
        readable.len()
    );
    assert!(
        readable
            .iter()
            .any(|bytes| gltf::read(bytes).is_ok_and(|mesh| !mesh.corner_normals.is_empty())),
        "no seed carries normals, so the placement layer's inverse transpose is unexercised"
    );
    assert!(
        readable
            .iter()
            .any(|bytes| gltf::read(bytes).is_ok_and(|mesh| mesh.triangles() > 1)),
        "no seed joins two primitives, so the concatenation path is unexercised"
    );

    // **A document that reads, not only a container that does.** The
    // whole second shape of this format is a file with no container
    // around it, and the corpus held exactly one seed of that shape that
    // succeeded -- deletable without any floor here noticing.
    assert!(
        readable.iter().any(|bytes| !bytes.starts_with(b"glTF")),
        "every seed that reads is a container, so the shape that carries its own geometry is \
         exercised by nothing"
    );
}

#[test]
#[ignore = "a census, not a gate: run it to update the numbers above"]
fn gltf_census() {
    let distinct: BTreeSet<Vec<u8>> = gltf_corpus().into_iter().collect();
    let reached: BTreeSet<&'static str> = distinct.iter().map(|b| gltf_outcome(b)).collect();
    println!(
        "{} distinct inputs, {} outcomes: {reached:?}",
        distinct.len(),
        reached.len()
    );
}

/// How many distinct `data:` URIs the corpus must still hold.
const DATA_URI_LOW_WATER: usize = 41;

/// How many distinct outcomes those seeds must still reach.
///
/// **Measured, not guessed** — `data_uri_census` below prints it. Eight
/// is every refusal this reader has plus success, which is the whole
/// vocabulary: unusually, nothing here is unreachable, because one reader
/// owns the enum and every variant has a seed.
const DATA_URI_DISTINCT_OUTCOMES: usize = 8;

/// Every refusal, each of which a seed must provoke.
///
/// The list is the enum. That is affordable here and not elsewhere: the
/// five geometry readers share one error type, so each of them names what
/// it cannot reach, while this reader can reach all of its own.
const DATA_URI_REQUIRED: [&str; 7] = [
    "NotADataUri",
    "NoPayload",
    "Unsupported",
    "BadDigit",
    "NotWholeGroups",
    "BadPadding",
    "NonCanonical",
];

// The encoder is the one the seeds were written with, shared rather than
// copied, so this gate cannot drift from the generator that fed it.
#[path = "shared/base64_encode.rs"]
mod base64_encode;

fn data_uri_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/data_uri_read")
}

fn data_uri_corpus() -> Vec<Vec<u8>> {
    let dir = data_uri_corpus_dir();
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

/// The answer a seed gets, as the name of the refusal or `Ok`.
///
/// Bytes become text the way the fuzz target does it, so a seed replays
/// here exactly as it is mutated there.
fn data_uri_outcome(bytes: &[u8]) -> &'static str {
    match data_uri::read(&String::from_utf8_lossy(bytes)) {
        Ok(_) => "Ok",
        Err(refusal) => refusal.name(),
    }
}

/// **Every committed URI answers, and everything it accepts re-encodes to
/// itself.**
///
/// The second half is the claim that matters. A decoder that quietly
/// tolerated whitespace, a missing pad, or a stray bit in the final group
/// would still return bytes — and those bytes would re-encode to
/// something other than the text it was given, which is what this catches
/// and a "did it crash" replay would not.
#[test]
fn every_recorded_uri_answers_and_re_encodes_to_itself() {
    for bytes in data_uri_corpus() {
        let text = String::from_utf8_lossy(&bytes);
        let Ok(read) = data_uri::read(&text) else {
            continue;
        };
        let payload = &text[text.find(',').expect("a URI that read has a comma") + 1..];
        assert_eq!(
            base64_encode::encode(&read.bytes),
            payload,
            "{text} decoded to bytes that spell something else"
        );
    }
}

/// The URI corpus keeps its strength.
#[test]
fn the_data_uri_corpus_still_covers_what_it_was_recorded_to_cover() {
    let inputs = data_uri_corpus();
    let distinct: BTreeSet<Vec<u8>> = inputs.iter().cloned().collect();
    assert!(
        distinct.len() >= DATA_URI_LOW_WATER,
        "the corpus holds {} distinct inputs and the floor is {DATA_URI_LOW_WATER}",
        distinct.len()
    );

    let reached: BTreeSet<&'static str> = distinct
        .iter()
        .map(|bytes| data_uri_outcome(bytes))
        .collect();
    assert!(
        reached.len() >= DATA_URI_DISTINCT_OUTCOMES,
        "the corpus reaches {} distinct answers and the floor is {DATA_URI_DISTINCT_OUTCOMES}. \
         Reached: {reached:?}",
        reached.len()
    );
    for required in DATA_URI_REQUIRED {
        assert!(
            reached.contains(required),
            "no committed seed reaches `{required}`, which is a refusal nothing is exercising. \
             Reached: {reached:?}"
        );
    }

    // **Seeds that decode, in every padding state.** A corpus of nothing
    // but refusals would leave the bit assembly and the canonical check
    // reachable only by luck, and those are the parts where a wrong
    // answer is silent rather than loud.
    let readable: Vec<&Vec<u8>> = distinct
        .iter()
        .filter(|bytes| data_uri_outcome(bytes) == "Ok")
        .collect();
    assert!(
        readable.len() >= 15,
        "the corpus needs URIs that decode, and holds {}",
        readable.len()
    );
    for pads in 0..=2_usize {
        assert!(
            readable.iter().any(|bytes| {
                let text = String::from_utf8_lossy(bytes);
                // **Counted in the payload, not in the whole URI.**
                // A media type may carry an `=` of its own --
                // `charset=x` does -- and counting those satisfied
                // this floor with seeds whose payloads hold no
                // padding at all, so the floor could have been met
                // by a corpus with no one-pad seed in it.
                let Some(comma) = text.find(',') else {
                    return false;
                };
                comma + 1 < text.len()
                    && text[comma + 1..]
                        .bytes()
                        .filter(|&byte| byte == b'=')
                        .count()
                        == pads
            }),
            "no seed that decodes has {pads} padding characters in its payload, and the three cases run \
             different arithmetic"
        );
    }
}

#[test]
#[ignore = "a census, not a gate: run it to update the numbers above"]
fn data_uri_census() {
    let distinct: BTreeSet<Vec<u8>> = data_uri_corpus().into_iter().collect();
    let reached: BTreeSet<&'static str> = distinct.iter().map(|b| data_uri_outcome(b)).collect();
    println!(
        "{} distinct inputs, {} outcomes: {reached:?}",
        distinct.len(),
        reached.len()
    );
}
