//! Chunked storage, and the hashes that make a large volume digestible.

use crate::{Cell, Voxel};

/// Cells along one edge of a chunk.
///
/// Sixteen gives 4,096 cells — eight kilobytes of voxel identifiers,
/// which is a comfortable unit to mark dirty and to re-mesh. Smaller
/// chunks multiply per-chunk bookkeeping; larger ones make a single
/// changed cell drag more work behind it.
pub const CHUNK: i32 = 16;

/// Cells in one chunk.
pub const CHUNK_CELLS: usize = (CHUNK * CHUNK * CHUNK) as usize;

/// The most chunks one volume may hold.
///
/// At two bytes a cell this is half a gigabyte of voxel — far past
/// anything this game intends and comfortably inside what the index
/// arithmetic can address. It exists so that [`Volume::new`] has a stated
/// answer instead of an overflow.
pub const MAX_CHUNKS: usize = 65_536;

/// A finite, chunked volume of voxel.
///
/// **Finite, and it says so at the edges.** A volume that answered "empty"
/// outside itself would let a body walk off it and fall forever with no
/// way to tell that from a hole; one that answered "solid" would trap it
/// at the boundary with no explanation. [`Volume::get`] returns nothing
/// outside, so the caller decides what its own edges mean.
#[derive(Clone, Debug)]
pub struct Volume {
    /// The lowest cell addressable, the corner of chunk zero.
    origin: Cell,
    /// Extent in chunks along each axis. Every component is at least one.
    chunks: (i32, i32, i32),
    /// Extent in cells, computed once. `index_of` is on the path the whole
    /// "millions of cells" argument is about, and recomputing three
    /// multiplies per read to save twelve bytes is the wrong trade.
    size: (i32, i32, i32),
    /// Every cell, chunk-major. Allocated once and never resized.
    cells: Vec<Voxel>,
    /// One running hash per chunk, maintained on write.
    hashes: Vec<u64>,
    /// One counter per chunk, bumped by any write that changed a cell.
    ///
    /// **A counter rather than a flag, because there is more than one
    /// consumer.** A flag has to be cleared by whoever read it, and the
    /// mesher and the automaton both want to know what changed — whichever
    /// cleared first would hide the change from the other. A counter is
    /// read without being consumed: each consumer remembers the value it
    /// last acted on and compares.
    versions: Vec<u32>,
    /// How many cells hold something.
    solid: usize,
}

impl Volume {
    /// A volume of `chunks` chunks whose lowest cell is `origin`, all empty.
    ///
    /// This is the only allocation the type ever performs.
    ///
    /// # Refusals
    ///
    /// Returns nothing when the request cannot be addressed: more than
    /// [`MAX_CHUNKS`] chunks, or an extent that would carry the highest
    /// cell past [`i32::MAX`]. **A refusal rather than a clamp**, because
    /// a volume quietly smaller than asked for is a world with an invisible
    /// wall in it, and the caller would find out by walking into one.
    /// Dimensions below one chunk *are* clamped up, since a volume with no
    /// cells has no behaviour to offer and every caller would have to check.
    #[must_use]
    pub fn new(origin: Cell, chunks: (i32, i32, i32)) -> Option<Self> {
        let chunks = (chunks.0.max(1), chunks.1.max(1), chunks.2.max(1));
        let count = usize_of(chunks.0)
            .checked_mul(usize_of(chunks.1))?
            .checked_mul(usize_of(chunks.2))?;
        if count > MAX_CHUNKS {
            return None;
        }
        let size = (
            chunks.0.checked_mul(CHUNK)?,
            chunks.1.checked_mul(CHUNK)?,
            chunks.2.checked_mul(CHUNK)?,
        );
        // The highest addressable cell has to exist as an `i32`, or the
        // top of the volume is unreachable and `set` silently refuses
        // there forever.
        origin.x.checked_add(size.0.checked_sub(1)?)?;
        origin.y.checked_add(size.1.checked_sub(1)?)?;
        origin.z.checked_add(size.2.checked_sub(1)?)?;

        Some(Self {
            origin,
            chunks,
            size,
            cells: vec![Voxel::EMPTY; count.checked_mul(CHUNK_CELLS)?],
            hashes: vec![0; count],
            versions: vec![0; count],
            solid: 0,
        })
    }

