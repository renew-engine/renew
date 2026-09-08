//! Readers for the mesh files other tools write.
//!
//! **Bytes in, validated geometry out, and nothing else.** This crate
//! never opens a file, never takes a path and never reads a clock. A
//! caller that reads a file owns the file and owns the bound on reading
//! it; what arrives here is a byte string, which is what lets the same
//! reader serve a file on disk, a member of an archive, and a chunk
//! carved out of the middle of something larger.
//!
//! # What a reader promises
//!
//! Every reader here answers, one way or the other, for every byte
//! string it can be handed. It does not panic, it does not read past
//! what it was given, and when it refuses it says which of the numbered
//! ways in [`MeshError`] the file was wrong, with the numbers and the
//! place. That is the vocabulary the rest of this repository's parsers
//! use and the reason a refusal here is worth more than a boolean.
//!
//! # What a reader does not promise
//!
//! **That the geometry is good, only that it is geometry.** A file whose
//! triangles are degenerate, wound inconsistently, or describe a shape
//! that is not closed reads successfully: those are facts about a model,
//! not about a file, and a reader that refused them would refuse a great
//! deal of real art. What it will not do is hand back a coordinate that
//! is not a finite number, because nothing downstream can bound one.
//!
//! # Example
//!
//! ```
//! use renew_mesh::stl;
//!
//! // A one-triangle binary STL, built here rather than read from disk.
//! let mut bytes = vec![0u8; 80];
//! bytes.extend_from_slice(&1u32.to_le_bytes());
//! for value in [0.0f32, 0.0, 1.0] {
//!     bytes.extend_from_slice(&value.to_le_bytes()); // the normal
//! }
//! for corner in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
//!     for value in corner {
//!         bytes.extend_from_slice(&value.to_le_bytes());
//!     }
//! }
//! bytes.extend_from_slice(&0u16.to_le_bytes()); // the attribute word
//!
//! let mesh = stl::read(&bytes).expect("a well-formed triangle");
//! assert_eq!(mesh.triangles(), 1);
//! assert_eq!(mesh.positions[0], [0.0, 0.0, 0.0]);
//! ```

#![forbid(unsafe_code)]
// A library crate prints nothing: a reader that wrote to a terminal
// would be deciding for its caller where diagnostics go. The same deny
// every engine crate carries at its root.
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod error;
pub mod stl;

pub use error::MeshError;

/// Triangles read out of a file, in the order the file stored them.
///
/// **Parallel arrays rather than a vector of vertex structs**, and the
/// reason is what happens next: this data is going into a packed vertex
/// buffer whose layout is the renderer's business and not this crate's.
/// A struct here would be a second opinion about that layout, and the
/// engine has been bitten twice by a second copy of a vertex record.
///
/// # Invariants a returned mesh holds
///
/// These are the reader's promises, and every one is checked before a
/// mesh is handed back rather than asserted here:
///
/// * `positions.len()` is a multiple of three and is not zero.
/// * `normals` is either empty or exactly `positions.len() / 3` long —
///   one per triangle, which is what the formats that carry normals
///   carry. Per-vertex normals are a later format's problem.
/// * Every float in either array is finite.
///
/// A caller that builds one of these by hand owns those invariants; the
/// fields are public because a reader that hid them would be asking
/// every consumer to copy the data to look at it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    /// Three positions per triangle, in the file's order and winding.
    ///
    /// The winding is not corrected. A format that stores a normal
    /// beside a winding can disagree with itself, and choosing which one
    /// to believe is a decision about a model that the reader has no
    /// standing to make; [`Mesh::winding_disagreements`] counts them so
    /// a caller can decide.
    pub positions: Vec<[f32; 3]>,
    /// One normal per triangle, or empty when the format carried none.
    ///
    /// **Empty rather than computed.** Deriving a normal here would put
    /// a value in the array that the file did not contain, and a caller
    /// cannot then tell what the exporter said from what this crate
    /// guessed. The renderer's own frame derivation is where that guess
    /// belongs, because it is where a caller opts into it.
    pub normals: Vec<[f32; 3]>,
}

impl Mesh {
    /// How many triangles the mesh holds.
    #[must_use]
    pub fn triangles(&self) -> usize {
        self.positions.len() / 3
    }

