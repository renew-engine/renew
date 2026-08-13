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
    /// How many chunk-changing writes this volume has ever seen.
    ///
    /// The mark a consumer holds. Sixty-four bits at one write per
    /// nanosecond is centuries, so unlike the per-chunk counters this one
    /// is never allowed to wrap: the arithmetic below subtracts marks, and
    /// a wrapped subtraction would silently report "nothing changed".
    generation: u64,
    /// A ring of chunk indices, one per change, oldest overwritten first.
    ///
    /// Length is fixed at construction and is the capacity; the entry for
    /// change number `n` lives at `(n - 1) % capacity`.
    log: Vec<u32>,
    /// The generation at which each chunk last changed.
    ///
    /// This is what lets the ring be read without repeats: an entry is
    /// yielded only when it is that chunk's most recent change, so a
    /// thousand writes into one chunk report it once.
    last_changed: Vec<u64>,
    /// How many cells hold something.
    solid: usize,
}

impl Volume {
    /// A volume of `size` **cells** whose lowest cell is `origin`, all
    /// empty.
    ///
    /// This is the only allocation the type ever performs.
    ///
    /// # The extent is in cells, and it is not rounded
    ///
    /// Storage is chunked and rounds up; the **addressable extent does
    /// not**. A volume of 41 cells allocates three chunks and answers
    /// nothing for the seven cells past the end, exactly as it answers
    /// nothing for the cell before the start.
    ///
    /// That distinction is load-bearing rather than tidy. A consumer's
    /// world is whatever size its world is, and a volume that quietly
    /// grew to the next multiple of sixteen would put empty cells where
    /// the caller believed there was nothing at all — which a mesher
    /// reads as *air against the outer wall* and draws a face for. The
    /// symptom is a world that suddenly has an outside.
    ///
    /// # Refusals
    ///
    /// Returns nothing when the request cannot be addressed: more than
    /// [`MAX_CHUNKS`] chunks of storage, or an extent that would carry the
    /// highest cell past [`i32::MAX`]. **A refusal rather than a clamp**,
    /// because a volume quietly smaller than asked for is a world with an
    /// invisible wall in it, and the caller would find out by walking into
    /// one. Dimensions below one cell *are* clamped up, since a volume
    /// with no cells has no behaviour to offer.
    #[must_use]
    pub fn new(origin: Cell, size: (i32, i32, i32)) -> Option<Self> {
        let size = (size.0.max(1), size.1.max(1), size.2.max(1));
        let in_chunks = |cells: i32| cells.checked_add(CHUNK - 1).map(|sum| sum / CHUNK);
        let chunks = (in_chunks(size.0)?, in_chunks(size.1)?, in_chunks(size.2)?);
        let count = usize_of(chunks.0)
            .checked_mul(usize_of(chunks.1))?
            .checked_mul(usize_of(chunks.2))?;
        if count > MAX_CHUNKS {
            return None;
        }
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
            generation: 0,
            // One slot per chunk, which is a bound that scales the right
            // way rather than a number picked out of the air. Falling off
            // the end of the ring means more changes have happened than
            // there are chunks, and at that point "every chunk" is both a
            // cheap answer and very nearly a true one. A small volume
            // overflows easily and is trivial to rescan; a large one gets
            // a large ring, which is where the scan actually hurt.
            log: vec![0; count],
            last_changed: vec![0; count],
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

    /// Extent in cells — what the caller asked for, not what was
    /// allocated.
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

    /// Whether this volume holds the cell at all.
    ///
    /// Distinct from [`Volume::get`] returning something, though they agree:
    /// a caller deciding whether a write will be *accepted* is asking about
    /// the extent, not about the contents, and saying so reads better than
    /// reading a cell in order to throw the value away.
    #[must_use]
    pub fn contains(&self, cell: Cell) -> bool {
        self.index_of(cell).is_some()
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
        self.record(chunk);

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
    /// Folds the origin, then **both** extents, then every chunk hash in
    /// ascending chunk index. **The shape is part of the digest**: without
    /// it, a volume of two chunks along x and one of two along y hash the
    /// same when their single occupied cells sit at the same offset, and
    /// so do two volumes whose contents differ only by where their origins
    /// are — neither of which shares a single cell with the other.
    ///
    /// # Both extents, because this crate has two
    ///
    /// The extent in chunks is what was allocated; the extent in cells is
    /// what is addressable, and the two differ because a volume sized in
    /// cells does not round up. Folding only the first was a real defect
    /// and not a tidiness one: sizes 33 to 48 all allocate three chunks
    /// along an axis, so a volume of 41 cells and one of 48 digested the
    /// same while disagreeing about which cells exist. They answer `get`
    /// differently, accept different writes, and — with no write at all —
    /// mesh differently, because the mesher walks the addressable extent
    /// and asks about the neighbour past its end. Two worlds no future
    /// write can ever make diverge, reported identical. 41 by 12 by 41 is
    /// the engine's own voxel sample, not a contrived size.
    ///
    /// Named here rather than silently correct, because a digest must
    /// say what it leaves out: it leaves out
    /// [`Volume::generation`], which is bookkeeping and has its own test.
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
        fold(u64::from(self.size.0.cast_unsigned()));
        fold(u64::from(self.size.1.cast_unsigned()));
        fold(u64::from(self.size.2.cast_unsigned()));
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

    /// Note that a chunk changed. The only writer of the change feed.
    fn record(&mut self, chunk: usize) {
        // No guard against a ring with no slots: `slot_of` already answers
        // nothing for one, and a volume with no chunks has no cell to
        // write, so this is never reached with an empty ring anyway. A
        // guard here would be a line nothing can execute.
        let capacity = self.log.len();
        self.generation += 1;
        // Both indices are in range by construction: the slot is a
        // remainder of the length, and `chunk` came from an index this
        // volume computed. The `if let`s say so without an assertion in
        // the hot path of every write.
        if let Some(entry) =
            slot_of(self.generation, capacity).and_then(|slot| self.log.get_mut(slot))
        {
            *entry = u32_of(chunk);
        }
        if let Some(when) = self.last_changed.get_mut(chunk) {
            *when = self.generation;
        }
    }

    /// How many chunk-changing writes this volume has seen.
    ///
    /// The mark to hold on to and hand back to [`Volume::changed_since`].
    /// Comparing two of these is the cheapest question a consumer can ask:
    /// **equal means nothing anywhere has changed**, which for a world at
    /// rest is the answer every frame and costs one comparison rather than
    /// one per chunk.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The chunks that changed since `mark`, each named at most once.
    ///
    /// Returns nothing — meaning **treat every chunk as changed** — when
    /// the mark is too old to answer from the ring, or when it is from the
    /// future and so cannot have come from this volume. Both are honest
    /// refusals rather than errors: a consumer that falls behind re-does
    /// its work over the whole volume, which is what it would have done
    /// anyway without a feed.
    ///
    /// # Why this exists beside the per-chunk versions
    ///
    /// [`Volume::chunk_versions`] answers *how much* each chunk changed,
    /// and answering "which changed" from it costs a comparison per chunk
    /// whether or not anything happened. That scan is the entire cost of a
    /// settled world, and it grows with the world rather than with the
    /// change. This answers the same question in time proportional to what
    /// actually moved.
    ///
    /// The versions stay, because they answer a question this cannot: a
    /// consumer that has been away longer than the ring, or that wants to
    /// compare against a snapshot rather than a moment, still needs them.
    ///
    /// # Order, and repeats
    ///
    /// Chunks come out in the order they last changed, oldest first. A
    /// chunk written a thousand times appears once, at its most recent
    /// change — which is why a consumer can drive work directly from this
    /// without collecting into a set first.
    ///
    /// # Two things it cannot do for you
    ///
    /// **A mark from another volume is undetectable** unless it happens to
    /// be larger than this volume's generation. Marks are positions in one
    /// volume's history and mean nothing in another's; a consumer holding
    /// several volumes holds a mark per volume, and the type system is not
    /// what stops it confusing them.
    ///
    /// **Reading borrows the volume**, so a consumer that writes while
    /// walking its own change list has to collect first. That is a
    /// consequence worth having rather than a limitation to route around:
    /// writes made during the walk would appear in the feed being walked,
    /// and a consumer feeding itself is a loop with no stated end.
    #[must_use]
    pub fn changed_since(&self, mark: u64) -> Option<ChangedChunks<'_>> {
        if mark > self.generation {
            return None;
        }
        if self.generation - mark > self.log.len() as u64 {
            return None;
        }
        Some(ChangedChunks {
            volume: self,
            next: mark,
        })
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

/// A chunk index narrowed for storage in the change ring.
///
/// Every chunk index is below [`MAX_CHUNKS`], which is far inside `u32`,
/// so the saturation is unreachable and exists only to keep the cast
/// total.
fn u32_of(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Where change number `generation` sits in a ring of `capacity` slots.
///
/// Both conversions are exact and neither is written as a cast. The
/// length widens to `u64` without loss, and the remainder is below the
/// capacity and so narrows back the same way — but "provably exact" is
/// what every truncation bug was believed to be, and this crate's
/// deny-list is what makes the belief unnecessary. Nothing (rather than
/// zero) for a ring with no slots, which also keeps the modulus off a
/// divisor that could be zero.
fn slot_of(generation: u64, capacity: usize) -> Option<usize> {
    let capacity = u64::try_from(capacity).ok()?;
    let position = generation.checked_sub(1)?.checked_rem(capacity)?;
    usize::try_from(position).ok()
}

/// The chunks that changed since a mark, oldest change first.
///
/// Each chunk appears at most once, at its most recent change. Yielded
/// lazily by walking the ring: an entry that is no longer its chunk's
/// latest is a superseded write and is skipped, which is what removes the
/// repeats without a set to collect into.
#[derive(Clone, Debug)]
pub struct ChangedChunks<'a> {
    volume: &'a Volume,
    /// The last generation already reported. The next candidate is the
    /// change after this one.
    next: u64,
}

impl Iterator for ChangedChunks<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let capacity = self.volume.log.len();
        while self.next < self.volume.generation {
            self.next += 1;
            let generation = self.next;
            let slot = slot_of(generation, capacity)?;
            let chunk = *self.volume.log.get(slot)? as usize;
            // Only the chunk's most recent change reports it. An earlier
            // entry for the same chunk is a write that has since been
            // superseded, and reporting it again would hand the consumer
            // the same work twice.
            if self.volume.last_changed.get(chunk) == Some(&generation) {
                return Some(chunk);
            }
        }
        None
    }
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
        Volume::new(Cell::new(0, 0, 0), (32, 32, 32)).expect("a small volume is addressable")
    }

