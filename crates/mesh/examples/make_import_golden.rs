//! Write the committed import golden: a source model, the bytes this
//! crate reads it to, and the sidecar that describes both.
//!
//! **The model is generated here rather than borrowed**, like every
//! other fixture beside this crate. Borrowed art carries a licence and a
//! provenance question, and neither belongs in a regression guard. Its
//! definition lives in `tests/shared/import_golden_source.rs`, shared
//! with the gate that reads it -- so the gate can hold the committed
//! file to the same code this one writes from.
//!
//! Run it to regenerate all three files after a deliberate change to the
//! reader or to the canonical form:
//!
//! ```text
//! cargo run -p renew-mesh --example make_import_golden
//! ```
//!
//! **Regenerating is the whole refresh ritual**, which is the difference
//! between this golden and a rendered one. An image golden needs a
//! pinned adapter and a workflow that uploads candidates, because no two
//! machines rasterize alike. Every operand here is a small dyadic
//! rational and the format converts endianness explicitly, so any
//! machine that can run this example produces the same files -- and a
//! diff after running it is a real change to what this crate reads.

// A generator writes files; that is its whole job.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
// It is a tool, not engine code: a failed write should say so and stop.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use renew_mesh::{blob, format};

#[path = "../tests/shared/import_golden_source.rs"]
mod import_golden_source;

/// Where the committed golden lives, beside the tests that read it.
fn goldens() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

/// The sidecar, written from the bytes rather than beside them.
///
/// **It carries a digest of the blob**, as the rendered goldens'
/// sidecars do, and for the same reason: a sidecar nothing binds to its
/// artifact describes whatever the artifact used to be. Writing it here
/// rather than by hand is what keeps the binding true.
fn provenance(document: &str, blob_bytes: &[u8]) -> String {
    format!(
        "panel.gltf \u{2014} a glTF 2.0 document, generated, carrying its own buffer as a\n\
         base64 payload so it stands alone: no container, no second file. {document} bytes.\n\
         \n\
         panel.msh \u{2014} this crate's canonical form, as `blob::write` produces it from\n\
         `format::detect(panel.gltf).read(..)`. {blob} bytes.\n\
         fnv1a-64 of panel.msh: {digest:#018x}\n\
         \n\
         all three files written by: crates/mesh/examples/make_import_golden.rs\n\
         the document itself is defined in: crates/mesh/tests/shared/import_golden_source.rs\n\
         refresh with: cargo run -p renew-mesh --example make_import_golden\n\
         \n\
         what the source reaches, deliberately:\n\
         \x20 two primitives in one mesh, so `place::append` runs and each primitive's\n\
         \x20 corners are resolved against its own accessors before being concatenated\n\
         \x20 a node with a translation, so positions arrive somewhere other than\n\
         \x20 where the accessor put them, and the placement path runs\n\
         \x20 indices, so the indexed path runs rather than the implicit one\n\
         \x20 per-corner normals and texture coordinates - the two optional streams a\n\
         \x20 glTF document can carry - so the blob's `present` bitfield is not zero\n\
         \x20 a material, a texture and an image, which change no geometry: they are\n\
         \x20 here so the golden proves they stay out of the canonical form\n\
         \n\
         what it does not reach: per-face normals (no glTF states them), the binary\n\
         container, the implicit unindexed path, byte strides, sparse accessors, and\n\
         component types other than 5126 and 5123.\n\
         \n\
         comparison: exact, and legitimate for THIS document rather than for the path\n\
         in general. Every accessor it reads a coordinate through is componentType\n\
         5126, so each one is copied out with from_le_bytes and never converted or\n\
         normalised - the two index accessors are 5123, and an index selects rather\n\
         than computes. The node states a translation and a scale, both exact powers\n\
         of two, so the composed matrix and its inverse-transpose are exact; and\n\
         every coordinate in the source is a small dyadic rational\n\
         whose product with that matrix rounds to itself. The placement path does\n\
         multiply and add - see crates/mesh/tests/place.rs, which compares within a\n\
         tolerance for exactly that reason - and this fixture is chosen so that it\n\
         need not. What the comparison pins is that a reader which began to\n\
         normalise, average or reorder would move a byte.\n\
         \n\
         endianness: the blob is little-endian on every target, not native -\n\
         `blob::write` writes to_le_bytes and the reader takes from_le_bytes - so\n\
         these bytes are the same on a machine of either endianness.\n\
         \n\
         refresh ritual: none needed. Unlike a rendered golden, where no two adapters\n\
         rasterize alike and candidates are uploaded from a pinned lane and adopted by\n\
         hand, any machine that runs the generator reproduces these files. The gate\n\
         checks that the committed source still matches the code above, so a\n\
         regeneration that was never committed is a failure rather than a surprise.\n",
        document = document.len(),
        blob = blob_bytes.len(),
        digest = import_golden_source::fnv1a_64(blob_bytes),
    )
}

fn main() {
    let directory = goldens();
    std::fs::create_dir_all(&directory).expect("the goldens directory is writable");

    let document = import_golden_source::source();
    let model = directory.join("panel.gltf");
    std::fs::write(&model, document.as_bytes()).expect("the source model is writable");

    // **Read through the same door a caller uses.** Detecting the format
    // from the bytes rather than calling `gltf::read` directly is part
    // of what this golden pins: a detector that stopped recognising a
    // document would change these bytes.
    let found = format::detect(document.as_bytes());
    let mesh = found
        .read(document.as_bytes())
        .expect("the golden's source carries geometry")
        .expect("the golden's source reads");
    let bytes = blob::write(&mesh);

    let blob_path = directory.join("panel.msh");
    std::fs::write(&blob_path, &bytes).expect("the blob is writable");

    let sidecar = directory.join("panel.provenance.txt");
    std::fs::write(&sidecar, provenance(&document, &bytes)).expect("the sidecar is writable");

    println!(
        "wrote {} ({} bytes), {} ({} bytes) and {}: {} triangles, {} positions",
        model.display(),
        document.len(),
        blob_path.display(),
        bytes.len(),
        sidecar.display(),
        mesh.triangles(),
        mesh.positions.len(),
    );
}
