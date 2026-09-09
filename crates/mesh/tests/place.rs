//! Geometry moved into another space, and pieces joined into one model.
//!
//! **The test this file exists for is the normal.** Under a rotation a
//! normal transformed as a direction and a normal transformed properly
//! are identical, so a reader that skips the inverse transpose passes
//! every rigidly-placed fixture. Only a non-uniform scale tells them
//! apart, and there is one here for that reason.

// An integration test is its own crate, so the `allow-*-in-tests`
// settings in `clippy.toml` do not reach it.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use renew_math::{Mat4, Quat, Vec3};
use renew_mesh::{Mesh, MeshError, place};

/// Compare coordinates by value, within the tolerance a transform earns.
///
/// **Not by bits here, unlike the assembly suite and the import
/// golden**: those compare fixtures whose operands make the arithmetic
/// exact, and this suite deliberately uses a non-uniform scale and a
/// rotation, where it is not. Placement multiplies and adds, so the
/// result is the arithmetic's rather than the file's, and an exact
/// comparison would be asserting something about rounding rather than
/// about the transform.
fn near(got: [f32; 3], want: [f32; 3], what: &str) {
    for (index, (left, right)) in got.iter().zip(&want).enumerate() {
        assert!(
            (left - right).abs() < 1e-5,
            "{what}: component {index} is {left}, not {right}"
        );
    }
}

/// One triangle with a normal per face and per corner.
fn furnished() -> Mesh {
    Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        face_normals: vec![[0.0, 0.0, 1.0]],
        // **Three different normals, not three copies of one.** A
        // fixture that repeats a value cannot see a reader that permutes
        // it, and this suite had three copies until a mutation walked
        // through it untouched.
        corner_normals: vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
        corner_texcoords: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
    }
}

/// One triangle and nothing optional.
fn bare() -> Mesh {
    Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        ..Mesh::default()
    }
}

/// A translation moves points and leaves normals where they were.
#[test]
fn a_translation_moves_points_and_not_normals() {
    let mut mesh = furnished();
    place::place(
        &mut mesh,
        Mat4::from_translation(Vec3::new(10.0, 20.0, 30.0)),
    )
    .expect("a translation is invertible");
    near(mesh.positions[0], [10.0, 20.0, 30.0], "the first corner");
    near(mesh.positions[1], [11.0, 20.0, 30.0], "the second");
    near(mesh.face_normals[0], [0.0, 0.0, 1.0], "the face normal");
    near(
        mesh.corner_normals[0],
        [0.0, 0.0, 1.0],
        "the first corner normal",
    );
    near(mesh.corner_normals[1], [0.0, 1.0, 0.0], "the second");
    near(mesh.corner_normals[2], [1.0, 0.0, 0.0], "the third");
}

