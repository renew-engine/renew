//! The PLY reader, against bytes nobody wrote on purpose.
//!
//! **The header is a schema an attacker writes, which is why this target
//! matters more than the STL one beside it.** An STL header is eighty
//! fixed bytes and a count; a PLY header declares elements, how many
//! rows each has, what columns those rows carry, how wide each column is
//! and how long each list is — and every one of those numbers multiplies
//! into an offset the body is read at. A reader that trusts the product
//! reads wherever the file tells it to.
//!
//! So the interesting inputs here are not corrupt bodies. They are
//! *coherent headers describing bodies that are not there*: a count of
//! four billion, a list length that overruns the file, a property whose
//! width pushes the cursor past the end on the last row of the last
//! element. Those are what the seeds start the fuzzer near.
//!
//! **A mesh that reads is then checked against its own type's
//! invariants**, because those are what the reader promises. The index
//! check is the one this format adds over STL: a face names vertices by
//! number, and a number one past the end is the difference between a
//! mesh and a read past a buffer.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_mesh::ply;

fuzz_target!(|data: &[u8]| {
    // Asked of every input, including the ones that are not PLY at all:
    // it is a public function and a caller may hand it anything.
    let _ = ply::looks_like(data);

    let Ok(mesh) = ply::read(data) else {
        // A refusal is an answer. Which refusal is the suite's business
        // beside the crate; that the call returned at all is this
        // target's.
        return;
    };

    assert_eq!(
        mesh.positions.len() % 3,
        0,
        "a mesh that read has whole triangles"
    );
    assert!(!mesh.is_empty(), "a mesh that read has geometry in it");
    assert!(
        mesh.face_normals.is_empty(),
        "this reader stores no normals: PLY carries them per vertex and a triangle here carries one"
    );
    for value in mesh.positions.iter().flatten() {
        assert!(
            value.is_finite(),
            "a coordinate nothing downstream can bound reached a caller"
        );
    }

    // Walks both arrays together, so a disagreement between their
    // lengths is a panic here rather than a wrong answer later.
    let _ = mesh.winding_disagreements();
});