    /// The lowest addressable cell.
    #[must_use]
    pub const fn origin(&self) -> Cell {
        self.origin
    }

    /// Extent in chunks.
    #[must_use]
    pub const fn chunks(&self) -> (i32, i32, i32) {
        self.chunks
    }

    /// Extent in cells.
    #[must_use]
    pub const fn size(&self) -> (i32, i32, i32) {
        self.size
    }

    /// The highest addressable cell.
    #[must_use]
    pub const fn max(&self) -> Cell {
        Cell::new(
            self.origin.x + self.size.0 - 1,
            self.origin.y + self.size.1 - 1,
            self.origin.z + self.size.2 - 1,
        )
    }

    /// Clamp a cell into the addressable range.
    #[must_use]
    pub const fn clamp(&self, cell: Cell) -> Cell {
        let max = self.max();
        Cell::new(
            if cell.x < self.origin.x {
                self.origin.x
            } else if cell.x > max.x {
                max.x
            } else {
                cell.x
            },
            if cell.y < self.origin.y {
                self.origin.y
            } else if cell.y > max.y {
                max.y
            } else {
                cell.y
            },
            if cell.z < self.origin.z {
                self.origin.z
            } else if cell.z > max.z {
                max.z
            } else {
                cell.z
            },
        )
    }

    /// How many cells hold something.
    #[must_use]
    pub const fn solid_count(&self) -> usize {
        self.solid
    }

    /// What is in a cell, or nothing if it lies outside.
    #[must_use]
    pub fn get(&self, cell: Cell) -> Option<Voxel> {
        let index = self.index_of(cell)?;
        self.cells.get(index).copied()
    }

    /// Whether a cell holds anything. Cells outside are not solid.
    ///
    /// **Outside reads as empty here on purpose**, unlike [`Volume::get`]:
    /// a traversal that has left the volume should stop finding surfaces,
    /// not find one everywhere. A caller that needs to distinguish "empty"
    /// from "not mine" asks `get`.
    #[must_use]
    pub fn is_solid(&self, cell: Cell) -> bool {
        self.get(cell).is_some_and(|voxel| !voxel.is_empty())
    }

    /// Put `voxel` in `cell`, returning whether anything changed.
    ///
    /// Writing outside the volume changes nothing and returns `false`.
    /// Writing the value already there is likewise no change: it does not
    /// dirty the chunk, because a re-mesh nobody needs is the cost this
    /// whole design exists to avoid.
    pub fn set(&mut self, cell: Cell, voxel: Voxel) -> bool {
        let Some(index) = self.index_of(cell) else {
            return false;
        };
        let Some(slot) = self.cells.get_mut(index) else {
            return false;
        };
        let previous = *slot;
        if previous == voxel {
            return false;
        }
        *slot = voxel;

        let chunk = index / CHUNK_CELLS;
        let within = index % CHUNK_CELLS;
        if let Some(hash) = self.hashes.get_mut(chunk) {
            // Exclusive-or is its own inverse, so retiring the old term and
            // admitting the new one is the whole update — no rescan, and
            // writing a cell back to what it was restores the hash exactly.
            if !previous.is_empty() {
                *hash ^= term(within, previous);
            }
            if !voxel.is_empty() {
                *hash ^= term(within, voxel);
            }
        }
        if let Some(version) = self.versions.get_mut(chunk) {
            // Wrapping is correct and not a compromise: consumers compare
            // for inequality, never for order, so the only failure would
            // be a consumer that missed exactly 2^32 changes to one chunk
            // and then looked.
            *version = version.wrapping_add(1);
        }

        match (previous.is_empty(), voxel.is_empty()) {
            (true, false) => self.solid += 1,
            (false, true) => self.solid -= 1,
            _ => {}
        }
        true
    }

