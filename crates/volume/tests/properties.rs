//! Properties the volume must hold under arbitrary write sequences.
//!
//! The unit tests pin specific cases that were reasoned about. These exist
//! for the sequences nobody would think to write down.
//!
//! **What these do NOT prove**, because it is worth being exact about:
//! they compare volumes that both reached their contents through
//! `Volume::set`, so they establish that the hash is independent of the
//! order writes arrived in and that it is exactly reversible. They cannot
//! catch a wrong mixing function or a write folded into the wrong chunk —
//! both sides would carry the same mistake and agree. The independent
//! check, which walks a chunk's cells and folds the terms directly, is
//! `the_maintained_hash_equals_one_walked_from_the_cells`, a unit test in
//! `volume.rs` where the private mixing function is reachable.

use proptest::prelude::*;
use renew_fixed::{Fixed, Vec3};
use renew_volume::{CHUNK, Cell, Volume, Voxel};

/// Raw fixed-point offsets covering a whole cell: half a unit either side
/// of the centre, with the upper seam excluded because it belongs to the
/// next cell.
const OFFSETS: std::ops::Range<i64> = -32_768..32_768;

/// Two chunks on a side, so writes land in several chunks and the
/// cross-chunk bookkeeping is actually exercised.
const CHUNKS: (i32, i32, i32) = (2, 2, 2);

// Test helper (called only from #[test] fns): the tests-only expect
// allowance covers #[test] fns and not their helpers, so it is extended
// here in the same spirit. The volume is small and statically addressable,
// so the refusal arm is unreachable by construction.
#[allow(clippy::expect_used)]
fn fresh() -> Volume {
    Volume::new(Cell::new(0, 0, 0), CHUNKS).expect("a small volume is addressable")
}

/// The span of cell coordinates the volume covers, plus a margin either
/// side so that out-of-bounds writes are generated too — they are supposed
/// to be no-ops and that is worth proving rather than assuming.
fn coordinate() -> impl Strategy<Value = i32> {
    -4i32..(CHUNK * 2 + 4)
}

fn write() -> impl Strategy<Value = (Cell, Voxel)> {
    (coordinate(), coordinate(), coordinate(), 0u16..4)
        .prop_map(|(x, y, z, m)| (Cell::new(x, y, z), Voxel(m)))
}

/// Replay a volume's contents into a new one, in enumeration order rather
/// than the order they were written.
// Test helper (called only from #[test] fns): the tests-only expect
// allowance covers #[test] fns and not their helpers, so it is extended
// here in the same spirit. The volume is small and statically addressable,
// so the refusal arm is unreachable by construction.
#[allow(clippy::expect_used)]
fn rebuilt(from: &Volume) -> Volume {
    let mut volume = fresh();
    for (cell, voxel) in from.solids() {
        volume.set(cell, voxel);
    }
    volume
}

