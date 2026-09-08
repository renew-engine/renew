//! Readers for the mesh files other tools write, and a writer for the
//! one this repository owns.
//!
//! **Bytes in, validated geometry out, and bytes back out again.** This
//! crate never opens a file, never takes a path and never reads a clock. A
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

pub mod accessor;
pub mod blob;
mod error;
pub mod format;
pub mod glb;
pub mod mtl;
pub mod obj;
pub mod ply;
pub mod stl;

/// The most geometry this reader will build out of one file, in bytes.
///
/// **A policy ceiling, not a representation limit, and the two are not
/// the same refusal.** A representation limit is reached only after the
/// allocation has been attempted; this one is a refusal that costs
/// nothing. It is the same reasoning the image decoder gives for its own
/// two hundred and fifty-six megabytes, and this is the same number, for
/// the same reason: far past any model a game loads and far short of
/// anything that hurts.
///
/// **The ceilings on the factors were not enough, which is the whole
/// point of this one.** `MAX_FACE_CORNERS` bounds a single face and
/// `refuse_impossible_count` bounds a row count against the bytes that
/// could supply it — and neither bounds their product. A fan turns a
/// face of `n` corners into `(n - 2) * 3` positions, so a file of a
/// megabyte, every byte of it legitimate, built fifty-eight megabytes of
/// geometry. Linear in the input and therefore inside the letter of the
/// rule that a refused input costs no more than its own length buys; and
/// a caller adopting the image decoder's own file bound would still have
/// been handed twelve gigabytes from one mesh. Amplification is the
/// danger, not allocation.
pub(crate) const MAX_GEOMETRY_BYTES: usize = 256 << 20;

/// How many positions that ceiling allows.
pub(crate) const MAX_POSITIONS: usize = MAX_GEOMETRY_BYTES / core::mem::size_of::<[f32; 3]>();

/// The ceiling is what keeps a corner count from overflowing the
/// arithmetic that turns it into a byte length, so it has to stay inside
/// the narrowest pointer this engine is built for.
///
/// **Checked here rather than trusted**, because the two numbers are
/// independent: raising `MAX_GEOMETRY_BYTES` far enough would let a
/// count through that `corners * 36` cannot hold on a 32-bit target, and
/// a release build has no overflow checks to notice. Thirty-six is the
/// bytes one corner costs with every optional array present — twelve
/// for the position, four for its share of a face normal, twelve for a
/// corner normal, eight for a coordinate.
const _: () = {
    assert!(
        MAX_POSITIONS < u32::MAX as usize / 36,
        "a corner count at the ceiling must survive being multiplied by the bytes a corner \
         costs, on the narrowest target this engine builds for"
    );
};

/// Refuse before the geometry arrives rather than after it.
///
/// `have` is what has been emitted, `adding` what the next face would
/// add. The sum is what the ceiling is on: a cap on one face's corners
/// and a cap on the row count bound neither their product, which is the
/// whole reason this exists.
///
/// **Shared by every reader that fans a polygon, and split out so it can be proved without allocating the
/// quarter-gigabyte it exists to prevent.** Reaching the branch in place
/// takes a thirty-megabyte input that first emits every position under
/// the ceiling; the arithmetic is the same either way, and the test that
/// pins it is beside the constant rather than inside a fixture nobody
/// would run twice.
pub(crate) fn refuse_over_ceiling(have: usize, adding: usize) -> Result<(), MeshError> {
    let total = have.saturating_add(adding);
    if total > MAX_POSITIONS {
        return Err(MeshError::TooLarge {
            field: "total geometry",
            // Reported in bytes, which is the unit the ceiling is
            // written in and the one a caller can act on.
            value: (total.saturating_mul(core::mem::size_of::<[f32; 3]>())) as u64,
        });
    }
    Ok(())
}

