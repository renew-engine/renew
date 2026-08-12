//! How the merged surface scales when the cells get smaller.
//!
//! The question this crate's consumers actually have: if I halve the cell
//! size, what happens to my mesh? The naive answer is "four times the
//! quads", which is what makes fine voxels unaffordable. This pins the
//! merged answer instead, on a shape whose *form* is fixed while its
//! resolution changes — so the only variable is how finely it is cut.
//!
//! Measured on this fixture at one, two, four and eight cells per unit:
//!
//! | cells per unit | unmerged faces | merged quads |
//! |---|---|---|
//! | 1 | 1,056 | 3 |
//! | 2 | 4,224 | 3 |
//! | 4 | 16,896 | 3 |
//! | 8 | 67,584 | 3 |
//!
//! The left column is an area and quadruples per step. The right one is
//! the shape, and the shape did not change. That is the whole argument
//! for merging, and it is why finer cells are affordable at all.
//!
//! The assertions below are deliberately looser than those numbers: they
//! pin the *scaling*, which is the property, rather than the exact counts,
//! which would make this a change-detector for the fixture.

use renew_volume::{Beyond, Cell, Volume, Voxel, faces};

const STONE: Voxel = Voxel(1);

/// A flat floor with a step in it, built at a given number of cells per
/// unit of world. The shape is the same at every scale.
// Test helper (called only from #[test] fns): the tests-only expect
// allowance covers #[test] fns and not their helpers, so it is extended
// here for the same reason it exists there.
#[allow(clippy::expect_used)]
fn terraced_floor(scale: i32) -> Volume {
    let side = 32 * scale;
    let mut volume = Volume::new(Cell::new(0, 0, 0), (side, 8 * scale, side)).expect("addressable");
    volume.fill(
        Cell::new(0, 0, 0),
        Cell::new(side - 1, scale - 1, side - 1),
        STONE,
    );
    volume.fill(
        Cell::new(0, scale, 0),
        Cell::new(side / 2, 2 * scale - 1, side - 1),
        STONE,
    );
    volume
}

/// Every exposed cell face, unmerged: what the obvious mesher emits.
fn unmerged(volume: &Volume) -> usize {
    faces(volume, Beyond::Solid)
        .iter()
        .map(|quad| usize::try_from(quad.merged()).unwrap_or(0))
        .sum()
}

#[test]
fn the_merged_count_follows_the_shape_and_not_the_resolution() {
    // The same terraced floor at one, two and four cells per unit. The
    // unmerged count quadruples each step, because it is an area in
    // cells. The merged count must not, because the shape did not change.
    let mut merged = Vec::new();
    let mut naive = Vec::new();
    for scale in [1, 2, 4] {
        let volume = terraced_floor(scale);
        merged.push(faces(&volume, Beyond::Solid).len());
        naive.push(unmerged(&volume));
    }

    assert!(
        naive[2] > naive[0] * 8,
        "the unmerged count is an area and must grow with the square of the \
         resolution: {naive:?}"
    );
    assert_eq!(
        merged[0], merged[2],
        "the shape did not change, so the merged count must not either: {merged:?}"
    );
    assert!(
        merged[2] * 200 < naive[2],
        "and at the finest resolution the saving must be large: {} merged \
         against {} unmerged",
        merged[2],
        naive[2]
    );
}
