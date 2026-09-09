//! Write the glTF reader's seed corpus, in both shapes it reads.
//!
//! **Every byte here is built by this program**, document and geometry
//! alike. A container wraps somebody's model, so a downloaded one would
//! arrive with a licence and an author; these carry a triangle whose
//! coordinates are written out below.
//!
//! A fuzzer finds the four-byte magic quickly and the chunk framing soon
//! after. What it will not stumble on is **a document that parses and
//! then means something**: a scene naming a node naming a mesh naming a
//! primitive naming an accessor naming a buffer view, with every index
//! landing on a row that exists and every length agreeing with the chunk
//! it addresses. Six layers have to be right at once before the reader
//! reaches the arithmetic worth testing, and a random walk essentially
//! never gets there. These seeds start inside.
//!
//! # The seed that is not here
//!
//! **A node hierarchy containing a cycle.** The reader refuses one, and
//! the guard is structural — every node is entered at most once. If that
//! guard were ever removed, a seed carrying a cycle would make the
//! fuzzer *and the merge-time replay gate* stop making progress rather
//! than fail, and a stall is the one outcome neither can report.
//! Probing that mutation locally did exactly that: one test failed, the
//! next stopped, and a timeout ended the run. So the cycle is pinned by
//! a deterministic test beside the crate instead.
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-mesh --example make_gltf_corpus
//! ```
//!
//! Existing files are left alone. The fuzzer adds its own finds to this
//! directory over time, and this program must never delete them.

// The crate bans filesystem access because the library never touches a
// file -- a caller that reads one owns it, and owns the bound on reading
// it. This program is that caller: writing the corpus is its whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes, never a path.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;
use std::process::ExitCode;

/// `JSON` and `BIN\0`, little-endian, as the container stores them.
const JSON: u32 = 0x4E4F_534A;
const BIN: u32 = 0x004E_4942;

