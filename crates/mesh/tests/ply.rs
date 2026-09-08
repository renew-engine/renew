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

use renew_mesh::{Mesh, MeshError, ply};

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

/// Read `bytes`, or say that the reader did not answer in time.
///
/// **A hang is not a failing test unless something is watching the
/// clock.** The merge-gating replay beside this crate says as much in
/// its own prose, and a reader that never returns is exactly the defect
/// this helper exists to catch: the harness has no per-test deadline, so
/// without one here a non-terminating read wedges the job instead of
/// reddening it.
///
/// The crate's own lints ban `thread::spawn`, and rightly: the *library*
/// never spawns, because parallelism belongs to the job system. A test
/// that asserts a call returns at all has no other way to observe that
/// it did not.
#[expect(
    clippy::disallowed_methods,
    reason = "the ban exists so the library never spawns; observing that a call did not return requires a thread that is not the one blocked in it"
)]
fn read_within(bytes: &'static [u8], seconds: u64) -> Result<Result<Mesh, MeshError>, ()> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(ply::read(bytes));
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(seconds))
        .map_err(|_| ())
}

/// **A header that asks the reader to run forever is refused instead.**
///
/// This is the defect that shipped, and it is worth stating exactly what
/// it was. Both row loops read `element.count` — a `u64` written by
/// whoever wrote the file — and there was a ceiling on how many elements
/// a header may declare, a ceiling on how many properties each may have,
/// a ceiling on how many corners a face may name, and none at all on the
/// number that costs time. An element with **no properties** consumes
/// nothing per row, so the loop body never ran out of anything and
/// `0..u64::MAX` spun on a two-hundred-byte file.
///
/// The nuisance element only has to sort before `vertex` and `face`,
/// because the reader locates those before it starts reading rows.
///
/// A count is now refused against the bytes that could possibly supply
/// it, which is the rule the pack reader applies to its entry table and
/// the rule `REFUSALS.md` states for every reader here. A zero-width row
/// makes that bound zero, so the file below is refused rather than run.
///
/// Probed by deleting the `refuse_impossible_count` call: the test hangs
/// rather than failing, which is why it carries its own deadline.
#[test]
fn a_header_that_would_run_forever_is_refused() {
    let poison: &'static str = "ply\nformat ascii 1.0\nelement pad 18446744073709551615\n\
                  element vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
                  element face 1\nproperty list uchar int vertex_indices\nend_header\n\
                  0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n";

    // The deadline is the assertion; see `read_within`.
    match read_within(poison.as_bytes(), 5) {
        Ok(answer) => assert!(
            answer.is_err(),
            "a count no body could supply must be refused"
        ),
        Err(()) => panic!(
            "`ply::read` did not answer within five seconds on a {}-byte file",
            poison.len()
        ),
    }

    // The same shape in the binary encoding, where the rows consume
    // cursor rather than words.
    let binary: &'static str = "ply\nformat binary_little_endian 1.0\nelement pad 18446744073709551615\n\
                  element vertex 1\nproperty float x\nproperty float y\nproperty float z\n\
                  element face 1\nproperty list uchar int vertex_indices\nend_header\n";
    match read_within(binary.as_bytes(), 5) {
        Ok(answer) => assert!(answer.is_err(), "the binary path must refuse it too"),
        Err(()) => panic!("`ply::read` did not answer within five seconds on a binary body"),
    }
}

/// **A count larger than the bytes could supply is refused before a row
/// is read**, which is the same rule stated positively.
///
/// The hang above is the extreme of this: a row width of zero means no
/// count at all is satisfiable. Ordinary over-declaration is the common
/// case, and it is what a truncated download looks like.
#[test]
fn a_count_larger_than_the_body_is_refused_before_reading() {
    // Eight vertices declared, three supplied, in a body far too small
    // for eight rows of three floats.
    let short = "ply\nformat binary_little_endian 1.0\nelement vertex 8\n\
                 property float x\nproperty float y\nproperty float z\n\
                 element face 1\nproperty list uchar int vertex_indices\nend_header\n";
    let mut bytes = short.as_bytes().to_vec();
    bytes.extend_from_slice(&[0u8; 12]);
    assert!(
        matches!(refusal(&bytes), MeshError::CountMismatch { .. }),
        "eight vertices need ninety-six bytes and twelve arrived"
    );
}