/// **A non-uniform scale is the case that tells the two ways of moving a
/// normal apart.**
///
/// The triangle lies in the z = 0 plane with normal `+z`. Squash x by
/// two and the surface is still in that plane, so the normal must stay
/// `+z` in direction — and the inverse transpose keeps it there while
/// scaling its length, whereas transforming it as a direction would
/// leave it unchanged here and wrong on a tilted face.
///
/// So the sharper claim is the tilted one below.
#[test]
fn a_non_uniform_scale_tilts_a_normal_the_other_way() {
    // A face tilted 45 degrees in the x-y plane: normal (1, 1, 0),
    // tangent (-1, 1, 0), both unnormalised on purpose so the arithmetic
    // is exact to read.
    let mut mesh = Mesh {
        positions: vec![[0.0, 0.0, 0.0], [-1.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        face_normals: vec![[1.0, 1.0, 0.0]],
        ..Mesh::default()
    };
    place::place(&mut mesh, Mat4::from_scale(Vec3::new(2.0, 1.0, 1.0)))
        .expect("a scale is invertible");

    // The tangent moved with the surface.
    let tangent = mesh.positions[1];
    near(tangent, [-2.0, 1.0, 0.0], "the tilted edge");

    // And the normal is still perpendicular to it.
    let normal = mesh.face_normals[0];
    let dot = tangent[0] * normal[0] + tangent[1] * normal[1] + tangent[2] * normal[2];
    assert!(
        dot.abs() < 1e-5,
        "the normal must stay perpendicular to its surface: {dot}"
    );

    // **The number a reader that skipped the inverse transpose would
    // have.** Transforming the normal as a direction gives (2, 1, 0),
    // whose dot with the moved tangent is -3.
    assert!(
        (normal[0] - 0.5).abs() < 1e-5,
        "the inverse transpose halves x where the matrix doubles it: {normal:?}"
    );
}

/// **A corner normal goes through the same inverse transpose**, and
/// nothing here proved it until a mutation said otherwise.
///
/// The suite exercised the inverse transpose on `face_normals` under a
/// scale and a rotation, and touched `corner_normals` only under a
/// translation -- where leaving them alone is the correct answer. So a
/// reader that transformed the face normals and copied the corner
/// normals through unchanged passed every test in this crate.
#[test]
fn a_non_uniform_scale_tilts_a_corner_normal_too() {
    let mut mesh = Mesh {
        positions: vec![[0.0, 0.0, 0.0], [-1.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        // Three distinct normals, each of which the matrix moves, so a
        // permutation of them is visible as well as a skipped transform.
        corner_normals: vec![[1.0, 1.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
        ..Mesh::default()
    };
    place::place(&mut mesh, Mat4::from_scale(Vec3::new(2.0, 1.0, 1.0)))
        .expect("a scale is invertible");

    // The inverse transpose halves x where the matrix doubles it, which
    // is the whole difference between transforming a normal and
    // transforming a direction.
    near(
        mesh.corner_normals[0],
        [0.5, 1.0, 0.0],
        "the first corner normal",
    );
    near(mesh.corner_normals[1], [1.0, 0.0, 0.0], "the second");
    near(mesh.corner_normals[2], [0.5, 0.0, 1.0], "the third");
}

/// Under a rotation the two ways agree, which is what hides the bug.
#[test]
fn a_rotation_moves_a_normal_the_same_way_either_route() {
    let turn = Mat4::from_quat(Quat::from_axis_angle(Vec3::Y, 0.9));
    let mut mesh = furnished();
    place::place(&mut mesh, turn).expect("a rotation is invertible");

    let expected = turn.transform_vector(Vec3::new(0.0, 0.0, 1.0));
    near(
        mesh.face_normals[0],
        [expected.x, expected.y, expected.z],
        "identical under a rotation, which is what hides a missing inverse transpose",
    );
}

/// **A transform that flattens space is refused only when there are
/// normals to transform.**
#[test]
fn a_singular_transform_is_refused_only_when_normals_need_it() {
    let flatten = Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0));

    let mut with_normals = furnished();
    assert_eq!(
        place::place(&mut with_normals, flatten).expect_err("no inverse to transpose"),
        MeshError::TransformNotInvertible
    );

    // **And the same transform on the same geometry without normals is
    // fine**, because flattening is what the file asked for and there is
    // nothing left to be wrong about.
    let mut without = bare();
    place::place(&mut without, flatten).expect("positions may be flattened");
    near(without.positions[2], [0.0, 0.0, 0.0], "y collapsed to zero");
    near(without.positions[1], [1.0, 0.0, 0.0], "and x did not");
}

/// A refused placement is refused before anything is written.
#[test]
fn a_refused_placement_leaves_the_mesh_alone() {
    let mut mesh = furnished();
    let before = mesh.positions.clone();
    let _ = place::place(&mut mesh, Mat4::from_scale(Vec3::new(0.0, 1.0, 1.0)));
    assert_eq!(
        mesh.positions, before,
        "the refusal comes before any coordinate is touched"
    );
}

/// A transform large enough to overflow is refused at the boundary.
#[test]
fn a_coordinate_transformed_out_of_range_is_refused() {
    let mut mesh = Mesh {
        positions: vec![[1e30, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        ..Mesh::default()
    };
    assert_eq!(
        place::place(&mut mesh, Mat4::from_scale(Vec3::new(1e30, 1.0, 1.0)))
            .expect_err("1e30 squared is not a float"),
        MeshError::NotFinite {
            field: "position",
            index: 0,
        }
    );
}

/// Two pieces that carry the same arrays join into one model.
#[test]
fn two_pieces_carrying_the_same_arrays_are_joined() {
    let mut first = furnished();
    let second = furnished();
    place::append(&mut first, &second).expect("both carry everything");
    assert_eq!(first.triangles(), 2);
    assert_eq!(first.positions.len(), 6);
    assert_eq!(first.face_normals.len(), 2);
    assert_eq!(first.corner_normals.len(), 6);
    assert_eq!(first.corner_texcoords.len(), 6);
}

/// Two bare pieces join too: agreeing about carrying nothing is
/// agreement.
#[test]
fn two_pieces_carrying_nothing_optional_are_joined() {
    let mut first = bare();
    place::append(&mut first, &bare()).expect("both carry nothing");
    assert_eq!(first.triangles(), 2);
    assert!(first.face_normals.is_empty());
}

/// **Pieces that disagree about an optional array are refused, by
/// name.**
///
/// Concatenating them would produce a mesh whose normal array is shorter
/// than its triangle count — the ragged shape the layer below refuses —
/// and whose second half silently has no lighting information.
#[test]
fn pieces_that_disagree_about_a_stream_are_refused() {
    let mut furnished_first = furnished();
    assert_eq!(
        place::append(&mut furnished_first, &bare()).expect_err("one has normals, one does not"),
        MeshError::StreamLengthMismatch {
            stream: "face normal",
            expected: 1,
            found: 0,
        }
    );

    // And the other way round, so the message is about which side is
    // missing rather than about argument order.
    let mut bare_first = bare();
    assert_eq!(
        place::append(&mut bare_first, &furnished()).expect_err("still a disagreement"),
        MeshError::StreamLengthMismatch {
            stream: "face normal",
            expected: 0,
            found: 1,
        }
    );

    // Texture coordinates are their own disagreement, not folded into
    // the normals'.
    let mut only_uvs = Mesh {
        corner_texcoords: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        ..bare()
    };
    let refused = place::append(&mut only_uvs, &bare()).expect_err("coordinates on one side");
    assert_eq!(refused.name(), "StreamLengthMismatch");
    assert!(
        refused.to_string().contains("corner texture coordinate"),
        "the message names which stream: {refused}"
    );
}

/// A refused append leaves both meshes as they were.
#[test]
fn a_refused_append_changes_nothing() {
    let mut first = furnished();
    let before = first.positions.len();
    let _ = place::append(&mut first, &bare());
    assert_eq!(first.positions.len(), before, "nothing was appended");
}
