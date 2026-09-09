//! Attribute streams and an index stream, assembled into geometry.
//!
//! This is where the numbers stop being numbers and become a mesh. The
//! layer below hands over typed views that are each individually sound —
//! every element inside the region, every region inside its buffer — and
//! **soundness one stream at a time is not soundness.** Positions and
//! normals describing different numbers of vertices are two valid
//! accessors and one impossible model; an index that addresses element
//! 900 of a 300-element stream is a valid index accessor pointing at
//! nothing.
//!
//! # De-indexing, and why the output has no indices
//!
//! [`Mesh`] is de-indexed triangles: three positions per face, in the
//! file's order. A format that shares a vertex between faces is expanded
//! here, once, while the bounds are being checked anyway. That costs
//! memory and buys the thing the rest of this crate is built on — every
//! reader produces the same shape, so nothing downstream has to ask
//! which format a mesh came from.
//!
//! It also means **an index is checked exactly once**, here, against a
//! count that is known. Nothing downstream reads index-buffer contents,
//! so an index past the end would draw a plausible wrong picture with
//! nothing left to notice.
//!
//! # Refusals, and where they come from
//!
//! These are geometry faults, so they speak [`MeshError`] rather than a
//! vocabulary of their own — unlike the container and the accessor
//! layers, whose faults send a caller somewhere else entirely. A file
//! whose streams disagree is a broken model, which is the same kind of
//! thing as a PLY with a face pointing past its vertex list.

use crate::accessor::{Indices, Shape, View};
use crate::error::MeshError;
use crate::{Mesh, refuse_over_ceiling};

/// How a primitive joins its vertices.
///
/// **All seven the format defines, and one this reader draws.** Keeping
/// the six it does not is what lets a refusal say "this is a triangle
/// fan and I read triangles" rather than "4 is not 6", which are
/// different sentences for the person holding the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Isolated points.
    Points,
    /// Isolated segments.
    Lines,
    /// A closed run of segments.
    LineLoop,
    /// An open run of segments.
    LineStrip,
    /// Isolated triangles — the one this reader assembles.
    Triangles,
    /// A run of triangles sharing an edge.
    TriangleStrip,
    /// A run of triangles sharing a vertex.
    TriangleFan,
}

impl Mode {
    /// The code the format spells this mode with.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Points => 0,
            Self::Lines => 1,
            Self::LineLoop => 2,
            Self::LineStrip => 3,
            Self::Triangles => 4,
            Self::TriangleStrip => 5,
            Self::TriangleFan => 6,
        }
    }

    /// The mode's own name, for a message a person reads.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Points => "points",
            Self::Lines => "lines",
            Self::LineLoop => "a line loop",
            Self::LineStrip => "a line strip",
            Self::Triangles => "triangles",
            Self::TriangleStrip => "a triangle strip",
            Self::TriangleFan => "a triangle fan",
        }
    }

    /// What a refusal says when this mode is not the one being
    /// assembled.
    ///
    /// **The triangles arm is reachable only from a test**, because
    /// `build` asks this exactly when it has established the mode is
    /// something else. It lives here rather than inline for that reason:
    /// inline it was a branch nothing could take, which reads as a case
    /// somebody considered and cannot be exercised by anyone who wants
    /// to know whether it works. Out here it is a small total function,
    /// and the arm says what it would say.
    #[must_use]
    pub const fn instead_of_triangles(self) -> &'static str {
        match self {
            Self::Points => "triangles, and this is points",
            Self::Lines => "triangles, and this is lines",
            Self::LineLoop => "triangles, and this is a line loop",
            Self::LineStrip => "triangles, and this is a line strip",
            Self::TriangleStrip => "triangles, and this is a triangle strip",
            Self::TriangleFan => "triangles, and this is a triangle fan",
            Self::Triangles => "triangles",
        }
    }

    /// The mode a code names.
    ///
    /// # Errors
    ///
    /// [`MeshError::Unsupported`] for a code outside the format's table.
    /// **That is a different fault from a mode this reader does not
    /// draw**, and they are refused in different places for that reason:
    /// a code of 9 is a file describing something the format has never
    /// defined, and a code of 5 is a perfectly good triangle strip this
    /// reader has not been written for. The first is a broken file; the
    /// second is a conversion somebody has to do.
    pub const fn from_code(code: u32) -> Result<Self, MeshError> {
        match code {
            0 => Ok(Self::Points),
            1 => Ok(Self::Lines),
            2 => Ok(Self::LineLoop),
            3 => Ok(Self::LineStrip),
            4 => Ok(Self::Triangles),
            5 => Ok(Self::TriangleStrip),
            6 => Ok(Self::TriangleFan),
            _ => Err(MeshError::Unsupported {
                wanted: "a primitive mode this format defines",
            }),
        }
    }
}

