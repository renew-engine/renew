//! The schedule itself: the accumulator, the step budget, the plan a
//! frame produces, and the interpolation factor for rendering between
//! steps.
//!
//! [`FrameLoop`] owns no loop and drives no application. Its whole job is
//! one total function — [`FrameLoop::begin_frame`] answers *given the
//! schedule so far and this instant, how many fixed steps are due, how
//! many did the budget refuse, and how far between steps is the
//! renderer.* The caller reads the one clock, executes the steps, and
//! renders.

use crate::time::{Nanos, StepBudget, Timestamp, Timestep};

/// The largest `f32` strictly below one, `0.999_999_94`.
const LARGEST_BELOW_ONE: f32 = f32::from_bits(0x3F7F_FFFF);

/// The fixed-timestep schedule: a passive integer state machine.
///
/// It never reads a clock — it *cannot*, having no dependency that offers
/// one — so a run is reproducible exactly to the extent that the sequence
/// of timestamps handed to [`FrameLoop::begin_frame`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameLoop {
    timestep: Timestep,
    budget: StepBudget,
    /// The instant the previous frame was planned at.
    last: Timestamp,
    /// Elapsed time not yet consumed by a step. Always below `timestep`
    /// once a frame has been planned.
    bank: Nanos,
    /// Steps executed since construction. The simulation's own clock is
    /// derived from this, never measured.
    tick: u64,
}

impl FrameLoop {
    /// A schedule anchored at `start`, with an empty bank and tick zero.
    ///
    /// Anchor *after* expensive bring-up (device creation, asset load):
    /// time banked before the first frame is time the budget has to
    /// refuse, so a schedule anchored too early opens with a clamped
    /// burst and a nonzero drop count that means nothing.
    #[must_use]
    pub const fn new(timestep: Timestep, budget: StepBudget, start: Timestamp) -> Self {
        Self {
            timestep,
            budget,
            last: start,
            bank: Nanos::ZERO,
            tick: 0,
        }
    }

    /// Advance the schedule to `now` and report what this frame must do.
    ///
    /// A pure state transition: a function of the timestep, the budget,
    /// prior state, and `now` — nothing else. It cannot fail, so it
    /// returns no `Result`; an uninhabitable error variant would be a lie
    /// about the API. No clock is read here or anywhere in this crate.
    pub fn begin_frame(&mut self, now: Timestamp) -> FramePlan {
        let dt = self.timestep.nanos().get();
        let bank = self
            .bank
            .get()
            .saturating_add(now.saturating_since(self.last).get());
        let due = bank / dt;
        // `due` is a `u64` and reaches 1.1e12 on a saturated bank, but the
        // executed count is bounded by the budget, so it is exactly a
        // `u32` and `step_count` needs no saturation of its own.
        let steps = u32::try_from(due)
            .unwrap_or(u32::MAX)
            .min(self.budget.get().get());
        let run = u64::from(steps);
        // THE DISCARD. Keeping the surplus banked is the spiral of death:
        // the next frame is also saturated, the bank never drains, and the
        // loop never recovers. Discarding means simulation time falls
        // permanently behind the wall — the game visibly slows — but the
        // loop recovers the instant the frame rate does. What makes that
        // honest rather than a lie is that the loss is reported: `dropped`
        // is exact and flows into the frame statistics, so a frame with a
        // nonzero drop count is a measurable budget violation.
        let remainder = Nanos::from_nanos(bank % dt);
        // `run` is `min(due, ..)`, so the difference cannot underflow.
        let plan = FramePlan {
            first_tick: self.tick,
            steps,
            dropped: due - run,
            remainder,
            dt: self.timestep,
        };
        self.bank = remainder;
        self.tick = self.tick.saturating_add(run);
        self.last = now;
        plan
    }

    /// Discard the gap since the last frame, keeping the sub-timestep
    /// remainder and the tick count.
    ///
    /// For pauses the caller *knows* about — a finished load, a resumed
    /// dormant window, a breakpoint. Never automatic: a "delta over
    /// threshold implies resync" heuristic would hide exactly the stall
    /// the step budget exists to expose.
    pub fn resync(&mut self, now: Timestamp) {
        self.last = now;
    }

