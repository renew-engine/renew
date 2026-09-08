//! The STL reader, against bytes nobody wrote on purpose.
//!
//! The reader takes a byte string from wherever a caller got one and
//! hands back either a mesh or a named refusal. Both are answers; a
//! panic, a hang, an allocation sized by a number the file chose, or a
//! read past the end are not, and this target exists to look for those.
//!
//! **The bytes go in unfiltered, invalid UTF-8 included.** The format
//! has two encodings and a file does not label which it is, so the
//! reader chooses between them by arithmetic on the length — which means
//! the dispatch itself is untrusted-input handling and has to be fuzzed
//! as such. A target that filtered to valid UTF-8 would only ever
//! exercise the text half.
//!
//! **A mesh that reads is then checked against its own type's
//! invariants**, because those are what the reader promises and a
//! promise nothing checks is a comment. The positions must divide into
//! triangles, the normals must be one per triangle or absent entirely,
//! and no coordinate may be a value nothing downstream can bound. The
//! winding count is walked too: it reads every triangle and every
//! normal, so it is where an off-by-one between the two arrays would
//! show up as a panic rather than as a wrong number.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_mesh::stl;

fuzz_target!(|data: &[u8]| {
    let Ok(mesh) = stl::read(data) else {
        // A refusal is an answer. Which refusal is the suite's business
        // beside the crate; that the call returned at all is this
        // target's.
        return;
    };

    // The invariants the reader's own documentation states. Asserted
    // rather than assumed: this is the only place they meet an input
    // nobody chose.
    assert_eq!(
        mesh.positions.len() % 3,
        0,
        "a mesh that read has whole triangles"
    );
    assert!(!mesh.is_empty(), "a mesh that read has geometry in it");
    assert!(
        mesh.normals.is_empty() || mesh.normals.len() == mesh.triangles(),
        "normals are one per triangle or none at all: {} normals for {} triangles",
        mesh.normals.len(),
        mesh.triangles()
    );
    for value in mesh.positions.iter().chain(&mesh.normals).flatten() {
        assert!(
            value.is_finite(),
            "a coordinate nothing downstream can bound reached a caller"
        );
    }

    // Walks both arrays together, so a disagreement between their
    // lengths is a panic here rather than a wrong answer later.
    let _ = mesh.winding_disagreements();
});
