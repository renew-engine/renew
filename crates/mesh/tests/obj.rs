//! What the OBJ reader accepts, and every way it refuses.
//!
//! The fixtures are written here as text rather than committed as files:
//! an OBJ is the one format in this crate a person can read at a glance,
//! and a fixture whose bytes are visible beside its assertion is a
//! fixture nobody has to open a hex editor to argue with.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` — which apply to `#[cfg(test)]` modules —
// do not reach it. A fixture this file wrote and then could not read
// back is a broken test rather than a condition to recover from.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::{MeshError, obj};

/// A unit triangle in the XY plane, with a normal and a coordinate on
/// every corner.
fn textured_triangle() -> String {
    String::from(
        "# a triangle\n\
         v 0 0 0\n\
         v 1 0 0\n\
         v 0 1 0\n\
         vt 0 0\n\
         vt 1 0\n\
         vt 0 1\n\
         vn 0 0 1\n\
         f 1/1/1 2/2/1 3/3/1\n",
    )
}

/// The refusal a byte string provokes, or a panic naming what it read.
fn refusal(bytes: &[u8]) -> MeshError {
    match obj::read(bytes) {
        Ok(mesh) => panic!("expected a refusal, read {} triangles", mesh.triangles()),
        Err(refusal) => refusal,
    }
}

