//! Property coverage of the plan arithmetic over the whole 64-bit domain:
//! every submitted nanosecond is accounted for exactly once, the budget is
//! never exceeded, the remainder never reaches the timestep, and alpha
//! never reaches one.
//!
//! The alpha properties are the ones that matter most. Alpha is excluded
//! from the determinism digest on the argument that it is a pure function
//! of two integers that *are* digested — so that argument is asserted here
//! rather than assumed, over inputs no hand-written case would reach.

use core::num::{NonZeroU32, NonZeroU64};

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use renew_frame::{FrameLoop, FrameStats, StepBudget, Timestamp, Timestep};

// Test helpers (called only from #[test] fns): the tests-only expect
// allowance covers #[test] fns, not their helpers; this allow extends it,
// same spirit. Both generators produce non-zero values by construction.
#[allow(clippy::expect_used)]
fn timestep(nanos: u64) -> Timestep {
    Timestep::from_nanos(NonZeroU64::new(nanos).expect("non-zero"))
}

#[allow(clippy::expect_used)]
fn budget(steps: u32) -> StepBudget {
    StepBudget::new(NonZeroU32::new(steps).expect("non-zero"))
}

proptest! {
    // Fixed RNG seed: the suite explores the same inputs on every run and
    // every machine, so a property failure anywhere reproduces everywhere.
    // Fresh exploration is a deliberate act (change the seed), never an
    // ambient one.
    #![proptest_config(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x0000_2601),
        cases: 64,
        ..ProptestConfig::default()
    })]

    /// The conservation law the whole schedule rests on: every nanosecond
    /// submitted either became an executed step, became a refused step, or
    /// is still banked — never two of those, never none. Checked against
    /// an independent running total, not against the loop's own.
    #[test]
    fn every_submitted_nanosecond_is_executed_dropped_or_banked(
        dt in 1u64..=1_000_000_000,
        allowance in 1u32..=64,
        deltas in prop::collection::vec(0u64..500_000_000, 1..24),
    ) {
        let mut frame = FrameLoop::new(timestep(dt), budget(allowance), Timestamp::from_nanos(0));
        let mut now = 0u64;
        let mut submitted = 0u64;
        let mut due_total = 0u64;
        let mut executed = 0u64;

        for delta in deltas {
            now += delta;
            submitted += delta;
            let plan = frame.begin_frame(Timestamp::from_nanos(now));

            prop_assert!(plan.step_count() <= allowance);
            prop_assert!(plan.remainder().get() < dt);
            prop_assert_eq!(plan.first_tick(), executed);
            prop_assert_eq!(
                u32::try_from(plan.steps().len()).expect("step count is a u32"),
                plan.step_count()
            );
            // The iterator hands out consecutive ticks from `first_tick`,
            // each carrying an exact simulation time.
            for (offset, step) in plan.steps().enumerate() {
                let tick = executed + u64::try_from(offset).expect("64-bit offset");
                prop_assert_eq!(step.tick, tick);
                prop_assert_eq!(step.dt.get(), dt);
                prop_assert_eq!(step.sim_time.get(), tick * dt);
            }

            executed += u64::from(plan.step_count());
            due_total += u64::from(plan.step_count()) + plan.dropped();
            prop_assert_eq!(frame.tick(), executed);
            prop_assert_eq!(frame.remainder(), plan.remainder());
            // The invariant a renderer's blend factor rests on,
            // stated where it is produced rather than where it is
            // divided: the remainder is always a proper fraction of
            // the timestep, so `Alpha::new` of the two can only
            // land in `[0, 1)`.
            prop_assert!(plan.remainder().get() < plan.timestep().nanos().get());
        }

        prop_assert_eq!(submitted, due_total * dt + frame.remainder().get());
        prop_assert_eq!(frame.simulated().get(), executed * dt);
    }

    /// Totality over the extremes: one-nanosecond and 584-year timesteps,
    /// a budget of one and of `u32::MAX`, and timestamps at both ends of
    /// the timeline in either order. Nothing may panic, and the two
    /// structural bounds hold whatever the input.
    #[test]
    fn the_plan_stays_bounded_over_the_whole_domain(
        dt in 1u64..=u64::MAX,
        allowance in 1u32..=u32::MAX,
        start in 0u64..=u64::MAX,
        first in 0u64..=u64::MAX,
        second in 0u64..=u64::MAX,
    ) {
        let mut frame = FrameLoop::new(
            timestep(dt),
            budget(allowance),
            Timestamp::from_nanos(start),
        );
        for now in [first, second] {
            let before = frame.tick();
            let plan = frame.begin_frame(Timestamp::from_nanos(now));
            prop_assert!(plan.step_count() <= allowance);
            prop_assert!(plan.remainder().get() < dt);
            prop_assert_eq!(plan.first_tick(), before);
            prop_assert_eq!(frame.tick(), before + u64::from(plan.step_count()));
            prop_assert!(plan.remainder().get() < plan.timestep().nanos().get());
        }
    }

    /// A clock that went the wrong way is inert: no step, no drop, an
    /// untouched bank. The failure this rules out is a wrapped `u64`
    /// delta, which would owe 1.1 trillion steps and clamp forever.
    #[test]
    fn a_backwards_timestamp_advances_nothing(
        dt in 1u64..=1_000_000_000,
        start in 0u64..=u64::MAX,
        back in 0u64..=u64::MAX,
    ) {
        let mut frame = FrameLoop::new(
            timestep(dt),
            StepBudget::DEFAULT,
            Timestamp::from_nanos(start),
        );
        let plan = frame.begin_frame(Timestamp::from_nanos(start.saturating_sub(back)));
        prop_assert_eq!(plan.step_count(), 0);
        prop_assert_eq!(plan.dropped(), 0);
        prop_assert_eq!(plan.remainder().get(), 0);
    }

    /// What this crate promises a renderer, over the whole
    /// `(timestep, remainder)` domain: the remainder is a *proper*
    /// fraction of the timestep. That integer invariant is what makes a
    /// blend factor built from the pair land in `[0, 1)`; the factor's
    /// own behaviour is `renew-math`'s property to state, and it does,
    /// over the same domain and over pairs no loop can produce.
    ///
    /// A loop that never reaches a step has a bank equal to its
    /// remainder, so generating `fraction % dt` sweeps every
    /// representable pair — including the one-nanosecond-short cases.
    #[test]
    fn the_remainder_is_always_a_proper_fraction_of_the_timestep(
        dt in 1u64..=u64::MAX,
        fraction in 0u64..=u64::MAX,
    ) {
        let remainder = fraction % dt;
        let mut frame = FrameLoop::new(
            timestep(dt),
            StepBudget::DEFAULT,
            Timestamp::from_nanos(0),
        );
        let plan = frame.begin_frame(Timestamp::from_nanos(remainder));
        prop_assert_eq!(plan.step_count(), 0);
        prop_assert_eq!(plan.remainder().get(), remainder);
        prop_assert!(plan.remainder().get() < plan.timestep().nanos().get());
    }

    /// The pair a renderer interpolates from depends on the remainder and
    /// the timestep and on nothing else. Two plans that land on the same
    /// remainder must present identically even though one arrived through
    /// a stall — different tick, different step count, hundreds of
    /// refused steps. This is the property the digest exclusion rests on,
    /// kept here after the division moved out, because it is a statement
    /// about *plans*, not about arithmetic.
    #[test]
    fn the_presented_pair_is_a_function_of_the_remainder_and_timestep_alone(
        dt in 2u64..=1_000_000_000,
        fraction in 0u64..=u64::MAX,
    ) {
        let remainder = fraction % dt;
        let mut quiet = FrameLoop::new(timestep(dt), budget(1), Timestamp::from_nanos(0));
        let calm = quiet.begin_frame(Timestamp::from_nanos(remainder));

        let mut busy = FrameLoop::new(timestep(dt), budget(1), Timestamp::from_nanos(0));
        let _ = busy.begin_frame(Timestamp::from_nanos(2 * dt));
        let stalled = busy.begin_frame(Timestamp::from_nanos(6 * dt + remainder));

        prop_assert_eq!(stalled.remainder(), calm.remainder());
        prop_assert_ne!(stalled.first_tick(), calm.first_tick());
        prop_assert_ne!(stalled.step_count(), calm.step_count());
        prop_assert!(stalled.dropped() > 0);
        prop_assert_eq!(stalled.timestep().nanos(), calm.timestep().nanos());
    }

    /// Identical input traces produce identical plans, identical loop
    /// state and identical digests, for arbitrary traces — the property
    /// the hand-written determinism suite pins to one canonical trace.
    #[test]
    fn identical_traces_produce_identical_schedules(
        dt in 1u64..=1_000_000_000,
        allowance in 1u32..=8,
        deltas in prop::collection::vec(0u64..2_000_000_000, 1..16),
    ) {
        let run = || {
            let mut frame = FrameLoop::new(
                timestep(dt),
                budget(allowance),
                Timestamp::from_nanos(0),
            );
            let mut stats = FrameStats::new();
            let mut now = 0u64;
            let plans: Vec<_> = deltas
                .iter()
                .map(|delta| {
                    now += delta;
                    let plan = frame.begin_frame(Timestamp::from_nanos(now));
                    stats.absorb(&plan);
                    plan
                })
                .collect();
            (plans, stats, frame)
        };
        prop_assert_eq!(run(), run());
    }
}