/// The streams one primitive is made of.
///
/// Every view here has already been validated against its own bytes.
/// What is left is whether they describe **the same vertices**, which no
/// one of them can answer alone.
#[derive(Clone, Copy, Debug)]
pub struct Primitive<'a> {
    /// How the vertices are joined.
    pub mode: Mode,
    /// The positions. Three components each, and the stream whose length
    /// every other stream is measured against.
    pub positions: View<'a>,
    /// One normal per vertex, if the file carried them.
    pub normals: Option<View<'a>>,
    /// One texture coordinate per vertex, if the file carried them.
    pub texcoords: Option<View<'a>>,
    /// The order the vertices are visited in, if the file carried one.
    ///
    /// `None` means the positions are already in order, three to a face,
    /// which the format permits and which this reader then checks
    /// divides.
    pub indices: Option<Indices<'a>>,
}

/// Check a stream's shape, naming it the way the file does.
fn require_shape(view: View<'_>, wanted: Shape, missing: &'static str) -> Result<(), MeshError> {
    if view.accessor().shape == wanted {
        return Ok(());
    }
    Err(MeshError::Unsupported { wanted: missing })
}

/// Check a stream's length against the positions, naming which one.
fn require_length(view: View<'_>, vertices: usize, stream: &'static str) -> Result<(), MeshError> {
    if view.len() == vertices {
        return Ok(());
    }
    Err(MeshError::StreamLengthMismatch {
        stream,
        expected: vertices,
        found: view.len(),
    })
}

/// Assemble a primitive into de-indexed triangles.
///
/// # Errors
///
/// A [`MeshError`] naming which stream disagreed and by how much, which
/// index pointed past the end and from which face, or which mode this
/// reader does not draw.
pub fn build(primitive: &Primitive<'_>) -> Result<Mesh, MeshError> {
    if primitive.mode != Mode::Triangles {
        // **Named, not numbered.** The caller is holding a file that is
        // fine and a reader that is narrow, and telling them which is
        // which is the whole value of keeping the other six modes.
        return Err(MeshError::Unsupported {
            wanted: primitive.mode.instead_of_triangles(),
        });
    }

    require_shape(
        primitive.positions,
        Shape::Vec3,
        "three-component positions",
    )?;
    let vertices = primitive.positions.len();

    if let Some(normals) = primitive.normals {
        require_shape(normals, Shape::Vec3, "three-component normals")?;
        require_length(normals, vertices, "NORMAL")?;
    }
    if let Some(texcoords) = primitive.texcoords {
        require_shape(texcoords, Shape::Vec2, "two-component texture coordinates")?;
        require_length(texcoords, vertices, "TEXCOORD_0")?;
    }

    // The corner count: the index stream when there is one, and the
    // positions themselves when there is not.
    let corners = primitive.indices.map_or(vertices, Indices::len);
    if !corners.is_multiple_of(3) {
        // The last face named one or two corners, and neither covers any
        // area.
        return Err(MeshError::NotAFace {
            face: u32::try_from(corners / 3).unwrap_or(u32::MAX),
            corners: corners % 3,
        });
    }
    refuse_over_ceiling(0, corners)?;

    let mut mesh = Mesh {
        positions: Vec::with_capacity(corners),
        face_normals: Vec::new(),
        corner_normals: Vec::new(),
        corner_texcoords: Vec::new(),
    };
    if primitive.normals.is_some() {
        mesh.corner_normals = Vec::with_capacity(corners);
    }
    if primitive.texcoords.is_some() {
        mesh.corner_texcoords = Vec::with_capacity(corners);
    }

    for corner in 0..corners {
        let vertex = match primitive.indices {
            // In range: `at` answers `None` only past the count, and
            // `corner` is below it.
            Some(indices) => {
                let index = indices.at(corner).unwrap_or_default();
                let vertex = usize::try_from(index).unwrap_or(usize::MAX);
                if vertex >= vertices {
                    // **The refusal an indexed format needs, checked
                    // once, here.** Nothing downstream reads index
                    // contents, so this is the last place that can
                    // notice.
                    return Err(MeshError::IndexOutOfRange {
                        index: i64::from(index),
                        count: vertices,
                        face: u32::try_from(corner / 3).unwrap_or(u32::MAX),
                    });
                }
                vertex
            }
            None => corner,
        };

        mesh.positions
            .push(read3(primitive.positions, vertex, "position")?);
        if let Some(normals) = primitive.normals {
            mesh.corner_normals.push(read3(normals, vertex, "normal")?);
        }
        if let Some(texcoords) = primitive.texcoords {
            mesh.corner_texcoords
                .push(read2(texcoords, vertex, "texture coordinate")?);
        }
    }

    // **No emptiness check, because emptiness is unreachable here.** An
    // accessor holds at least one element or it was refused, and a
    // corner count that is not a multiple of three was refused above, so
    // the smallest thing this function can return is one triangle. A
    // check would be a branch nothing can take, which is worse than no
    // check: it reads as a case somebody considered and cannot be
    // exercised by anyone who wants to know whether it works.
    Ok(mesh)
}

