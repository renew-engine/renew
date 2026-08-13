//! The saturation counter: how an overflow that is handled stays visible.

use core::cell::Cell;

thread_local! {
    /// Saturations on this thread since it started.
    ///
    /// Thread-local rather than a global atomic, and that is the design
    /// rather than a concession. Simulation is single-threaded, so
    /// per-thread is per-simulation: a count attributable to the run that
    /// produced it, where one global atomic would have merged unrelated
    /// threads into a number nobody could act on.
    ///
    /// It is also the shape this engine's threading rules already permit —
    /// thread-local storage for diagnostics only, drained through an
    /// explicit call on the owning thread — so it needs no exception to the
    /// rule against global mutable state, which a global atomic would have.
    static SATURATIONS: Cell<u64> = const { Cell::new(0) };
}

/// How many times arithmetic on this thread saturated.
///
/// **Diagnostic only.** Never simulation state, never digested, never
/// consulted for control flow — a simulation that branched on this would
/// have made the count part of its own behaviour, and two machines whose
/// counts differ would then diverge for a reason no digest could explain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Saturations(pub u64);

impl Saturations {
    /// Nothing saturated, which is what a test asserts.
    #[must_use]
    pub const fn is_clean(self) -> bool {
        self.0 == 0
    }
}

/// Read this thread's saturation count.
///
/// The explicit snapshot call: the counter is never read from another
/// thread and never published except through here.
///
/// # Example
///
/// The shape a test uses — the counter is the alarm, and saturation is what
/// it is an alarm about:
///
/// ```
/// # use renew_fixed::{Fixed, saturations};
/// let before = saturations();
/// let _ = Fixed::MAX + Fixed::ONE;
/// assert_eq!(saturations().0, before.0 + 1);
/// ```
#[must_use]
pub fn saturations() -> Saturations {
    Saturations(SATURATIONS.with(Cell::get))
}

/// Record one saturation. Called only from the arithmetic that saturated.
pub(crate) fn record() {
    SATURATIONS.with(|count| count.set(count.get().saturating_add(1)));
}

#[cfg(test)]
mod tests {
    use super::{record, saturations};

    #[test]
    fn the_counter_counts_and_is_readable_only_through_the_snapshot() {
        let before = saturations();
        record();
        record();
        assert_eq!(saturations().0, before.0 + 2);
        assert!(!saturations().is_clean());
    }

    /// The counter must not itself overflow into wrapping, which would be a
    /// diagnostic silently claiming fewer failures than occurred.
    #[test]
    fn the_counter_saturates_rather_than_wrapping() {
        super::SATURATIONS.with(|count| count.set(u64::MAX));
        record();
        assert_eq!(saturations().0, u64::MAX);
        super::SATURATIONS.with(|count| count.set(0));
    }

    /// Per-thread, which is the property that makes it attributable. A
    /// count raised on another thread must not appear on this one.
    #[test]
    // Spawning is disallowed in this crate for good reason: arithmetic has
    // nothing to parallelise. Proving the counter is *per-thread* is the one
    // thing that cannot be done without a second thread, so the exemption is
    // taken here, narrowly, in the test that exists to establish the
    // property the ban's neighbour depends on.
    #[expect(
        clippy::disallowed_methods,
        reason = "the property under test is thread-locality, which needs a thread"
    )]
    fn a_count_on_another_thread_is_not_visible_here() {
        let before = saturations();
        std::thread::spawn(|| {
            record();
            record();
            record();
        })
        .join()
        .expect("the counting thread finished");
        assert_eq!(saturations(), before, "another thread's count leaked here");
    }
}
