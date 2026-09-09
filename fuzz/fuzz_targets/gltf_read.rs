//! A whole glTF asset, against bytes nobody wrote on purpose.
//!
//! **The one target in this tranche whose input needs no encoding
//! invented for it.** A glTF asset is a file -- a container, or the
//! document on its own -- so the generator writes both shapes, this
//! reads them, and the merge-time replay gate reads the same ones.
//! The accessor target had to carry six parameters in a head because an
//! accessor is not a file — this is what it looks like when the layer
//! under test takes bytes.
//!
//! # What is deliberately not seeded
//!
//! **A node hierarchy containing a cycle.** The reader refuses one, and
//! the refusal is checked by construction — every node is entered at
//! most once. But if that guard were ever removed, a seed carrying a
//! cycle would make this target *hang* rather than fail, and a hang is
//! the one outcome a harness cannot report: the run would stop making
//! progress with nothing to show for it, here and in the merge gate
//! alike.
//!
//! That is not a guess. Probing the mutation locally did exactly that —
//! one test failed, the next stopped, and a timeout ended the run. So
//! the cycle is pinned by a deterministic test beside the crate, where a
//! wedged run is a failed test instead of a silent stall.
//!
//! # What is asserted
//!
//! Everything a caller relies on and no refusal reveals: geometry that
//! comes back is whole triangles, holds coordinates a bounding box can
//! bound, and has optional arrays whose lengths match the geometry they
//! describe. A reader that returned a mesh with four positions, or with
//! one normal for two triangles, would have answered rather than
//! crashed — and every layer above it would then be reading past the
//! end of something.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_mesh::gltf;

fuzz_target!(|data: &[u8]| {
    let Ok(mesh) = gltf::read(data) else {
        // A refusal is an answer. Which refusal is the suite's business
        // beside the crate; that the call returned at all is this
        // target's.
        return;
    };

    assert_eq!(
        mesh.positions.len() % 3,
        0,
        "geometry that read is whole triangles"
    );
    assert!(!mesh.is_empty(), "an empty read is refused, not returned");
    assert_eq!(mesh.positions.len(), mesh.triangles() * 3);

    // **The optional arrays describe the geometry beside them or are
    // absent.** A mesh carrying one normal for two triangles is the
    // shape every layer above would read past the end of.
    assert!(
        mesh.face_normals.is_empty() || mesh.face_normals.len() == mesh.triangles(),
        "a face normal per triangle or none: {} for {}",
        mesh.face_normals.len(),
        mesh.triangles()
    );
    assert!(
        mesh.corner_normals.is_empty() || mesh.corner_normals.len() == mesh.positions.len(),
        "a normal per corner or none: {} for {}",
        mesh.corner_normals.len(),
        mesh.positions.len()
    );
    assert!(
        mesh.corner_texcoords.is_empty() || mesh.corner_texcoords.len() == mesh.positions.len(),
        "a coordinate per corner or none: {} for {}",
        mesh.corner_texcoords.len(),
        mesh.positions.len()
    );

    // **Every array, not just the positions.** A node transform moves
    // normals through a different matrix from the one it moves positions
    // through, so a transform large enough to overflow can produce a
    // value in one array and not the other.
    for value in mesh
        .positions
        .iter()
        .chain(&mesh.face_normals)
        .chain(&mesh.corner_normals)
        .flatten()
    {
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

    // Reading twice answers the same, which is what makes a recorded
    // corpus mean anything: a reader whose answer depended on anything
    // but its input could not be reasoned about from bytes at all.
    let again = gltf::read(data).expect("what read once reads again");
    assert_eq!(again, mesh, "the same bytes read to the same geometry");
});
