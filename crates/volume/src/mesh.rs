//! Turning a volume into the smallest set of rectangles that covers its
//! surface.
//!
//! # Why the faces are merged rather than counted
//!
//! One quad per exposed cell face is the obvious mesher and it scales with
//! surface *area in cells*, which is the wrong quantity: halving the cell
//! size quadruples it. A flat floor of a thousand cells a side is a
//! million quads that draw exactly what one quad draws.
//!
//! Merging coplanar runs of the same voxel — greedy meshing — makes the
//! count scale with the surface's *complexity* instead. The floor above
//! becomes one quad, and a world of small cells becomes affordable for the
//! reason it should be: smaller cells make the shape finer, not the shape
//! more complicated.
//!
//! # What is deliberately not here
//!
//! **No corners, no winding, no vertex format.** A quad names a rectangle
//! of the lattice; turning that into triangles decides handedness, texture
//! orientation and index order, and those belong to whoever is drawing.
//! Returning corners would settle them for every consumer at once.
//!
//! **No smoothing, no marching cubes.** This mesher is exact: the surface
//! it emits is the boundary of the solid cells, to the cell. A rounded
//! isosurface is a different output from the same data and would be a
//! second function, not an option on this one.

use crate::{Cell, Face, Volume, Voxel};

/// What lies outside the volume, for deciding whether its outer shell is
/// a surface.
///
/// The volume itself refuses to answer this — [`Volume::get`] returns
/// nothing outside precisely so the caller decides — and a mesher has to
/// decide it, because "is the cell beside this one empty" has no answer at
/// the boundary otherwise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Beyond {
    /// Outside is solid: the outer shell is not a surface and gets no
    /// faces.
    ///
    /// For a world closed by walls the player is never outside of. Drawing
    /// those faces paints the back of the sky, visible the moment the
    /// camera leaves the world or a wall becomes transparent.
    Solid,
    /// Outside is empty: the outer shell is a surface and gets faces.
    ///
    /// For a volume meant to be seen from outside — a floating island, a
    /// model, a single chunk being previewed on its own.
    Empty,
}

/// A rectangle of coplanar faces, all of one voxel, drawn as one.
///
/// # How to read it
///
/// `cell` is the **lowest** cell of the run by `(x, y, z)`, and `extent`
/// is how many cells the rectangle spans along each axis. The component
/// on the face's own axis is always one — a face has no thickness — and
/// the other two are at least one. A single unmerged face is an extent of
/// `(1, 1, 1)`.
///
/// That is an extent in cells rather than a width and a height on purpose.
/// A width and a height need a stated in-plane basis per face, and every
/// consumer would have to learn it and could silently disagree with it;
/// three axes are the same three axes everywhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Quad {
    /// The lowest cell of the run.
    pub cell: Cell,
    /// Which side of those cells the rectangle covers.
    pub face: Face,
    /// What they are made of.
    pub voxel: Voxel,
    /// How many cells the rectangle spans along x, y and z.
    pub extent: (i32, i32, i32),
}

impl Quad {
    /// How many cell faces this one rectangle stands for.
    #[must_use]
    pub const fn merged(&self) -> i32 {
        self.extent.0 * self.extent.1 * self.extent.2
    }
}

/// The six sides, in the order they are emitted.
const SIDES: [Face; 6] = [
    Face::East,
    Face::West,
    Face::Top,
    Face::Bottom,
    Face::North,
    Face::South,
];

/// Every visible face of `volume`, merged, in a stated order.
///
/// # Order
///
/// Faces in the order of [`SIDES`] — east, west, top, bottom, north,
/// south — and within one side, ascending along the face's own axis, then
/// ascending along the remaining two with x fastest where it is in-plane.
/// Stated because a mesh that came out in a different order every run
/// would make a rendered image a function of iteration order rather than
/// of the world.
#[must_use]
pub fn faces(volume: &Volume, beyond: Beyond) -> Vec<Quad> {
    let mut quads = Vec::new();
    let origin = volume.origin();
    let size = volume.size();
    for face in SIDES {
        merge_side(volume, face, beyond, origin, size, &mut quads);
    }
    quads
}

