//! A chunked voxel volume of opaque voxel identifiers.
//!
//! **This crate names no voxel.** It stores a [`Voxel`] per cell and
//! has no opinion about which ones exist or what any of them does — sand,
//! stone and wood are the game's vocabulary and live above this, and the
//! boundary is the whole reason this crate could move upstream one day.
//! It draws nothing and, after construction, allocates nothing.
//!
//! # Why the hashes are maintained on write
//!
//! Digesting a volume by walking every cell is correct and free at a few
//! thousand cells. This is aimed at millions, where a full walk per tick
//! is not a cost to optimise later but a decision that has to be made
//! now: **a running per-chunk hash is not retrofittable**, because
//! retrofitting one means finding every write that ever existed and
//! proving none was missed. So every mutation updates its chunk's hash as
//! it happens, and [`Volume::digest`] folds chunk hashes rather than
//! cells.
//!
//! # Determinism
//!
//! Every iteration order this crate exposes is stated and stable, and no
//! floating point appears anywhere — positions are fixed point, and the
//! crate denies `clippy::float_arithmetic` so that a lapse reddens the
//! lint gate rather than surfacing as a divergence three platforms later.
//! That deny is a lint and not a compiler check: it fires under `cargo
//! clippy`, not `cargo build`, it does not reach the integration tests in
//! `tests/`, and it catches the arithmetic operators rather than every
//! mention of a float. It is a tripwire, not a proof.
//!
//! Cells are **centred on integers**, matching the convention the engine's
//! own voxel sample uses: cell zero spans −0.5 to +0.5, so every cell's
//! half-extent is exactly one half and the arithmetic stays exact.

#![deny(clippy::float_arithmetic)]

mod pick;
mod sweep;
mod volume;

pub use pick::{MAX_PICK_STEPS, Pick};
pub use sweep::Hit;
pub use volume::{CHUNK, CHUNK_CELLS, Volume};

use renew_fixed::{Fixed, Vec3};

/// What a cell is made of, as an opaque identifier.
///
/// **Deliberately meaningless here.** The volume compares voxels for
/// equality and stores them; it never asks what one *is*. [`Voxel::EMPTY`]
/// is the single exception, because "is there anything here" is a question
/// about storage rather than about anybody's voxel table.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Voxel(pub u16);

impl Voxel {
    /// Absence.
    pub const EMPTY: Self = Self(0);

    /// Whether this cell holds anything at all.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == Self::EMPTY.0
    }
}

/// A cell address in the volume's lattice.
///
/// Ordering is lexicographic by `(x, y, z)` and exists so that ties between
/// equally-close candidates resolve the same way on every machine, which is
/// what keeps a sweep from depending on iteration order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Cell {
    /// East.
    pub x: i32,
    /// Up.
    pub y: i32,
    /// North.
    pub z: i32,
}

impl Cell {
    /// A cell.
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The centre of this cell in world space.
    #[must_use]
    pub fn centre(self) -> Vec3 {
        Vec3::new(
            Fixed::from_int(self.x),
            Fixed::from_int(self.y),
            Fixed::from_int(self.z),
        )
    }

    /// This cell offset by whole steps, saturating rather than wrapping.
    #[must_use]
    pub const fn offset(self, x: i32, y: i32, z: i32) -> Self {
        Self::new(
            self.x.saturating_add(x),
            self.y.saturating_add(y),
            self.z.saturating_add(z),
        )
    }

    /// Which cell a world position falls in.
    ///
    /// Rounds to nearest, which is what pairs with centring cells on
    /// integers. A position exactly on a boundary resolves to the higher
    /// cell for both signs, so two bodies on the same seam agree about
    /// where they are.
    #[must_use]
    pub fn containing(position: Vec3) -> Self {
        Self::new(
            round_to_cell(position.x),
            round_to_cell(position.y),
            round_to_cell(position.z),
        )
    }
}

/// Round a world coordinate to the cell containing it.
fn round_to_cell(value: Fixed) -> i32 {
    // Adding half a unit and flooring puts the boundary at the higher cell
    // for both signs. Truncation alone rounds toward zero, which would
    // split the boundary rule between positive and negative coordinates —
    // a seam that behaves differently either side of the origin.
    let raised = value + Fixed::from_ratio(1, 2);
    let floored = raised.to_bits().div_euclid(FIXED_ONE_RAW);
    i32::try_from(floored).unwrap_or(if floored < 0 { i32::MIN } else { i32::MAX })
}