/// Nine floats: one triangle in the plane.
fn triangle() -> Vec<u8> {
    [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// A triangle, then three normals, then three texture coordinates, then
/// three indices — the layout a real exporter writes into one chunk.
fn furnished() -> Vec<u8> {
    let mut out = triangle();
    for value in [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for index in [0u16, 1, 2] {
        out.extend_from_slice(&index.to_le_bytes());
    }
    out
}

/// Wrap a document and a payload in a container, padding both and
/// writing a truthful total length.
fn container(document: &str, binary: &[u8]) -> Vec<u8> {
    let mut json = document.as_bytes().to_vec();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut payload = binary.to_vec();
    while !payload.len().is_multiple_of(4) {
        payload.push(0);
    }

    let mut out = b"glTF".to_vec();
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(json.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&JSON.to_le_bytes());
    out.extend_from_slice(&json);
    if !payload.is_empty() {
        out.extend_from_slice(
            &u32::try_from(payload.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        out.extend_from_slice(&BIN.to_le_bytes());
        out.extend_from_slice(&payload);
    }
    let total = u32::try_from(out.len()).unwrap_or(u32::MAX);
    out[8..12].copy_from_slice(&total.to_le_bytes());
    out
}

/// One triangle, one node, one scene: the smallest thing that reads.
const SIMPLEST: &str = r#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[0]}],
"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}],
"buffers":[{"byteLength":36}],
"bufferViews":[{"buffer":0,"byteLength":36}]}"#;

/// Everything named: normals, coordinates and an index stream.
const EVERYTHING: &str = r#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[0]}],
"nodes":[{"mesh":0}],"meshes":[{"primitives":[{
"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3}]}],
"accessors":[
{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},
{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"},
{"bufferView":2,"componentType":5126,"count":3,"type":"VEC2"},
{"bufferView":3,"componentType":5123,"count":3,"type":"SCALAR"}],
"buffers":[{"byteLength":102}],
"bufferViews":[
{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":36},
{"buffer":0,"byteOffset":72,"byteLength":24},{"buffer":0,"byteOffset":96,"byteLength":6}]}"#;

/// Containers that read, so the search has somewhere to mutate *from*.
fn readable_seeds() -> Vec<(String, Vec<u8>)> {
    vec![
        ("simplest".to_owned(), container(SIMPLEST, &triangle())),
        ("everything".to_owned(), container(EVERYTHING, &furnished())),
        // A node with a transform, which is the path through the
        // placement layer.
        (
            "placed".to_owned(),
            container(
                &SIMPLEST.replace(
                    r#"{"mesh":0}"#,
                    r#"{"mesh":0,"translation":[1,2,3],"scale":[2,2,2]}"#,
                ),
                &triangle(),
            ),
        ),
        // A parent and a child, which is the path that composes two
        // transforms.
        (
            "nested".to_owned(),
            container(
                &SIMPLEST.replace(
                    r#""nodes":[{"mesh":0}]"#,
                    r#""nodes":[{"children":[1],"translation":[10,0,0]},{"mesh":0}]"#,
                ),
                &triangle(),
            ),
        ),
        // Two primitives under one mesh, which is the path that joins.
        (
            "two-primitives".to_owned(),
            container(
                &SIMPLEST.replace(
                    r#""primitives":[{"attributes":{"POSITION":0}}]"#,
                    r#""primitives":[{"attributes":{"POSITION":0}},{"attributes":{"POSITION":0}}]"#,
                ),
                &triangle(),
            ),
        ),
        // A matrix rather than the three parts.
        (
            "matrix-transform".to_owned(),
            container(
                &SIMPLEST.replace(
                    r#"{"mesh":0}"#,
                    r#"{"mesh":0,"matrix":[2,0,0,0,0,2,0,0,0,0,2,0,1,2,3,1]}"#,
                ),
                &triangle(),
            ),
        ),
    ]
}

/// Canonical base64, so a document can carry its own geometry.
///
/// The same encoder six other targets share, included rather than
/// copied: a seed that embeds a payload has to spell it, and a payload
/// spelled by hand is a fixture that stops meaning what its name says.
#[path = "../tests/shared/base64_encode.rs"]
mod base64_encode;

/// Documents that arrive on their own, with no container around them.
///
/// **The corpus had none of this shape.** Every seed was a container, so
/// the path a `.gltf` takes -- no chunk, geometry embedded in the
/// document as payloads -- was reachable only by a mutation that
/// destroyed the magic, and a mutation that destroys the magic usually
/// destroys everything after it too. These start inside that path.
fn document_seeds() -> Vec<(String, Vec<u8>)> {
    let payload = base64_encode::encode(&triangle());
    let embedded = SIMPLEST.replace(
        r#""buffers":[{"byteLength":36}]"#,
        &format!(
            r#""buffers":[{{"byteLength":36,"uri":"data:application/octet-stream;base64,{payload}"}}]"#
        ),
    );

    vec![
        // The whole point of the shape: one file, geometry included.
        (
            "document-embedded".to_owned(),
            embedded.clone().into_bytes(),
        ),
        // The same document wanting a chunk that a lone document can
        // never have, which is what an exporter that split its output
        // and forgot to say so produces.
        (
            "document-wants-a-chunk".to_owned(),
            SIMPLEST.as_bytes().to_vec(),
        ),
        // A document naming a file beside it, which this reader will not
        // open and says so.
        (
            "document-names-a-file".to_owned(),
            SIMPLEST
                .replace(
                    r#""buffers":[{"byteLength":36}]"#,
                    r#""buffers":[{"byteLength":36,"uri":"geometry.bin"}]"#,
                )
                .into_bytes(),
        ),
        // JSON that parses and is not a document, which the detector
        // must not claim and the reader must refuse by name.
        (
            "json-that-is-not-a-document".to_owned(),
            br#"{"name":"something else","version":"2.0"}"#.to_vec(),
        ),
        // A payload whose media type is not one a buffer may declare.
        (
            "document-wrong-media-type".to_owned(),
            embedded
                .replace("application/octet-stream", "image/png")
                .into_bytes(),
        ),
        // A payload that will not decode, which reaches the decoder's own
        // refusals through the document -- five layers of wrapping, and
        // nothing exercised it until this seed.
        (
            "document-payload-will-not-decode".to_owned(),
            embedded.replace("base64,", "base64,!!").into_bytes(),
        ),
        // A buffer declaring more than its payload holds, which is the
        // document and its own bytes disagreeing.
        (
            "document-buffer-too-short".to_owned(),
            embedded
                .replace(r#""byteLength":36"#, r#""byteLength":600"#)
                .into_bytes(),
        ),
    ]
}

/// Documents carrying materials, which the geometry path never reads.
///
/// **A material table is reached only by asking for it**, so a corpus of
/// documents that carry none leaves that layer's arithmetic — the
/// factors, their stated ranges, the alpha modes, the maps — attacked
/// by nothing. These carry one, in the shapes that decide the answer.
fn material_seeds() -> Vec<(String, Vec<u8>)> {
    // A document whose material names textures has to have them: the
    // reader bounds every index by the table it points at, and a seed
    // that named one out of thin air would be exercising that refusal
    // rather than the material read it is named for.
    let with_textures = |materials: &str| {
        SIMPLEST
            .replace(
                r#""asset":{"version":"2.0"}"#,
                &format!(
                    r#""asset":{{"version":"2.0"}},"textures":[{{}},{{}},{{}},{{}},{{}}],"materials":{materials}"#
                ),
            )
            .into_bytes()
    };

    let with = |materials: &str| {
        SIMPLEST
            .replace(
                r#""asset":{"version":"2.0"}"#,
                &format!(r#""asset":{{"version":"2.0"}},"materials":{materials}"#),
            )
            .into_bytes()
    };

    vec![
        // The empty material, which is legal and means every default.
        ("material-empty".to_owned(), with("[{}]")),
        // Every member the format states, so the whole read is walked.
        (
            "material-whole".to_owned(),
            with_textures(
                r#"[{"name":"brushed",
                "pbrMetallicRoughness":{"baseColorFactor":[0.5,0.25,0.125,1],
                "metallicFactor":0.75,"roughnessFactor":0.25,
                "baseColorTexture":{"index":0,"texCoord":1},
                "metallicRoughnessTexture":{"index":1}},
                "normalTexture":{"index":2,"scale":2.5},
                "occlusionTexture":{"index":3,"strength":0.5},
                "emissiveTexture":{"index":4},"emissiveFactor":[0.1,0.2,0.3],
                "alphaMode":"MASK","alphaCutoff":0.25,"doubleSided":true}]"#,
            ),
        ),
        // The two alpha modes that carry no cutoff, so the arm that
        // reads one is entered from every state.
        (
            "material-blended".to_owned(),
            with(r#"[{"alphaMode":"BLEND","alphaCutoff":0.75}]"#),
        ),
        // A factor outside the range the schema states, which is the
        // refusal this layer exists to make.
        (
            "material-factor-out-of-range".to_owned(),
            with(r#"[{"emissiveFactor":[0,0,4]}]"#),
        ),
        // An alpha mode the format does not have.
        (
            "material-unknown-alpha-mode".to_owned(),
            with(r#"[{"alphaMode":"DITHER"}]"#),
        ),
        // A map naming no texture, which is not a map.
        (
            "material-map-without-texture".to_owned(),
            with(r#"[{"emissiveTexture":{"texCoord":0}}]"#),
        ),
    ]
}

/// Documents carrying images, which no other seed reaches.
///
/// An image table is read only by asking for it, and it is where two
/// layers meet that otherwise never touch: the buffer bytes an accessor
/// addresses, and the payload decoding a buffer's own URI uses. These
/// carry one, in the shapes that decide the answer.
fn image_seeds() -> Vec<(String, Vec<u8>)> {
    // **Built from nothing rather than from the geometry document.** A
    // seed that reused it would carry a buffer wanting a container chunk
    // it does not have, so every one of these would be refused for a
    // reason that has nothing to do with images -- and the table they
    // exist to exercise would never be reached.
    let with =
        |images: &str| format!(r#"{{"asset":{{"version":"2.0"}},"images":{images}}}"#).into_bytes();

    // The one shape that needs a buffer carries its own, as a payload,
    // so it stands alone too.
    let stored = |images: &str| {
        format!(
            r#"{{"asset":{{"version":"2.0"}},
"buffers":[{{"byteLength":4,"uri":"data:application/octet-stream;base64,AQIDBA=="}}],
"bufferViews":[{{"buffer":0,"byteLength":4}}],"images":{images}}}"#
        )
        .into_bytes()
    };

    vec![
        // A payload, which is the self-contained shape.
        (
            "image-embedded".to_owned(),
            with(r#"[{"name":"grain","uri":"data:image/png;base64,AQIDBA=="}]"#),
        ),
        // Stored in the document's own buffer, which is the other one --
        // and the only seed where an image reaches the buffer table.
        (
            "image-in-a-view".to_owned(),
            stored(r#"[{"bufferView":0,"mimeType":"image/png"}]"#),
        ),
        // Both sources, which the format's `oneOf` forbids.
        (
            "image-two-sources".to_owned(),
            stored(
                r#"[{"bufferView":0,"mimeType":"image/png","uri":"data:image/png;base64,AQIDBA=="}]"#,
            ),
        ),
        // Neither.
        (
            "image-no-source".to_owned(),
            with(r#"[{"mimeType":"image/png"}]"#),
        ),
        // A view with nothing said about what its bytes are.
        (
            "image-view-untyped".to_owned(),
            stored(r#"[{"bufferView":0}]"#),
        ),
        // One resource, two names for it.
        (
            "image-type-disagrees".to_owned(),
            with(r#"[{"mimeType":"image/png","uri":"data:image/jpeg;base64,AQIDBA=="}]"#),
        ),
        // A second file, which this crate will not open.
        (
            "image-names-a-file".to_owned(),
            with(r#"[{"uri":"grain.png"}]"#),
        ),
    ]
}

/// One container per refusal, each wrong in exactly one way.
fn refused_seeds() -> Vec<(String, Vec<u8>)> {
    let mut wrong_magic = container(SIMPLEST, &triangle());
    wrong_magic[0] = b'X';

    // **The magic left intact, the version broken.** A seed whose magic
    // is wrong is not a container at all and is tried as a document
    // instead, so without this one nothing in the corpus reaches the
    // container layer's refusals -- which is a hole the document seeds
    // opened, because they took the only seed that used to reach it.
    let mut bad_version = container(SIMPLEST, &triangle());
    bad_version[4] = 9;

    vec![
        ("not-a-container".to_owned(), b"solid teapot\n".to_vec()),
        ("wrong-magic".to_owned(), wrong_magic),
        ("container-version".to_owned(), bad_version),
        (
            "not-a-document".to_owned(),
            container(r#"{"asset":"#, &triangle()),
        ),
        (
            "no-scenes".to_owned(),
            container(r#"{"asset":{"version":"2.0"}}"#, &triangle()),
        ),
        (
            "count-past-the-chunk".to_owned(),
            container(
                &SIMPLEST.replace(r#""count":3"#, r#""count":999"#),
                &triangle(),
            ),
        ),
        (
            "view-past-the-chunk".to_owned(),
            container(
                &SIMPLEST.replace(
                    r#"{"byteLength":36}"#,
                    r#"{"byteOffset":24,"byteLength":36}"#,
                ),
                &triangle(),
            ),
        ),
        (
            "unknown-component".to_owned(),
            container(
                &SIMPLEST.replace(r#""componentType":5126"#, r#""componentType":5124"#),
                &triangle(),
            ),
        ),
        (
            "matrix-shape".to_owned(),
            container(
                &SIMPLEST.replace(r#""type":"VEC3""#, r#""type":"MAT4""#),
                &triangle(),
            ),
        ),
        (
            "second-buffer".to_owned(),
            container(
                &SIMPLEST.replace(
                    r#""buffers":[{"byteLength":36}]"#,
                    r#""buffers":[{"byteLength":36},{"byteLength":36}]"#,
                ),
                &triangle(),
            ),
        ),
        (
            "accessor-past-the-table".to_owned(),
            container(
                &SIMPLEST.replace(r#""POSITION":0"#, r#""POSITION":7"#),
                &triangle(),
            ),
        ),
        (
            "triangle-strip".to_owned(),
            container(
                &SIMPLEST.replace(
                    r#"{"attributes":{"POSITION":0}}"#,
                    r#"{"mode":5,"attributes":{"POSITION":0}}"#,
                ),
                &triangle(),
            ),
        ),
        (
            "flattened-with-normals".to_owned(),
            container(
                &EVERYTHING.replace(r#"{"mesh":0}"#, r#"{"mesh":0,"scale":[1,0,1]}"#),
                &furnished(),
            ),
        ),
        (
            "empty-scene".to_owned(),
            container(
                r#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[]}]}"#,
                &[],
            ),
        ),
        // A scene with no `nodes` member at all, which is legal and a
        // different document from one with an empty list.
        (
            "scene-without-nodes".to_owned(),
            container(r#"{"asset":{"version":"2.0"},"scenes":[{}]}"#, &[]),
        ),
    ]
}

fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/gltf_read");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {error}", dir.display());
        return ExitCode::FAILURE;
    }

    let mut written = 0usize;
    let mut kept = 0usize;
    for (name, bytes) in readable_seeds()
        .into_iter()
        .chain(refused_seeds())
        .chain(document_seeds())
        .chain(material_seeds())
        .chain(image_seeds())
    {
        // **The extension follows the bytes.** Half these seeds are
        // documents rather than containers, and `.glb` means container
        // everywhere else in this tree; deriving it from the magic keeps
        // the two from drifting apart as seeds are added.
        let shape = if bytes.starts_with(b"glTF") {
            "glb"
        } else {
            "gltf"
        };
        let path = dir.join(format!("{name}.{shape}"));
        if path.exists() {
            kept += 1;
            continue;
        }
        // A generator that swallows a write failure and then reports
        // success is worse than one that crashes: the caller sees a
        // count and believes the corpus is whole.
        if let Err(error) = std::fs::write(&path, &bytes) {
            eprintln!("cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        written += 1;
    }

    println!(
        "{written} written, {kept} already present, in {}",
        dir.display()
    );
    ExitCode::SUCCESS
}