    /// Whether the mesh holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// How many triangles carry a stored normal that disagrees with the
    /// one their winding implies.
    ///
    /// **The most common thing wrong with a real STL, and a fact rather
    /// than a refusal.** The format stores both a normal and a corner
    /// order, so a file can say a face points two ways at once, and
    /// exporters disagree about which is authoritative often enough that
    /// refusing would refuse a great deal of working art. Counting them
    /// lets a caller decide: a few in a large model is noise from an
    /// exporter's rounding, and *all* of them is a file with its winding
    /// convention inverted, which is worth knowing before it is drawn
    /// inside out.
    ///
    /// Disagreement means the two point into opposite half-spaces — a
    /// negative dot product — rather than any tighter angle. A stored
    /// normal is often a rounded or smoothed thing, so a threshold that
    /// asked for agreement to a few degrees would count ordinary files
    /// as broken.
    ///
    /// Triangles with no stored normal, a zero stored normal, or no
    /// plane of their own are not counted: none of them disagrees with
    /// anything.
    #[must_use]
    pub fn winding_disagreements(&self) -> usize {
        self.normals
            .iter()
            .enumerate()
            .filter(|(triangle, stored)| {
                let at = triangle * 3;
                let Some(corners) = self.positions.get(at..at + 3) else {
                    return false;
                };
                let edge = sub(corners[1], corners[0]);
                let other = sub(corners[2], corners[0]);
                let wound = cross(edge, other);
                // A degenerate triangle has no plane and a zero normal
                // has no direction; neither can disagree with anything.
                dot(wound, **stored) < 0.0
            })
            .count()
    }
}

fn sub(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::Mesh;

    fn triangle(corners: [[f32; 3]; 3], normal: [f32; 3]) -> Mesh {
        Mesh {
            positions: corners.to_vec(),
            normals: vec![normal],
        }
    }

    /// Counting, not measuring: a triangle wound against its own stored
    /// normal is counted, and one wound with it is not.
    ///
    /// Probed by reversing the comparison: red, agreement is counted
    /// instead.
    #[test]
    fn a_normal_against_its_winding_is_counted_once() {
        let corners = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        // These corners wind counter-clockwise seen from +Z.
        assert_eq!(
            triangle(corners, [0.0, 0.0, 1.0]).winding_disagreements(),
            0
        );
        assert_eq!(
            triangle(corners, [0.0, 0.0, -1.0]).winding_disagreements(),
            1
        );
    }

    /// A rounded normal still agrees; only the half-space matters.
    ///
    /// **This is the assertion that keeps the count useful.** An
    /// exporter that writes a smoothed or eight-bit-rounded normal is
    /// not writing a wrong one, and a check that asked for agreement to
    /// a few degrees would report ordinary files as inverted.
    #[test]
    fn a_normal_that_merely_leans_still_agrees() {
        let corners = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        // Forty-five degrees off the true normal, and still the same
        // side of the surface.
        assert_eq!(
            triangle(corners, [0.7, 0.0, 0.7]).winding_disagreements(),
            0
        );
        // Just past the surface, and now it is the other side.
        assert_eq!(
            triangle(corners, [0.7, 0.0, -0.7]).winding_disagreements(),
            1
        );
    }

    /// Nothing without a direction disagrees with anything.
    ///
    /// A zero normal, a degenerate triangle and a mesh with no normals
    /// at all each count zero — and each for its own reason, which is
    /// why all three are here rather than one standing for the others.
    #[test]
    fn what_has_no_direction_disagrees_with_nothing() {
        let corners = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        assert_eq!(
            triangle(corners, [0.0, 0.0, 0.0]).winding_disagreements(),
            0
        );

        let collinear = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [2.0, 2.0, 2.0]];
        assert_eq!(
            triangle(collinear, [0.0, 0.0, -1.0]).winding_disagreements(),
            0
        );

        let unsigned = Mesh {
            positions: corners.to_vec(),
            normals: Vec::new(),
        };
        assert_eq!(unsigned.winding_disagreements(), 0);
    }

    /// The shape of an empty mesh, and of one that is not.
    #[test]
    fn a_mesh_counts_its_triangles() {
        let empty = Mesh::default();
        assert!(empty.is_empty());
        assert_eq!(empty.triangles(), 0);

        let one = triangle(
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0.0, 0.0, 1.0],
        );
        assert!(!one.is_empty());
        assert_eq!(one.triangles(), 1);
    }
}
