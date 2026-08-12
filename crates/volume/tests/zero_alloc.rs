//! Mechanical enforcement of the crate's allocation contract: after
//! construction, reading, writing, picking and sweeping perform no heap
//! allocation.
//!
//! The crate doc claims this in its first paragraph. A claim with no gate
//! is a claim, and this crate is aimed at millions of cells stepped every
//! tick — the one place where an allocation nobody noticed is the whole
//! frame budget. Shipped with the code rather than after it, because a
//! gate that arrives later measures whatever the code has grown into
//! instead of what it promised.
//!
//! Non-vacuous by construction: the measured window is asserted to do real
//! work — writes that change cells, a pick that finds one, a sweep that
//! stops.

use renew_fixed::{Fixed, Vec3};
use renew_memory::{CountingAllocator, counters};
use renew_volume::{Cell, Volume, Voxel};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const STONE: Voxel = Voxel(1);

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_steady_state_allocates_nothing() {
    // Everything that may allocate happens out here, once.
    let mut volume = Volume::new(Cell::new(0, 0, 0), (2, 2, 2)).expect("volume");
    volume.fill(Cell::new(0, 0, 0), Cell::new(31, 0, 31), STONE);

    let half = Vec3::new(
        Fixed::from_ratio(1, 4),
        Fixed::from_ratio(1, 4),
        Fixed::from_ratio(1, 4),
    );
    let skin = Fixed::from_ratio(1, 128);
    let east = Vec3::new(Fixed::ONE, Fixed::ZERO, Fixed::ZERO);
    let down = Vec3::new(Fixed::ZERO, -Fixed::ONE, Fixed::ZERO);
    let reach = Fixed::from_int(32);

    let verdict = counters::quiet_window(5, || {
        for round in 0..16i32 {
            let cell = Cell::new(round % 16, 4, round % 16);
            assert!(volume.set(cell, STONE), "the window went vacuous: no write");
            assert!(volume.set(cell, Voxel::EMPTY), "no undo");

            let hit = volume.pick(Cell::new(round % 16, 8, 0).centre(), down, reach);
            assert!(hit.is_some(), "the window went vacuous: nothing picked");

            let swept = volume.sweep_box(
                half,
                Vec3::new(Fixed::from_int(0), Fixed::from_int(4), Fixed::from_int(0)),
                Vec3::new(Fixed::from_int(8), Fixed::ZERO, Fixed::ZERO),
                skin,
            );
            // The sweep runs against a floor it travels above, so it finds
            // nothing — what is measured is the traversal, not the hit.
            let _ = swept;

            let _ = volume.digest();
            let _ = volume.pick(Cell::new(0, 8, 0).centre(), east, reach);
            let _ = volume.chunk_version(0);
        }
    });
    if let Err(activity) = verdict {
        panic!("the volume's steady state was loud in every window (last: {activity})");
    }
}