    /// Fill the inclusive box between two cells, returning how many changed.
    ///
    /// **The box is clamped to the volume before anything is walked.** A
    /// fill from `i32::MIN` to `i32::MAX` is a reasonable way to say
    /// "everything", and walking it cell by cell to reject each one would
    /// be four billion refusals per axis.
    pub fn fill(&mut self, from: Cell, to: Cell, voxel: Voxel) -> usize {
        let low = self.clamp(Cell::new(
            from.x.min(to.x),
            from.y.min(to.y),
            from.z.min(to.z),
        ));
        let high = self.clamp(Cell::new(
            from.x.max(to.x),
            from.y.max(to.y),
            from.z.max(to.z),
        ));
        let mut changed = 0;
        for z in low.z..=high.z {
            for y in low.y..=high.y {
                for x in low.x..=high.x {
                    if self.set(Cell::new(x, y, z), voxel) {
                        changed += 1;
                    }
                }
            }
        }
        changed
    }

    /// The running hash of one chunk, or nothing if the index is past the end.
    ///
    /// **An empty chunk hashes to zero. The converse does not hold** and
    /// must not be relied on: the hash is an exclusive-or of 64-bit terms,
    /// and any 65 of those are linearly dependent, so populated chunks
    /// hashing to zero exist and can be constructed deliberately. Reaching
    /// one by accident is a 2⁻⁶⁴ event, but a consumer asking "is there
    /// anything here" should ask [`Volume::solid_count`] or read the cells,
    /// not read a hash.
    #[must_use]
    pub fn chunk_hash(&self, chunk: usize) -> Option<u64> {
        self.hashes.get(chunk).copied()
    }