    /// A chunk's hash computed by walking its cells, with no reference to
    /// the incremental path.
    ///
    /// **This is the independent side of the equivalence.** Comparing two
    /// volumes that both went through `set` proves only that exclusive-or
    /// commutes; if `term` were wrong, or `set` folded the wrong chunk, both
    /// sides would carry the same mistake and agree.
    fn hash_from_scratch(volume: &Volume, chunk: usize) -> u64 {
        // Read the storage directly rather than through `cell_at` and
        // `get`: going the long way needed two defensive arms nothing
        // could ever take, and an unreachable branch in the one helper
        // that is supposed to be the independent check is the last place
        // to put something nobody can exercise.
        let mut hash = 0u64;
        for (within, voxel) in volume
            .cells
            .iter()
            .skip(chunk * CHUNK_CELLS)
            .take(CHUNK_CELLS)
            .enumerate()
        {
            if !voxel.is_empty() {
                hash ^= term(within, *voxel);
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
    fn containment_is_about_the_extent_and_not_the_contents() {
        let mut v = volume();
        let inside = Cell::new(3, 3, 3);
        assert!(v.contains(inside), "an empty cell is still a cell");
        v.set(inside, STONE);
        assert!(v.contains(inside));
        assert!(!v.contains(Cell::new(-1, 0, 0)));
        assert!(!v.contains(v.max().offset(1, 0, 0)));
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
        let mut here = Volume::new(Cell::new(0, 0, 0), (16, 16, 16)).expect("volume");
        here.set(Cell::new(0, 0, 0), STONE);
        let mut there = Volume::new(Cell::new(100, 100, 100), (1, 1, 1)).expect("volume");
        there.set(Cell::new(100, 100, 100), STONE);
        assert_ne!(here.digest(), there.digest());
    }

    #[test]
    fn two_volumes_of_different_shape_do_not_share_a_digest() {
        let mut wide = Volume::new(Cell::new(0, 0, 0), (32, 16, 16)).expect("volume");
        wide.set(Cell::new(CHUNK, 0, 0), STONE);
        let mut tall = Volume::new(Cell::new(0, 0, 0), (16, 32, 16)).expect("volume");
        tall.set(Cell::new(0, CHUNK, 0), STONE);
        assert_ne!(
            wide.digest(),
            tall.digest(),
            "the extent is part of what a volume is"
        );
    }

    #[test]
    fn two_volumes_that_allocate_alike_and_address_differently_do_not_share_a_digest() {
        // The case the test above cannot see, because it varies only the
        // extent in chunks. Sizes 33 through 48 all allocate three chunks
        // along an axis, so these two are identical in everything the
        // digest used to fold and disagree about which cells exist.
        //
        // 41 by 12 by 41 is the engine's own voxel sample, so this is the
        // shipped size rather than a constructed one.
        let ragged = Volume::new(Cell::new(0, 0, 0), (41, 12, 41)).expect("volume");
        let whole = Volume::new(Cell::new(0, 0, 0), (48, 12, 48)).expect("volume");
        assert_eq!(
            ragged.chunks(),
            whole.chunks(),
            "the premise: they allocate identically"
        );
        assert_ne!(
            ragged.contains(Cell::new(45, 1, 1)),
            whole.contains(Cell::new(45, 1, 1)),
            "and they disagree about which cells exist"
        );
        assert_ne!(
            ragged.digest(),
            whole.digest(),
            "so a digest that called them equal would be reporting two \
             worlds no later write could ever make diverge as one"
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
    fn a_chunk_past_the_end_has_no_origin() {
        let v = volume();
        assert!(v.chunk_origin(v.chunk_count() - 1).is_some());
        assert_eq!(v.chunk_origin(v.chunk_count()), None);
        assert_eq!(v.chunk_origin(usize::MAX), None);
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
        let v = Volume::new(Cell::new(-8, 5, -3), (32, 16, 32)).expect("volume");
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
        let mut v = Volume::new(Cell::new(0, 0, 0), (16, 16, 16)).expect("volume");
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
        // Rounding the extent up to whole chunks must not be the thing
        // that overflows: the request is refused, not wrapped.
        assert!(Volume::new(Cell::new(0, 0, 0), (i32::MAX, 1, 1)).is_none());
        // Past the storage ceiling, counted in chunks however the caller
        // spelled it in cells.
        let too_many_cells = i32::try_from(MAX_CHUNKS + 1).unwrap_or(i32::MAX / CHUNK) * CHUNK;
        assert!(Volume::new(Cell::new(0, 0, 0), (too_many_cells, CHUNK, CHUNK)).is_none());
    }

    #[test]
    fn an_extent_that_is_not_a_whole_number_of_chunks_stays_that_extent() {
        // The defect the sample migration found. Storage rounds up; the
        // addressable extent does not. A volume that quietly grew to the
        // next multiple of sixteen would put empty cells where the caller
        // believed there was nothing — which a mesher reads as air against
        // the outer wall and draws a face for, so the world grows an
        // outside.
        let v = Volume::new(Cell::new(-20, 0, -20), (41, 12, 41)).expect("volume");
        assert_eq!(v.size(), (41, 12, 41), "the extent was rounded");
        assert_eq!(v.chunks(), (3, 1, 3), "but the storage was not");
        assert_eq!(v.max(), Cell::new(20, 11, 20));
        assert_eq!(v.get(Cell::new(20, 11, 20)), Some(Voxel::EMPTY));
        for past in [
            Cell::new(21, 0, 0),
            Cell::new(0, 12, 0),
            Cell::new(0, 0, 21),
        ] {
            assert_eq!(v.get(past), None, "{past:?} is past the extent asked for");
        }
    }

    #[test]
    fn a_volume_whose_top_would_not_fit_is_refused() {
        // Every cell a volume claims has to be addressable; saturating at
        // the ceiling would leave the top cells permanently unwritable.
        assert!(Volume::new(Cell::new(i32::MAX - 5, 0, 0), (32, 16, 16)).is_none());
        let v = Volume::new(Cell::new(i32::MAX - 5, 0, 0), (16, 16, 16));
        assert!(v.is_none(), "sixteen cells do not fit in six");
    }

    #[test]
    fn the_top_cell_of_a_volume_is_writable() {
        let mut v = Volume::new(Cell::new(-3, -3, -3), (32, 16, 16)).expect("volume");
        let top = v.max();
        assert!(v.set(top, STONE), "the highest cell must be reachable");
        assert_eq!(v.get(top), Some(STONE));
        assert!(
            !v.set(top.offset(1, 0, 0), STONE),
            "and one past it must not"
        );
    }

    /// Chunks reported since a mark, collected in the order they came.
    fn since(v: &Volume, mark: u64) -> Option<Vec<usize>> {
        v.changed_since(mark).map(Iterator::collect)
    }

    #[test]
    fn a_volume_nobody_has_written_to_reports_nothing() {
        let v = volume();
        assert_eq!(v.generation(), 0);
        assert_eq!(since(&v, 0), Some(vec![]));
    }

    #[test]
    fn a_write_that_changed_nothing_does_not_advance_the_mark() {
        // The three ways to write without changing a cell: outside the
        // volume, and the value that is already there — twice, once when
        // that value is empty and once when it is not. None may cost a
        // consumer a re-mesh, which is the whole point of the feed.
        let mut v = volume();
        v.set(Cell::new(1, 1, 1), STONE);
        let mark = v.generation();

        assert!(!v.set(Cell::new(-5, 0, 0), STONE), "outside");
        assert!(!v.set(Cell::new(1, 1, 1), STONE), "already stone");
        assert!(!v.set(Cell::new(2, 2, 2), Voxel::EMPTY), "already empty");

        assert_eq!(v.generation(), mark);
        assert_eq!(since(&v, mark), Some(vec![]));
    }

    #[test]
    fn only_the_chunks_that_changed_are_reported() {
        let mut v = volume();
        let mark = v.generation();
        let far = Cell::new(CHUNK + 1, 1, 1);
        v.set(Cell::new(1, 1, 1), STONE);
        v.set(far, SAND);

        let reported = since(&v, mark).expect("the mark is fresh");
        assert_eq!(
            reported,
            vec![
                v.chunk_of(Cell::new(1, 1, 1)).expect("inside"),
                v.chunk_of(far).expect("inside"),
            ],
            "in the order they changed, and nothing else"
        );
    }

    #[test]
    fn a_chunk_written_many_times_is_reported_once() {
        // The property that lets a consumer drive work straight from the
        // feed. Without it an automaton that touched one chunk a thousand
        // times would hand the mesher a thousand identical jobs.
        let mut v = volume();
        let mark = v.generation();
        for x in 0..8 {
            v.set(Cell::new(x, 0, 0), STONE);
        }
        let chunk = v.chunk_of(Cell::new(0, 0, 0)).expect("inside");
        assert_eq!(since(&v, mark), Some(vec![chunk]));
        assert_eq!(v.generation(), 8, "though every write was counted");
    }

    #[test]
    fn a_chunk_reported_once_is_reported_again_when_it_changes_again() {
        // The mirror of the test above, and the failure it guards is
        // worse: a dedup that remembered "already told you" rather than
        // "this is the latest" would silently stop reporting a chunk that
        // keeps changing.
        let mut v = volume();
        let chunk = v.chunk_of(Cell::new(0, 0, 0)).expect("inside");
        let first = v.generation();
        v.set(Cell::new(0, 0, 0), STONE);
        assert_eq!(since(&v, first), Some(vec![chunk]));

        let second = v.generation();
        v.set(Cell::new(1, 0, 0), STONE);
        assert_eq!(since(&v, second), Some(vec![chunk]));
        assert_eq!(
            since(&v, first),
            Some(vec![chunk]),
            "and the older mark still names it exactly once"
        );
    }

    #[test]
    fn two_consumers_read_the_same_feed_without_disturbing_each_other() {
        // The reason this is a mark rather than a dirty flag. A flag has
        // to be cleared by whoever read it, and the first reader would
        // hide the change from the second.
        let mut v = volume();
        let mesher = v.generation();
        v.set(Cell::new(1, 1, 1), STONE);
        let automaton = v.generation();
        v.set(Cell::new(CHUNK + 1, 1, 1), SAND);

        let near = v.chunk_of(Cell::new(1, 1, 1)).expect("inside");
        let far = v.chunk_of(Cell::new(CHUNK + 1, 1, 1)).expect("inside");
        assert_eq!(
            since(&v, mesher),
            Some(vec![near, far]),
            "the one who has been away longer sees both"
        );
        assert_eq!(
            since(&v, automaton),
            Some(vec![far]),
            "and reading it did not consume anything"
        );
    }

    #[test]
    fn a_mark_older_than_the_ring_asks_for_everything() {
        // Overflow is not an error; it is the feed saying the answer is no
        // longer cheaper than a rescan. The refusal must be reported
        // rather than answered wrongly, because answering with whatever
        // survived in the ring would silently lose chunks.
        let mut v = Volume::new(Cell::new(0, 0, 0), (CHUNK, CHUNK, CHUNK)).expect("one chunk");
        assert_eq!(v.chunk_count(), 1, "so the ring holds exactly one change");
        let mark = v.generation();
        v.set(Cell::new(0, 0, 0), STONE);
        assert!(
            since(&v, mark).is_some(),
            "one change still fits the ring exactly"
        );
        v.set(Cell::new(1, 0, 0), STONE);
        assert_eq!(since(&v, mark), None, "two do not");
        assert!(
            since(&v, v.generation()).is_some(),
            "but a fresh mark is always answerable"
        );
    }

    #[test]
    fn a_mark_from_the_future_is_refused_rather_than_answered() {
        // It cannot have come from this volume. Subtracting it would
        // underflow, and the tempting reading — "nothing has changed
        // since a moment that has not happened" — is the one answer
        // guaranteed to be wrong for the consumer that got here by
        // holding a mark from a different volume.
        let v = volume();
        assert_eq!(since(&v, 1), None);
        assert_eq!(since(&v, u64::MAX), None);
    }

    #[test]
    fn the_feed_agrees_with_the_versions_it_exists_to_replace() {
        // The independent check: whatever the ring says changed must be
        // exactly the set the per-chunk version scan finds. This is what
        // would catch a ring index that drifted from the generation.
        let mut v = volume();
        let before: Vec<u32> = v.chunk_versions().to_vec();
        let mark = v.generation();

        v.set(Cell::new(1, 1, 1), STONE);
        v.set(Cell::new(CHUNK + 1, 1, 1), SAND);
        v.set(Cell::new(1, CHUNK + 1, 1), STONE);
        v.set(Cell::new(1, 1, 1), SAND);

        let mut from_feed: Vec<usize> = since(&v, mark).expect("fresh");
        from_feed.sort_unstable();
        let from_versions: Vec<usize> = v
            .chunk_versions()
            .iter()
            .zip(&before)
            .enumerate()
            .filter_map(|(chunk, (now, then))| (now != then).then_some(chunk))
            .collect();
        assert_eq!(from_feed, from_versions);
        assert_eq!(from_feed.len(), 3, "and it is not vacuously empty");
    }

    #[test]
    fn the_generation_is_not_in_the_digest() {
        // Bookkeeping, not content. Two volumes holding the same cells
        // must digest the same however many writes it took to get there,
        // or a replay that reached the same world by a different route
        // would report a divergence that is not one.
        let mut direct = volume();
        direct.set(Cell::new(2, 2, 2), STONE);

        let mut roundabout = volume();
        roundabout.set(Cell::new(2, 2, 2), SAND);
        roundabout.set(Cell::new(2, 2, 2), Voxel::EMPTY);
        roundabout.set(Cell::new(2, 2, 2), STONE);

        assert_ne!(direct.generation(), roundabout.generation());
        assert_eq!(direct.digest(), roundabout.digest());
    }
}
