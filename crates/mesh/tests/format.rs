//! Which reader owns which bytes.
//!
//! **These tests are the reason this lives in the crate.** The order
//! `detect` tries formats in is a fact about the formats, and it depends
//! on what each reader accepts — so a change to `mtl::looks_like` or to
//! PLY's magic must be seen by tests that sit beside those readers, not
//! by a tool that happens to call them.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules —
// do not reach it. A fixture this file wrote and then could not read
// back is a broken test rather than a condition to recover from.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::format::{self, Format};
use renew_mesh::{Mesh, blob};

const AN_OBJ: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
const A_PLY: &str = "ply\nformat ascii 1.0\nelement vertex 3\n\
                     property float x\nproperty float y\nproperty float z\n\
                     element face 1\nproperty list uchar int vertex_indices\n\
                     end_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n";
const A_TEXT_STL: &str = "solid one\nfacet normal 0 0 1\n  outer loop\n\
                          vertex 0 0 0\n vertex 1 0 0\n vertex 0 1 0\n\
                          endloop\nendfacet\nendsolid one\n";
const AN_MTL: &str = "newmtl steel\nKd 0.4 0.4 0.45\n";

fn a_blob() -> Vec<u8> {
    blob::write(&Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        ..Mesh::default()
    })
}

/// **Every format this crate reads is identified as itself.**
#[test]
fn each_format_is_recognised() {
    assert_eq!(format::detect(AN_OBJ.as_bytes()), Format::Obj);
    assert_eq!(format::detect(A_PLY.as_bytes()), Format::Ply);
    assert_eq!(format::detect(A_TEXT_STL.as_bytes()), Format::Stl);
    assert_eq!(format::detect(AN_MTL.as_bytes()), Format::Mtl);
    assert_eq!(format::detect(&a_blob()), Format::Blob);
}

/// **A blob is recognised as itself and not as a truncated STL.**
///
/// It has an eight-byte magic, so this is the one identification here
/// that is certain rather than a guess. Before this existed, feeding a
/// blob back to the tool that wrote it fell through to the STL fallback
/// and was reported as a file too short to hold an 84-byte header —
/// true of an STL, and no help at all.
///
/// Probed by removing the blob arm: red, and the message a user gets
/// talks about a header the file was never meant to have.
#[test]
fn a_blob_is_not_mistaken_for_a_truncated_stl() {
    let bytes = a_blob();
    assert_eq!(format::detect(&bytes), Format::Blob);
    let read = Format::Blob
        .read(&bytes)
        .expect("a blob carries geometry")
        .expect("and this one is well-formed");
    assert_eq!(read.triangles(), 1);
}

/// **A material library is recognised before the fallback.**
///
/// Otherwise it reaches the STL reader and is refused as a truncated
/// mesh — a true sentence that sends its reader nowhere useful.
///
/// Probed by removing the MTL arm: red, it comes back as `Stl`.
#[test]
fn a_material_library_is_recognised_and_carries_no_geometry() {
    assert_eq!(format::detect(AN_MTL.as_bytes()), Format::Mtl);
    assert!(!Format::Mtl.carries_geometry());
    assert!(
        Format::Mtl.read(AN_MTL.as_bytes()).is_none(),
        "there is no geometry to read, which is an answer rather than a refusal"
    );
    for other in [Format::Obj, Format::Ply, Format::Stl, Format::Blob] {
        assert!(other.carries_geometry(), "{} does", other.name());
    }
}

/// **Anything unrecognised falls to STL, because STL cannot answer for
/// itself.**
///
/// The format has no magic number, so "these are not STL bytes" and
/// "these are STL bytes cut short" are the same observation. Making it
/// the fallback is what lets a truncated STL reach the reader whose
/// refusals describe it.
#[test]
fn what_nothing_claims_is_offered_to_stl() {
    for bytes in [
        &b""[..],
        &b"nonsense"[..],
        &[0xFF, 0xFE, 0x00][..],
        // A binary STL's eighty-byte header is arbitrary bytes, so it
        // claims nothing and must land here.
        &[0u8; 84][..],
    ] {
        assert_eq!(format::detect(bytes), Format::Stl, "{bytes:?}");
    }
}

/// **The name is what a machine keys on, and every format has a
/// distinct one.**
#[test]
fn every_format_has_its_own_stable_name() {
    let all = [
        Format::Obj,
        Format::Mtl,
        Format::Stl,
        Format::Ply,
        Format::Blob,
    ];
    let mut names: Vec<&str> = all.iter().map(|format| format.name()).collect();
    names.sort_unstable();
    let distinct = names.len();
    names.dedup();
    assert_eq!(names.len(), distinct, "two formats share a name: {names:?}");
    assert_eq!(names, vec!["blob", "mtl", "obj", "ply", "stl"]);
}

/// **`detect` answers for every byte string, and what it names can be
/// read or is honestly empty.**
///
/// The sweep that matters for a function standing in front of five
/// readers: no input makes it panic, and whatever it names either reads,
/// refuses, or carries no geometry.
#[test]
fn every_byte_string_gets_a_format() {
    let mut seed = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let alphabet = b"plyendhaformtscivnwmKd 0123456789./#RENWMSH\n";
    let span = u64::try_from(alphabet.len()).unwrap_or(1);
    for _ in 0..2000 {
        let length = usize::try_from(next() % 90).unwrap_or(0);
        let bytes: Vec<u8> = (0..length)
            .map(|_| alphabet[usize::try_from(next() % span).unwrap_or(0)])
            .collect();
        let found = format::detect(&bytes);
        match found.read(&bytes) {
            None => assert_eq!(found, Format::Mtl, "only a library carries no geometry"),
            Some(Ok(mesh)) => {
                assert_eq!(
                    mesh.positions.len() % 3,
                    0,
                    "{found:?} gave whole triangles"
                );
                assert!(!mesh.is_empty());
            }
            Some(Err(_)) => {}
        }
    }
}
