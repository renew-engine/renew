//! Process-wide allocation counters, written by [`crate::CountingAllocator`]
//! and read through [`snapshot`]. Diagnostics only: monotonic counts and a
//! byte gauge, never consulted for control flow.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES_IN_USE: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

/// One coherent-enough read of the counters. Individual fields are read
/// independently (relaxed), so under concurrent allocation the fields may
/// be from slightly different instants — fine for diagnostics, never for
/// logic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Successful allocations (including the alloc half of realloc).
    pub allocations: u64,
    /// Deallocations (including the dealloc half of realloc).
    pub deallocations: u64,
    /// Bytes currently allocated through the counting allocator.
    pub bytes_in_use: usize,
    /// The largest `bytes_in_use` observed by any single allocation's
    /// own accounting. Under concurrent allocation this can sit slightly
    /// below the true instantaneous peak (each thread records the total
    /// it saw, not a global maximum at every instant) — diagnostics
    /// precision, not ledger precision.
    pub peak_bytes: usize,
}

/// Read the counters.
#[must_use]
pub fn snapshot() -> Snapshot {
    Snapshot {
        allocations: ALLOCATIONS.load(Ordering::Relaxed),
        deallocations: DEALLOCATIONS.load(Ordering::Relaxed),
        bytes_in_use: BYTES_IN_USE.load(Ordering::Relaxed),
        peak_bytes: PEAK_BYTES.load(Ordering::Relaxed),
    }
}

pub(crate) fn record_alloc(size: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    let now = BYTES_IN_USE
        .fetch_add(size, Ordering::Relaxed)
        .wrapping_add(size);
    PEAK_BYTES.fetch_max(now, Ordering::Relaxed);
}

pub(crate) fn record_dealloc(size: usize) {
    DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    BYTES_IN_USE.fetch_sub(size, Ordering::Relaxed);
}
