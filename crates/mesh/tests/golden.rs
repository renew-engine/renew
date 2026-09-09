//! A model, and the bytes this crate reads it to, both committed.
//!
//! **This is the one gate that pins the whole path in one artifact.**
//! `tests/gltf.rs` asserts values too - `a_nodes_transform_moves_its_geometry`
//! and its neighbours compare positions by bits against fixtures written
//! in the same file - but each of those pins one layer against a
//! document written to exercise it. This pins every layer at once
//! against bytes that were *committed*, so a change no single assertion
//! covers still moves a byte and still fails.
//!
//! What that catches, which the corpus gates cannot: a reader that
//! answers confidently and wrongly. A document coming back with its
//! second primitive dropped, or its texture coordinates attributed to
//! the wrong corners, answers perfectly well - `Ok`, no panic, nothing
//! read past the end.
//!
//! # Why exact comparison is legitimate for *this* document
//!
//! Not for the path in general, and the difference matters.
//!
//! This document's accessors are all `componentType: 5126`, so
//! `View::float` copies each coordinate out with `from_le_bytes` and
//! never converts or normalises - a document of `5121` would take a
//! division instead. Its node states a translation only, so the composed
//! matrix and the inverse-transpose that carries the normals are exact.
//! And every coordinate in the source is a small dyadic rational whose
//! product with that matrix rounds to itself.
//!
//! **The placement path does multiply and add** - a 4x4 product per
//! node, twelve multiplies and nine adds per vertex, and a reciprocal
//! inside `normal_matrix`. `tests/place.rs` compares within a tolerance
//! for exactly that reason, and says so. This fixture is chosen so that
//! it need not.
//!
//! So what the comparison pins is narrower than "the reader does no
//! arithmetic" and more useful: **that this document still reads to
//! these bytes.** A reader which began to normalise, average or reorder
//! would move one, and that is the day this file earns its keep.
//!
//! The bytes are stable across machines because the format converts
//! explicitly - `blob::write` writes `to_le_bytes` on every target and
//! the reader takes `from_le_bytes` - not because memory layout is
//! universal. A big-endian target spells an `f32` differently and reads
//! this blob identically.
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

// **What was probed, and where each probe lands.** A golden nobody has
// watched fail is a file rather than a gate, so each of these was run:
//
// * A bit flipped in the committed `panel.msh` -- red in the byte
//   comparison, by offset.
// * The node's translation removed from the committed `panel.gltf` --
//   red twice, in the byte comparison and in the vacuity check.
// * The second primitive removed from the committed `panel.gltf` -- red
//   twice, including by length.
//
// **Editing the generator and re-running it is not one of them, and
// would not be a probe at all**: it rewrites the source and the blob
// together, so a comparison of one against the other stays green. That
// is what `the_committed_source_is_still_the_one_this_code_describes`
// is for.

use std::path::PathBuf;

use renew_mesh::{blob, format, gltf};

#[path = "shared/import_golden_source.rs"]
mod import_golden_source;

fn goldens() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

/// Compare coordinates by bits.
///
/// **The honest comparison for this fixture, and not a way around the
/// lint.** The placement path multiplies and adds in general -- which is
/// why `tests/place.rs` uses a tolerance and says so -- but every
/// operand in this document is a small dyadic rational under a
/// translation, so the arithmetic is exact and a tolerance would hide
/// precisely the change this gate exists to notice.
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

    assert_eq!(mesh.triangles(), 3, "a triangle and a quad's two, appended");
    assert_eq!(mesh.positions.len(), 9, "nine corners, not six");
    assert_eq!(
        mesh.corner_normals.len(),
        9,
        "the per-corner normal stream is present"
    );
    assert_eq!(
        mesh.corner_texcoords.len(),
        9,
        "and the per-corner texture coordinates"
    );

    // **Nine corners, and the `present` bitfield is six.** Those two
    // numbers sit adjacent in the blob's header, so a fixture where they
    // were equal left the committed bytes identical under swapping the
    // fields -- and this gate could not see it.
    assert_ne!(
        mesh.positions.len(),
        6,
        "the corner count must not equal the `present` bitfield, or the header's two \
         adjacent fields become indistinguishable in the committed bytes"
    );

    // Where each coordinate *lands* is asserted by value in
    // `the_committed_bytes_decode_to_the_model_they_should`, against
    // numbers worked out by hand. Repeating a subset of them here would
    // be two places to update and one of them would go stale -- which is
    // exactly what happened the first time this fixture changed.
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

    // **The material's numbers, not merely its name.** They are in the
    // committed source and nothing else here reads them, so a reader
    // that swapped metallic for roughness would pass every other check
    // in this file.
    let material = &tables.materials[0];
    assert_eq!(material.metallic.to_bits(), 0.75_f32.to_bits());
    assert_eq!(material.roughness.to_bits(), 0.5_f32.to_bits());
    assert!(material.double_sided);
    for (got, want) in material.base_color.iter().zip(&[0.5_f32, 0.25, 0.125, 1.0]) {
        assert_eq!(got.to_bits(), want.to_bits(), "base colour");
    }
    for (got, want) in material.emissive.iter().zip(&[0.0_f32, 0.125, 0.25]) {
        assert_eq!(got.to_bits(), want.to_bits(), "emissive");
    }

    // **And none of it is in the blob.** The canonical form is a header
    // and the geometry streams -- there is nowhere in it for a material
    // to be, which is why a change that started folding one in would
    // move a committed byte. What the blob *does* hold is asserted by
    // value in `the_committed_bytes_decode_to_the_model_they_should`;
    // repeating a count here is a second place to update, and it went
    // stale the first time the fixture changed.
    let mesh = blob::read(&read("panel.msh")).expect("what this crate wrote, this crate reads");
    assert!(
        mesh.corner_texcoords.len() == mesh.positions.len(),
        "the streams the blob carries are per-corner, and a material is not among them"
    );
}

