//! The PLY reader, against files this test builds.
//!
//! **Every fixture is generated here.** A model downloaded from a sample
//! repository is a licence question, and this repository's rule is that
//! test fixtures are authored or generated rather than borrowed.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules —
// do not reach it. A fixture this file built and then could not read
// back is a broken test rather than a condition to recover from.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::{MeshError, ply};

/// A square, as two triangles over four shared corners.
///
/// **Four vertices and two faces, not six vertices.** That sharing is
/// the whole difference between PLY and STL, and it is what makes an
/// index a thing that can be wrong.
const SQUARE: [[f32; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 0.0],
];

fn ascii_square() -> String {
    use core::fmt::Write as _;
    let mut out = String::from(
        "ply\nformat ascii 1.0\ncomment built by a test\n\
         element vertex 4\nproperty float x\nproperty float y\nproperty float z\n\
         element face 2\nproperty list uchar int vertex_indices\nend_header\n",
    );
    for corner in SQUARE {
        let _ = writeln!(out, "{} {} {}", corner[0], corner[1], corner[2]);
    }
    out.push_str("3 0 1 2\n3 0 2 3\n");
    out
}

/// The same square with the body written in binary.
fn binary_square(big: bool) -> Vec<u8> {
    let format = if big {
        "binary_big_endian"
    } else {
        "binary_little_endian"
    };
    let mut out = format!(
        "ply\nformat {format} 1.0\n\
         element vertex 4\nproperty float x\nproperty float y\nproperty float z\n\
         element face 2\nproperty list uchar int vertex_indices\nend_header\n"
    )
    .into_bytes();
    for corner in SQUARE {
        for value in corner {
            let bytes = if big {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            };
            out.extend_from_slice(&bytes);
        }
    }
    for face in [[0u32, 1, 2], [0, 2, 3]] {
        out.push(3);
        for index in face {
            let bytes = if big {
                index.to_be_bytes()
            } else {
                index.to_le_bytes()
            };
            out.extend_from_slice(&bytes);
        }
    }
    out
}

fn refusal(bytes: &[u8]) -> MeshError {
    ply::read(bytes).expect_err("these bytes should have been refused")
}

/// The two triangles a square expands into, in face order.
fn square_triangles() -> Vec<[f32; 3]> {
    vec![
        SQUARE[0], SQUARE[1], SQUARE[2], // the first face
        SQUARE[0], SQUARE[2], SQUARE[3], // the second
    ]
}

/// **All three encodings read the same file to the same geometry.**
///
/// The header is the schema in every case, so a reader that got the
/// binary widths wrong, or the byte order, produces different positions
/// from the same square — which is what this compares against.
#[test]
fn every_encoding_reads_the_same_square() {
    let files: [(&str, Vec<u8>); 3] = [
        ("ascii", ascii_square().into_bytes()),
        ("little endian", binary_square(false)),
        ("big endian", binary_square(true)),
    ];
    for (which, bytes) in files {
        let mesh = ply::read(&bytes).unwrap_or_else(|error| panic!("{which}: {error}"));
        assert_eq!(mesh.triangles(), 2, "{which}");
        assert_eq!(mesh.positions, square_triangles(), "{which}");
        assert!(
            mesh.normals.is_empty(),
            "{which}: PLY stores normals per vertex and a triangle here carries one"
        );
    }
}

/// **Indices are what this format adds, and a wrong one is refused.**
///
/// A face naming a vertex past the end is the difference between a mesh
/// and a read past a buffer. STL cannot have this defect — it repeats
/// every corner — so it is the refusal this reader exists to add.
#[test]
fn a_face_naming_a_vertex_that_is_not_there_is_refused() {
    let file = ascii_square().replace("3 0 2 3", "3 0 2 9");
    assert_eq!(
        refusal(file.as_bytes()),
        MeshError::IndexOutOfRange {
            index: 9,
            count: 4,
            face: 1
        }
    );

    // One past the end is the interesting one: an off-by-one in a
    // writer, and the value a bounds check written with `<=` would let
    // through.
    let edge = ascii_square().replace("3 0 1 2", "3 0 1 4");
    assert_eq!(
        refusal(edge.as_bytes()),
        MeshError::IndexOutOfRange {
            index: 4,
            count: 4,
            face: 0
        }
    );
}