/// **A list before the coordinates does not move them.**
///
/// The reader located `x`, `y` and `z` by their position in the schema
/// and then read them out of a row holding only scalars, so a list
/// declared before them shifted every coordinate after it. With enough
/// trailing scalars to absorb the shift the file read successfully and
/// returned a neighbouring column as the position — `Ok`, with geometry
/// the file does not describe, which is the worst answer a reader can
/// give because nothing downstream can detect it.
///
/// Probed by numbering the coordinates by schema position again: red,
/// the first vertex comes back at (20, 30, 40) where the file says
/// (10, 20, 30).
#[expect(
    clippy::float_cmp,
    reason = "the claim is that the file's own numbers came back unchanged, so equality with what the file says is exactly what must hold; a tolerance would pass a reader off by a whole column"
)]
#[test]
fn a_list_before_the_coordinates_does_not_shift_them() {
    let file = "ply\nformat ascii 1.0\nelement vertex 3\n\
                property list uchar int junk\nproperty float x\nproperty float y\n\
                property float z\nproperty float w\nproperty float v\n\
                element face 1\nproperty list uchar int vertex_indices\nend_header\n\
                0 10 20 30 40 50\n0 11 21 31 41 51\n0 12 22 32 42 52\n3 0 1 2\n";
    let mesh = ply::read(file.as_bytes()).expect("a list is a legal property");
    assert_eq!(
        mesh.positions[0],
        [10.0, 20.0, 30.0],
        "the file says the first vertex is at (10, 20, 30)"
    );
}

/// **An index that is not a vertex number is refused, not coerced.**
///
/// The conversion was `entry.max(0.0) as u64`, with a note beside it
/// saying an out-of-range index is caught later against the vertex
/// count. It was not: `max` had already turned a negative index into
/// vertex zero before anything could see it, `NaN.max(0.0)` is `0.0` so
/// a NaN index became vertex zero too, and a fractional index was
/// truncated. All three produced `Ok` and geometry the file does not
/// describe.
#[test]
fn an_index_that_is_not_a_vertex_number_is_refused() {
    // A signed corner type, with -1 written as 0xFF.
    let mut bytes = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                     property float x\nproperty float y\nproperty float z\n\
                     element face 1\nproperty list uchar char vertex_indices\nend_header\n"
        .as_bytes()
        .to_vec();
    for corner in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
        for value in corner {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&[3, 0, 1, 0xFF]);
    assert!(
        matches!(refusal(&bytes), MeshError::NotANumber { .. }),
        "vertex minus one is not vertex zero"
    );

    // A float corner type carrying a value between two vertices.
    let mut fractional = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                          property float x\nproperty float y\nproperty float z\n\
                          element face 1\nproperty list uchar float vertex_indices\nend_header\n"
        .as_bytes()
        .to_vec();
    for corner in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
        for value in corner {
            fractional.extend_from_slice(&value.to_le_bytes());
        }
    }
    fractional.push(3);
    for index in [0.0f32, 1.0, 2.7] {
        fractional.extend_from_slice(&index.to_le_bytes());
    }
    assert!(
        matches!(refusal(&fractional), MeshError::NotANumber { .. }),
        "vertex two-point-seven is not vertex two"
    );
}

/// **A comment may say `end_header` without ending the header.**
///
/// The terminator was found by searching for those bytes anywhere, so a
/// legal file naming a tool in a comment had its header truncated at the
/// comment and was then refused for lacking the schema that followed.
/// Exporters write tool names and paths into comments; that is what
/// comments are for.
#[test]
fn a_comment_may_name_the_terminator_without_being_it() {
    let file = "ply\nformat ascii 1.0\ncomment written by the end_header exporter\n\
                element vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
                element face 1\nproperty list uchar int vertex_indices\nend_header\n\
                0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n";
    let mesh = ply::read(file.as_bytes()).expect("a comment is not a terminator");
    assert_eq!(mesh.triangles(), 1);
}

/// One type under test: what the header calls it, what value the
/// column carries, and that value's bytes in a given byte order.
///
/// Named rather than left as a tuple because the shape is the point.
/// The value is chosen so that a plausible mistake about this type
/// reads it as something else — a negative for the signed types, a
/// value above the signed maximum for the unsigned ones, a fraction
/// for the floating ones — and the encoder is what makes the file
/// say it.
struct Case {
    name: &'static str,
    value: f32,
    encode: fn(bool) -> Vec<u8>,
}

fn order<const N: usize>(big: bool, be: [u8; N], le: [u8; N]) -> Vec<u8> {
    if big { be.to_vec() } else { le.to_vec() }
}