    /// How many chunks there are.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.hashes.len()
    }

    /// The whole volume in one value.
    ///
    /// Folds the origin and the extent first, then every chunk hash in
    /// ascending chunk index. **The shape is part of the digest**: without
    /// it, a volume of two chunks along x and one of two along y hash the
    /// same when their single occupied cells sit at the same offset, and
    /// so do two volumes whose contents differ only by where their origins
    /// are — neither of which shares a single cell with the other.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut accumulator: u64 = 0xcbf2_9ce4_8422_2325;
        let mut fold = |value: u64| {
            accumulator ^= value;
            accumulator = accumulator.wrapping_mul(0x0000_0100_0000_01b3);
        };
        fold(u64::from(self.origin.x.cast_unsigned()));
        fold(u64::from(self.origin.y.cast_unsigned()));
        fold(u64::from(self.origin.z.cast_unsigned()));
        fold(u64::from(self.chunks.0.cast_unsigned()));
        fold(u64::from(self.chunks.1.cast_unsigned()));
        fold(u64::from(self.chunks.2.cast_unsigned()));
        for hash in &self.hashes {
            fold(*hash);
        }
        accumulator
    }

    /// How many times a chunk has changed, or nothing past the end.
    ///
    /// The number itself means nothing; **only inequality does**. A
    /// consumer keeps the value it last acted on per chunk and re-does its
    /// work wherever the two differ, which lets any number of consumers
    /// track the same volume without agreeing with each other about when
    /// to forget.
    #[must_use]
    pub fn chunk_version(&self, chunk: usize) -> Option<u32> {
        self.versions.get(chunk).copied()
    }

    /// Every chunk's version, for a consumer taking its first snapshot.
    #[must_use]
    pub fn chunk_versions(&self) -> &[u32] {
        &self.versions
    }

    /// Which chunk a cell belongs to, or nothing if it lies outside.
    ///
    /// The unit consumers work in: a change is reported per chunk, so
    /// anything that wants to act on one has to be able to name it.
    #[must_use]
    pub fn chunk_of(&self, cell: Cell) -> Option<usize> {
        self.index_of(cell).map(|index| index / CHUNK_CELLS)
    }

    /// The lowest cell of a chunk, or nothing if the index is past the end.
    #[must_use]
    pub fn chunk_origin(&self, chunk: usize) -> Option<Cell> {
        if chunk >= self.hashes.len() {
            return None;
        }
        let per_layer = usize_of(self.chunks.0) * usize_of(self.chunks.1);
        let wide = usize_of(self.chunks.0);
        let z = chunk / per_layer;
        let y = (chunk % per_layer) / wide;
        let x = chunk % wide;
        // Every product below is bounded by the extent `new` already
        // proved addressable, so none of these can leave the range.
        let scale = |v: usize| i32::try_from(v).unwrap_or(0).saturating_mul(CHUNK);
        Some(Cell::new(
            self.origin.x.saturating_add(scale(x)),
            self.origin.y.saturating_add(scale(y)),
            self.origin.z.saturating_add(scale(z)),
        ))
    }

    /// Every cell holding something, with what it holds.
    ///
    /// **The order is part of the contract**: ascending chunk index, then
    /// ascending cell index within a chunk, which is x fastest, then y,
    /// then z. A consumer that folds this into a digest of its own gets the
    /// same answer on every machine because of that sentence.
    pub fn solids(&self) -> impl Iterator<Item = (Cell, Voxel)> + '_ {
        self.cells
            .iter()
            .enumerate()
            .filter(|(_, voxel)| !voxel.is_empty())
            .filter_map(move |(index, voxel)| Some((self.cell_at(index)?, *voxel)))
    }

    /// The flat index of a cell, or nothing if it lies outside.
    fn index_of(&self, cell: Cell) -> Option<usize> {
        let local = (
            cell.x.checked_sub(self.origin.x)?,
            cell.y.checked_sub(self.origin.y)?,
            cell.z.checked_sub(self.origin.z)?,
        );
        if local.0 < 0
            || local.1 < 0
            || local.2 < 0
            || local.0 >= self.size.0
            || local.1 >= self.size.1
            || local.2 >= self.size.2
        {
            return None;
        }
        let chunk = (local.0 / CHUNK, local.1 / CHUNK, local.2 / CHUNK);
        let within = (local.0 % CHUNK, local.1 % CHUNK, local.2 % CHUNK);
        let chunk_index = (usize_of(chunk.2) * usize_of(self.chunks.1) + usize_of(chunk.1))
            * usize_of(self.chunks.0)
            + usize_of(chunk.0);
        let cell_index = (usize_of(within.2) * usize_of(CHUNK) + usize_of(within.1))
            * usize_of(CHUNK)
            + usize_of(within.0);
        Some(chunk_index * CHUNK_CELLS + cell_index)
    }

    /// The cell a flat index addresses — the inverse of `index_of`.
    fn cell_at(&self, index: usize) -> Option<Cell> {
        let origin = self.chunk_origin(index / CHUNK_CELLS)?;
        let within = index % CHUNK_CELLS;
        let edge = usize_of(CHUNK);
        let z = within / (edge * edge);
        let y = (within / edge) % edge;
        let x = within % edge;
        let axis = |v: usize| i32::try_from(v).unwrap_or(0);
        Some(Cell::new(
            origin.x.saturating_add(axis(x)),
            origin.y.saturating_add(axis(y)),
            origin.z.saturating_add(axis(z)),
        ))
    }
}

/// A non-negative `i32` as a `usize`, and zero for anything negative.
///
/// Every caller here has already established the value is positive; this
/// exists so that establishing it a second time does not need an `unwrap`
/// the deny-list forbids.
fn usize_of(value: i32) -> usize {
    usize::try_from(value.max(0)).unwrap_or(0)
}