/// **The committed source is still the document this code describes.**
///
/// Without this, "any machine that runs the generator reproduces these
/// files" is a claim nothing checks -- and the byte comparison above
/// cannot check it, because regenerating rewrites both sides together.
/// A change to the model that was never committed fails here.
#[test]
fn the_committed_source_is_still_the_one_this_code_describes() {
    let committed = read("panel.gltf");
    let described = import_golden_source::source();
    assert_eq!(
        String::from_utf8_lossy(&committed),
        described,
        "`panel.gltf` and the code that describes it disagree. Rerun the generator and \
         commit what it writes."
    );
}

/// The sidecar still describes the blob it sits beside.
///
/// The digest is what binds them, and the rendered goldens' sidecars
/// carry one for the same reason: without it a regenerated artifact
/// leaves its provenance describing whatever the file used to be.
#[test]
fn the_sidecar_still_describes_the_committed_blob() {
    let sidecar = String::from_utf8(read("panel.provenance.txt")).expect("the sidecar is text");
    let digest = import_golden_source::fnv1a_64(&read("panel.msh"));
    let stated = format!("fnv1a-64 of panel.msh: {digest:#018x}");
    assert!(
        sidecar.contains(&stated),
        "the sidecar does not carry the committed blob's digest; expected `{stated}`"
    );
}

/// **What the committed bytes mean, against values derived by hand.**
///
/// This is the test that stops the refresh command laundering a broken
/// reader. Every other check here is symmetric: the byte comparison
/// holds the blob to what the reader produces *now*, and regenerating
/// rewrites both sides together — so a reader that started shifting
/// texture coordinates by one corner would fail, be "fixed" by one
/// documented command, and land as a binary diff nobody can read. The
/// values below are worked out from the document by hand, so a refresh
/// that changes what the model *means* fails here instead.
///
/// The arithmetic, for the next reader to check rather than trust:
///
/// * The node scales by `(2, 1, 0.5)` and then translates by
///   `(2, 0.5, -1)`, so a source vertex `(x, y, z)` lands at
///   `(2x + 2, y + 0.5, z/2 - 1)`.
/// * Normals go through the inverse transpose of that scale,
///   `(0.5, 1, 2)`, and are **not** renormalised — `place` moves them
///   and checks they are finite, nothing more.
/// * The triangle's indices are `[2, 0, 1]` and the quad's are
///   `[0, 1, 2, 0, 2, 3]`, so the corners arrive in that order and the
///   quad's vertices 0 and 2 arrive twice each.
///
/// Every factor is a power of two, so none of it rounds.
#[test]
fn the_committed_bytes_decode_to_the_model_they_should() {
    let mesh = blob::read(&read("panel.msh")).expect("the committed blob reads");

    // The triangle, in index order `[2, 0, 1]`, then the quad in
    // `[0, 1, 2, 0, 2, 3]`.
    let want = [
        [2.0, 1.5, -1.0],
        [2.0, 0.5, -1.0],
        [4.0, 0.5, -1.0],
        [2.0, 0.5, -0.75],
        [4.0, 0.5, -0.75],
        [4.0, 1.5, -0.75],
        [2.0, 0.5, -0.75],
        [4.0, 1.5, -0.75],
        [2.0, 1.5, -0.75],
    ];
    assert_eq!(
        mesh.positions.len(),
        want.len(),
        "nine corners: a triangle and a quad"
    );
    for (index, want) in want.into_iter().enumerate() {
        same(mesh.positions[index], want, &format!("position {index}"));
    }

    // **Every one different**, which is the point: a reader that
    // shuffled normals within a primitive was invisible when each
    // primitive carried one repeated normal.
    let want = [
        [0.0, 0.0, 2.0],
        [0.5, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [-0.5, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, -2.0],
        [-0.5, 0.0, 0.0],
        [0.0, 0.0, -2.0],
        [0.25, 0.0, 0.0],
    ];
    assert_eq!(mesh.corner_normals.len(), want.len());
    for (index, want) in want.into_iter().enumerate() {
        same(mesh.corner_normals[index], want, &format!("normal {index}"));
    }

    // Texture coordinates pass through untouched, which is the point of
    // asserting them: a reader that attributed them to the wrong corners
    // would answer perfectly well and fail only here.
    let want: [[f32; 2]; 9] = [
        [0.0, 1.0],
        [0.0, 0.0],
        [1.0, 0.0],
        [0.25, 0.25],
        [0.75, 0.25],
        [0.75, 0.75],
        [0.25, 0.25],
        [0.75, 0.75],
        [0.25, 0.75],
    ];
    assert_eq!(mesh.corner_texcoords.len(), want.len());
    for (index, want) in want.into_iter().enumerate() {
        let got = mesh.corner_texcoords[index];
        assert_eq!(
            (got[0].to_bits(), got[1].to_bits()),
            (want[0].to_bits(), want[1].to_bits()),
            "texture coordinate {index} is {got:?}, not {want:?}"
        );
    }

    // No glTF states a per-face normal, so this stream must be empty --
    // a reader that started deriving one would be inventing a value the
    // file did not carry.
    assert!(
        mesh.face_normals.is_empty(),
        "a per-face normal appeared, and no glTF document states one"
    );
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
