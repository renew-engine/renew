//! The canonical mesh blob, against bytes nobody wrote on purpose.
//!
//! **This is the only reader in the tree whose format the tree also
//! writes**, and that changes what is worth fuzzing. The four format
//! readers have to survive files other people's tools produced; this one
//! has to survive a file *nobody* produced — a blob that has been
//! truncated in transit, edited in place, or fabricated whole by someone
//! who read the header layout.
//!
//! So the header is where the interesting inputs are. It is twenty
//! bytes, four of which are a corner count that **sizes an allocation**,
//! and the arrays are located by arithmetic on that count rather than by
//! scanning for anything. A reader that believed the count would reserve
//! whatever it was told to; a reader that checked it against the file's
//! own length but did so after reserving would be no better.
//!
//! Past the header, the assertion that matters is the pairing: the
//! optional arrays are flagged rather than counted, so their lengths are
//! *derived*, and a mesh whose arrays walk out of step with its
//! positions would be a wrong answer rather than a refusal.
//!
//! **The round-trip is asserted here too**, because it is the one claim
//! this format makes that the others cannot: whatever `read` accepts,
//! `write` must reproduce byte for byte. A blob that reads to a mesh
//! that writes to different bytes is a canonical form that is not
//! canonical, and it would break every cache and digest above it.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_mesh::blob;

fuzz_target!(|data: &[u8]| {
    let Ok(mesh) = blob::read(data) else {
        // A refusal is an answer. Which refusal is the suite's business
        // beside the crate; that the call returned at all is this
        // target's.
        return;
    };

    assert_eq!(
        mesh.positions.len() % 3,
        0,
        "a blob that read holds whole triangles"
    );
    assert!(!mesh.is_empty(), "a blob that read holds geometry");
    assert!(
        mesh.face_normals.is_empty() || mesh.face_normals.len() == mesh.triangles(),
        "a face normal per triangle or none: {} for {} triangles",
        mesh.face_normals.len(),
        mesh.triangles()
    );
    assert!(
        mesh.corner_normals.is_empty() || mesh.corner_normals.len() == mesh.positions.len(),
        "a normal per corner or none: {} for {} corners",
        mesh.corner_normals.len(),
        mesh.positions.len()
    );
    assert!(
        mesh.corner_texcoords.is_empty() || mesh.corner_texcoords.len() == mesh.positions.len(),
        "a coordinate per corner or none: {} for {} corners",
        mesh.corner_texcoords.len(),
        mesh.positions.len()
    );
    for value in mesh.positions.iter().chain(&mesh.corner_normals).flatten() {
        assert!(
            value.is_finite(),
            "a coordinate nothing downstream can bound reached a caller"
        );
    }

    // The canonical claim: what reads must write back to itself.
    //
    // Not `write(read(x)) == x`, which is false for good reason — the
    // input may carry trailing bytes or a header this reader normalises
    // — but `write(read(x))` read again, which must give the same mesh
    // and the same bytes.
    let again = blob::write(&mesh);
    let Ok(twice) = blob::read(&again) else {
        panic!("what this crate wrote, this crate must read");
    };
    assert_eq!(twice, mesh, "a blob read and rewritten is the same mesh");
    assert_eq!(
        blob::write(&twice),
        again,
        "and the same bytes: a canonical form that is not canonical breaks every cache above it"
    );
});