/// A face with fewer than three corners covers no area.
///
/// Refused rather than dropped: a reader that silently skipped it would
/// be deciding that a file claiming two faces really has one.
#[test]
fn a_face_that_is_not_a_surface_is_refused() {
    let file = ascii_square().replace("3 0 2 3", "2 0 2");
    assert_eq!(
        refusal(file.as_bytes()),
        MeshError::NotAFace {
            face: 1,
            corners: 2
        }
    );
}

/// A well-formed file this reader cannot use says so in its own terms.
///
/// **Separate from a malformed file, because the caller's next move
/// differs**: a point cloud is a valid PLY and the answer is to convert
/// it, not to re-export it.
#[test]
fn a_file_with_no_geometry_this_reader_can_use_says_which_part_is_missing() {
    // A point cloud: vertices, no faces.
    let cloud = "ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\n\
                 property float y\nproperty float z\nend_header\n0 0 0\n1 1 1\n";
    assert_eq!(
        refusal(cloud.as_bytes()),
        MeshError::Unsupported { wanted: "face" }
    );

    // A vertex element with no coordinates in it.
    let colours = "ply\nformat ascii 1.0\nelement vertex 1\nproperty uchar red\n\
                   element face 1\nproperty list uchar int vertex_indices\nend_header\n\
                   255\n3 0 0 0\n";
    assert_eq!(
        refusal(colours.as_bytes()),
        MeshError::Unsupported { wanted: "x" }
    );
}

/// **Properties this reader has no use for cost their width and nothing
/// else**, which is the whole benefit of the header being a schema.
///
/// The coordinates are found by name, so a file that interleaves colour,
/// confidence and normals around them reads exactly as the plain one
/// does. Files from scanners look like this and files from modellers do
/// not, so a reader that assumed position order would work on half the
/// world's PLY files.
#[test]
fn a_schema_full_of_things_this_reader_ignores_still_reads() {
    use core::fmt::Write as _;
    let mut out = String::from(
        "ply\nformat ascii 1.0\n\
         element vertex 4\n\
         property uchar red\nproperty float x\nproperty double confidence\n\
         property float y\nproperty float nx\nproperty float z\nproperty uchar alpha\n\
         element face 2\nproperty list uchar int vertex_indices\n\
         property uchar flags\nend_header\n",
    );
    for corner in SQUARE {
        let _ = writeln!(
            out,
            "255 {} 0.5 {} 0.0 {} 128",
            corner[0], corner[1], corner[2]
        );
    }
    out.push_str("3 0 1 2 7\n3 0 2 3 7\n");

    let mesh = ply::read(out.as_bytes()).expect("a schema is a schema");
    assert_eq!(mesh.positions, square_triangles());
}

/// A quad becomes two triangles, fanned from its first corner.
#[test]
fn a_quad_becomes_a_fan() {
    let file = ascii_square().replace("3 0 1 2\n3 0 2 3\n", "4 0 1 2 3\n");
    let file = file.replace("element face 2", "element face 1");
    let mesh = ply::read(file.as_bytes()).expect("a quad is a face");
    assert_eq!(
        mesh.positions,
        square_triangles(),
        "the fan over 0,1,2,3 is exactly the two triangles the square was written as"
    );
}

/// **A header is a schema an attacker writes, and its numbers are
/// refused rather than believed.**
///
/// Each of these is a small file asking for a very large amount of work,
/// which is the shape that separates a reader from a denial of service.
#[test]
fn a_header_asking_for_more_than_it_can_hold_is_refused() {
    use core::fmt::Write as _;
    // A vertex count with no body behind it. The refusal is a count
    // mismatch rather than an allocation.
    let lying = "ply\nformat ascii 1.0\nelement vertex 4000000000\nproperty float x\n\
                 property float y\nproperty float z\nelement face 1\n\
                 property list uchar int vertex_indices\nend_header\n";
    assert!(
        matches!(
            refusal(lying.as_bytes()),
            MeshError::NoGeometry | MeshError::CountMismatch { .. }
        ),
        "a count with no rows behind it must not be reserved for"
    );

    // A face claiming more corners than any real face has.
    let fan = ascii_square().replace("3 0 1 2", "5000 0 1 2");
    assert_eq!(
        refusal(fan.as_bytes()),
        MeshError::TooLarge {
            field: "face corner count",
            value: 5000
        }
    );

    // A schema with more properties than a schema has.
    let mut wide = String::from("ply\nformat ascii 1.0\nelement vertex 1\n");
    for index in 0..2000 {
        let _ = writeln!(wide, "property float p{index}");
    }
    wide.push_str("end_header\n");
    assert!(
        matches!(refusal(wide.as_bytes()), MeshError::TooLarge { .. }),
        "a header declaring two thousand properties is describing nothing real"
    );
}

