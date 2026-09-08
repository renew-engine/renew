//! The canonical blob, written and read back.
//!
//! **The blob is the one format here this crate itself writes**, which
//! changes what the tests are for. The other four suites prove a reader
//! survives somebody else's file; this one has to prove two things at
//! once — that what `write` produces `read` accepts unchanged, and that
//! `read` survives a byte string `write` never produced.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules —
// do not reach it. A fixture this file wrote and then could not read
// back is a broken test rather than a condition to recover from.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::blob::{self, MAGIC};
use renew_mesh::{Mesh, MeshError};

/// A mesh with every optional array filled.
fn furnished() -> Mesh {
    Mesh {
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        face_normals: vec![[0.0, 0.0, 1.0], [0.0, 0.0, -1.0]],
        corner_normals: vec![
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0],
            [0.0, -1.0, 0.0],
            [-1.0, 0.0, 0.0],
        ],
        corner_texcoords: vec![
            [0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0],
            [1.0, 1.0],
            [0.25, 0.5],
            [-2.0, 3.5],
        ],
    }
}

/// The refusal a byte string provokes, or a panic naming what it read.
fn refusal(bytes: &[u8]) -> MeshError {
    match blob::read(bytes) {
        Ok(mesh) => panic!("expected a refusal, read {} triangles", mesh.triangles()),
        Err(refusal) => refusal,
    }
}

/// **Everything written comes back, and nothing else does.**
#[test]
fn a_furnished_mesh_survives_the_round_trip() {
    let mesh = furnished();
    let bytes = blob::write(&mesh);
    let back = blob::read(&bytes).expect("what this crate wrote, this crate reads");
    assert_eq!(back, mesh);
}

/// **An empty optional array comes back empty, not absent-and-guessed.**
///
/// The presence bits are the whole of what distinguishes "the file said
/// nothing about normals" from "the file said there are none", and a
/// mesh with no optional arrays is what the PLY reader produces.
#[test]
fn a_mesh_with_no_optional_arrays_survives_the_round_trip() {
    let bare = Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        ..Mesh::default()
    };
    let bytes = blob::write(&bare);
    let back = blob::read(&bytes).expect("a bare mesh is a mesh");
    assert_eq!(back, bare);
    assert!(back.face_normals.is_empty());
    assert!(back.corner_normals.is_empty());
    assert!(back.corner_texcoords.is_empty());
}

/// **Every combination of the three optional arrays round-trips.**
///
/// Eight shapes, and the presence bits have to be independent: a reader
/// that mixed two of them up would still pass a test that only ever set
/// all three or none.
///
/// Probed by giving two arrays the same flag: red, the shapes that hold
/// one and not the other come back wrong.
#[test]
fn every_combination_of_the_optional_arrays_round_trips() {
    let full = furnished();
    for shape in 0..8u8 {
        let mesh = Mesh {
            positions: full.positions.clone(),
            face_normals: if shape & 1 != 0 {
                full.face_normals.clone()
            } else {
                Vec::new()
            },
            corner_normals: if shape & 2 != 0 {
                full.corner_normals.clone()
            } else {
                Vec::new()
            },
            corner_texcoords: if shape & 4 != 0 {
                full.corner_texcoords.clone()
            } else {
                Vec::new()
            },
        };
        let bytes = blob::write(&mesh);
        let back = blob::read(&bytes).unwrap_or_else(|error| panic!("shape {shape}: {error}"));
        assert_eq!(back, mesh, "shape {shape} did not come back");
    }
}

