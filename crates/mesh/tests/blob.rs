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

/// **The buffer is reserved exactly once, for exactly what is written.**
///
/// `capacity == len` after the fact is the deterministic way to assert
/// this — a timing test would say the same thing more slowly and less
/// reliably. If the reservation counts too little the buffer grows and
/// capacity overshoots; if it counts too much, capacity overshoots the
/// other way. Either is a mismatch here.
///
/// **The reservation used to count the positions and nothing else**,
/// then append three more arrays into the same buffer: three times under
/// on a mesh carrying all of them.
///
/// Probed by restoring that: red on every shape that carries an optional
/// array.
#[test]
fn the_buffer_is_reserved_for_exactly_what_is_written() {
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
        assert_eq!(
            bytes.capacity(),
            bytes.len(),
            "shape {shape}: reserved {} for {} bytes, so the buffer either grew or was              over-asked",
            bytes.capacity(),
            bytes.len()
        );
    }
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

/// **The magic keeps the shape the rest of the tree uses.**
///
/// A convention two formats keep and a third does not is worth less
/// than no convention, because it is the third that gets trusted
/// wrongly. This was that third one.
#[test]
fn the_magic_keeps_the_shape_the_other_formats_in_this_tree_use() {
    assert_eq!(MAGIC.len(), 8, "eight bytes, like the others");
    assert!(
        MAGIC.starts_with(b"RENEW"),
        "the shared prefix is what makes an unknown file recognisably \
         from here rather than merely unreadable"
    );
    assert_eq!(
        MAGIC[7], 0,
        "the terminator is the part that was missing: it stops a longer tag being read as a \
         shorter one plus body, and it lets a person print the magic as a C string when a file \
         will not open"
    );
    assert!(
        MAGIC[5..7].iter().all(u8::is_ascii_uppercase),
        "a two-letter tag between the prefix and the terminator: {MAGIC:?}"
    );
}

/// **An opening that is not this format is refused by name.**
#[test]
fn bytes_that_do_not_open_with_the_magic_are_refused() {
    let mut bytes = blob::write(&furnished());
    bytes[0] = b'X';
    let MeshError::NotThisFormat { expected } = refusal(&bytes) else {
        panic!("a blob opens with its magic");
    };
    assert_eq!(expected, "RENEWMS\\0");
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

/// **A value that is not finite is refused in EVERY array, naming its
/// record.**
///
/// **This used to plant its infinity in a position and nowhere else**,
/// and the gap was not theoretical: replacing the checked reads in
/// `pairs` with raw ones — so texture coordinates could come back as
/// NaN — left this file, the corpus gate and the property suite all
/// green. The fuzz target could not have found it either, because its
/// finiteness chain covered positions and corner normals and stopped
/// there.
///
/// The offsets below are the four arrays of `furnished()`: 6 positions,
/// 2 face normals, 6 corner normals, 6 coordinates, after a twenty-byte
/// header.
///
/// Probed by unchecking each of the four reads in turn: red, each on its
/// own array.
#[test]
fn a_value_that_is_not_finite_is_refused_in_every_array() {
    for (at, field, what) in [
        (20, "position", "the first position"),
        (20 + 72, "normal", "the first face normal"),
        (20 + 72 + 24, "normal", "the first corner normal"),
        (
            20 + 72 + 24 + 72,
            "texture coordinate",
            "the first coordinate",
        ),
    ] {
        for poison in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            let mut bytes = blob::write(&furnished());
            bytes[at..at + 4].copy_from_slice(&poison.to_le_bytes());
            let MeshError::NotFinite {
                field: named,
                index,
            } = refusal(&bytes)
            else {
                panic!("{what} is {poison} and nothing downstream can bound it");
            };
            assert_eq!(named, field, "{what}");
            assert_eq!(index, 0, "{what} is record 0 of its own array");
        }
    }

    // And the record is still counted per array rather than per file.
    let mut bytes = blob::write(&furnished());
    let second_position = 20 + 12;
    bytes[second_position..second_position + 4].copy_from_slice(&f32::INFINITY.to_le_bytes());
    let MeshError::NotFinite { index, .. } = refusal(&bytes) else {
        panic!("an infinite coordinate bounds nothing");
    };
    assert_eq!(index, 1, "the second position, counted as a record");
}