    /// Steps executed since construction.
    #[must_use]
    pub const fn tick(&self) -> u64 {
        self.tick
    }

    /// The simulation's own clock: `tick × timestep`, saturating. Exact by
    /// construction, and therefore never the measured wall time.
    #[must_use]
    pub const fn simulated(&self) -> Nanos {
        Nanos::from_nanos(self.tick.saturating_mul(self.timestep.nanos().get()))
    }

    /// Elapsed time banked but not yet consumed by a step; below
    /// [`FrameLoop::timestep`] once a frame has been planned.
    #[must_use]
    pub const fn remainder(&self) -> Nanos {
        self.bank
    }

    #[must_use]
    pub const fn timestep(&self) -> Timestep {
        self.timestep
    }

    #[must_use]
    pub const fn budget(&self) -> StepBudget {
        self.budget
    }
}

/// What one frame must do: the steps to execute, the steps the budget
/// refused, and how far past the last step the renderer stands.
///
/// A `Copy` value that borrows nothing, so iterating its steps never
/// conflicts with touching the rest of the caller's state.
#[must_use = "a frame plan's steps must be executed and its alpha rendered"]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramePlan {
    first_tick: u64,
    steps: u32,
    dropped: u64,
    remainder: Nanos,
    dt: Timestep,
}

impl FramePlan {
    /// The steps to execute, in tick order — exactly
    /// [`FramePlan::step_count`] of them.
    #[must_use]
    pub const fn steps(&self) -> Steps {
        Steps {
            next: self.first_tick,
            remaining: self.steps,
            dt: self.dt,
        }
    }

    #[must_use]
    pub const fn step_count(&self) -> u32 {
        self.steps
    }

    /// The tick index of the first step, which is also the loop's tick
    /// count before this frame.
    #[must_use]
    pub const fn first_tick(&self) -> u64 {
        self.first_tick
    }

    /// Steps the budget refused: simulation time fell behind wall time by
    /// `dropped × timestep`, permanently. Reported, never silently banked.
    #[must_use]
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Elapsed time carried into the next frame; always below
    /// [`FramePlan::timestep`].
    #[must_use]
    pub const fn remainder(&self) -> Nanos {
        self.remainder
    }

    #[must_use]
    pub const fn timestep(&self) -> Timestep {
        self.dt
    }

    /// Render interpolation in `[0, 1)`: how far past the last executed
    /// step the frame stands, as a fraction of the timestep.
    ///
    /// Never an input to simulation, and deliberately excluded from the
    /// schedule digest — it is a pure function of two integers that *are*
    /// digested, so hashing it would only make the oracle float-dependent.
    /// [`FramePlan::remainder`] and [`FramePlan::timestep`] stay public so
    /// a consumer that wants the exact rational never goes through the
    /// float at all.
    #[must_use]
    pub fn alpha(&self) -> Alpha {
        // The `f64` intermediate and the explicit bound are both
        // load-bearing, and neither is defensive. Measured: the naive
        // `rem as f32 / dt as f32` returns exactly 1.0 at 30 Hz
        // (dt = 33_333_333, rem = dt - 1), and the `f64` division still
        // rounds up to 1.0 at 1 Hz. An alpha of 1.0 is a renderer popping
        // a full tick ahead of the state it interpolates from — a bug
        // that would have been hunted in the renderer for a week.
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let ratio = (self.remainder.get() as f64 / self.dt.nanos().get() as f64) as f32;
        Alpha(ratio.min(LARGEST_BELOW_ONE))
    }
}

/// One simulation step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// The tick this step advances, counted from the loop's construction
    /// and monotonically increasing across the whole run.
    pub tick: u64,
    /// The fixed timestep, repeated here so a world function needs only
    /// the step.
    pub dt: Nanos,
    /// The simulation clock at the *start* of this step: `tick × dt`,
    /// saturating. Defined rather than measured, so it is exact whatever
    /// the wall clock does.
    pub sim_time: Nanos,
}