/// Every visible face of one chunk, merged, in the same order.
///
/// The faces of a chunk's outermost cells are decided by looking into the
/// neighbouring chunk, so a chunk re-meshed alone still agrees with its
/// neighbours about the seam. **What it does not do is re-mesh the
/// neighbour**: a write that hides a face on the far side of a chunk
/// boundary changes two chunks, and only one of them is in the change
/// feed. A consumer meshing per chunk re-meshes the neighbours of every
/// changed chunk too, or it leaves faces behind inside solid rock.
///
/// Appends to `quads` rather than returning, so a caller re-meshing many
/// chunks keeps one buffer instead of one allocation per chunk.
pub fn chunk_faces(volume: &Volume, chunk: usize, beyond: Beyond, quads: &mut Vec<Quad>) {
    let Some(origin) = volume.chunk_origin(chunk) else {
        return;
    };
    let size = chunk_extent(volume, origin);
    for face in SIDES {
        merge_side(volume, face, beyond, origin, size, quads);
    }
}

/// How far a chunk extends before the volume's own edge cuts it short.
///
/// The last chunk on an axis is a full chunk of storage but not
/// necessarily a full chunk of *addressable* cells: a volume is sized in
/// cells and does not round up.
///
/// **This is a work-saving clamp, not a correctness one, and the
/// difference was established rather than assumed.** Removing it does not
/// change the surface: [`Volume::get`] answers nothing outside the extent,
/// so a cell past the end never enters the mask whichever way `beyond`
/// is set. What it changes is how much is scanned and how large the mask
/// is — a one-cell-deep last chunk would otherwise walk all sixteen. The
/// first version of this comment claimed the clamp prevented phantom
/// faces; a mutation of it left every test passing, which is what said
/// the claim was false.
fn chunk_extent(volume: &Volume, origin: Cell) -> (i32, i32, i32) {
    let max = volume.max();
    (
        (max.x - origin.x + 1).min(crate::CHUNK),
        (max.y - origin.y + 1).min(crate::CHUNK),
        (max.z - origin.z + 1).min(crate::CHUNK),
    )
}

/// Whether the cell on the far side of `face` leaves this one exposed.
fn exposed(volume: &Volume, cell: Cell, face: Face, beyond: Beyond) -> bool {
    let (dx, dy, dz) = face.step();
    match volume.get(cell.offset(dx, dy, dz)) {
        Some(voxel) => voxel.is_empty(),
        // Outside the volume. `None` is not "empty" — that is the whole
        // reason the volume refuses to guess here.
        None => beyond == Beyond::Empty,
    }
}

/// Merge one side of one box of cells into rectangles.
///
/// Walks slice by slice along the face's own axis. Within a slice the
/// exposed cells form a two-dimensional picture of voxel identifiers, and
/// the merge is the usual greedy one: take the lowest unclaimed cell,
/// widen along the first in-plane axis while the voxel matches, then grow
/// along the second while the whole row matches, and claim what it covered.
///
/// The mask is a flat `Vec` reused across slices rather than one per
/// slice, because the allocation would otherwise be per slice per side —
/// six times the depth of the volume, every re-mesh.
fn merge_side(
    volume: &Volume,
    face: Face,
    beyond: Beyond,
    origin: Cell,
    size: (i32, i32, i32),
    quads: &mut Vec<Quad>,
) {
    let (normal, first, second) = axes(face);
    let depth = axis(size, normal);
    let width = axis(size, first);
    let height = axis(size, second);
    // No guard against a dimension below one: the loops below are exclusive
    // ranges that simply do not run, and `usize_of` floors the mask at
    // zero. An early return would be a line nothing can execute, since a
    // volume clamps every dimension up to one at construction.
    let area = usize_of(width) * usize_of(height);
    let mut mask: Vec<Option<Voxel>> = vec![None; area];

    for slice in 0..depth {
        // Fill the mask for this slice: what is exposed, and of what.
        for v in 0..height {
            for u in 0..width {
                let cell = compose(origin, normal, slice, first, u, second, v);
                let visible = volume
                    .get(cell)
                    .filter(|voxel| !voxel.is_empty())
                    .filter(|_| exposed(volume, cell, face, beyond));
                if let Some(slot) = mask.get_mut(usize_of(v) * usize_of(width) + usize_of(u)) {
                    *slot = visible;
                }
            }
        }

        // Greedily claim rectangles out of it.
        for v in 0..height {
            let mut u = 0;
            while u < width {
                let at = usize_of(v) * usize_of(width) + usize_of(u);
                let Some(voxel) = mask.get(at).copied().flatten() else {
                    u += 1;
                    continue;
                };

                // Widen while the voxel matches.
                let mut run = 1;
                while u + run < width
                    && mask.get(at + usize_of(run)).copied().flatten() == Some(voxel)
                {
                    run += 1;
                }

                // Grow while every cell of the next row matches across the
                // whole run. A partial match stops the growth rather than
                // splitting the run, which is what keeps the result a
                // rectangle.
                let mut rows = 1;
                'grow: while v + rows < height {
                    let row = usize_of(v + rows) * usize_of(width) + usize_of(u);
                    for step in 0..usize_of(run) {
                        if mask.get(row + step).copied().flatten() != Some(voxel) {
                            break 'grow;
                        }
                    }
                    rows += 1;
                }

                // Claim it, so later scans skip what this quad covered.
                for row in 0..rows {
                    let base = usize_of(v + row) * usize_of(width) + usize_of(u);
                    for step in 0..usize_of(run) {
                        if let Some(slot) = mask.get_mut(base + step) {
                            *slot = None;
                        }
                    }
                }

                quads.push(Quad {
                    cell: compose(origin, normal, slice, first, u, second, v),
                    face,
                    voxel,
                    extent: extent_of(normal, first, run, second, rows),
                });
                u += run;
            }
        }
    }
}

