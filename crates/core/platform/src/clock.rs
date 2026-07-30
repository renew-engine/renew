//! Monotonic time as integer nanoseconds — the engine's only clock.

use std::time::Instant;

/// A monotonic clock anchored at [`Clock::start`].
///
/// Time is integer nanoseconds to match the engine's fixed-timestep
/// vocabulary; no floating-point time exists anywhere in the engine.
/// Not for simulation state: simulation consumes fixed steps, this
/// feeds frame pacing and diagnostics.
///
/// Thread affinity: `Copy + Send + Sync` — copies share the anchor, and
/// the clock may cross threads. Monotonicity is per call sequence: two
/// reads *ordered by the program* never go backwards, but reads on
/// different threads carry no cross-thread ordering claim beyond what
/// their synchronization already provides.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    epoch: Instant,
}

impl Clock {
    /// A clock anchored to now.
    #[must_use]
    pub fn start() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }

    /// Whole nanoseconds since the anchor. Monotonic: never decreases
    /// between ordered calls on the same clock (or a copy). Saturates at
    /// `u64::MAX` (≈ 584 years) — a documented bound, not an error path.
    #[must_use]
    pub fn elapsed_nanos(&self) -> u64 {
        saturate_nanos(self.epoch.elapsed().as_nanos())
    }
}

/// u128 nanoseconds to u64, saturating — split out so the bound is
/// testable without a 584-year wait.
fn saturate_nanos(nanos: u128) -> u64 {
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_is_monotonic() {
        let clock = Clock::start();
        let first = clock.elapsed_nanos();
        let second = clock.elapsed_nanos();
        assert!(second >= first, "time went backwards: {first} -> {second}");
    }

    #[test]
    fn independent_clocks_have_independent_anchors() {
        let older = Clock::start();
        // Spin until a comfortable margin (1 ms) separates the anchors,
        // with an iteration bound so a broken clock fails loudly instead
        // of hanging. The margin makes a scheduler preemption between
        // the two reads below unable to flip the comparison in practice.
        let mut iterations = 0u64;
        while older.elapsed_nanos() < 1_000_000 {
            iterations += 1;
            assert!(iterations < 2_000_000_000, "clock never advanced");
            core::hint::black_box(iterations);
        }
        let newer = Clock::start();
        assert!(older.elapsed_nanos() >= newer.elapsed_nanos());
    }

    #[test]
    fn nanosecond_conversion_saturates_at_the_documented_bound() {
        assert_eq!(saturate_nanos(0), 0);
        assert_eq!(saturate_nanos(123_456_789), 123_456_789);
        assert_eq!(saturate_nanos(u128::from(u64::MAX)), u64::MAX);
        assert_eq!(saturate_nanos(u128::from(u64::MAX) + 1), u64::MAX);
        assert_eq!(saturate_nanos(u128::MAX), u64::MAX);
    }
}
