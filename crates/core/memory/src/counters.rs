//! Process-wide allocation counters, written by [`crate::CountingAllocator`]
//! and read through [`snapshot`]. Diagnostics only: monotonic counts and a
//! byte gauge, never consulted for engine control flow. The one sanctioned
//! consumer of these numbers as a decision is [`quiet_window`], the test
//! oracle that lives here so the policy interpreting the counters sits
//! beside the counters it interprets — one place to change, however many
//! suites call it.

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

/// Allocator activity across one measured window: what [`quiet_window`]
/// reports when no window came back quiet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivityDelta {
    /// Allocations recorded during the window.
    pub allocations: u64,
    /// Deallocations recorded during the window.
    pub deallocations: u64,
}

impl core::fmt::Display for ActivityDelta {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "+{} allocations, +{} deallocations",
            self.allocations, self.deallocations
        )
    }
}

/// Run `window` up to `attempts` times and succeed on the first run
/// with zero allocator activity — allocations and deallocations both.
///
/// **This is the retry-until-quiet policy, in its one home.** The
/// counters are process-wide and a test harness's own threads may
/// allocate concurrently, so a single window cannot distinguish
/// neighbor noise from a real regression. Retrying can: one-shot noise
/// rides out, while a defect on the measured path reproduces in every
/// window and still fails. On failure the returned delta is the LAST
/// window's activity, for the panic message the caller writes.
///
/// The window itself must not allocate on its success path — that is
/// the very property being measured — and this function allocates
/// nothing either: the delta formats only when someone reports it.
///
/// # Errors
///
/// The last window's [`ActivityDelta`] when every attempt saw activity.
pub fn quiet_window(attempts: usize, mut window: impl FnMut()) -> Result<(), ActivityDelta> {
    // Zero attempts would return a zeros delta that reads as "loud
    // with no activity" — a caller absurdity the old copies shared
    // silently; refused by name in dev builds now.
    debug_assert!(attempts > 0, "a quiet window needs at least one attempt");
    let mut last = ActivityDelta {
        allocations: 0,
        deallocations: 0,
    };
    for _ in 0..attempts {
        let before = snapshot();
        window();
        let after = snapshot();
        last = ActivityDelta {
            allocations: after.allocations - before.allocations,
            deallocations: after.deallocations - before.deallocations,
        };
        if last.allocations == 0 && last.deallocations == 0 {
            return Ok(());
        }
    }
    Err(last)
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