/// A face's own axis and the two in-plane axes, as indices into `(x, y, z)`.
///
/// The in-plane pair is always in ascending axis order, which is what
/// makes the emitted order "x fastest where x is in-plane" true rather
/// than accidental.
const fn axes(face: Face) -> (usize, usize, usize) {
    match face {
        Face::East | Face::West => (0, 1, 2),
        Face::Top | Face::Bottom => (1, 0, 2),
        Face::North | Face::South => (2, 0, 1),
    }
}

/// One component of an extent, by axis index.
const fn axis(size: (i32, i32, i32), index: usize) -> i32 {
    match index {
        0 => size.0,
        1 => size.1,
        _ => size.2,
    }
}

/// The cell at the given offsets along the three named axes.
fn compose(
    origin: Cell,
    normal: usize,
    slice: i32,
    first: usize,
    u: i32,
    second: usize,
    v: i32,
) -> Cell {
    let mut offset = [0; 3];
    if let Some(slot) = offset.get_mut(normal) {
        *slot = slice;
    }
    if let Some(slot) = offset.get_mut(first) {
        *slot = u;
    }
    if let Some(slot) = offset.get_mut(second) {
        *slot = v;
    }
    origin.offset(offset[0], offset[1], offset[2])
}

/// An extent of one on the face's axis and the run lengths on the others.
fn extent_of(normal: usize, first: usize, run: i32, second: usize, rows: i32) -> (i32, i32, i32) {
    let mut extent = [1; 3];
    if let Some(slot) = extent.get_mut(normal) {
        *slot = 1;
    }
    if let Some(slot) = extent.get_mut(first) {
        *slot = run;
    }
    if let Some(slot) = extent.get_mut(second) {
        *slot = rows;
    }
    (extent[0], extent[1], extent[2])
}

