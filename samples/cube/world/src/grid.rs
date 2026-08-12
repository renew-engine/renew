//! The blocks, and where they are.

use renew_volume::Voxel;

/// What occupies one cell.
///
/// A byte rather than an enum, because a voxel world's whole point is that
/// there are many kinds and they arrive as data. Zero is air by convention and
/// everything else is solid for now — the distinction a game makes between
/// stone and glass belongs to the game.
pub type Block = u8;

/// Nothing there.
pub const AIR: Block = 0;
/// What the world is built from.
pub const STONE: Block = 1;

/// What a player puts back.
///
/// **A second kind, so building is visible.** A block placed into a mound
/// of the same stone is indistinguishable from the mound: the world knows
/// something changed and the player cannot see it. Breaking and placing
/// is the whole of what this game does, and half of it was invisible.
///
/// It is a *placed* block rather than a second material, and the
/// distinction matters for what comes next: nothing here decides what a
/// player is holding, because there is no inventory and no choice. When
/// there is, this becomes one of several and the name will have to change
/// with it.
pub const BRICK: Block = 2;

// The cell address and the half-extent are the engine's now: a cell is a
// cell whoever is asking, and two definitions of where one sits is two
// places for an off-by-one to live. Re-exported rather than re-declared so
// every consumer of this module keeps one import path.
pub use renew_volume::{Cell, cell_half_extent as block_half_extent};

/// A finite box of cells.
///
/// **Finite, and it says so at the edges.** A world that answered "air" for
/// everything outside would let a player walk off it and fall forever with no
/// way to tell that from a hole; one that answered "solid" would trap them at
/// the boundary with no explanation. Asking is answered by [`Grid::get`],
/// which returns nothing outside — so a caller decides.
#[derive(Clone, Debug)]
pub struct Grid {
    /// The engine's chunked volume, holding the blocks.
    ///
    /// **Storage only.** The bounds and the iteration order below stay
    /// here: this grid's `solids` order is part of *its* contract — a
    /// digest walks it — and the volume walks chunk-major, which is the
    /// right order for a volume and the wrong one for a hash somebody
    /// already pinned.
    volume: Option<renew_volume::Volume>,
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
        let size = (size.0.max(0), size.1.max(0), size.2.max(0));
        // A dimension of zero gives a grid with no cells rather than a
        // refusal, which the volume does not express — it clamps to one.
        // `None` is that case, and every query below answers as it always
        // did: outside.
        let volume = if size.0 == 0 || size.1 == 0 || size.2 == 0 {
            None
        } else {
            renew_volume::Volume::new(min, size)
        };
        Self { volume, min, size }
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

    /// Whether a cell is one this grid holds.
    fn holds(&self, cell: Cell) -> bool {
        let (Some(x), Some(y), Some(z)) = (
            cell.x.checked_sub(self.min.x),
            cell.y.checked_sub(self.min.y),
            cell.z.checked_sub(self.min.z),
        ) else {
            return false;
        };
        x >= 0 && y >= 0 && z >= 0 && x < self.size.0 && y < self.size.1 && z < self.size.2
    }

    /// What is in a cell, or nothing if it is outside the grid.
    #[must_use]
    pub fn get(&self, cell: Cell) -> Option<Block> {
        let voxel = self.volume.as_ref()?.get(cell)?;
        // Blocks are a byte and voxels are two; the narrowing cannot lose
        // anything this grid put there, and anything else reads as air
        // rather than as an arbitrary block.
        Some(u8::try_from(voxel.0).unwrap_or(AIR))
    }

    /// Whether a cell blocks movement. **Outside the grid is not solid**, so a
    /// player can leave — falling out of the world is a game's decision to
    /// make, not the grid's.
    #[must_use]
    pub fn is_solid(&self, cell: Cell) -> bool {
        self.get(cell).is_some_and(|block| block != AIR)
    }

    /// Put a block in a cell. `false` if the cell is outside the grid, or if
    /// it is on the shell and the block is air.
    ///
    /// **The shell cannot be cleared, and that is the world being closed
    /// rather than a special case.** Outside the grid is neither solid nor
    /// air, so a player who gets out of it falls forever with nothing to land
    /// on and no way to tell that from a deep hole.
    ///
    /// Two attempts at this were too weak, and both are worth recording
    /// because each looked sufficient:
    ///
    /// - **A thicker floor.** Anything that can dig once can dig again, so no
    ///   thickness closes a floor against a script that repeats.
    /// - **An unbreakable bottom layer.** It stopped the digging and not the
    ///   climbing: `build` places blocks under itself, so it built a tower
    ///   past the three-high walls and walked off the top.
    ///
    /// The shell is the boundary of the box in all three axes, so there is no
    /// direction left to leave by. Filling is unaffected: only clearing is
    /// refused, and only on the boundary.
    pub fn set(&mut self, cell: Cell, block: Block) -> bool {
        if block == AIR && self.on_shell(cell) {
            return false;
        }
        let Some(volume) = self.volume.as_mut() else {
            return false;
        };
        // The volume reports whether anything CHANGED; this reports whether
        // the write was ACCEPTED. Writing stone over stone is a legal write
        // that changed nothing, and a caller counting broken blocks needs
        // the second answer, not the first.
        if !volume.contains(cell) {
            return false;
        }
        volume.set(cell, Voxel(u16::from(block)));
        true
    }

    /// Whether a cell is on the outer boundary of the grid in any axis.
    ///
    /// Cells outside the grid are not on its shell — they are not in it at
    /// all, and [`Self::set`] refuses them for that reason instead.
    #[must_use]
    pub fn on_shell(&self, cell: Cell) -> bool {
        let (width, height, depth) = self.size;
        if !self.holds(cell) {
            return false;
        }
        cell.x == self.min.x
            || cell.y == self.min.y
            || cell.z == self.min.z
            || cell.x == self.min.x + width - 1
            || cell.y == self.min.y + height - 1
            || cell.z == self.min.z + depth - 1
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
        self.volume
            .as_ref()
            .map_or(0, renew_volume::Volume::solid_count)
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