/// The header's own grammar is checked, and each refusal says where.
#[test]
fn a_header_that_is_not_one_is_refused_by_line() {
    assert_eq!(
        refusal(b"not a ply at all\nend_header\n"),
        MeshError::ExpectedKeyword {
            expected: "ply, format, comment, element, property or end_header",
            found: "not".to_owned(),
            line: 1
        }
    );
    assert_eq!(
        refusal(b"ply\nformat ebcdic 1.0\nend_header\n"),
        MeshError::ExpectedKeyword {
            expected: "ascii, binary_little_endian or binary_big_endian",
            found: "ebcdic".to_owned(),
            line: 2
        }
    );
    // Every PLY in existence is 1.0; a file claiming otherwise describes
    // a format this reader has not been written against.
    assert_eq!(
        refusal(b"ply\nformat ascii 2.0\nend_header\n"),
        MeshError::ExpectedKeyword {
            expected: "1.0",
            found: "2.0".to_owned(),
            line: 2
        }
    );
    // A property type this reader does not know is refused rather than
    // guessed at: guessing its width misaligns every column after it.
    assert_eq!(
        refusal(b"ply\nformat ascii 1.0\nelement vertex 1\nproperty quadruple x\nend_header\n"),
        MeshError::ExpectedKeyword {
            expected: "a property type",
            found: "quadruple".to_owned(),
            line: 4
        }
    );
    // A file with no header terminator at all.
    assert_eq!(
        refusal(b"ply\nformat ascii 1.0\n"),
        MeshError::ExpectedKeyword {
            expected: "end_header",
            found: String::new(),
            line: 1
        }
    );
}

/// A coordinate that is not a number a bounding box can hold.
#[test]
fn a_coordinate_that_is_not_finite_is_refused() {
    let file = ascii_square().replace("1 0 0\n", "inf 0 0\n");
    assert_eq!(
        refusal(file.as_bytes()),
        MeshError::NotFinite {
            field: "position",
            index: 1
        }
    );

    // And through the binary path, where the bytes are a NaN rather than
    // a word: the check has to be on the value, not on its spelling.
    let mut bytes = binary_square(false);
    let header = bytes
        .windows(11)
        .position(|w| w == b"end_header\n")
        .expect("the fixture has a header")
        + 11;
    bytes[header..header + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    assert_eq!(
        refusal(&bytes),
        MeshError::NotFinite {
            field: "position",
            index: 0
        }
    );
}

/// **Every byte string gets an answer.**
///
/// Not a claim about which answer — a claim that there is one, with no
/// panic and no read past the end.
#[test]
fn every_byte_string_gets_an_answer() {
    let good = binary_square(false);
    for at in 0..good.len() {
        let mut broken = good.clone();
        broken[at] ^= 0xFF;
        let _ = ply::read(&broken);
    }
    for len in 0..good.len() {
        let _ = ply::read(&good[..len]);
    }

    let text = ascii_square().into_bytes();
    for at in 0..text.len() {
        let mut broken = text.clone();
        broken[at] ^= 0x20;
        let _ = ply::read(&broken);
    }

    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for len in 0..300 {
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                u8::try_from(seed >> 56).unwrap_or(0)
            })
            .collect();
        let _ = ply::read(&bytes);
    }
}

/// The magic word is what lets this format say "not mine".
///
/// STL cannot: it has no magic number, so it cannot tell a foreign file
/// from a broken one. PLY can, and `looks_like` is where a caller with
/// bytes of unknown provenance asks.
#[test]
fn the_magic_word_separates_this_format_from_others() {
    assert!(ply::looks_like(b"ply\nformat ascii 1.0\n"));
    assert!(
        ply::looks_like(b"\n  ply\n"),
        "leading space is still a ply"
    );
    assert!(!ply::looks_like(b"solid teapot\n"));
    assert!(!ply::looks_like(&[0u8; 84]));
    assert!(!ply::looks_like(b""));
}