fn width_cases() -> [Case; 8] {
    [
        Case {
            name: "char",
            value: -2.0,
            encode: |_| vec![(-2i8).cast_unsigned()],
        },
        Case {
            name: "uchar",
            value: 254.0,
            encode: |_| vec![254u8],
        },
        Case {
            name: "short",
            value: -300.0,
            encode: |big| order(big, (-300i16).to_be_bytes(), (-300i16).to_le_bytes()),
        },
        Case {
            name: "ushort",
            value: 65236.0,
            encode: |big| order(big, 65_236u16.to_be_bytes(), 65_236u16.to_le_bytes()),
        },
        Case {
            name: "int",
            value: -70000.0,
            encode: |big| order(big, (-70_000i32).to_be_bytes(), (-70_000i32).to_le_bytes()),
        },
        Case {
            name: "uint",
            value: 3_000_000_000.0,
            encode: |big| {
                order(
                    big,
                    3_000_000_000u32.to_be_bytes(),
                    3_000_000_000u32.to_le_bytes(),
                )
            },
        },
        Case {
            name: "float",
            value: -1.5,
            encode: |big| order(big, (-1.5f32).to_be_bytes(), (-1.5f32).to_le_bytes()),
        },
        Case {
            name: "double",
            value: -1.5,
            encode: |big| order(big, (-1.5f64).to_be_bytes(), (-1.5f64).to_le_bytes()),
        },
    ]
}

/// **Every scalar type the format defines, decoded at its own width,
/// signedness and byte order.**
///
/// Nothing pinned this. The fixture that reads a binary PLY uses only
/// `float`, `int` and `uchar`, and the corpus seed named
/// `binary-every-width.seed` carries `char`, `short` and `double` — so
/// the name overstated the file, `ushort` was decoded by nothing at all,
/// and four separate mutations of the decoder survived the whole suite:
/// a `double` read four bytes wide, a `short` with its byte order
/// flipped, a `uint` read as `int`, and a `char` read unsigned.
///
/// The shape that catches all of them is to type a *coordinate* with
/// each scalar in turn and give it a value that reads differently when
/// the width, the sign or the order is wrong. A skipped property cannot
/// do it: skipping is by width, so a wrong width shifts what follows,
/// but a wrong *signedness* on a skipped column is invisible.
///
/// Probed by each of the four mutations above in turn: red on the type
/// concerned, and on every type after it when the width was wrong.
#[expect(
    clippy::float_cmp,
    reason = "the claim is that a named byte pattern decoded to a named value; a tolerance would pass a decoder that read the wrong width, which is the whole subject"
)]
#[test]
fn every_scalar_type_is_decoded_at_its_own_width_and_sign() {
    for Case {
        name,
        value: expected,
        encode,
    } in width_cases()
    {
        for big in [false, true] {
            let order = if big {
                "binary_big_endian"
            } else {
                "binary_little_endian"
            };
            // `x` carries the type under test; `y` and `z` are floats, so
            // a wrong width for `x` shifts them and they come back wrong
            // too — which is the second half of what this catches.
            let mut bytes = format!(
                "ply\nformat {order} 1.0\nelement vertex 3\n\
                 property {name} x\nproperty float y\nproperty float z\n\
                 element face 1\nproperty list uchar int vertex_indices\nend_header\n"
            )
            .into_bytes();
            for corner in 0..3u32 {
                bytes.extend_from_slice(&encode(big));
                for value in [f32::from(u8::try_from(corner).unwrap_or(0)), 7.5] {
                    bytes.extend_from_slice(&if big {
                        value.to_be_bytes()
                    } else {
                        value.to_le_bytes()
                    });
                }
            }
            bytes.push(3);
            for index in [0u32, 1, 2] {
                bytes.extend_from_slice(&if big {
                    index.to_be_bytes()
                } else {
                    index.to_le_bytes()
                });
            }

            let which = format!("`{name}` {order}");
            let mesh = ply::read(&bytes).unwrap_or_else(|error| panic!("{which}: {error}"));
            assert_eq!(mesh.triangles(), 1, "{which}");
            for (corner, position) in mesh.positions.iter().enumerate() {
                assert_eq!(
                    position[0], expected,
                    "{which}: x came back wrong, which is the width, the sign or the order"
                );
                assert_eq!(
                    position[1],
                    f32::from(u8::try_from(corner).unwrap_or(0)),
                    "{which}: y moved, so `{name}` was read at the wrong width"
                );
                assert_eq!(position[2], 7.5, "{which}: z moved with it");
            }
        }
    }
}