/// **The ordinary case, with all three streams present.**
#[test]
fn a_triangle_with_every_stream_reads() {
    let mesh = obj::read(textured_triangle().as_bytes()).expect("an ordinary triangle");
    assert_eq!(mesh.triangles(), 1);
    assert_eq!(
        mesh.positions,
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
    );
    assert_eq!(
        mesh.corner_texcoords,
        vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]
    );
    assert_eq!(
        mesh.corner_normals,
        vec![[0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
        "the same normal index used three times is three corners carrying it"
    );
    assert!(
        mesh.face_normals.is_empty(),
        "OBJ states no normal for a face as a whole"
    );
}

/// **Positions alone, with neither optional stream, leave both arrays
/// empty rather than filled with zeroes.**
#[test]
fn a_face_of_bare_indices_carries_no_optional_stream() {
    let plain = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let mesh = obj::read(plain.as_bytes()).expect("a face may name positions and nothing else");
    assert_eq!(mesh.triangles(), 1);
    assert!(mesh.corner_texcoords.is_empty());
    assert!(mesh.corner_normals.is_empty());
}

/// **`v//vn` names a normal and skips the texture slot.**
///
/// The empty middle field is the format's own spelling and a reader that
/// treated it as a missing word would refuse a great many real files.
#[test]
fn a_corner_may_skip_the_texture_slot() {
    let normals_only = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvn 0 0 1\nf 1//1 2//1 3//1\n";
    let mesh = obj::read(normals_only.as_bytes()).expect("`v//vn` is a corner");
    assert_eq!(mesh.corner_normals.len(), 3);
    assert!(mesh.corner_texcoords.is_empty());
}

/// **A quad becomes two triangles, fanned from its first corner.**
#[test]
fn a_quad_becomes_a_fan() {
    let quad = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n";
    let mesh = obj::read(quad.as_bytes()).expect("a quad is a surface");
    assert_eq!(mesh.triangles(), 2);
    assert_eq!(
        mesh.positions,
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        "both triangles start at the first corner"
    );
}

/// **A negative index counts back from what has been declared so far.**
///
/// The relative spelling is what an exporter emits when it concatenates
/// objects without tracking a running base, and it means the same file
/// read two ways must give the same mesh.
///
/// Probed by resolving a negative index as `raw - 1` like a positive
/// one: red, the face names vertices that were never declared.
#[test]
fn a_negative_index_counts_back_from_the_end() {
    let relative = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\n";
    let absolute = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let counted_back = obj::read(relative.as_bytes()).expect("a relative face is a face");
    let counted_forward = obj::read(absolute.as_bytes()).expect("an absolute face is a face");
    assert_eq!(
        counted_back.positions, counted_forward.positions,
        "the two spellings name the same three corners"
    );
}

/// **A negative index reaching before the first vertex is refused, and
/// says what the file asked for rather than its magnitude.**
#[test]
fn a_negative_index_past_the_beginning_is_refused() {
    let too_far = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf -4 -2 -1\n";
    let MeshError::IndexOutOfRange { index, count, face } = refusal(too_far.as_bytes()) else {
        panic!("an index before the first vertex is out of range");
    };
    assert_eq!(
        index, -4,
        "the refusal spells the index the way the file did"
    );
    assert_eq!(count, 3);
    assert_eq!(face, 0);
}

/// **Zero is refused by name, because this format numbers from one.**
///
/// Probed by folding it into the range check: red, the refusal becomes
/// `IndexOutOfRange` and a reader chasing an uninitialised field is sent
/// looking for an arithmetic mistake instead.
#[test]
fn an_index_of_zero_is_refused_as_zero() {
    let zeroed = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n";
    let MeshError::IndexZero { line } = refusal(zeroed.as_bytes()) else {
        panic!("zero has no meaning in a one-based format");
    };
    assert_eq!(line, 4, "the line an editor would go to");
}

/// **An index past the end names the number it asked for and the number
/// there were.**
#[test]
fn an_index_past_the_end_is_refused() {
    let past = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 4\n";
    let MeshError::IndexOutOfRange { index, count, .. } = refusal(past.as_bytes()) else {
        panic!("a fourth vertex was never declared");
    };
    assert_eq!(index, 4);
    assert_eq!(count, 3);
}

/// **Two corners is a line, and a line is not a surface.**
#[test]
fn a_face_with_two_corners_is_refused() {
    let line = "v 0 0 0\nv 1 0 0\nf 1 2\n";
    let MeshError::NotAFace { face, corners } = refusal(line.as_bytes()) else {
        panic!("two corners cover no area");
    };
    assert_eq!(face, 0);
    assert_eq!(corners, 2);
}

/// **A file that declares no face declares no geometry.**
///
/// Vertices alone are a point cloud, which is a legal thing to write and
/// not a thing this reader turns into triangles.
#[test]
fn vertices_with_no_face_are_no_geometry() {
    let cloud = "v 0 0 0\nv 1 0 0\nv 0 1 0\n";
    assert!(matches!(refusal(cloud.as_bytes()), MeshError::NoGeometry));
}

/// **Keywords this reader does not implement cost the file nothing.**
///
/// Real files are full of them, and none changes the geometry. Refusing
/// one would refuse most of the format as it is actually written.
///
/// Probed by refusing an unknown keyword: red, an ordinary export stops
/// being readable.
#[test]
fn keywords_this_reader_does_not_know_are_skipped() {
    let furnished = "# exported by something\n\
                     mtllib scene.mtl\n\
                     o cube\n\
                     g default\n\
                     s off\n\
                     usemtl steel\n\
                     v 0 0 0\n\
                     v 1 0 0\n\
                     v 0 1 0\n\
                     cstype bezier\n\
                     f 1 2 3\n";
    let mesh = obj::read(furnished.as_bytes()).expect("the geometry is still in there");
    assert_eq!(mesh.triangles(), 1);
}

/// **A line this reader does claim to understand is not skipped.**
///
/// The lenience above is for keywords, not for malformed lines: a `v`
/// with two numbers is a broken position, and reading it as if the third
/// were zero would put a point in the mesh that the file does not
/// contain.
#[test]
fn a_position_short_of_a_component_is_refused() {
    let short = "v 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    assert!(matches!(
        refusal(short.as_bytes()),
        MeshError::NotANumber { .. }
    ));
}

/// **A coordinate that is not a number, and one that is not finite.**
///
/// `inf` parses as a float and is not a position; the two refusals are
/// separate because the fixes are.
#[test]
fn a_coordinate_that_is_not_a_finite_number_is_refused() {
    let word = "v 0 0 zero\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let MeshError::NotANumber { found, line } = refusal(word.as_bytes()) else {
        panic!("`zero` is not a number");
    };
    assert!(found.contains("zero"), "the refusal quotes it: {found}");
    assert_eq!(line, 1);

    let infinite = "v 0 0 inf\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let MeshError::NotFinite { field, .. } = refusal(infinite.as_bytes()) else {
        panic!("an infinite coordinate bounds nothing");
    };
    assert_eq!(field, "position");
}

