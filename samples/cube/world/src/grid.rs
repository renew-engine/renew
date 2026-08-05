//! The blocks, and where they are.

use renew_fixed::{Fixed, Vec3};

/// What occupies one cell.
///
/// A byte rather than an enum, because a voxel world's whole point is that
/// there are many kinds and they arrive as data. Zero is air by convention and
/// everything else is solid for now — the distinction a game makes between
/// stone and glass belongs to the game.
pub type Block = u8;

/// Nothing there.
pub const AIR: Block = 0;
/// The one solid kind this sample has.
pub const STONE: Block = 1;

/// Integer coordinates of one cell.
///
/// **Signed, and that is deliberate.** A world that started at zero would need
/// a translation between where a player is and which cell that is, and the
/// translation is exactly where an off-by-one lives. Negative coordinates are
/// ordinary here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    ///
    /// A cell spans one unit, so cell zero covers −0.5 to +0.5 and its centre
    /// is the origin. Centring on integers rather than cornering on them means
    /// a block's half-extent is exactly one half and the arithmetic stays
    /// exact.
    #[must_use]
    pub fn centre(self) -> Vec3 {
        Vec3::new(
            Fixed::from_int(self.x),
            Fixed::from_int(self.y),
            Fixed::from_int(self.z),
        )
    }

    /// This cell offset by whole steps.
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
    /// Rounds to nearest, which is what pairs with centring cells on integers.
    /// A position exactly on a boundary goes to the higher cell, consistently,
    /// so two players standing on the same seam agree about where they are.
    #[must_use]
    pub fn containing(position: Vec3) -> Self {
        Self::new(
            round_to_cell(position.x),
            round_to_cell(position.y),
            round_to_cell(position.z),
        )
    }
}

/// Round a coordinate to the cell that contains it.
fn round_to_cell(value: Fixed) -> i32 {
    // Adding half a unit and truncating toward negative infinity puts the
    // boundary at the higher cell for both signs — `trunc_int` alone rounds
    // toward zero, which would split the boundary rule between positive and
    // negative coordinates.
    let raised = value + Fixed::from_ratio(1, 2);
    let floored = raised.to_bits().div_euclid(65536);
    i32::try_from(floored).unwrap_or(0)
}

/// Half a block, which is every block's half-extent.
#[must_use]
pub fn block_half_extent() -> Vec3 {
    let half = Fixed::from_ratio(1, 2);
    Vec3::new(half, half, half)
}

/// A finite box of cells.
///
/// **Finite, and it says so at the edges.** A world that answered "air" for
/// everything outside would let a player walk off it and fall forever with no
/// way to tell that from a hole; one that answered "solid" would trap them at
/// the boundary with no explanation. Asking is answered by [`Grid::get`],
/// which returns nothing outside — so a caller decides.
#[derive(Clone, Debug)]
pub struct Grid {
    blocks: Vec<Block>,
    min: Cell,
    size: (i32, i32, i32),
}

impl Grid {
    /// An empty grid spanning `size` cells from `min`.
    ///
    /// A dimension of zero or less gives an empty grid rather than a refusal:
    /// a world with no blocks is a legitimate thing to simulate, and it is
    /// what a test that only cares about falling wants.
    #[must_use]
    pub fn new(min: Cell, size: (i32, i32, i32)) -> Self {
        let count = size
            .0
            .max(0)
            .saturating_mul(size.1.max(0))
            .saturating_mul(size.2.max(0));
        Self {
            blocks: vec![AIR; usize::try_from(count).unwrap_or(0)],
            min,
            size: (size.0.max(0), size.1.max(0), size.2.max(0)),
        }
    }

    /// The lowest cell this grid holds.
    #[must_use]
    pub const fn min(&self) -> Cell {
        self.min
    }

    /// How many cells on each axis.
    #[must_use]
    pub const fn size(&self) -> (i32, i32, i32) {
        self.size
    }

    fn index(&self, cell: Cell) -> Option<usize> {
        let (x, y, z) = (
            cell.x.checked_sub(self.min.x)?,
            cell.y.checked_sub(self.min.y)?,
            cell.z.checked_sub(self.min.z)?,
        );
        if x < 0 || y < 0 || z < 0 || x >= self.size.0 || y >= self.size.1 || z >= self.size.2 {
            return None;
        }
        let flat = i64::from(x)
            + i64::from(y) * i64::from(self.size.0)
            + i64::from(z) * i64::from(self.size.0) * i64::from(self.size.1);
        usize::try_from(flat).ok()
    }

    /// What is in a cell, or nothing if it is outside the grid.
    #[must_use]
    pub fn get(&self, cell: Cell) -> Option<Block> {
        self.blocks.get(self.index(cell)?).copied()
    }

    /// Whether a cell blocks movement. **Outside the grid is not solid**, so a
    /// player can leave — falling out of the world is a game's decision to
    /// make, not the grid's.
    #[must_use]
    pub fn is_solid(&self, cell: Cell) -> bool {
        self.get(cell).is_some_and(|block| block != AIR)
    }

    /// Put a block in a cell. `false` if the cell is outside the grid.
    pub fn set(&mut self, cell: Cell, block: Block) -> bool {
        let Some(index) = self.index(cell) else {
            return false;
        };
        match self.blocks.get_mut(index) {
            Some(slot) => {
                *slot = block;
                true
            }
            None => false,
        }
    }

    /// Fill a rectangular region, inclusive of both corners.
    pub fn fill(&mut self, from: Cell, to: Cell, block: Block) {
        for x in from.x.min(to.x)..=from.x.max(to.x) {
            for y in from.y.min(to.y)..=from.y.max(to.y) {
                for z in from.z.min(to.z)..=from.z.max(to.z) {
                    self.set(Cell::new(x, y, z), block);
                }
            }
        }
    }

    /// How many cells hold something.
    #[must_use]
    pub fn solid_count(&self) -> usize {
        self.blocks.iter().filter(|&&block| block != AIR).count()
    }

    /// Every cell holding something, in a fixed order.
    ///
    /// The order is part of the contract rather than an accident: a digest
    /// walks it, and a walk whose order depended on the representation would
    /// hash two identical worlds differently.
    pub fn solids(&self) -> impl Iterator<Item = (Cell, Block)> + '_ {
        (0..self.size.2).flat_map(move |z| {
            (0..self.size.1).flat_map(move |y| {
                (0..self.size.0).filter_map(move |x| {
                    let cell = Cell::new(self.min.x + x, self.min.y + y, self.min.z + z);
                    let block = self.get(cell)?;
                    (block != AIR).then_some((cell, block))
                })
            })
        })
    }
}