/// **The bytes are a function of the mesh, not of the machine.**
///
/// Little-endian on every target, so a blob written on one machine is
/// the blob another reads. Pinned against the actual bytes rather than
/// against a second call to `write`, which would agree with itself on
/// any convention.
///
/// Probed by writing the corner count big-endian: red. Note that the
/// obvious mutant, "write native-endian", would prove nothing on this
/// host, where native IS little-endian — which is exactly why the
/// expectation below is spelled out byte by byte rather than compared
/// against a second call to `write`.
#[test]
fn the_header_is_little_endian_on_every_target() {
    let bare = Mesh {
        positions: vec![[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        ..Mesh::default()
    };
    let bytes = blob::write(&bare);
    assert_eq!(&bytes[..8], &MAGIC[..]);
    assert_eq!(&bytes[8..12], &[1, 0, 0, 0], "version 1, low byte first");
    assert_eq!(
        &bytes[12..16],
        &[3, 0, 0, 0],
        "three corners, low byte first"
    );
    assert_eq!(&bytes[16..20], &[0, 0, 0, 0], "no optional arrays");
    // 1.0f32 is 0x3F800000, so little-endian puts the 0x3F last.
    assert_eq!(
        &bytes[20..24],
        &[0x00, 0x00, 0x80, 0x3F],
        "1.0 low byte first"
    );
}

/// **Bytes too short to hold a header are refused as that.**
#[test]
fn something_shorter_than_a_header_is_refused() {
    let MeshError::TooShortForHeader { needs, len } = refusal(&MAGIC[..]) else {
        panic!("eight bytes is not a header");
    };
    assert_eq!(needs, 20);
    assert_eq!(len, 8);
}

/// **An opening that is not this format is refused by name.**
#[test]
fn bytes_that_do_not_open_with_the_magic_are_refused() {
    let mut bytes = blob::write(&furnished());
    bytes[0] = b'X';
    let MeshError::ExpectedKeyword { expected, .. } = refusal(&bytes) else {
        panic!("a blob opens with its magic");
    };
    assert_eq!(expected, "RENEWMSH");
}

/// **A version this build does not read, and a flag it does not know.**
///
/// Both are `Unsupported` rather than malformed, and for the same
/// reason: a blob from a later build is well-formed and unusable here,
/// where the caller's next move is a newer build rather than a
/// re-export.
///
/// Probed by ignoring the reserved bits: red, a blob carrying an array
/// this version cannot see is read as though it were not there, and its
/// bytes are then a length mismatch or, worse, silently short.
#[test]
fn a_version_or_a_flag_from_a_later_build_is_unsupported() {
    let mut newer = blob::write(&furnished());
    newer[8] = 2;
    let MeshError::Unsupported { wanted } = refusal(&newer) else {
        panic!("version 2 is not version 1");
    };
    assert!(wanted.contains('1'), "it says which version: {wanted}");

    let mut flagged = blob::write(&furnished());
    // Bit 3 belongs to no array this version defines.
    flagged[16] |= 0b0000_1000;
    let MeshError::Unsupported { wanted } = refusal(&flagged) else {
        panic!("an unknown array is not readable");
    };
    assert!(wanted.contains("array"), "it says what it is: {wanted}");
}

/// **A corner count that is not whole triangles is refused, and says
/// which face was short.**
#[test]
fn a_corner_count_that_is_not_whole_triangles_is_refused() {
    let mut bytes = blob::write(&furnished());
    // Six corners becomes seven: two whole triangles and one corner.
    bytes[12] = 7;
    let MeshError::NotAFace { face, corners } = refusal(&bytes) else {
        panic!("seven corners is not whole triangles");
    };
    assert_eq!(face, 2, "the third face is the short one");
    assert_eq!(corners, 1);
}

/// **A count that accounts for a different number of bytes than are
/// present is refused before anything is read.**
///
/// This is the refusal a truncated blob gets, and the one a blob whose
/// header was edited gets. Deriving the optional arrays' lengths is what
/// makes both land here rather than in three different places.
#[test]
fn a_count_that_does_not_match_the_body_is_refused() {
    let mesh = furnished();
    let full = blob::write(&mesh);

    let truncated = &full[..full.len() - 4];
    let MeshError::CountMismatch {
        declared,
        actual,
        count,
    } = refusal(truncated)
    else {
        panic!("a truncated blob accounts for more than it holds");
    };
    assert_eq!(declared, full.len() as u64);
    assert_eq!(actual, full.len() - 4);
    assert_eq!(count, 6, "the corner count it claimed");

    let mut padded = full.clone();
    padded.push(0);
    assert!(
        matches!(refusal(&padded), MeshError::CountMismatch { .. }),
        "trailing bytes are as wrong as missing ones: a blob is exactly its own length"
    );
}

/// **A blob with no geometry is refused rather than read as empty.**
#[test]
fn a_blob_declaring_no_corners_is_refused() {
    let mut bytes = blob::write(&furnished());
    bytes[12] = 0;
    assert!(matches!(refusal(&bytes), MeshError::NoGeometry));
}

/// **A coordinate that is not finite is refused, naming its record.**
#[test]
fn a_coordinate_that_is_not_finite_is_refused() {
    let mut bytes = blob::write(&furnished());
    // The second position's first component, past the twenty-byte
    // header and one twelve-byte vector.
    let at = 20 + 12;
    bytes[at..at + 4].copy_from_slice(&f32::INFINITY.to_le_bytes());
    let MeshError::NotFinite { field, index } = refusal(&bytes) else {
        panic!("an infinite coordinate bounds nothing");
    };
    assert_eq!(field, "position");
    assert_eq!(index, 1, "the second position, counted as a record");
}

/// **A corner count no file could supply is refused before it is
/// believed.**
///
/// Four bytes of header say how much memory to reserve, which is the
/// amplification a ceiling exists to stop: the refusal has to arrive
/// before the allocation, not after it.
///
/// **Not probed by moving the ceiling**, deliberately: the mutant that
/// would prove this assertion is one that reserves four billion vectors,
/// and a probe whose red is an out-of-memory kill tells nobody anything
/// they can read. What is checked instead is the refusal's own name and
/// that it arrives at all, which is the observable half.
#[test]
fn a_corner_count_larger_than_any_file_is_refused_before_allocating() {
    let mut bytes = blob::write(&furnished());
    // Just under four billion corners, in twenty-four bytes of file.
    bytes[12..16].copy_from_slice(&0xFFFF_FFF0_u32.to_le_bytes());
    let MeshError::TooLarge { field, .. } = refusal(&bytes) else {
        panic!("four billion corners is not a mesh");
    };
    assert_eq!(field, "total geometry");
}

/// **Every byte string gets an answer.**
///
/// The sweep that matters for a reader taking bytes nobody here wrote:
/// no input panics, no input hangs, and an accepted one holds whole
/// triangles whose coordinates are finite and whose optional arrays are
/// the length the corner count implies.
#[test]
fn every_byte_string_gets_an_answer() {
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let template = blob::write(&furnished());
    for _ in 0..4000 {
        let mut bytes = template.clone();
        // Corrupt a handful of bytes, so the search stays near a blob
        // rather than wandering into noise that fails at the magic.
        for _ in 0..=next() % 6 {
            let at = usize::try_from(next() % 64).unwrap_or(0) % bytes.len();
            bytes[at] = u8::try_from(next() % 256).unwrap_or(0);
        }
        // And sometimes cut it short.
        if next() % 4 == 0 {
            let keep = usize::try_from(next() % 64).unwrap_or(0) % bytes.len();
            bytes.truncate(keep);
        }
        let Ok(mesh) = blob::read(&bytes) else {
            continue;
        };
        assert_eq!(mesh.positions.len() % 3, 0, "whole triangles");
        assert!(!mesh.is_empty(), "an accepted blob has geometry");
        assert!(
            mesh.face_normals.is_empty() || mesh.face_normals.len() == mesh.triangles(),
            "a face normal per triangle or none"
        );
        assert!(
            mesh.corner_normals.is_empty() || mesh.corner_normals.len() == mesh.positions.len(),
            "a normal per corner or none"
        );
        assert!(
            mesh.corner_texcoords.is_empty() || mesh.corner_texcoords.len() == mesh.positions.len(),
            "a coordinate per corner or none"
        );
        for value in mesh.positions.iter().flatten() {
            assert!(value.is_finite(), "a coordinate nothing can bound");
        }
    }
}

/// **Every refusal this reader can make is reachable, and every one it
/// cannot make says why.**
///
/// No wildcard arm, so a variant added later stops this file compiling
/// until somebody decides which it is.
fn blob_cannot_reach(refusal: &MeshError) -> Option<&'static str> {
    match refusal {
        // Reachable, and each is provoked by a byte string in this file.
        MeshError::TooShortForHeader { .. }
        | MeshError::CountMismatch { .. }
        | MeshError::TooLarge { .. }
        | MeshError::ExpectedKeyword { .. }
        | MeshError::NotFinite { .. }
        | MeshError::NotAFace { .. }
        | MeshError::Unsupported { .. }
        | MeshError::NoGeometry => None,
        MeshError::NotANumber { .. } => {
            Some("nothing here is text, so there is no word that failed to be a number")
        }
        MeshError::IndexOutOfRange { .. } | MeshError::IndexZero { .. } => {
            Some("this format is de-indexed: a corner is written out, never pointed at")
        }
    }
}

/// The census above and the byte strings here agree.
#[test]
fn the_census_and_the_bytes_agree() {
    let full = blob::write(&furnished());

    let mut wrong_magic = full.clone();
    wrong_magic[0] = b'X';
    let mut later_version = full.clone();
    later_version[8] = 2;
    let mut short_face = full.clone();
    short_face[12] = 7;
    let mut no_corners = full.clone();
    no_corners[12] = 0;
    let mut infinite = full.clone();
    infinite[20..24].copy_from_slice(&f32::INFINITY.to_le_bytes());
    let mut enormous = full.clone();
    enormous[12..16].copy_from_slice(&0xFFFF_FFF0_u32.to_le_bytes());

    let provocations: [(&str, Vec<u8>); 8] = [
        ("TooShortForHeader", MAGIC.to_vec()),
        ("ExpectedKeyword", wrong_magic),
        ("Unsupported", later_version),
        ("NotAFace", short_face),
        ("NoGeometry", no_corners),
        ("NotFinite", infinite),
        ("TooLarge", enormous),
        ("CountMismatch", full[..full.len() - 4].to_vec()),
    ];

    for (name, bytes) in &provocations {
        let got = refusal(bytes);
        assert!(
            blob_cannot_reach(&got).is_none(),
            "`{name}` is provoked by bytes here, and the census calls it unreachable"
        );
        assert_eq!(
            got.name(),
            *name,
            "the bytes meant to provoke `{name}` provoked something else"
        );
    }

    for refusal in [
        MeshError::NotANumber {
            found: String::new(),
            line: 1,
        },
        MeshError::IndexOutOfRange {
            index: 1,
            count: 0,
            face: 0,
        },
        MeshError::IndexZero { line: 1 },
    ] {
        assert!(
            blob_cannot_reach(&refusal).is_some(),
            "{refusal:?} is claimed reachable and nothing here provokes it"
        );
    }
}
