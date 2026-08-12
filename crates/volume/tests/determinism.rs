//! The determinism trigger: same inputs, bit-identical digest, every run.
//!
//! The manifest declares `simulation = true`, and the engine's constitution
//! makes that declaration carry an obligation — a simulation system owes a
//! test that the same inputs produce a bit-identical state hash across
//! repeated runs. This crate is the storage half of that simulation, so it
//! owes one.
//!
//! **The committed constant is a regression guard, not evidence of
//! cross-platform determinism.** It catches a change to the mixing, the
//! packing, the fold, or the iteration order on *this* machine. Proving the
//! stronger claim needs the same input replayed on the other target
//! platforms and the digests compared against each other, which is work for
//! the milestone that first has more than one machine to run on.

use renew_volume::{Cell, Volume, Voxel};

/// A deliberately mixed write sequence: several chunks, several voxels,
/// overwrites, undos, and writes that fall outside the volume.
// Test helper (called only from #[test] fns): the tests-only expect
// allowance covers #[test] fns and not their helpers, so it is extended
// here in the same spirit. The volume is small and statically addressable,
// so the refusal arm is unreachable by construction.
#[allow(clippy::expect_used)]
fn build() -> Volume {
    let mut volume = Volume::new(Cell::new(-16, -16, -16), (32, 32, 32)).expect("volume");
    let mut cursor: i64 = 1;
    for step in 0..512i64 {
        // A cheap, fully-determined walk over the address space — no RNG,
        // so the sequence is part of this file rather than of a seed.
        cursor = cursor
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let pick = |shift: u32| i32::try_from((cursor >> shift) & 0x3f).unwrap_or(0) - 20;
        let cell = Cell::new(pick(8), pick(16), pick(24));
        let voxel = Voxel(u16::try_from(step % 5).unwrap_or(0));
        volume.set(cell, voxel);
    }
    volume
}

/// Measured on this machine, then pinned — never computed by the code it
/// checks. A change here is either a real regression or a deliberate format
/// change that has to be re-recorded and explained.
///
/// Measured 2026-08-12, x86_64-pc-windows-msvc, rustc stable.
const EXPECTED: u64 = 0xac36_1146_05fe_4e3b;

#[test]
fn the_same_writes_produce_the_same_digest_every_run() {
    let first = build().digest();
    for run in 1..8 {
        assert_eq!(build().digest(), first, "run {run} disagreed with run 0");
    }
}

#[test]
fn the_digest_matches_the_committed_value() {
    let measured = build().digest();
    assert_eq!(
        measured, EXPECTED,
        "the volume's digest moved: measured {measured:#018x}, committed {EXPECTED:#018x}. \
         If this was intended, re-record the constant and say why in the commit."
    );
}

#[test]
fn the_order_writes_arrive_in_does_not_change_the_result() {
    // The same final contents reached by replaying them in enumeration
    // order rather than in the order they were written.
    let built = build();
    let mut replayed = Volume::new(built.origin(), built.size()).expect("volume");
    for (cell, voxel) in built.solids() {
        replayed.set(cell, voxel);
    }
    assert_eq!(built.digest(), replayed.digest());
}
