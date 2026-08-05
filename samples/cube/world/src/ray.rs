//! Which block is being looked at.

use crate::grid::{Cell, Grid};
use renew_fixed::{Fixed, Vec3};

/// Which side of a block a ray entered through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// The whole-step offset from a block to the cell on this side of it.
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

/// A block a ray met, and which way it came in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    /// The block.
    pub cell: Cell,
    /// The face entered through.
    pub face: Face,
}

impl Pick {
    /// The empty cell on the near side of the face.
    ///
    /// **Where a placed block goes.** Placing into the picked cell would
    /// replace the block being looked at, which is what digging does — a
    /// distinction every player expects and every implementation has to make
    /// explicitly.
    #[must_use]
    pub const fn neighbour(self) -> Cell {
        let (x, y, z) = self.face.step();
        self.cell.offset(x, y, z)
    }
}

/// How many cells a pick may step through before giving up.
///
/// The reach bounds it in world units and this bounds it in work: a ray very
/// nearly parallel to an axis crosses many cells per unit travelled, and a
/// grid the ray never leaves would otherwise walk until the reach ran out one
/// tiny step at a time. Fixed rather than tuned, because a pick that visited a
/// machine-dependent number of cells would pick machine-dependent blocks.
pub const MAX_STEPS: u32 = 256;

/// The first solid block along a ray, within `reach`.
///
/// # How
///
/// Grid traversal rather than a shape query: step from cell to cell along the
/// ray, always crossing whichever axis boundary comes first. That visits every
/// cell the ray actually passes through, in order, and stops at the first
/// solid one — where testing every block in reach would be thousands of tests
/// to answer a question about a handful.
///
/// The alternative worth naming is sampling the ray at fixed intervals, which
/// is simpler and wrong: a step longer than a cell skips blocks, and a step
/// short enough not to visits the same cell many times over.
#[must_use]
pub fn pick(grid: &Grid, origin: Vec3, direction: Vec3, reach: Fixed) -> Option<Pick> {
    let mut cell = Cell::containing(origin);

    // Per axis: which way we step, how far along the ray one whole cell is,
    // and how far along the ray the next boundary is.
    let mut step = [0i32; 3];
    let mut delta = [Fixed::ZERO; 3];
    let mut next = [Fixed::ZERO; 3];
    let components = [direction.x, direction.y, direction.z];
    let positions = [origin.x, origin.y, origin.z];
    let cells = [cell.x, cell.y, cell.z];

    for axis in 0..3 {
        let towards = components[axis];
        if towards == Fixed::ZERO {
            // Parallel to this axis: never crosses one of its boundaries. A
            // boundary infinitely far away is the honest encoding, and it
            // keeps the comparison below uniform instead of special-casing.
            step[axis] = 0;
            delta[axis] = Fixed::MAX;
            next[axis] = Fixed::MAX;
            continue;
        }
        let Some(per_cell) = Fixed::ONE.checked_div(towards.abs()) else {
            step[axis] = 0;
            delta[axis] = Fixed::MAX;
            next[axis] = Fixed::MAX;
            continue;
        };
        delta[axis] = per_cell;
        // The cell spans half a unit either side of its centre, so the
        // distance to the boundary ahead depends on which way we are going.
        let centre = Fixed::from_int(cells[axis]);
        let boundary = if towards > Fixed::ZERO {
            step[axis] = 1;
            centre + Fixed::from_ratio(1, 2)
        } else {
            step[axis] = -1;
            centre - Fixed::from_ratio(1, 2)
        };
        let gap = (boundary - positions[axis]).abs();
        next[axis] = gap.saturating_mul(per_cell);
    }

    // The origin itself may be inside a block, which a player standing in one
    // should be told about rather than having the ray start beyond it.
    if grid.is_solid(cell) {
        return Some(Pick {
            cell,
            // Entered from nowhere; the face the ray is heading away from is
            // the useful answer, since that is where a block would go.
            face: entry_face(0, -step[0].signum()).unwrap_or(Face::Top),
        });
    }

    for _ in 0..MAX_STEPS {
        // Whichever boundary comes first. Ties go to the lower axis, so a ray
        // through a corner enters through a stated face rather than one that
        // depends on comparison order.
        let mut axis = 0;
        for candidate in 1..3 {
            if next[candidate] < next[axis] {
                axis = candidate;
            }
        }
        if next[axis] > reach {
            return None;
        }

        let (dx, dy, dz) = match axis {
            0 => (step[0], 0, 0),
            1 => (0, step[1], 0),
            _ => (0, 0, step[2]),
        };
        cell = cell.offset(dx, dy, dz);
        next[axis] = next[axis] + delta[axis];

        if grid.is_solid(cell) {
            return Some(Pick {
                cell,
                face: entry_face(axis, -step[axis])?,
            });
        }
    }
    None
}

/// The face a step of this direction on this axis enters through.
fn entry_face(axis: usize, towards: i32) -> Option<Face> {
    Some(match (axis, towards.signum()) {
        (0, 1) => Face::East,
        (0, -1) => Face::West,
        (1, 1) => Face::Top,
        (1, -1) => Face::Bottom,
        (2, 1) => Face::North,
        (2, -1) => Face::South,
        _ => return None,
    })
}