proptest! {
    #[test]
    fn the_hash_does_not_depend_on_the_order_the_writes_arrived_in(
        writes in prop::collection::vec(write(), 0..64)
    ) {
        let mut volume = fresh();
        for (cell, voxel) in writes {
            volume.set(cell, voxel);
        }
        let replayed = rebuilt(&volume);
        prop_assert_eq!(volume.digest(), replayed.digest());
        for chunk in 0..volume.chunk_count() {
            prop_assert_eq!(volume.chunk_hash(chunk), replayed.chunk_hash(chunk));
        }
    }

    #[test]
    fn the_solid_count_is_what_the_enumeration_finds(
        writes in prop::collection::vec(write(), 0..64)
    ) {
        let mut volume = fresh();
        for (cell, voxel) in writes {
            volume.set(cell, voxel);
        }
        prop_assert_eq!(volume.solid_count(), volume.solids().count());
    }

    #[test]
    fn everything_enumerated_is_readable_and_non_empty(
        writes in prop::collection::vec(write(), 0..64)
    ) {
        let mut volume = fresh();
        for (cell, voxel) in writes {
            volume.set(cell, voxel);
        }
        for (cell, voxel) in volume.solids() {
            prop_assert!(!voxel.is_empty());
            prop_assert_eq!(volume.get(cell), Some(voxel));
            prop_assert!(volume.is_solid(cell));
        }
    }

    #[test]
    fn emptying_everything_returns_the_volume_to_its_initial_digest(
        writes in prop::collection::vec(write(), 0..64)
    ) {
        // The strongest statement of reversibility: whatever route the
        // volume took, undoing every write must land exactly where it
        // started. A hash that drifts by one term fails here and nowhere
        // else.
        let mut volume = fresh();
        let empty = volume.digest();
        for (cell, voxel) in writes {
            volume.set(cell, voxel);
        }
        let occupied: Vec<Cell> = volume.solids().map(|(cell, _)| cell).collect();
        for cell in occupied {
            volume.set(cell, Voxel::EMPTY);
        }
        prop_assert_eq!(volume.digest(), empty);
        prop_assert_eq!(volume.solid_count(), 0);
    }

    #[test]
    fn a_write_outside_the_volume_is_never_a_change(
        x in -64i32..-1, y in -64i32..-1, z in -64i32..-1, m in 1u16..4
    ) {
        let mut volume = fresh();
        let before = volume.digest();
        prop_assert!(!volume.set(Cell::new(x, y, z), Voxel(m)));
        prop_assert_eq!(volume.digest(), before);
        let untouched = fresh();
        prop_assert_eq!(volume.chunk_versions(), untouched.chunk_versions());
    }

    #[test]
    fn a_bumped_chunk_is_always_one_that_exists(
        writes in prop::collection::vec(write(), 1..32)
    ) {
        let before: Vec<u32> = fresh().chunk_versions().to_vec();
        let mut volume = fresh();
        for (cell, voxel) in writes {
            volume.set(cell, voxel);
        }
        for (chunk, was) in before.iter().enumerate().take(volume.chunk_count()) {
            if volume.chunk_version(chunk) != Some(*was) {
                prop_assert!(volume.chunk_origin(chunk).is_some());
            }
        }
    }

    #[test]
    fn only_the_chunk_written_to_is_ever_bumped(
        writes in prop::collection::vec(write(), 1..16)
    ) {
        // The property the active set will rest on: a write must never
        // wake a chunk it did not touch, or the saving is nothing.
        let mut volume = fresh();
        for (cell, voxel) in writes {
            let before: Vec<u32> = volume.chunk_versions().to_vec();
            let owner = volume.chunk_of(cell);
            let changed = volume.set(cell, voxel);
            for (chunk, was) in before.iter().enumerate() {
                let bumped = volume.chunk_version(chunk) != Some(*was);
                prop_assert_eq!(bumped, changed && owner == Some(chunk));
            }
        }
    }

    #[test]
    fn any_point_inside_a_cell_resolves_to_that_cell(
        x in coordinate(), y in coordinate(), z in coordinate(),
        dx in OFFSETS, dy in OFFSETS, dz in OFFSETS,
    ) {
        // Not just centres. A cell spans half a unit either side of its
        // centre, with the boundary resolving upward, so every raw offset
        // in [-0.5, +0.5) belongs to the same cell — and the conversion
        // must not quietly depend on landing exactly on the centre.
        let cell = Cell::new(x, y, z);
        let centre = cell.centre();
        let inside = Vec3::new(
            centre.x + Fixed::from_bits(dx),
            centre.y + Fixed::from_bits(dy),
            centre.z + Fixed::from_bits(dz),
        );
        prop_assert_eq!(Cell::containing(inside), cell);
    }

    #[test]
    fn the_boundary_above_a_cell_belongs_to_the_next_one(
        x in coordinate(), y in coordinate(), z in coordinate()
    ) {
        // The other side of the same rule, which is what makes it a rule
        // rather than a rounding accident.
        let cell = Cell::new(x, y, z);
        let centre = cell.centre();
        let half = Fixed::from_ratio(1, 2);
        let seam = Vec3::new(centre.x + half, centre.y + half, centre.z + half);
        prop_assert_eq!(Cell::containing(seam), cell.offset(1, 1, 1));
    }
}