/// The raw value of one whole unit in the engine's fixed-point scalar.
///
/// Read from the type rather than written as a literal, so a change to the
/// engine's fractional width is a compile-time fact here rather than a
/// silent factor-of-something bug in every coordinate conversion.
const FIXED_ONE_RAW: i64 = Fixed::ONE.to_bits();

/// Half of one cell, which is every cell's half-extent.
#[must_use]
pub fn cell_half_extent() -> Vec3 {
    let half = Fixed::from_ratio(1, 2);
    Vec3::new(half, half, half)
}

/// Which side of a cell something arrived through.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Face {
    /// Toward +x.
    East,
    /// Toward −x.
    West,
    /// Toward +y.
    Top,
    /// Toward −y.
    Bottom,
    /// Toward +z.
    North,
    /// Toward −z.
    South,
}

impl Face {
    /// The whole-step offset from a cell to its neighbour on this side.
    #[must_use]
    pub const fn step(self) -> (i32, i32, i32) {
        match self {
            Self::East => (1, 0, 0),
            Self::West => (-1, 0, 0),
            Self::Top => (0, 1, 0),
            Self::Bottom => (0, -1, 0),
            Self::North => (0, 0, 1),
            Self::South => (0, 0, -1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cell, FIXED_ONE_RAW, Face, Voxel};
    use renew_fixed::{Fixed, Vec3};

    #[test]
    fn the_empty_voxel_is_the_only_one_that_reads_as_absent() {
        assert!(Voxel::EMPTY.is_empty());
        assert!(!Voxel(1).is_empty());
        assert!(!Voxel(u16::MAX).is_empty());
    }

    #[test]
    fn a_cell_centre_round_trips_to_its_own_cell() {
        for (x, y, z) in [(0, 0, 0), (3, -4, 7), (-1, -1, -1), (100, 0, -100)] {
            let cell = Cell::new(x, y, z);
            assert_eq!(Cell::containing(cell.centre()), cell);
        }
    }

    #[test]
    fn the_boundary_resolves_upward_on_both_sides_of_the_origin() {
        let half = Fixed::from_ratio(1, 2);
        // +0.5 is the seam between cell 0 and cell 1, and −0.5 the seam
        // between −1 and 0. Both must go to the higher cell, or the rule
        // changes sign at the origin.
        assert_eq!(
            Cell::containing(Vec3::new(half, half, half)),
            Cell::new(1, 1, 1)
        );
        assert_eq!(
            Cell::containing(Vec3::new(-half, -half, -half)),
            Cell::new(0, 0, 0)
        );
    }

    #[test]
    fn one_unit_is_read_from_the_engines_type_not_assumed() {
        // If the engine ever changes its fractional width, this is the test
        // that says so rather than every coordinate quietly shifting.
        assert_eq!(FIXED_ONE_RAW, Fixed::ONE.to_bits());
        assert_eq!(Fixed::from_int(1).to_bits(), FIXED_ONE_RAW);
    }

    #[test]
    fn every_face_steps_to_a_distinct_neighbour() {
        let faces = [
            Face::East,
            Face::West,
            Face::Top,
            Face::Bottom,
            Face::North,
            Face::South,
        ];
        let origin = Cell::new(0, 0, 0);
        let mut seen = Vec::new();
        for face in faces {
            let (x, y, z) = face.step();
            let neighbour = origin.offset(x, y, z);
            assert_ne!(neighbour, origin, "{face:?} did not move");
            assert!(!seen.contains(&neighbour), "{face:?} collided with another");
            seen.push(neighbour);
        }
    }

    #[test]
    fn offsetting_saturates_rather_than_wrapping() {
        let edge = Cell::new(i32::MAX, i32::MIN, 0);
        let moved = edge.offset(1, -1, 0);
        assert_eq!(moved.x, i32::MAX);
        assert_eq!(moved.y, i32::MIN);
    }
}
