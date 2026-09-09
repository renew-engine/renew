//! A model, and the bytes this crate reads it to, both committed.
//!
//! Every other gate beside this crate asks whether a reader *answers* —
//! `Ok` or a named refusal, no panic, no read past the end. That is the
//! right question for a corpus of hostile inputs and it is nearly blind
//! to a reader that answers confidently and wrongly. A document that
//! comes back with its second primitive dropped, or its transform not
//! applied, or its texture coordinates shifted by one vertex, answers
//! perfectly well.
//!
//! **So one model is committed beside the bytes it reads to**, and the
//! comparison is exact.
//!
//! # Why exact comparison is legitimate here
//!
//! The canonical form is little-endian `f32` and `u32` arrays copied out
//! of the document, and the path between them does no arithmetic on a
//! coordinate beyond the node transform the document itself states. There
//! is no rounding to differ over, no iteration order that reaches the
//! bytes, and no platform difference in how an `f32` is spelled in
//! memory. **That is the claim this file pins**, and the day it stops
//! being true — a reader that normalises, or averages, or reorders — is
//! the day this golden earns its keep by failing.
//!
//! # Refreshing it
//!
//! ```text
//! cargo run -p renew-mesh --example make_import_golden
//! ```
//!
//! Any machine that can run that produces the same two files, which is
//! why this golden needs no candidate ritual and no pinned lane — unlike
//! a rendered one, where no two adapters rasterize alike. A diff after
//! running it is a real change to what this crate reads.

// The tripwire ban on filesystem access protects engine code; comparing
// against committed artifacts is this harness's whole job.
#![allow(clippy::disallowed_methods)]
#![allow(clippy::disallowed_types)]
// A missing golden is a broken checkout, not a condition to recover from.
#![allow(clippy::panic, clippy::expect_used)]

use std::path::PathBuf;

use renew_mesh::{blob, format, gltf};

fn goldens() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

/// Compare coordinates by bits.
///
/// **The honest comparison here, and not a way around the lint.** This
/// file's whole argument is that the path from document to canonical
/// form does no arithmetic a tolerance would need to absorb -- the one
/// exception being the node transform, which is a translation, and a
/// translation of an exact value by an exact value is exact. A tolerance
/// would hide precisely the change this gate exists to notice.
fn same(got: [f32; 3], want: [f32; 3], what: &str) {
    for (index, (left, right)) in got.iter().zip(&want).enumerate() {
        assert_eq!(
            left.to_bits(),
            right.to_bits(),
            "{what}: component {index} is {left}, not {right}"
        );
    }
}

fn read(name: &str) -> Vec<u8> {
    let path = goldens().join(name);
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "the committed golden `{}` is unreadable: {error}. It is committed beside this test; \
             a checkout missing it is broken rather than merely untested.",
            path.display()
        )
    })
}

/// **The whole claim, in one comparison.**
///
/// Probed by changing one coordinate in the generator: red. Probed by
/// dropping the second primitive: red. Probed by removing the node's
/// translation: red.
#[test]
fn the_committed_model_reads_to_the_committed_bytes() {
    let document = read("panel.gltf");
    let expected = read("panel.msh");

    // Through the detector, not straight to the glTF reader: a detector
    // that stopped recognising this document would change these bytes,
    // and that is a regression this golden should catch.
    let found = format::detect(&document);
    assert_eq!(found.name(), "gltf", "the golden's source is a document");

    let mesh = found
        .read(&document)
        .expect("a document that carries geometry")
        .expect("the golden's source reads");
    let actual = blob::write(&mesh);

    assert_eq!(
        actual.len(),
        expected.len(),
        "the canonical form changed length: {} bytes now, {} committed. If that was meant, \
         rerun the generator; if it was not, this is the change to look at.",
        actual.len(),
        expected.len()
    );
    // Compared by index rather than as two slices, so a failure names
    // the first byte that differs instead of printing two blobs.
    for (index, (got, want)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(
            got, want,
            "byte {index} of the canonical form is {got}, committed as {want}"
        );
    }
}

/// **The golden is not vacuous**, which a byte comparison alone cannot
/// promise: two empty files match perfectly.
///
/// Each of these is a layer the document was built to reach, asserted
/// here so that a generator quietly producing less would fail loudly
/// rather than committing a smaller golden that still matches itself.
#[test]
fn the_golden_reaches_every_layer_it_was_built_for() {
    let document = read("panel.gltf");
    let mesh = format::detect(&document)
        .read(&document)
        .expect("geometry")
        .expect("reads");

    assert_eq!(mesh.triangles(), 2, "both primitives survive the append");
    assert_eq!(mesh.positions.len(), 6, "three corners each, appended");
    assert_eq!(
        mesh.corner_normals.len(),
        6,
        "the per-corner normal stream is present"
    );
    assert_eq!(
        mesh.corner_texcoords.len(),
        6,
        "and the per-corner texture coordinates"
    );

    // **The transform is applied**, which is the one place this path
    // does arithmetic on a coordinate. The document places the node at
    // (2, 0.5, -1) and the first vertex at the origin.
    same(
        mesh.positions[0],
        [2.0, 0.5, -1.0],
        "the node's translation reached the vertex",
    );
    // And the second primitive is displaced in z by its own geometry,
    // so the two are not the same triangle written twice.
    same(
        mesh.positions[3],
        [2.0, 0.5, -0.5],
        "the second primitive is where its own coordinates put it",
    );
}

/// **What the canonical form deliberately does not carry.**
///
/// The source states a material and an image. Neither is geometry, and
/// neither reaches the blob — so a change that started folding them in
/// would change the committed bytes and fail the comparison above. This
/// test says *why* that comparison would fail, so the next reader does
/// not have to work it out from a byte offset.
#[test]
fn the_tables_are_read_from_the_same_document_and_stay_out_of_the_blob() {
    let document = read("panel.gltf");
    let tables = gltf::tables(&document, gltf::ImageBytes::Counted)
        .expect("the golden's source states a material and an image");

    assert_eq!(tables.materials.len(), 1, "the material is there to read");
    assert_eq!(tables.materials[0].name.as_deref(), Some("panel"));
    assert_eq!(tables.textures, [Some(0)], "and the join between them");
    assert_eq!(tables.images.len(), 1, "and the image");
    assert_eq!(tables.images[0].len, 4);

    // The blob is exactly the geometry: a header, then the streams the
    // mesh carries. Nothing here leaves room for a material.
    let expected = read("panel.msh");
    let mesh = blob::read(&expected).expect("what this crate wrote, this crate reads");
    assert_eq!(mesh.triangles(), 2);
    assert_eq!(mesh.positions.len(), 6);
}

/// The committed blob is what this crate reads back, not merely what it
/// wrote once.
#[test]
fn the_committed_blob_round_trips() {
    let expected = read("panel.msh");
    let mesh = blob::read(&expected).expect("the committed blob reads");
    assert_eq!(
        blob::write(&mesh),
        expected,
        "writing what was read gives the bytes back"
    );
}