/// **A mesh whose arrays disagree with its corner count is a defect, and
/// it asserts.**
///
/// Not a refusal: the lengths are what tell the reader where each array
/// begins, so writing one produces a *different, well-formed* mesh
/// rather than a broken file. One corner normal and six coordinates
/// comes back as three of each, and nothing downstream can tell. The
/// contract is on [`blob::write`] and D5 says a contract violation
/// asserts.
#[test]
#[should_panic(expected = "a normal per corner or none")]
fn writing_a_ragged_mesh_is_a_defect_not_a_refusal() {
    let ragged = Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        corner_normals: vec![[9.0, 9.0, 9.0]],
        corner_texcoords: vec![[0.1, 0.2]; 6],
        ..Mesh::default()
    };
    let _ = blob::write(&ragged);
}

/// **An empty mesh is a defect too, for the same reason.**
///
/// `Mesh` derives `Default`, so `Mesh::default()` is a reachable public
/// value, and writing it produced a twenty-byte blob that `read` then
/// refused — which made "what this crate wrote, this crate reads" false
/// of the crate's own default.
#[test]
#[should_panic(expected = "whole triangles and at least one")]
fn writing_an_empty_mesh_is_a_defect() {
    let _ = blob::write(&Mesh::default());
}

/// **A corner count no file could supply is refused before it is
/// believed.**
///
/// **The name of this used to claim the wrong property.** It said the
/// refusal arrives "before allocating", as though four bytes of header
/// could make this reader reserve a quarter of a gigabyte. They cannot:
/// the length check requires the byte total the header implies to equal
/// the file's own length, so what is allocated is what was handed over
/// minus a header, and the worst case from a twenty-byte file is
/// nothing.
///
/// What the ceiling guards is the arithmetic that computes that total,
/// which runs after it. So what this test can honestly assert is that a
/// count no file could supply is refused, by name, and that is what it
/// does.
///
/// **Not probed by moving the ceiling**: the mutant would be one that
/// multiplies four billion by twelve, and a probe whose red is an
/// arithmetic wrap in a release build and a panic in a debug one tells a
/// reader less than the arithmetic does. The `const` assertion beside
/// `MAX_POSITIONS` is what holds the two constants together.
#[test]
fn a_corner_count_no_file_could_supply_is_refused() {
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
            // **Modulo the length, not modulo 64.** The first version of
            // this sweep took `next() % 64` and so never touched a byte
            // past offset 63 — which on this template is most of the
            // positions and the whole of all three optional arrays.
            // Every input it accepted was the template's own shape, so
            // the three pairing assertions below only ever saw one
            // configuration.
            let at = usize::try_from(next() % 4096).unwrap_or(0) % bytes.len();
            bytes[at] = u8::try_from(next() % 256).unwrap_or(0);
        }
        // And sometimes cut it short, anywhere.
        if next() % 4 == 0 {
            let keep = usize::try_from(next() % 4096).unwrap_or(0) % bytes.len();
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
        MeshError::Gltf(_) => Some(
            "the blob is a format this repository owns and has no document layer at all: its\n             header is the whole description",
        ),
        MeshError::TransformNotInvertible => Some(
            "the blob stores geometry already in its own space and this reader applies\n             nothing to it",
        ),
        // Reachable, and each is provoked by a byte string in this file.
        MeshError::NotThisFormat { .. }
        | MeshError::TooShortForHeader { .. }
        | MeshError::CountMismatch { .. }
        | MeshError::TooLarge { .. }
        | MeshError::NotFinite { .. }
        | MeshError::NotAFace { .. }
        | MeshError::Unsupported { .. }
        | MeshError::NoGeometry => None,
        MeshError::StreamLengthMismatch { .. } => Some(
            "the optional arrays are flagged rather than counted, so their
            lengths are derived from the corner count rather than declared
            beside it",
        ),
        MeshError::NotANumber { .. } => {
            Some("nothing here is text, so there is no word that failed to be a number")
        }
        MeshError::ExpectedKeyword { .. } => Some(
            "nothing here is text, so no word can be missing. The one place              this reader used to say it was about its magic, and that is now              `NotThisFormat` — which is the whole point of the new variant.",
        ),
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
        ("NotThisFormat", wrong_magic),
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
        MeshError::ExpectedKeyword {
            expected: "end_header",
            found: String::new(),
            line: 1,
        },
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