pub use accessor::{Accessor, AccessorError, Component, Shape};
pub use error::MeshError;
pub use glb::{Container, GlbError};

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
/// * `face_normals` is either empty or exactly `positions.len() / 3`
///   long — one per triangle.
/// * `corner_normals` and `corner_texcoords` are each either empty or
///   exactly `positions.len()` long — one per corner.
/// * Every float in every array is finite.
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
    ///
    /// A *face* normal is one the file stated for the whole triangle,
    /// which is what STL stores. It is not the same fact as a normal per
    /// corner, and the two live in separate arrays rather than one
    /// array with a convention, because a reader that folded them would
    /// be answering a question about smoothing that belongs to whoever
    /// draws the mesh.
    pub face_normals: Vec<[f32; 3]>,
    /// One normal per corner — so `positions.len()` of them — or empty
    /// when the format carried none.
    ///
    /// This is what a format with an indexed, per-vertex normal stream
    /// carries: two triangles sharing an edge can name different normals
    /// at the same point, which is how a hard edge and a smooth one are
    /// told apart, and averaging them into one per face would throw that
    /// away irrecoverably.
    pub corner_normals: Vec<[f32; 3]>,
    /// One texture coordinate per corner, or empty when the format
    /// carried none.
    ///
    /// Per corner rather than per position for the same reason as the
    /// normals: a seam in a UV map is exactly one position carrying two
    /// different coordinates in two different faces.
    ///
    /// **Not clamped, and not flipped.** Coordinates outside the unit
    /// square are ordinary — they are how a texture is made to repeat —
    /// and which end of the vertical axis is zero is a convention the
    /// file does not state, so a reader that flipped it would be
    /// guessing on the caller's behalf.
    pub corner_texcoords: Vec<[f32; 2]>,
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
        self.face_normals
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
            face_normals: vec![normal],
            ..Mesh::default()
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

    /// **Each triangle's normal is compared against its own corners.**
    ///
    /// The count walks two arrays together, and every test of it used a
    /// mesh whose triangles share their corners — so `at = triangle * 3`
    /// was invisible, and a mutant comparing every normal against
    /// triangle zero survived the whole suite. Two triangles wound
    /// differently is what separates them.
    #[test]
    fn a_normal_is_compared_against_its_own_triangle() {
        // Both lie in the XY plane and both carry +Z, but the second is
        // wound the other way round, so its own corners say -Z. Answered
        // per-triangle that is one disagreement; answered against
        // triangle zero for both it is none.
        //
        // A first attempt put the second triangle in the XZ plane, where
        // its wound normal is PERPENDICULAR to the stored one — which is
        // neither agreement nor disagreement, so the count was zero
        // either way and the fixture could not tell the two apart.
        let mesh = Mesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
            ],
            face_normals: vec![[0.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
            ..Mesh::default()
        };
        assert_eq!(
            mesh.winding_disagreements(),
            1,
            "the second triangle is wound against the normal it carries"
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
            face_normals: Vec::new(),
            ..Mesh::default()
        };
        assert_eq!(unsigned.winding_disagreements(), 0);
    }

    /// **A normal with no triangle under it is not counted.**
    ///
    /// The count zips normals against position triples and asks
    /// `positions.get(at..at + 3)`, so a mesh carrying more normals than
    /// triangles takes the `None` arm. No reader here produces one — both
    /// check the pairing — but `Mesh` is a public type a caller builds by
    /// hand, and the arm existed with nothing reaching it.
    ///
    /// Probed by returning `true` there: red, the spare normal counts as
    /// a disagreement.
    #[test]
    fn a_normal_past_the_last_triangle_is_not_counted() {
        let one_triangle_two_normals = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            // The first agrees with the winding; the second has no
            // corners to agree or disagree with.
            face_normals: vec![[0.0, 0.0, 1.0], [0.0, 0.0, -1.0]],
            ..Mesh::default()
        };
        assert_eq!(one_triangle_two_normals.winding_disagreements(), 0);
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

#[cfg(test)]
mod ceiling_tests {
    use super::{MAX_POSITIONS, MeshError, refuse_over_ceiling};

    /// **The ceiling is on the product, and it refuses at the boundary
    /// rather than past it.**
    ///
    /// Probed by deleting the check: red, the over-ceiling case is
    /// accepted. Probed by widening `>` to `>=`: red, the exactly-full
    /// case is refused when it fits.
    #[test]
    fn the_geometry_ceiling_counts_what_is_there_and_what_is_coming() {
        refuse_over_ceiling(MAX_POSITIONS - 3, 3)
            .expect("a mesh that exactly fills the ceiling is not over it");

        // Compared whole rather than destructured: a `let ... else`
        // panic is a line only a failing run reaches, and this test is
        // measured like the code beside it.
        assert_eq!(
            refuse_over_ceiling(MAX_POSITIONS - 3, 6),
            Err(MeshError::TooLarge {
                field: "total geometry",
                // Bytes, not positions: the unit the ceiling is written
                // in and the one a caller can act on.
                value: (MAX_POSITIONS + 3) as u64 * 12,
            }),
            "three positions past the ceiling is over it, and it says so by name"
        );

        // Neither factor alone reaches it, which is the case a ceiling
        // on the factors would miss.
        refuse_over_ceiling(MAX_POSITIONS - 1, 1).expect("still inside");
        refuse_over_ceiling(usize::MAX, 1)
            .expect_err("the sum saturates rather than wrapping under the ceiling");
    }
}
