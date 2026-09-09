//! Streams that are each sound, assembled into geometry that may not be.
//!
//! **Every refusal here is about a relationship**, which is what makes
//! this layer worth its own suite: each view arrives already checked
//! against its own bytes, so nothing below can be at fault. What is left
//! is whether the streams describe the same vertices, whether the
//! indices address vertices that exist, and whether the corners divide
//! into faces.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` do not reach it.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_mesh::MeshError;
use renew_mesh::accessor::{Accessor, Component, Indices, Shape, View};
use renew_mesh::primitive::{self, Mode, Primitive};

/// Compare coordinates by bits.
///
/// **The honest comparison here, not a way around the lint.** Assembly
/// does no arithmetic on a coordinate: it reads four bytes out of the
/// fixture and puts them in an array. Anything but an exact match is a
/// value that came from somewhere other than where it was written, and a
/// tolerance would hide exactly that.
fn same(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: different lengths");
    for (index, (left, right)) in got.iter().zip(want).enumerate() {
        assert_eq!(
            left.to_bits(),
            right.to_bits(),
            "{what}: component {index} is {left}, not {right}"
        );
    }
}

/// Bytes for `values`, little-endian, as a file would store them.
fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// A tightly packed float view over `bytes`.
fn view(bytes: &[u8], shape: Shape, count: usize) -> View<'_> {
    Accessor {
        component: Component::F32,
        shape,
        count,
        byte_offset: 0,
        byte_stride: None,
        normalized: false,
    }
    .view(bytes)
    .expect("the fixture fits")
}

/// A tightly packed index view over `bytes`.
fn indices(bytes: &[u8], count: usize) -> Indices<'_> {
    Accessor {
        component: Component::U16,
        shape: Shape::Scalar,
        count,
        byte_offset: 0,
        byte_stride: None,
        normalized: false,
    }
    .indices(bytes)
    .expect("the fixture fits")
}

/// Four positions in a square, as two triangles will share them.
fn square() -> Vec<u8> {
    floats(&[
        0.0, 0.0, 0.0, // 0
        1.0, 0.0, 0.0, // 1
        1.0, 1.0, 0.0, // 2
        0.0, 1.0, 0.0, // 3
    ])
}