/// A non-negative count as an index.
fn usize_of(value: i32) -> usize {
    usize::try_from(value.max(0)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{Beyond, Quad, chunk_faces, faces};
    use crate::{CHUNK, Cell, Face, Volume, Voxel};

    const STONE: Voxel = Voxel(1);
    const SAND: Voxel = Voxel(2);

    fn volume() -> Volume {
        Volume::new(Cell::new(0, 0, 0), (32, 32, 32)).expect("a small volume is addressable")
    }

    /// Every cell face the merged quads stand for, as (cell, face) pairs.
    ///
    /// The independent check: expanding the merged output has to give back
    /// exactly the naive per-cell mesher's answer. It is written by
    /// expanding rather than by re-deriving, so it cannot share a mistake
    /// with the merger.
    fn expanded(quads: &[Quad]) -> Vec<(Cell, Face)> {
        let mut out = Vec::new();
        for quad in quads {
            for z in 0..quad.extent.2 {
                for y in 0..quad.extent.1 {
                    for x in 0..quad.extent.0 {
                        out.push((quad.cell.offset(x, y, z), quad.face));
                    }
                }
            }
        }
        out.sort_by_key(|(cell, face)| (cell.x, cell.y, cell.z, format!("{face:?}")));
        out
    }

    /// The obvious mesher: one quad per exposed cell face, no merging.
    fn naive(volume: &Volume, beyond: Beyond) -> Vec<(Cell, Face)> {
        let mut out = Vec::new();
        for (cell, _) in volume.solids() {
            for face in super::SIDES {
                if super::exposed(volume, cell, face, beyond) {
                    out.push((cell, face));
                }
            }
        }
        out.sort_by_key(|(cell, face)| (cell.x, cell.y, cell.z, format!("{face:?}")));
        out
    }

    #[test]
    fn an_empty_volume_has_no_surface() {
        assert!(faces(&volume(), Beyond::Solid).is_empty());
        assert!(faces(&volume(), Beyond::Empty).is_empty());
    }

    #[test]
    fn one_cell_alone_has_six_faces_and_none_of_them_merge() {
        let mut v = volume();
        v.set(Cell::new(4, 4, 4), STONE);
        let quads = faces(&v, Beyond::Solid);
        assert_eq!(quads.len(), 6);
        assert!(
            quads.iter().all(|quad| quad.extent == (1, 1, 1)),
            "a lone cell has nothing to merge with"
        );
    }

    #[test]
    fn a_flat_slab_becomes_one_quad_per_side() {
        // The headline case, and the reason this exists: a floor of 400
        // cells has 400 top faces and they are one rectangle.
        let mut v = volume();
        v.fill(Cell::new(1, 1, 1), Cell::new(20, 1, 20), STONE);
        let quads = faces(&v, Beyond::Solid);

        let top: Vec<&Quad> = quads.iter().filter(|q| q.face == Face::Top).collect();
        assert_eq!(top.len(), 1, "one rectangle, not four hundred");
        assert_eq!(top[0].cell, Cell::new(1, 1, 1));
        assert_eq!(top[0].extent, (20, 1, 20));
        assert_eq!(top[0].merged(), 400);
    }

    #[test]
    fn two_materials_do_not_merge_into_one_rectangle() {
        // The failure this guards is a floor that takes the colour of
        // whichever material the merge happened to start from.
        let mut v = volume();
        v.fill(Cell::new(1, 1, 1), Cell::new(4, 1, 4), STONE);
        v.fill(Cell::new(5, 1, 1), Cell::new(8, 1, 4), SAND);
        let top: Vec<Quad> = faces(&v, Beyond::Solid)
            .into_iter()
            .filter(|q| q.face == Face::Top)
            .collect();

        assert_eq!(top.len(), 2);
        assert_eq!(top.iter().filter(|q| q.voxel == STONE).count(), 1);
        assert_eq!(top.iter().filter(|q| q.voxel == SAND).count(), 1);
    }

    #[test]
    fn merging_covers_exactly_the_faces_the_naive_mesher_finds() {
        // The property that makes every other test about counts safe: no
        // face invented, none lost, none covered twice. Run against a
        // shape with holes, an overhang and two materials, because a
        // merger is only interesting where the mask is ragged.
        let mut v = volume();
        v.fill(Cell::new(1, 1, 1), Cell::new(12, 3, 12), STONE);
        v.fill(Cell::new(4, 2, 4), Cell::new(6, 3, 6), Voxel::EMPTY);
        v.fill(Cell::new(2, 4, 2), Cell::new(9, 4, 3), SAND);
        v.set(Cell::new(8, 8, 8), STONE);

        for beyond in [Beyond::Solid, Beyond::Empty] {
            let merged = expanded(&faces(&v, beyond));
            let naive = naive(&v, beyond);
            assert_eq!(merged, naive, "with {beyond:?} outside");
            assert!(!naive.is_empty(), "and the comparison is not vacuous");
        }
    }

    #[test]
    fn the_merged_output_is_smaller_than_the_naive_one() {
        // Not a performance claim - a count, checked. If a change made the
        // merger stop merging, every other test here would still pass.
        let mut v = volume();
        v.fill(Cell::new(1, 1, 1), Cell::new(20, 4, 20), STONE);
        let merged = faces(&v, Beyond::Solid).len();
        let naive = naive(&v, Beyond::Solid).len();
        assert!(
            merged * 20 < naive,
            "a solid box should merge to far fewer than {naive} faces, got {merged}"
        );
    }

    #[test]
    fn the_outer_shell_is_a_surface_only_when_outside_is_empty() {
        // The distinction the volume refuses to make for the mesher.
        let mut v = Volume::new(Cell::new(0, 0, 0), (4, 4, 4)).expect("volume");
        v.fill(Cell::new(0, 0, 0), Cell::new(3, 3, 3), STONE);

        assert!(
            faces(&v, Beyond::Solid).is_empty(),
            "a solid volume closed by walls shows nothing"
        );
        let open = faces(&v, Beyond::Empty);
        assert_eq!(open.len(), 6, "and opened, it is a cube of six rectangles");
        assert!(open.iter().all(|quad| quad.merged() == 16));
    }

    #[test]
    fn a_chunk_meshed_alone_agrees_with_the_whole_volume() {
        // Chunk-local meshing is what a change feed is for, so it has to
        // give the same surface. The seam is the interesting part: cells
        // on a chunk boundary decide their faces by looking into the next
        // chunk, which a naive per-chunk mesher gets wrong by treating the
        // chunk edge as the world edge.
        //
        // Both a chunk-aligned volume and a ragged one, because the last
        // chunk of a ragged volume is a full chunk of storage holding
        // fewer addressable cells, and that is where a mesher walking
        // storage rather than extent would show itself.
        for size in [(32, 32, 32), (20, 20, 20)] {
            let mut v = Volume::new(Cell::new(0, 0, 0), size).expect("volume");
            let top = size.1 - 1;
            v.fill(
                Cell::new(0, 0, 0),
                Cell::new(size.0 - 1, 2, size.2 - 1),
                STONE,
            );
            v.fill(
                Cell::new(CHUNK - 2, 3, CHUNK - 2),
                Cell::new(CHUNK + 1, 3, CHUNK + 1),
                SAND,
            );
            v.set(Cell::new(size.0 - 1, top, size.2 - 1), STONE);

            for beyond in [Beyond::Solid, Beyond::Empty] {
                let whole = expanded(&faces(&v, beyond));
                let mut per_chunk = Vec::new();
                for chunk in 0..v.chunk_count() {
                    chunk_faces(&v, chunk, beyond, &mut per_chunk);
                }
                assert_eq!(expanded(&per_chunk), whole, "{size:?} with {beyond:?}");
                assert!(!whole.is_empty(), "and not vacuously");
            }
        }
    }

    #[test]
    fn a_chunk_that_does_not_exist_contributes_nothing() {
        // A consumer driving this from a change feed passes indices it
        // got from the volume, so this should not happen — but "should
        // not" is not "cannot", and appending nothing is the only answer
        // that cannot corrupt a buffer the caller is still filling.
        let mut v = volume();
        v.set(Cell::new(1, 1, 1), STONE);
        let mut quads = vec![];
        chunk_faces(&v, v.chunk_count(), Beyond::Solid, &mut quads);
        chunk_faces(&v, usize::MAX, Beyond::Solid, &mut quads);
        assert!(quads.is_empty());
    }

    #[test]
    fn no_face_is_ever_reported_outside_the_volume() {
        // Guaranteed by `get` refusing outside rather than by the chunk
        // clamp — see `chunk_extent`. Kept because it is the property
        // consumers rely on, and it should keep holding whichever
        // mechanism happens to provide it.
        let mut v = Volume::new(Cell::new(-3, -3, -3), (20, 20, 20)).expect("not a whole chunk");
        v.fill(Cell::new(-3, -3, -3), Cell::new(16, 16, 16), STONE);
        let mut quads = Vec::new();
        for chunk in 0..v.chunk_count() {
            chunk_faces(&v, chunk, Beyond::Empty, &mut quads);
        }
        assert!(!quads.is_empty());
        for (cell, _) in expanded(&quads) {
            assert!(
                v.contains(cell),
                "{cell:?} is outside the volume the caller asked for"
            );
        }
    }
}