/// The steps of one [`FramePlan`], in tick order.
///
/// Borrows nothing from the loop or the plan: the plan is `Copy`, so a
/// caller can touch the rest of its own state inside the step loop
/// without fighting a partial borrow.
#[derive(Clone, Debug)]
pub struct Steps {
    next: u64,
    remaining: u32,
    dt: Timestep,
}

impl Iterator for Steps {
    type Item = Step;

    fn next(&mut self) -> Option<Step> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let tick = self.next;
        self.next = tick.saturating_add(1);
        let dt = self.dt.nanos().get();
        Some(Step {
            tick,
            dt: Nanos::from_nanos(dt),
            sim_time: Nanos::from_nanos(tick.saturating_mul(dt)),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::try_from(self.remaining).unwrap_or(usize::MAX);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Steps {}

impl core::iter::FusedIterator for Steps {}

/// The render interpolation factor, in `[0, 1)`.
///
/// A newtype with no arithmetic surface: the name is the contract. It is
/// a hint for the renderer and never an input to simulation.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Alpha(f32);

impl Alpha {
    /// Exactly on a step boundary.
    pub const ZERO: Self = Self(0.0);

    #[must_use]
    pub const fn get(self) -> f32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Alpha, FrameLoop, LARGEST_BELOW_ONE, StepBudget, Timestamp, Timestep};
    use crate::time::Nanos;
    use core::num::{NonZeroU32, NonZeroU64};

    /// 60 Hz in whole nanoseconds — the value every case below is written
    /// against.
    const DT: u64 = 16_666_667;

    fn at(nanos: u64) -> Timestamp {
        Timestamp::from_nanos(nanos)
    }

    fn timestep(nanos: u64) -> Timestep {
        Timestep::from_nanos(NonZeroU64::new(nanos).expect("non-zero"))
    }

    fn budget(steps: u32) -> StepBudget {
        StepBudget::new(NonZeroU32::new(steps).expect("non-zero"))
    }

    fn loop_at_60hz() -> FrameLoop {
        FrameLoop::new(Timestep::HZ_60, StepBudget::DEFAULT, at(0))
    }

    #[test]
    fn a_fresh_loop_reports_its_configuration_and_an_empty_schedule() {
        let frame = loop_at_60hz();
        assert_eq!(frame.timestep(), Timestep::HZ_60);
        assert_eq!(frame.budget(), StepBudget::DEFAULT);
        assert_eq!(frame.tick(), 0);
        assert_eq!(frame.remainder(), Nanos::ZERO);
        assert_eq!(frame.simulated(), Nanos::ZERO);
    }

    #[test]
    fn no_elapsed_time_yields_no_steps() {
        let mut frame = loop_at_60hz();
        let plan = frame.begin_frame(at(0));
        assert_eq!(plan.step_count(), 0);
        assert_eq!(plan.dropped(), 0);
        assert_eq!(plan.first_tick(), 0);
        assert_eq!(plan.remainder(), Nanos::ZERO);
        assert_eq!(plan.timestep(), Timestep::HZ_60);
        assert_eq!(plan.steps().count(), 0);
    }

    #[test]
    fn exactly_one_timestep_yields_one_step_and_an_empty_bank() {
        let mut frame = loop_at_60hz();
        let plan = frame.begin_frame(at(DT));
        assert_eq!(plan.step_count(), 1);
        assert_eq!(plan.remainder(), Nanos::ZERO);
        assert_eq!(frame.tick(), 1);
        assert_eq!(frame.simulated(), Nanos::from_nanos(DT));
    }

    #[test]
    fn the_remainder_carries_across_frames() {
        let mut frame = loop_at_60hz();
        let first = frame.begin_frame(at(DT + 5));
        assert_eq!(first.step_count(), 1);
        assert_eq!(first.remainder(), Nanos::from_nanos(5));
        // Five nanoseconds short of a step: the bank must be consulted,
        // not thrown away.
        let second = frame.begin_frame(at(DT + 5 + DT - 5));
        assert_eq!(second.step_count(), 1);
        assert_eq!(second.remainder(), Nanos::ZERO);
        assert_eq!(frame.tick(), 2);
    }

    #[test]
    fn sub_timestep_frames_bank_until_a_step_is_due() {
        let mut frame = loop_at_60hz();
        let half = DT / 2;
        assert_eq!(frame.begin_frame(at(half)).step_count(), 0);
        assert_eq!(frame.remainder(), Nanos::from_nanos(half));
        // The timestep is odd, so two halves fall one nanosecond short.
        assert_eq!(frame.begin_frame(at(2 * half)).step_count(), 0);
        assert_eq!(frame.remainder(), Nanos::from_nanos(DT - 1));
        assert_eq!(frame.begin_frame(at(2 * half + 1)).step_count(), 1);
        assert_eq!(frame.remainder(), Nanos::ZERO);
    }

    #[test]
    fn several_whole_timesteps_run_in_one_frame_while_the_budget_allows() {
        let mut frame = loop_at_60hz();
        let plan = frame.begin_frame(at(3 * DT + 7));
        assert_eq!(plan.step_count(), 3);
        assert_eq!(plan.dropped(), 0);
        assert_eq!(plan.remainder(), Nanos::from_nanos(7));
    }

    /// The measured stall case: a 200 ms hitch at 60 Hz owes twelve steps
    /// and the default budget runs five.
    #[test]
    fn a_stall_is_clamped_and_the_refused_steps_are_reported() {
        let mut frame = loop_at_60hz();
        let plan = frame.begin_frame(at(200_000_000));
        assert_eq!(plan.step_count(), 5);
        assert_eq!(plan.dropped(), 200_000_000 / DT - 5);
        // Clamp-and-discard, not clamp-and-keep: the surplus is gone, so
        // the very next frame starts from the sub-timestep remainder and
        // the loop recovers immediately.
        assert_eq!(plan.remainder(), Nanos::from_nanos(200_000_000 % DT));
        let recovered = frame.begin_frame(at(200_000_000 + DT));
        assert_eq!(recovered.step_count(), 1);
        assert_eq!(recovered.dropped(), 0);
    }

    #[test]
    fn a_saturated_bank_drops_billions_of_steps_without_wrapping() {
        let mut frame = loop_at_60hz();
        let plan = frame.begin_frame(at(u64::MAX));
        assert_eq!(plan.step_count(), 5);
        assert_eq!(plan.dropped(), u64::MAX / DT - 5);
        assert_eq!(plan.remainder(), Nanos::from_nanos(u64::MAX % DT));
        assert_eq!(frame.tick(), 5);
    }

    /// The refused count must be a `u64`: at one-nanosecond steps a
    /// saturated bank owes more steps than a `u32` can name.
    #[test]
    fn the_refused_count_exceeds_the_thirty_two_bit_range() {
        let mut frame = FrameLoop::new(timestep(1), budget(1), at(0));
        let plan = frame.begin_frame(at(u64::MAX));
        assert_eq!(plan.step_count(), 1);
        assert_eq!(plan.dropped(), u64::MAX - 1);
        assert!(plan.dropped() > u64::from(u32::MAX));
    }

    #[test]
    fn a_backwards_clock_advances_nothing_and_leaves_the_bank_alone() {
        let mut frame = loop_at_60hz();
        let _ = frame.begin_frame(at(DT + 11));
        let backwards = frame.begin_frame(at(1));
        assert_eq!(backwards.step_count(), 0);
        assert_eq!(backwards.dropped(), 0);
        assert_eq!(backwards.remainder(), Nanos::from_nanos(11));
        assert_eq!(frame.tick(), 1);
        // The loop is now anchored at the backwards instant, so the next
        // forward frame is measured from there — defined behaviour, not a
        // wrapped `u64` worth 1.1 trillion phantom steps.
        assert_eq!(frame.begin_frame(at(1 + DT)).step_count(), 1);
    }

    #[test]
    fn resync_discards_the_gap_but_keeps_the_tick_and_the_remainder() {
        let mut frame = loop_at_60hz();
        let _ = frame.begin_frame(at(DT + 11));
        assert_eq!(frame.tick(), 1);
        // A ten-second pause the caller knows about.
        frame.resync(at(10_000_000_000));
        assert_eq!(frame.tick(), 1);
        assert_eq!(frame.remainder(), Nanos::from_nanos(11));
        let plan = frame.begin_frame(at(10_000_000_000 + DT - 11));
        assert_eq!(plan.step_count(), 1, "the pause was not banked");
        assert_eq!(plan.dropped(), 0);
        assert_eq!(plan.remainder(), Nanos::ZERO);
    }

    #[test]
    fn the_simulated_clock_is_tick_times_timestep_and_saturates() {
        let half = u64::MAX / 2;
        let mut frame = FrameLoop::new(timestep(half), budget(2), at(0));
        let _ = frame.begin_frame(at(u64::MAX));
        assert_eq!(frame.tick(), 2);
        assert_eq!(frame.simulated(), Nanos::from_nanos(2 * half));

        // Re-anchor at the origin (a backwards clock banks nothing) and
        // run the timeline again. The tick count now outruns what
        // `tick × dt` can represent, and both the loop's clock and the
        // step's own `sim_time` saturate rather than wrapping.
        let _ = frame.begin_frame(at(0));
        let plan = frame.begin_frame(at(u64::MAX));
        assert_eq!(plan.step_count(), 2);
        assert_eq!(frame.tick(), 4);
        assert_eq!(frame.simulated(), Nanos::from_nanos(u64::MAX));
        let last = plan.steps().last().expect("two steps");
        assert_eq!(last.tick, 3);
        assert_eq!(last.sim_time, Nanos::from_nanos(u64::MAX));
    }

    /// A bank owing more steps than a `u32` can name still produces an
    /// exact `u32` step count, because the budget bounds it first.
    #[test]
    fn a_due_count_beyond_the_thirty_two_bit_range_is_still_budget_bounded() {
        let mut frame = FrameLoop::new(timestep(1), budget(u32::MAX), at(0));
        let plan = frame.begin_frame(at(u64::MAX));
        assert_eq!(plan.step_count(), u32::MAX);
        assert_eq!(plan.dropped(), u64::MAX - u64::from(u32::MAX));
        assert_eq!(frame.tick(), u64::from(u32::MAX));
    }

    #[test]
    fn the_steps_of_a_plan_are_consecutive_ticks_with_exact_simulation_times() {
        let mut frame = loop_at_60hz();
        let _ = frame.begin_frame(at(DT));
        let plan = frame.begin_frame(at(4 * DT));
        assert_eq!(plan.first_tick(), 1);
        let steps: Vec<_> = plan.steps().collect();
        assert_eq!(steps.len(), 3);
        for (offset, step) in steps.iter().enumerate() {
            let tick = 1 + offset as u64;
            assert_eq!(step.tick, tick);
            assert_eq!(step.dt, Nanos::from_nanos(DT));
            assert_eq!(step.sim_time, Nanos::from_nanos(tick * DT));
        }
        // The last step's start plus one timestep is where the loop's own
        // clock now stands: the two definitions agree.
        assert_eq!(frame.simulated(), Nanos::from_nanos(4 * DT));
    }

    #[test]
    fn the_step_iterator_reports_its_exact_length_and_then_stays_empty() {
        let mut frame = loop_at_60hz();
        let plan = frame.begin_frame(at(2 * DT));
        let mut steps = plan.steps();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps.size_hint(), (2, Some(2)));
        assert!(steps.next().is_some());
        assert_eq!(steps.len(), 1);
        assert!(steps.next().is_some());
        assert_eq!(steps.len(), 0);
        assert!(steps.next().is_none());
        assert!(steps.next().is_none(), "fused");
        // The plan is `Copy`, so asking again yields the same steps.
        assert_eq!(plan.steps().count(), 2);
    }

    #[test]
    fn alpha_is_zero_on_a_step_boundary_and_proportional_between_steps() {
        let mut frame = loop_at_60hz();
        let plan = frame.begin_frame(at(DT));
        assert_eq!(plan.alpha(), Alpha::ZERO);
        assert!((plan.alpha().get() - 0.0).abs() < f32::EPSILON);

        let plan = frame.begin_frame(at(DT + DT / 2));
        let alpha = plan.alpha().get();
        assert!((alpha - 0.5).abs() < 1e-6, "alpha was {alpha}");
    }

    /// The rounding table from the design note, as a test. A naive
    /// `rem as f32 / dt as f32` returns exactly 1.0 for the 30 Hz row, and
    /// the `f64` intermediate alone still returns 1.0 for the 1 Hz row —
    /// so the explicit bound is mandatory, not defensive.
    #[test]
    fn alpha_never_reaches_one_even_one_nanosecond_short_of_a_step() {
        for dt in [
            16_666_667_u64,
            4_166_667,
            8_000_000,
            33_333_333,
            1_000_000_000,
        ] {
            let mut frame = FrameLoop::new(timestep(dt), budget(1), at(0));
            let plan = frame.begin_frame(at(dt - 1));
            let alpha = plan.alpha().get();
            assert_eq!(plan.remainder(), Nanos::from_nanos(dt - 1));
            assert!(alpha < 1.0, "dt {dt} produced alpha {alpha}");
            assert!(alpha <= LARGEST_BELOW_ONE);
            assert!(alpha > 0.999_99, "dt {dt} produced alpha {alpha}");
        }
    }

    /// Asserted on bit patterns rather than values: the next representable
    /// `f32` above the bound is exactly one, so nothing lies between them
    /// and the clamp loses no resolution a renderer could use.
    #[test]
    fn the_bound_is_the_largest_float_below_one() {
        assert_eq!(LARGEST_BELOW_ONE.to_bits(), 0x3F7F_FFFF);
        assert_eq!(LARGEST_BELOW_ONE.to_bits() + 1, 1.0_f32.to_bits());
    }

    /// The parity oracle for absorbing hello-engine's `Accumulator`: its
    /// committed quick-start scenario — a fast frame, an exact frame, a
    /// slow frame and a two-tick spike, cycled over sixty frames — driven
    /// through this crate's semantics instead. The numbers asserted here
    /// are the ones its README quotes as observed output, so the
    /// absorption is output-preserving or this test says so.
    ///
    /// The two implementations are not equivalent in general: the old
    /// `advance` executed every whole timestep in the bank and saturated
    /// its *count* at `u32::MAX`, where this one clamps and discards. For
    /// this pattern (at most two ticks per frame, budget five) the paths
    /// coincide, which the drop count below asserts rather than assumes.
    #[test]
    fn the_absorbed_accumulator_reproduces_hello_engines_committed_output() {
        let pattern = [15_000_000_u64, 16_666_667, 18_000_000, 33_333_334];
        let mut frame = loop_at_60hz();
        let mut now = 0u64;
        let mut dropped = 0u64;
        for delta in pattern.iter().copied().cycle().take(60) {
            now += delta;
            dropped += frame.begin_frame(at(now)).dropped();
        }
        assert_eq!(now, 1_245_000_015, "time submitted");
        assert_eq!(frame.tick(), 74, "ticks executed");
        assert_eq!(
            frame.remainder(),
            Nanos::from_nanos(11_666_657),
            "time pending"
        );
        assert_eq!(dropped, 0, "the pattern never reaches the budget");
        // Submitted time is accounted for exactly: every nanosecond either
        // became a step or is still banked.
        assert_eq!(frame.simulated().get() + frame.remainder().get(), now);
    }
}
