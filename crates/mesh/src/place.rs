//! Moving geometry into another space, and joining what was separate.
//!
//! A format that arranges its meshes in a hierarchy hands a reader two
//! things this crate has so far had no use for: a transform per node,
//! and several pieces of geometry that are one model. Both are handled
//! here, and both have a refusal that is easy to leave out.
//!
//! # A normal is not a direction
//!
//! Under a non-uniform scale a surface tilts one way and its normal
//! tilts the other, so a normal is transformed by the **inverse
//! transpose** rather than by the matrix. Under a rotation the two are
//! identical, which is exactly why a reader that skips it looks correct
//! on every rigidly-placed model and is wrong the moment something is
//! squashed.
//!
//! # Why a singular transform is refused only sometimes
//!
//! A scale of zero on one axis is how a modelling tool flattens
//! something, and it is a legal thing for a file to ask for: the
//! positions land on a plane, which is what was asked. **What cannot be
//! done is transforming a normal**, because there is no inverse to
//! transpose — and a reader that pressed on would produce a normal of
//! zero length or of nothing at all, manufacturing a bad value out of a
//! file that contained none.
//!
//! So geometry that carries no normals is placed under a singular
//! transform without complaint, and geometry that carries them is
//! refused. The refusal is about what the reader was asked to compute,
//! not about the matrix in the abstract.

use renew_math::{Mat4, Vec3};

use crate::error::MeshError;
use crate::{Mesh, refuse_over_ceiling};

/// Move `mesh` into the space `transform` describes.
///
/// Positions move by the matrix; normals move by its inverse transpose.
/// **Normals are not renormalised**, because a scale changes their
/// length and whoever needs a unit vector knows better than this
/// function whether a short one is a refusal or a degenerate face.
///
/// # Errors
///
/// [`MeshError::TransformNotInvertible`] when the mesh carries normals
/// and the transform has no inverse, and [`MeshError::NotFinite`] when a
/// transformed coordinate is not a number a bounding box can hold —
/// which a large enough transform can produce from a file whose own
/// coordinates were all finite.
pub fn place(mesh: &mut Mesh, transform: Mat4) -> Result<(), MeshError> {
    let carries_normals = !mesh.face_normals.is_empty() || !mesh.corner_normals.is_empty();
    let normal_matrix = match transform.normal_matrix() {
        Some(matrix) => Some(matrix),
        None if carries_normals => return Err(MeshError::TransformNotInvertible),
        // Legal: the positions flatten, which is what the file asked
        // for, and there are no normals to be wrong about.
        None => None,
    };

    for (index, position) in mesh.positions.iter_mut().enumerate() {
        let moved = transform.transform_point(Vec3::new(position[0], position[1], position[2]));
        *position = finite(moved, "position", index)?;
    }

    let Some(normals) = normal_matrix else {
        return Ok(());
    };
    for (index, normal) in mesh.face_normals.iter_mut().enumerate() {
        let moved = normals.transform_vector(Vec3::new(normal[0], normal[1], normal[2]));
        *normal = finite(moved, "face normal", index)?;
    }
    for (index, normal) in mesh.corner_normals.iter_mut().enumerate() {
        let moved = normals.transform_vector(Vec3::new(normal[0], normal[1], normal[2]));
        *normal = finite(moved, "corner normal", index)?;
    }
    Ok(())
}

/// A transformed vector, refused at the boundary if it is not finite.
fn finite(value: Vec3, field: &'static str, index: usize) -> Result<[f32; 3], MeshError> {
    let out = [value.x, value.y, value.z];
    for component in out {
        if !component.is_finite() {
            return Err(MeshError::NotFinite {
                field,
                index: u32::try_from(index).unwrap_or(u32::MAX),
            });
        }
    }
    Ok(out)
}

/// Append `other`'s geometry to `mesh`.
///
/// **The refusal here is that two meshes disagree about what they
/// carry.** A format that stores one model as several pieces lets each
/// piece declare its own attributes, so one may have normals and the
/// next may not — and a reader that simply concatenated would produce a
/// mesh whose normal array is shorter than its triangle count, which is
/// the ragged shape [`MeshError::StreamLengthMismatch`] exists to
/// prevent one layer down.
///
/// **Empty is not a disagreement about nothing.** A piece that carries
/// no normals and a piece that does cannot be joined, even though
/// appending would "work" — the result would silently be a model whose
/// second half has no lighting information and whose arrays no longer
/// line up.
///
/// # Errors
///
/// [`MeshError::StreamLengthMismatch`] when the two disagree about an
/// optional array, and [`MeshError::TooLarge`] when the total passes the
/// ceiling this crate sets on geometry.
pub fn append(mesh: &mut Mesh, other: &Mesh) -> Result<(), MeshError> {
    let pairs: [(&str, bool, bool); 3] = [
        (
            "face normal",
            mesh.face_normals.is_empty(),
            other.face_normals.is_empty(),
        ),
        (
            "corner normal",
            mesh.corner_normals.is_empty(),
            other.corner_normals.is_empty(),
        ),
        (
            "corner texture coordinate",
            mesh.corner_texcoords.is_empty(),
            other.corner_texcoords.is_empty(),
        ),
    ];
    for (stream, here, there) in pairs {
        if here != there {
            // The counts say which way the disagreement runs: one side
            // has a stream for its geometry and the other has none.
            return Err(MeshError::StreamLengthMismatch {
                stream,
                expected: if here { 0 } else { mesh.triangles() },
                found: if there { 0 } else { other.triangles() },
            });
        }
    }

    refuse_over_ceiling(mesh.positions.len(), other.positions.len())?;

    mesh.positions.extend_from_slice(&other.positions);
    mesh.face_normals.extend_from_slice(&other.face_normals);
    mesh.corner_normals.extend_from_slice(&other.corner_normals);
    mesh.corner_texcoords
        .extend_from_slice(&other.corner_texcoords);
    Ok(())
}
