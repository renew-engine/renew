//! The import golden's source model, written once for the two targets
//! that need it.
//!
//! **Included by `#[path]` from both sides of the golden**, because
//! Cargo compiles a `tests/` subdirectory for nobody:
//!
//! * `crates/mesh/examples/make_import_golden.rs` writes it to disk.
//! * `crates/mesh/tests/golden.rs` holds the committed file to it.
//!
//! That second consumer is the point. A generator that writes both the
//! source and the bytes it reads to will happily rewrite them together,
//! so a golden compared only against its own regeneration proves the
//! reader agrees with itself and nothing more.

#[path = "base64_encode.rs"]
mod base64_encode;

/// The source model, spelled out.
///
/// **Every value in it is distinct wherever a reader could confuse
/// two.** That is the whole design rule here, and it was learned the
/// hard way: an earlier version gave each primitive one repeated normal,
/// and a reader that shuffled normals within a primitive was then
/// invisible — to this golden and to every other test in the crate.
/// A fixture whose values repeat cannot see a permutation of them.
///
/// So, deliberately:
///
/// * **Two primitives, of different sizes** — a triangle and a quad, so
///   the corner count is nine. Nine is not the `present` bitfield's
///   value, which matters because the two sit adjacent in the blob's
///   header: when both were six, swapping the fields left the committed
///   bytes identical and the golden could not see it.
/// * **Non-identity indices**, `[2,0,1]` and `[0,1,2,0,2,3]`. With
///   `[0,1,2]` the indexed and unindexed paths produce the same bytes,
///   so "the indexed path runs" was true structurally and invisible
///   observationally. The quad's list also reads two of its vertices
///   twice, which nothing else here does.
/// * **A distinct normal at every corner**, so which normal lands where
///   is observable at all.
/// * **A node with a translation and a non-uniform scale**, so positions
///   move *and* normals go through the inverse transpose. A translation
///   alone leaves that matrix the identity — which would have pinned the
///   placement path while leaving the arithmetic that made `renew-math`
///   this crate's first dependency untested. Both factors are exact
///   powers of two, so the exactness argument survives.
/// * **Per-corner normals and texture coordinates**, the two optional
///   streams a glTF document can carry, so the blob's `present` bitfield
///   is not zero. The third, per-face normals, no glTF states.
/// * **A material, a texture and an image**, which change no geometry at
///   all — they are here so the document exercises those tables while
///   the golden proves they stay out of the canonical form.
///
/// The buffer carries its own bytes as a payload, so the file stands
/// alone: no container, no second file, nothing outside itself. The
/// `POSITION` accessors state `min` and `max` because the format
/// requires them there — this reader ignores both, and a committed
/// fixture that no independent validator would accept is worth less
/// than one that would.
pub fn source() -> String {
    let mut buffer: Vec<u8> = Vec::new();

    // POSITION, the triangle: three corners in z = 0.
    push_f32(&mut buffer, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    // POSITION, the quad: four corners in z = 0.5, so the two primitives
    // are not coplanar and the second is not the first repeated.
    push_f32(
        &mut buffer,
        &[0.0, 0.0, 0.5, 1.0, 0.0, 0.5, 1.0, 1.0, 0.5, 0.0, 1.0, 0.5],
    );
    // NORMAL, the triangle: three different axes, so a shuffle shows.
    push_f32(&mut buffer, &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    // NORMAL, the quad: four more, none equal to another.
    push_f32(
        &mut buffer,
        &[
            -1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0, 0.5, 0.0, 0.0,
        ],
    );
    // TEXCOORD_0 for each, again all distinct.
    push_f32(&mut buffer, &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
    push_f32(
        &mut buffer,
        &[0.25, 0.25, 0.75, 0.25, 0.75, 0.75, 0.25, 0.75],
    );
    // Indices: a rotation for the triangle, and a quad's two triangles
    // sharing an edge, which reads vertices 0 and 2 twice each.
    push_u16(&mut buffer, &[2, 0, 1]);
    push_u16(&mut buffer, &[0, 1, 2, 0, 2, 3]);

    // **Every coordinate above is a small dyadic rational**, which is
    // not decoration: it is what makes the golden's byte comparison
    // legitimate. See `tests/golden.rs` for the argument.
    let payload = base64_encode::encode(&buffer);
    format!(
        r#"{{"asset":{{"version":"2.0"}},
"scene":0,
"scenes":[{{"nodes":[0]}}],
"nodes":[{{"mesh":0,"translation":[2.0,0.5,-1.0],"scale":[2.0,1.0,0.5]}}],
"meshes":[{{"primitives":[
{{"attributes":{{"POSITION":0,"NORMAL":2,"TEXCOORD_0":4}},"indices":6,"material":0}},
{{"attributes":{{"POSITION":1,"NORMAL":3,"TEXCOORD_0":5}},"indices":7,"material":0}}]}}],
"accessors":[
{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3",
"min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}},
{{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3",
"min":[0.0,0.0,0.5],"max":[1.0,1.0,0.5]}},
{{"bufferView":2,"componentType":5126,"count":3,"type":"VEC3"}},
{{"bufferView":3,"componentType":5126,"count":4,"type":"VEC3"}},
{{"bufferView":4,"componentType":5126,"count":3,"type":"VEC2"}},
{{"bufferView":5,"componentType":5126,"count":4,"type":"VEC2"}},
{{"bufferView":6,"componentType":5123,"count":3,"type":"SCALAR"}},
{{"bufferView":7,"componentType":5123,"count":6,"type":"SCALAR"}}],
"bufferViews":[
{{"buffer":0,"byteOffset":0,"byteLength":36}},
{{"buffer":0,"byteOffset":36,"byteLength":48}},
{{"buffer":0,"byteOffset":84,"byteLength":36}},
{{"buffer":0,"byteOffset":120,"byteLength":48}},
{{"buffer":0,"byteOffset":168,"byteLength":24}},
{{"buffer":0,"byteOffset":192,"byteLength":32}},
{{"buffer":0,"byteOffset":224,"byteLength":6}},
{{"buffer":0,"byteOffset":230,"byteLength":12}}],
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

fn push_f32(buffer: &mut Vec<u8>, values: &[f32]) {
    for value in values {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
}

fn push_u16(buffer: &mut Vec<u8>, values: &[u16]) {
    for value in values {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
}

/// FNV-1a over the bytes, for the provenance sidecar.
///
/// **The same digest the other goldens' sidecars carry**, and it is
/// there for the same reason: without it the sidecar describes a file
/// nothing binds it to, and an artifact regenerated without its
/// provenance drifts silently.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