/// One cell's contribution to its chunk's hash.
///
/// Mixed rather than combined plainly so that neighbouring cells and
/// adjacent voxel identifiers — which is what a real volume is full of —
/// do not produce terms that cancel each other under exclusive-or. The
/// packing is injective over everything reachable: `cell_index` is below
/// [`CHUNK_CELLS`] and the voxel is sixteen bits, so the two never
/// overlap.
fn term(cell_index: usize, voxel: Voxel) -> u64 {
    let packed = ((cell_index as u64) << 16) | u64::from(voxel.0);
    let mut x = packed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::{CHUNK, CHUNK_CELLS, MAX_CHUNKS, Volume, term};
    use crate::{Cell, Voxel};

    const STONE: Voxel = Voxel(1);
    const SAND: Voxel = Voxel(2);

    fn volume() -> Volume {
        Volume::new(Cell::new(0, 0, 0), (2, 2, 2)).expect("a small volume is addressable")
    }

    /// A chunk's hash computed by walking its cells, with no reference to
    /// the incremental path.
    ///
    /// **This is the independent side of the equivalence.** Comparing two
    /// volumes that both went through `set` proves only that exclusive-or
    /// commutes; if `term` were wrong, or `set` folded the wrong chunk, both
    /// sides would carry the same mistake and agree.
    fn hash_from_scratch(volume: &Volume, chunk: usize) -> u64 {
        let mut hash = 0u64;
        for within in 0..CHUNK_CELLS {
            let index = chunk * CHUNK_CELLS + within;
            let Some(cell) = volume.cell_at(index) else {
                continue;
            };
            let Some(voxel) = volume.get(cell) else {
                continue;
            };
            if !voxel.is_empty() {
                hash ^= term(within, voxel);
            }
        }
        hash
    }

    #[test]
    fn the_maintained_hash_equals_one_walked_from_the_cells() {
        let mut v = volume();
        for (cell, voxel) in [
            (Cell::new(0, 0, 0), STONE),
            (Cell::new(15, 15, 15), SAND),
            (Cell::new(16, 0, 3), STONE),
            (Cell::new(7, 9, 11), SAND),
            (Cell::new(31, 31, 31), STONE),
        ] {
            v.set(cell, voxel);
        }
        for chunk in 0..v.chunk_count() {
            assert_eq!(
                v.chunk_hash(chunk),
                Some(hash_from_scratch(&v, chunk)),
                "chunk {chunk} drifted from what its cells say"
            );
        }
    }

    #[test]
    fn a_fresh_volume_is_empty_everywhere_and_hashes_to_zero() {
        let v = volume();
        assert_eq!(v.solid_count(), 0);
        for chunk in 0..v.chunk_count() {
            assert_eq!(v.chunk_hash(chunk), Some(0), "chunk {chunk}");
        }
        assert_eq!(v.solids().count(), 0);
    }

    #[test]
    fn outside_is_not_solid_but_is_distinguishable_from_empty() {
        let v = volume();
        let outside = Cell::new(-1, 0, 0);
        assert_eq!(v.get(outside), None, "outside must be tellable from empty");
        assert!(!v.is_solid(outside), "outside must not read as a surface");
        assert_eq!(v.get(Cell::new(0, 0, 0)), Some(Voxel::EMPTY));
    }

    #[test]
    fn writing_outside_changes_nothing() {
        let mut v = volume();
        let before = v.digest();
        assert!(!v.set(Cell::new(-1, -1, -1), STONE));
        assert_eq!(v.digest(), before);
        assert_eq!(v.solid_count(), 0);
    }

    #[test]
    fn a_write_and_its_undo_restore_the_hash_exactly() {
        let mut v = volume();
        let before = v.digest();
        let cell = Cell::new(3, 4, 5);
        assert!(v.set(cell, STONE));
        assert_ne!(v.digest(), before, "a write must be visible in the digest");
        assert!(v.set(cell, Voxel::EMPTY));
        assert_eq!(
            v.digest(),
            before,
            "the running hash must be exactly reversible, or it drifts"
        );
        assert_eq!(v.solid_count(), 0);
    }

    #[test]
    fn writing_the_same_value_is_not_a_change() {
        let mut v = volume();
        let cell = Cell::new(1, 1, 1);
        assert!(v.set(cell, STONE));
        let settled: Vec<u32> = v.chunk_versions().to_vec();
        assert!(!v.set(cell, STONE), "an unchanged write must report false");
        assert_eq!(
            v.chunk_versions(),
            settled.as_slice(),
            "and must not bump the chunk"
        );
    }

    #[test]
    fn two_voxels_swapped_between_cells_change_the_digest() {
        let mut one = volume();
        one.set(Cell::new(1, 0, 0), STONE);
        one.set(Cell::new(2, 0, 0), SAND);
        let mut other = volume();
        other.set(Cell::new(1, 0, 0), SAND);
        other.set(Cell::new(2, 0, 0), STONE);
        assert_ne!(one.digest(), other.digest());
    }

    #[test]
    fn two_volumes_sharing_no_cell_do_not_share_a_digest() {
        // The digest has to say where the volume is. Same contents at the
        // same offset, different origins — not one cell in common.
        let mut here = Volume::new(Cell::new(0, 0, 0), (1, 1, 1)).expect("volume");
        here.set(Cell::new(0, 0, 0), STONE);
        let mut there = Volume::new(Cell::new(100, 100, 100), (1, 1, 1)).expect("volume");
        there.set(Cell::new(100, 100, 100), STONE);
        assert_ne!(here.digest(), there.digest());
    }

    #[test]
    fn two_volumes_of_different_shape_do_not_share_a_digest() {
        let mut wide = Volume::new(Cell::new(0, 0, 0), (2, 1, 1)).expect("volume");
        wide.set(Cell::new(CHUNK, 0, 0), STONE);
        let mut tall = Volume::new(Cell::new(0, 0, 0), (1, 2, 1)).expect("volume");
        tall.set(Cell::new(0, CHUNK, 0), STONE);
        assert_ne!(
            wide.digest(),
            tall.digest(),
            "the extent is part of what a volume is"
        );
    }

    #[test]
    fn a_write_bumps_exactly_its_own_chunk() {
        let mut v = volume();
        let before: Vec<u32> = v.chunk_versions().to_vec();
        v.set(Cell::new(0, 0, 0), STONE);
        let after: Vec<u32> = v.chunk_versions().to_vec();
        let moved: Vec<usize> = (0..v.chunk_count())
            .filter(|index| before[*index] != after[*index])
            .collect();
        assert_eq!(moved, vec![0]);

        v.set(Cell::new(CHUNK, 0, 0), STONE);
        let later: Vec<u32> = v.chunk_versions().to_vec();
        let moved: Vec<usize> = (0..v.chunk_count())
            .filter(|index| after[*index] != later[*index])
            .collect();
        assert_eq!(moved, vec![1], "the neighbouring chunk, not the first");
    }

    #[test]
    fn a_version_is_not_consumed_by_reading_it() {
        // The whole reason it is a counter: two consumers watching the
        // same volume must not be able to hide changes from each other.
        let mut v = volume();
        let mesher: Vec<u32> = v.chunk_versions().to_vec();
        let automaton: Vec<u32> = v.chunk_versions().to_vec();
        v.set(Cell::new(1, 1, 1), STONE);
        assert_ne!(v.chunk_version(0), Some(mesher[0]));
        assert_ne!(
            v.chunk_version(0),
            Some(automaton[0]),
            "one consumer's read hid the change from the other"
        );
    }

    #[test]
    fn a_cell_names_the_chunk_it_lives_in() {
        let v = volume();
        assert_eq!(v.chunk_of(Cell::new(0, 0, 0)), Some(0));
        assert_eq!(v.chunk_of(Cell::new(CHUNK, 0, 0)), Some(1));
        assert_eq!(v.chunk_of(Cell::new(-1, 0, 0)), None);
    }

    #[test]
    fn solids_come_back_in_the_stated_order() {
        let mut v = volume();
        let written = [Cell::new(2, 0, 0), Cell::new(0, 0, 0), Cell::new(1, 0, 0)];
        for cell in written {
            v.set(cell, STONE);
        }
        let seen: Vec<Cell> = v.solids().map(|(cell, _)| cell).collect();
        assert_eq!(
            seen,
            vec![Cell::new(0, 0, 0), Cell::new(1, 0, 0), Cell::new(2, 0, 0)],
            "x must advance fastest, whatever order writes arrived in"
        );
    }

    #[test]
    fn every_cell_round_trips_through_the_index_and_back() {
        let v = Volume::new(Cell::new(-8, 5, -3), (2, 1, 2)).expect("volume");
        let (sx, sy, sz) = v.size();
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    let cell = Cell::new(v.origin().x + x, v.origin().y + y, v.origin().z + z);
                    let index = v.index_of(cell).expect("inside the volume");
                    assert_eq!(v.cell_at(index), Some(cell), "at {cell:?}");
                }
            }
        }
    }

    #[test]
    fn filling_counts_only_what_changed() {
        let mut v = volume();
        let changed = v.fill(Cell::new(0, 0, 0), Cell::new(1, 1, 1), STONE);
        assert_eq!(changed, 8);
        assert_eq!(v.solid_count(), 8);
        let again = v.fill(Cell::new(0, 0, 0), Cell::new(1, 1, 1), STONE);
        assert_eq!(again, 0, "a fill over its own result changes nothing");
    }

    #[test]
    fn filling_everything_is_bounded_by_the_volume() {
        // Without the clamp this walks four billion cells per axis to
        // refuse each one, which is a hang rather than a wrong answer.
        let mut v = Volume::new(Cell::new(0, 0, 0), (1, 1, 1)).expect("volume");
        let changed = v.fill(
            Cell::new(i32::MIN, i32::MIN, i32::MIN),
            Cell::new(i32::MAX, i32::MAX, i32::MAX),
            STONE,
        );
        assert_eq!(changed, CHUNK_CELLS, "exactly the volume, and no more");
        assert_eq!(v.solid_count(), CHUNK_CELLS);
    }

    #[test]
    fn a_volume_cannot_be_built_with_no_cells() {
        let v = Volume::new(Cell::new(0, 0, 0), (0, -4, 0)).expect("clamped up to one");
        assert_eq!(v.chunks(), (1, 1, 1));
        assert_eq!(v.chunk_count(), 1);
    }

    #[test]
    fn a_volume_too_large_to_address_is_refused_rather_than_clamped() {
        // A clamp here would hand back a world with an invisible wall.
        assert!(Volume::new(Cell::new(0, 0, 0), (2_000_000, 2_000_000, 2_000_000)).is_none());
        assert!(Volume::new(Cell::new(0, 0, 0), (i32::MAX, 1, 1)).is_none());
        let too_many = i32::try_from(MAX_CHUNKS).unwrap_or(i32::MAX) + 1;
        assert!(Volume::new(Cell::new(0, 0, 0), (too_many, 1, 1)).is_none());
    }

    #[test]
    fn a_volume_whose_top_would_not_fit_is_refused() {
        // Every cell a volume claims has to be addressable; saturating at
        // the ceiling would leave the top cells permanently unwritable.
        assert!(Volume::new(Cell::new(i32::MAX - 5, 0, 0), (2, 1, 1)).is_none());
        let v = Volume::new(Cell::new(i32::MAX - 5, 0, 0), (1, 1, 1));
        assert!(v.is_none(), "sixteen cells do not fit in six");
    }

    #[test]
    fn the_top_cell_of_a_volume_is_writable() {
        let mut v = Volume::new(Cell::new(-3, -3, -3), (2, 1, 1)).expect("volume");
        let top = v.max();
        assert!(v.set(top, STONE), "the highest cell must be reachable");
        assert_eq!(v.get(top), Some(STONE));
        assert!(
            !v.set(top.offset(1, 0, 0), STONE),
            "and one past it must not"
        );
    }
}
