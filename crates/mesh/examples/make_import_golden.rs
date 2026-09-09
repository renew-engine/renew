//! Write the committed import golden: a source model, and what this
//! crate reads it to.
//!
//! **The model is generated here rather than borrowed**, like every
//! other fixture beside this crate. Borrowed art carries a licence and a
//! provenance question, and neither belongs in a regression guard.
//!
//! Run it to regenerate both files after a deliberate change to the
//! reader or to the canonical form:
//!
//! ```text
//! cargo run -p renew-mesh --example make_import_golden
//! ```
//!
//! **Regenerating is the whole refresh ritual**, which is the difference
//! between this golden and a rendered one. An image golden needs a
//! pinned adapter and a workflow that uploads candidates, because no two
//! machines rasterize alike. These bytes are little-endian `f32` copied
//! out of a document with no arithmetic on the way, so any machine that
//! can run this example produces the same file -- and a diff after
//! running it is a real change to what this crate reads, every time.

// A generator writes files; that is its whole job.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
// It is a tool, not engine code: a failed write should say so and stop.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use renew_mesh::{blob, format};

/// Where the committed golden lives, beside the tests that read it.
fn goldens() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

/// The source model, spelled out.
///
/// **One document that reaches every layer this crate has learned.** A
/// golden over a bare triangle would pin the geometry path and nothing
/// else, and the whole point of committing bytes is to notice a change
/// nobody meant -- so this carries, deliberately:
///
/// * **Two primitives in one mesh**, so the appending path runs and the
///   second primitive's indices have to be shifted by the first's vertex
///   count. That shift is arithmetic, and arithmetic is what a golden
///   catches.
/// * **A node with a transform**, so positions arrive somewhere other
///   than where the accessor put them.
/// * **Per-corner normals and texture coordinates**, so the optional
///   streams are present and the blob's `present` bitfield is not zero.
/// * **Indices**, so the index path runs rather than the implicit one.
/// * **A material and an image**, which change no geometry at all and
///   therefore change none of these bytes -- they are here so that the
///   document exercises the tables while the golden proves they stay out
///   of the canonical form.
///
/// The buffer carries its own bytes as a payload, so the file stands
/// alone: no container, no second file, nothing outside itself.
fn source() -> String {
    // Two triangles' worth of positions, normals and texture
    // coordinates, then six indices -- laid out as the accessors below
    // describe them, and encoded once as one buffer.
    let mut buffer: Vec<u8> = Vec::new();

    // POSITION, six vertices: a unit triangle in z = 0, and a second
    // one displaced in z so the two are not coplanar.
    for value in [
        0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.5, 1.0, 0.0, 0.5, 0.0, 1.0, 0.5,
    ] {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    // NORMAL, one per vertex: +z for the first triangle, -z for the
    // second, so the two are told apart by more than position.
    for value in [
        0.0_f32, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, //
        0.0, 0.0, -1.0, 0.0, 0.0, -1.0, 0.0, 0.0, -1.0,
    ] {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    // TEXCOORD_0, one per vertex.
    for value in [
        0.0_f32, 0.0, 1.0, 0.0, 0.0, 1.0, //
        0.25, 0.25, 0.75, 0.25, 0.25, 0.75,
    ] {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    // Two index triples, one per primitive, each numbered from its own
    // primitive's first vertex -- which is what makes the appending
    // shift observable.
    for value in [0_u16, 1, 2, 0, 1, 2] {
        buffer.extend_from_slice(&value.to_le_bytes());
    }

    let payload = base64(&buffer);
    format!(
        r#"{{"asset":{{"version":"2.0"}},
"scene":0,
"scenes":[{{"nodes":[0]}}],
"nodes":[{{"mesh":0,"translation":[2.0,0.5,-1.0]}}],
"meshes":[{{"primitives":[
{{"attributes":{{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2}},"indices":3,"material":0}},
{{"attributes":{{"POSITION":4,"NORMAL":5,"TEXCOORD_0":6}},"indices":7,"material":0}}]}}],
"accessors":[
{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},
{{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}},
{{"bufferView":2,"componentType":5126,"count":3,"type":"VEC2"}},
{{"bufferView":3,"componentType":5123,"count":3,"type":"SCALAR"}},
{{"bufferView":0,"byteOffset":36,"componentType":5126,"count":3,"type":"VEC3"}},
{{"bufferView":1,"byteOffset":36,"componentType":5126,"count":3,"type":"VEC3"}},
{{"bufferView":2,"byteOffset":24,"componentType":5126,"count":3,"type":"VEC2"}},
{{"bufferView":3,"byteOffset":6,"componentType":5123,"count":3,"type":"SCALAR"}}],
"bufferViews":[
{{"buffer":0,"byteOffset":0,"byteLength":72}},
{{"buffer":0,"byteOffset":72,"byteLength":72}},
{{"buffer":0,"byteOffset":144,"byteLength":48}},
{{"buffer":0,"byteOffset":192,"byteLength":12}}],
"buffers":[{{"byteLength":{length},"uri":"data:application/octet-stream;base64,{payload}"}}],
"textures":[{{"source":0}}],
"materials":[{{"name":"panel","pbrMetallicRoughness":{{
"baseColorFactor":[0.5,0.25,0.125,1.0],"metallicFactor":0.75,"roughnessFactor":0.5,
"baseColorTexture":{{"index":0}}}},"emissiveFactor":[0.0,0.125,0.25],"doubleSided":true}}],
"images":[{{"name":"grain","uri":"data:image/png;base64,AQIDBA=="}}]}}"#,
        length = buffer.len(),
        payload = payload,
    )
}

/// Base64, standard alphabet with padding.
///
/// Written here rather than reached for: this crate's own decoder is
/// what the golden is partly testing, and a generator that shared it
/// could encode a mistake the decoder makes and call the pair agreement.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let mut packed = 0_u32;
        for (index, byte) in group.iter().enumerate() {
            packed |= u32::from(*byte) << (16 - 8 * index);
        }
        for index in 0..4 {
            if index <= group.len() {
                let digit = (packed >> (18 - 6 * index)) & 0x3f;
                out.push(char::from(ALPHABET[digit as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn main() {
    let directory = goldens();
    std::fs::create_dir_all(&directory).expect("the goldens directory is writable");

    let document = source();
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

    println!(
        "wrote {} ({} bytes) and {} ({} bytes): {} triangles, {} positions",
        model.display(),
        document.len(),
        blob_path.display(),
        bytes.len(),
        mesh.triangles(),
        mesh.positions.len(),
    );
}
