//! The OBJ reader, against bytes nobody wrote on purpose.
//!
//! **This is the first reader here whose whole input is a grammar**, and
//! that changes where the interesting failures live. STL dispatches on
//! arithmetic and PLY on a header; an OBJ has neither, so every line is
//! a fresh decision and there is no point after which the file is
//! "understood". A mutation anywhere is a mutation in the parser's
//! working set.
//!
//! The inputs that matter are therefore the ones a random mutator
//! reaches essentially never: **a face whose indices are coherent
//! against streams that are almost long enough.** Off-by-one at either
//! end, a negative index reaching one past the beginning, a face that
//! names a texture coordinate when the file declared none — each is one
//! character away from a file that reads, and each is a different branch
//! of the resolver. The seeds put the fuzzer's starting point there.
//!
//! **A mesh that reads is checked against its own type's invariants.**
//! The per-corner arrays are the new promise this format brings: each is
//! either empty or exactly as long as the positions, because a corner
//! without a normal has no normal that could be invented for it. An
//! input that breaks that pairing has produced a mesh whose arrays walk
//! out of step, which is a wrong answer rather than a refusal.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_mesh::obj;

fuzz_target!(|data: &[u8]| {
    // Asked of every input, including the ones that are not OBJ at all:
    // it is a public function and a caller may hand it anything.
    let _ = obj::looks_like(data);

    // A second public entry point taking the same untrusted bytes. It
    // answers for every one of them too, and what it returns is bounded
    // by what it was given: every name is a run copied out of the input,
    // so a file cannot ask for more memory than it spends.
    if let Ok(found) = obj::materials(data) {
        let returned: usize = found
            .libraries
            .iter()
            .chain(&found.used)
            .map(String::len)
            .sum();
        assert!(
            returned <= data.len(),
            "{returned} bytes of names came out of {} bytes of file",
            data.len()
        );
    }

    let Ok(mesh) = obj::read(data) else {
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
        "this format states no normal for a face as a whole"
    );
    assert!(
        mesh.corner_normals.is_empty() || mesh.corner_normals.len() == mesh.positions.len(),
        "normals are one per corner or none at all: {} normals for {} corners",
        mesh.corner_normals.len(),
        mesh.positions.len()
    );
    assert!(
        mesh.corner_texcoords.is_empty() || mesh.corner_texcoords.len() == mesh.positions.len(),
        "coordinates are one per corner or none at all: {} coordinates for {} corners",
        mesh.corner_texcoords.len(),
        mesh.positions.len()
    );
    for value in mesh.positions.iter().chain(&mesh.corner_normals).flatten() {
        assert!(
            value.is_finite(),
            "a coordinate nothing downstream can bound reached a caller"
        );
    }
    for value in mesh.corner_texcoords.iter().flatten() {
        assert!(
            value.is_finite(),
            "a texture coordinate nothing downstream can bound reached a caller"
        );
    }

    // Walks the positions and the face normals together, so a
    // disagreement between their lengths is a panic here rather than a
    // wrong answer later.
    let _ = mesh.winding_disagreements();
});