/// Three components, refused at the boundary if any is not finite.
fn read3(view: View<'_>, element: usize, field: &'static str) -> Result<[f32; 3], MeshError> {
    let mut out = [0.0f32; 3];
    for (component, slot) in out.iter_mut().enumerate() {
        // In range: the shape was checked and `element` is below the
        // count.
        let value = view.float(element, component).unwrap_or_default();
        if !value.is_finite() {
            return Err(MeshError::NotFinite {
                field,
                index: u32::try_from(element).unwrap_or(u32::MAX),
            });
        }
        *slot = value;
    }
    Ok(out)
}

/// Two components, refused at the boundary if either is not finite.
fn read2(view: View<'_>, element: usize, field: &'static str) -> Result<[f32; 2], MeshError> {
    let mut out = [0.0f32; 2];
    for (component, slot) in out.iter_mut().enumerate() {
        let value = view.float(element, component).unwrap_or_default();
        if !value.is_finite() {
            return Err(MeshError::NotFinite {
                field,
                index: u32::try_from(element).unwrap_or(u32::MAX),
            });
        }
        *slot = value;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::Mode;

    /// Every mode code round-trips, and one outside the table is refused.
    #[test]
    fn every_mode_code_round_trips() {
        for mode in [
            Mode::Points,
            Mode::Lines,
            Mode::LineLoop,
            Mode::LineStrip,
            Mode::Triangles,
            Mode::TriangleStrip,
            Mode::TriangleFan,
        ] {
            assert_eq!(Mode::from_code(mode.code()), Ok(mode));
            assert!(!mode.name().is_empty(), "{mode:?} says nothing");
        }
        assert_eq!(Mode::Triangles.code(), 4);

        // **Every arm of the refusal text, including the one no refusal
        // routes to.** `build` asks for it only when the mode is not
        // triangles, so this is the only thing that reaches that arm —
        // which is the difference between a covered line and an exempted
        // one.
        for mode in [
            Mode::Points,
            Mode::Lines,
            Mode::LineLoop,
            Mode::LineStrip,
            Mode::TriangleStrip,
            Mode::TriangleFan,
        ] {
            let said = mode.instead_of_triangles();
            assert!(
                said.starts_with("triangles, and this is "),
                "{mode:?}: {said}"
            );
        }
        assert_eq!(Mode::Triangles.instead_of_triangles(), "triangles");
        assert!(Mode::from_code(7).is_err(), "the table stops at six");
        assert!(Mode::from_code(u32::MAX).is_err());
    }
}