/// **A file that names a normal on some corners and not others cannot be
/// represented, and says so rather than inventing one.**
///
/// The per-corner arrays are either empty or exactly as long as the
/// positions. Filling a gap would put a normal in the mesh that the file
/// does not contain, and dropping the stream would throw away one it
/// does.
#[test]
fn a_face_shape_that_changes_partway_is_refused() {
    let mixed = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvn 0 0 1\nf 1//1 2//1 3\n";
    let MeshError::Unsupported { wanted } = refusal(mixed.as_bytes()) else {
        panic!("a half-normalled face has no representation here");
    };
    assert!(
        wanted.contains("normal"),
        "the refusal names which stream: {wanted}"
    );
}

/// **A body that is not text is refused rather than lossily converted.**
#[test]
fn bytes_that_are_not_text_are_refused() {
    let mut bytes = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n".to_vec();
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    let MeshError::ExpectedKeyword { expected, .. } = refusal(&bytes) else {
        panic!("a file that is not text is not this format");
    };
    assert_eq!(expected, "text");
}

/// **A face naming more corners than a face may have is refused before
/// the corners are read.**
#[test]
fn a_face_with_too_many_corners_is_refused() {
    let mut source = String::from("v 0 0 0\nv 1 0 0\nv 0 1 0\n");
    source.push('f');
    for _ in 0..2000 {
        source.push_str(" 1");
    }
    source.push('\n');
    let MeshError::TooLarge { field, .. } = refusal(source.as_bytes()) else {
        panic!("a face past the corner ceiling is refused by name");
    };
    assert_eq!(field, "face corner count");
}

/// **`looks_like` answers on the keywords, and answers no for another
/// format.**
#[test]
fn the_shape_of_the_lines_separates_this_format_from_others() {
    assert!(obj::looks_like(textured_triangle().as_bytes()));
    assert!(
        !obj::looks_like(b"ply\nformat ascii 1.0\nend_header\n"),
        "a PLY names none of these keywords"
    );
    assert!(!obj::looks_like(&[0xFF, 0xFE]), "not even text");
    assert!(
        !obj::looks_like(b"# just a comment\n"),
        "a comment is not evidence of anything"
    );
}

/// **Every byte string gets an answer.**
///
/// The sweep that matters for a reader taking bytes nobody here wrote:
/// no input panics, no input hangs, and an accepted one holds whole
/// triangles whose coordinates are finite and whose per-corner arrays
/// are either empty or exactly as long as the positions.
#[test]
fn every_byte_string_gets_an_answer() {
    let mut seed = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let alphabet = b"vtnf/ 0123456789-.#\ne";
    let span = u64::try_from(alphabet.len()).unwrap_or(1);
    for _ in 0..2000 {
        let length = usize::try_from(next() % 60).unwrap_or(0);
        let bytes: Vec<u8> = (0..length)
            .map(|_| alphabet[usize::try_from(next() % span).unwrap_or(0)])
            .collect();
        let Ok(mesh) = obj::read(&bytes) else {
            continue;
        };
        assert_eq!(mesh.positions.len() % 3, 0, "whole triangles");
        assert!(!mesh.is_empty(), "an accepted mesh has geometry");
        assert!(
            mesh.corner_normals.is_empty() || mesh.corner_normals.len() == mesh.positions.len(),
            "a per-corner array is empty or one per corner"
        );
        assert!(
            mesh.corner_texcoords.is_empty() || mesh.corner_texcoords.len() == mesh.positions.len(),
            "a per-corner array is empty or one per corner"
        );
        for value in mesh.positions.iter().flatten() {
            assert!(value.is_finite(), "a coordinate nothing can bound");
        }
    }
}
