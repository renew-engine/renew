//! Write the binary glTF reader's seed corpus.
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
"bufferViews":[{"byteLength":36}]}"#;

/// Everything named: normals, coordinates and an index stream.
const EVERYTHING: &str = r#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[0]}],
"nodes":[{"mesh":0}],"meshes":[{"primitives":[{
"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3}]}],
"accessors":[
{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},
{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"},
{"bufferView":2,"componentType":5126,"count":3,"type":"VEC2"},
{"bufferView":3,"componentType":5123,"count":3,"type":"SCALAR"}],
"bufferViews":[
{"byteOffset":0,"byteLength":36},{"byteOffset":36,"byteLength":36},
{"byteOffset":72,"byteLength":24},{"byteOffset":96,"byteLength":6}]}"#;

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

/// One container per refusal, each wrong in exactly one way.
fn refused_seeds() -> Vec<(String, Vec<u8>)> {
    let mut wrong_magic = container(SIMPLEST, &triangle());
    wrong_magic[0] = b'X';

    vec![
        ("not-a-container".to_owned(), b"solid teapot\n".to_vec()),
        ("wrong-magic".to_owned(), wrong_magic),
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
                &SIMPLEST.replace(r#"{"byteLength":36}"#, r#"{"buffer":1,"byteLength":36}"#),
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
    for (name, bytes) in readable_seeds().into_iter().chain(refused_seeds()) {
        let path = dir.join(format!("{name}.glb"));
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