/// Six indices naming those four vertices as two triangles.
fn two_faces() -> Vec<u8> {
    [0u16, 1, 2, 0, 2, 3]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

fn only_positions<'a>(positions: View<'a>, order: Option<Indices<'a>>) -> Primitive<'a> {
    Primitive {
        mode: Mode::Triangles,
        positions,
        normals: None,
        texcoords: None,
        indices: order,
    }
}

/// **Indexed vertices are expanded, once, in the order the file names
/// them.**
#[test]
fn an_indexed_primitive_becomes_de_indexed_triangles() {
    let points = square();
    let order = two_faces();
    let mesh = primitive::build(&only_positions(
        view(&points, Shape::Vec3, 4),
        Some(indices(&order, 6)),
    ))
    .expect("two faces over four vertices");

    assert_eq!(mesh.positions.len(), 6, "three corners per face, expanded");
    assert_eq!(mesh.triangles(), 2);
    same(&mesh.positions[0], &[0.0, 0.0, 0.0], "corner 0");
    same(&mesh.positions[2], &[1.0, 1.0, 0.0], "corner 2");
    // The shared vertex appears twice, which is what de-indexing means.
    same(&mesh.positions[3], &mesh.positions[0], "the shared vertex");
    same(&mesh.positions[5], &[0.0, 1.0, 0.0], "corner 5");
    assert!(
        mesh.face_normals.is_empty(),
        "empty rather than computed: the file carried none"
    );
}

/// Without an index stream the positions are already in order.
#[test]
fn an_unindexed_primitive_takes_its_positions_in_order() {
    let points = floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let mesh =
        primitive::build(&only_positions(view(&points, Shape::Vec3, 3), None)).expect("one face");
    assert_eq!(mesh.triangles(), 1);
    same(&mesh.positions[1], &[1.0, 0.0, 0.0], "corner 1");
}

/// Normals and texture coordinates follow their vertices through the
/// expansion.
#[test]
fn the_optional_streams_are_expanded_with_the_positions() {
    let points = square();
    let order = two_faces();
    let normals = floats(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
    let uvs = floats(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]);

    let mesh = primitive::build(&Primitive {
        normals: Some(view(&normals, Shape::Vec3, 4)),
        texcoords: Some(view(&uvs, Shape::Vec2, 4)),
        ..only_positions(view(&points, Shape::Vec3, 4), Some(indices(&order, 6)))
    })
    .expect("two faces with everything");

    assert_eq!(
        mesh.corner_normals.len(),
        6,
        "one per corner, not per vertex"
    );
    assert_eq!(mesh.corner_texcoords.len(), 6);
    same(
        &mesh.corner_texcoords[2],
        &[1.0, 1.0],
        "vertex 2's coordinate",
    );
    same(
        &mesh.corner_texcoords[3],
        &mesh.corner_texcoords[0],
        "the shared vertex brought its coordinate with it",
    );
}

/// **Streams describing different numbers of vertices, which is the
/// refusal a format with one payload never needs.**
#[test]
fn streams_of_different_lengths_are_refused_by_name() {
    let points = square();
    let three_normals = floats(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);

    let refused = primitive::build(&Primitive {
        normals: Some(view(&three_normals, Shape::Vec3, 3)),
        ..only_positions(view(&points, Shape::Vec3, 4), None)
    })
    .expect_err("three normals for four vertices");
    assert_eq!(
        refused,
        MeshError::StreamLengthMismatch {
            stream: "NORMAL",
            expected: 4,
            found: 3,
        }
    );

    let two_uvs = floats(&[0.0, 0.0, 1.0, 0.0]);
    let refused = primitive::build(&Primitive {
        texcoords: Some(view(&two_uvs, Shape::Vec2, 2)),
        ..only_positions(view(&points, Shape::Vec3, 4), None)
    })
    .expect_err("two coordinates for four vertices");
    assert_eq!(
        refused,
        MeshError::StreamLengthMismatch {
            stream: "TEXCOORD_0",
            expected: 4,
            found: 2,
        }
    );
}

/// A corner count that does not divide into faces.
#[test]
fn corners_that_do_not_divide_into_faces_are_refused() {
    let points = square();
    let four: Vec<u8> = [0u16, 1, 2, 3]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert_eq!(
        primitive::build(&only_positions(
            view(&points, Shape::Vec3, 4),
            Some(indices(&four, 4)),
        ))
        .expect_err("four corners is a face and a corner"),
        MeshError::NotAFace {
            face: 1,
            corners: 1,
        }
    );

    // And without indices, the positions themselves must divide.
    assert_eq!(
        primitive::build(&only_positions(view(&points, Shape::Vec3, 4), None))
            .expect_err("four positions is a face and a corner"),
        MeshError::NotAFace {
            face: 1,
            corners: 1,
        }
    );
}

/// **An index past the end, carrying the face that asked.**
///
/// The last place anything can notice: nothing downstream reads index
/// contents, so this would otherwise draw a plausible wrong picture.
#[test]
fn an_index_past_the_end_is_refused_with_its_face() {
    let points = square();
    let order: Vec<u8> = [0u16, 1, 2, 0, 9, 3]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert_eq!(
        primitive::build(&only_positions(
            view(&points, Shape::Vec3, 4),
            Some(indices(&order, 6)),
        ))
        .expect_err("vertex 9 of four"),
        MeshError::IndexOutOfRange {
            index: 9,
            count: 4,
            face: 1,
        }
    );

    // Equal to the count is out of range too, which is the whole
    // off-by-one this category is about.
    let edge: Vec<u8> = [0u16, 1, 4].iter().flat_map(|v| v.to_le_bytes()).collect();
    assert_eq!(
        primitive::build(&only_positions(
            view(&points, Shape::Vec3, 4),
            Some(indices(&edge, 3)),
        ))
        .expect_err("vertex 4 of four is one past"),
        MeshError::IndexOutOfRange {
            index: 4,
            count: 4,
            face: 0,
        }
    );
}

/// **A mode this reader does not draw is named, not numbered.**
#[test]
fn a_mode_this_reader_does_not_draw_says_which_it_was() {
    let points = square();
    for (mode, wanted) in [
        (Mode::Points, "points"),
        (Mode::Lines, "lines"),
        (Mode::LineLoop, "line loop"),
        (Mode::LineStrip, "line strip"),
        (Mode::TriangleStrip, "triangle strip"),
        (Mode::TriangleFan, "triangle fan"),
    ] {
        let refused = primitive::build(&Primitive {
            mode,
            ..only_positions(view(&points, Shape::Vec3, 4), None)
        })
        .expect_err("only triangles are assembled");
        assert_eq!(refused.name(), "Unsupported");
        assert!(
            refused.to_string().contains(wanted),
            "{mode:?} should say `{wanted}`: {refused}"
        );
    }
}

/// A code the format never defined is a different fault from a mode this
/// reader does not draw.
#[test]
fn a_mode_code_outside_the_table_is_refused_at_the_table() {
    let refused = Mode::from_code(9).expect_err("the table stops at six");
    assert_eq!(refused.name(), "Unsupported");
    assert!(
        refused.to_string().contains("this format defines"),
        "a broken file, not a narrow reader: {refused}"
    );
}

/// A stream of the wrong shape for what it is being used as.
#[test]
fn a_stream_of_the_wrong_shape_is_refused() {
    let flat = floats(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0]);
    let refused = primitive::build(&only_positions(view(&flat, Shape::Vec2, 3), None))
        .expect_err("positions have three components");
    assert!(refused.to_string().contains("three-component positions"));

    let points = square();
    let uvs_as_vec3 = floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
    let refused = primitive::build(&Primitive {
        texcoords: Some(view(&uvs_as_vec3, Shape::Vec3, 4)),
        ..only_positions(view(&points, Shape::Vec3, 4), None)
    })
    .expect_err("texture coordinates have two components");
    assert!(refused.to_string().contains("two-component"));
}

/// A coordinate nothing downstream can bound is refused at the boundary.
#[test]
fn a_position_that_is_not_finite_is_refused() {
    let points = floats(&[0.0, 0.0, 0.0, 1.0, f32::INFINITY, 0.0, 0.0, 1.0, 0.0]);
    assert_eq!(
        primitive::build(&only_positions(view(&points, Shape::Vec3, 3), None))
            .expect_err("an infinite coordinate"),
        MeshError::NotFinite {
            field: "position",
            index: 1,
        }
    );

    // **Three vertices, not four.** The divisibility check runs before
    // any value is read, so a four-vertex fixture never reaches the
    // finiteness test at all — which is the right order and was the
    // wrong fixture.
    let good = floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let nan_uvs = floats(&[0.0, 0.0, 1.0, f32::NAN, 1.0, 1.0]);
    assert_eq!(
        primitive::build(&Primitive {
            texcoords: Some(view(&nan_uvs, Shape::Vec2, 3)),
            ..only_positions(view(&good, Shape::Vec3, 3), None)
        })
        .expect_err("a coordinate that is not a number"),
        MeshError::NotFinite {
            field: "texture coordinate",
            index: 1,
        }
    );
}
