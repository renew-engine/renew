//! The time vocabulary: a duration, an instant, the fixed timestep, and
//! the per-frame step budget.
//!
//! Durations and instants are separate newtypes because confusing them is
//! otherwise silent: feed a duration where an instant belongs and the
//! schedule sees time running permanently backwards, freezing the
//! simulation while the window keeps drawing. Distinct types make that a
//! compile error.
//!
//! `core::time::Duration` is deliberately not used anywhere in this
//! crate. It is 12–16 bytes where 8 do, its `Sub` panics on underflow —
//! banned in engine code — and its `as_secs_f32` is exactly the float-time
//! door this engine does not have. Integer nanoseconds are the whole
//! determinism argument: with `f32` seconds the banked time accumulates
//! representation error and the step count becomes a function of rounding
//! history, while with `u64` every operation is exact and the step count
//! is a pure integer function of the input sequence.

use core::num::{NonZeroU32, NonZeroU64};

/// A span of time in whole nanoseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Nanos(u64);

impl Nanos {
    /// No time at all — what a backwards clock yields.
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A point on a monotonic timeline, in nanoseconds since an origin the
/// caller chooses. Only differences between timestamps mean anything; the
/// origin itself never enters the schedule.
///
/// Absolute timestamps rather than per-frame deltas, on purpose. One
/// subtraction happens in one place (every caller computing its own can
/// compute it wrongly), a clock that went the wrong way becomes
/// [`Nanos::ZERO`] instead of 1.1 trillion phantom steps, the first-frame
/// branch disappears into the constructor's `start` argument, and
/// resynchronizing after a known pause is trivially correct.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(u64);

impl Timestamp {
    #[must_use]
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// The raw offset from the caller's own origin. Meaningful only to
    /// whoever chose that origin — for logging and for building a
    /// synthetic timeline, never for arithmetic against another clock.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The span from `earlier` to `self`, or [`Nanos::ZERO`] if `self` is
    /// the earlier of the two. A backwards clock is a defined, harmless
    /// input rather than a wrapped `u64`.
    #[must_use]
    pub const fn saturating_since(self, earlier: Self) -> Nanos {
        Nanos(self.0.saturating_sub(earlier.0))
    }

    /// `self + span`, saturating at the end of the representable
    /// timeline (~584 years).
    #[must_use]
    pub const fn saturating_add(self, span: Nanos) -> Self {
        Self(self.0.saturating_add(span.0))
    }
}

/// The fixed simulation timestep, in nanoseconds. Non-zero by type, so no
/// division in the schedule can trap and no constructor can fail: the
/// type carries the guarantee, so nothing has to check for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timestep(NonZeroU64);

impl Timestep {
    /// 60 Hz, rounded to whole nanoseconds.
    ///
    /// 60 Hz is not representable: `60 × 16_666_667 = 1_000_000_020`, so
    /// sixty ticks run 20 ns long against the wall. That is closed by
    /// definition rather than by rounding — a step's `sim_time` and the
    /// loop's `simulated()` are `tick × dt`, so the simulation's own clock
    /// is exact by construction and the 20 ns is a property of the wall
    /// clock's relation to the simulation, never of the simulation's own
    /// arithmetic. Exact divisors exist (64 Hz = `15_625_000`,
    /// 125 Hz = `8_000_000`) if ticks ever need to land on whole seconds.
    // The `match` runs at compile time and emits no code: `unwrap` is
    // unavailable under the crate's panic policy, so a literal that was
    // edited to zero would select the fallback instead of failing loudly.
    // Every test in this file compares against the literal, so that edit
    // cannot pass unnoticed.
    pub const HZ_60: Self = Self(match NonZeroU64::new(16_666_667) {
        Some(nanos) => nanos,
        None => NonZeroU64::MIN,
    });

    #[must_use]
    pub const fn from_nanos(nanos: NonZeroU64) -> Self {
        Self(nanos)
    }

    /// The timestep in nanoseconds, still carrying its non-zero proof so
    /// a consumer computing its own exact interpolation needs no guard.
    #[must_use]
    pub const fn nanos(self) -> NonZeroU64 {
        self.0
    }
}

/// The most simulation steps one frame may execute. Everything beyond it
/// is discarded and reported, never banked.
///
/// The budget is the only guard on per-frame work, deliberately. An
/// elapsed-time clamp in front of it was considered and rejected: it only
/// changes how much time is discarded versus reported, destroying the
/// information about how big the hitch was in exchange for a second knob,
/// a second branch, and a second coverage obligation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepBudget(NonZeroU32);

impl StepBudget {
    /// Five steps — about 83 ms of simulation per frame at 60 Hz.
    ///
    /// Whether five is the right number is an open question the first
    /// frame-time capture on the reference machine settles, not an
    /// argument.
    // Compile-time `match` for the same reason as `Timestep::HZ_60`.
    pub const DEFAULT: Self = Self(match NonZeroU32::new(5) {
        Some(steps) => steps,
        None => NonZeroU32::MIN,
    });

    #[must_use]
    pub const fn new(steps: NonZeroU32) -> Self {
        Self(steps)
    }

    #[must_use]
    pub const fn get(self) -> NonZeroU32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Nanos, StepBudget, Timestamp, Timestep};
    use core::num::{NonZeroU32, NonZeroU64};

    #[test]
    fn nanos_round_trips_and_zero_is_zero() {
        assert_eq!(Nanos::ZERO.get(), 0);
        assert_eq!(Nanos::from_nanos(7).get(), 7);
        assert_eq!(Nanos::from_nanos(u64::MAX).get(), u64::MAX);
        assert!(Nanos::ZERO < Nanos::from_nanos(1));
    }

    #[test]
    fn a_timestamp_reports_the_span_since_an_earlier_one() {
        let earlier = Timestamp::from_nanos(1_000);
        let later = Timestamp::from_nanos(1_700);
        assert_eq!(later.saturating_since(earlier), Nanos::from_nanos(700));
        assert_eq!(earlier.get(), 1_000);
    }

    #[test]
    fn a_backwards_clock_yields_zero_rather_than_a_wrapped_span() {
        let earlier = Timestamp::from_nanos(1_000);
        let later = Timestamp::from_nanos(1_700);
        assert_eq!(earlier.saturating_since(later), Nanos::ZERO);
        assert_eq!(later.saturating_since(later), Nanos::ZERO);
    }

    #[test]
    fn adding_a_span_saturates_at_the_end_of_the_timeline() {
        let start = Timestamp::from_nanos(5);
        assert_eq!(
            start.saturating_add(Nanos::from_nanos(10)),
            Timestamp::from_nanos(15)
        );
        assert_eq!(
            start.saturating_add(Nanos::from_nanos(u64::MAX)),
            Timestamp::from_nanos(u64::MAX)
        );
    }

    /// The constant is compared against its literal here so that editing
    /// it to zero — the one input the compile-time fallback would swallow
    /// — cannot pass unnoticed.
    #[test]
    fn the_sixty_hertz_timestep_is_the_rounded_nanosecond_value() {
        assert_eq!(Timestep::HZ_60.nanos().get(), 16_666_667);
        // The rounding, stated as a test rather than as a comment: sixty
        // ticks run 20 ns long against the wall.
        assert_eq!(60 * Timestep::HZ_60.nanos().get(), 1_000_000_020);
    }

    #[test]
    fn a_timestep_round_trips_through_its_non_zero_nanoseconds() {
        let step = Timestep::from_nanos(NonZeroU64::new(8_000_000).expect("non-zero"));
        assert_eq!(step.nanos().get(), 8_000_000);
        assert_ne!(step, Timestep::HZ_60);
    }

    #[test]
    fn the_default_step_budget_is_five() {
        assert_eq!(StepBudget::DEFAULT.get().get(), 5);
    }

    #[test]
    fn a_step_budget_round_trips_through_its_non_zero_count() {
        let budget = StepBudget::new(NonZeroU32::new(12).expect("non-zero"));
        assert_eq!(budget.get().get(), 12);
        assert_ne!(budget, StepBudget::DEFAULT);
    }
}
