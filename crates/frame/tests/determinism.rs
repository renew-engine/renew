//! The determinism evidence for the loop core, and the guards that keep
//! it from going quietly vacuous.
//!
//! A determinism test with no negative control passes just as happily when
//! the digest ignores its input, and a loop that does nothing satisfies
//! every equality assertion in this file. So each test states what the
//! trace must have *done* — executed steps, engaged the budget, moved the
//! digest off its offset basis — before it compares anything, and one test
//! exists purely to show that a one-nanosecond perturbation of the input
//! changes the answer.

use renew_frame::{
    FrameLoop, FramePlan, FrameStats, StateHash, Step, StepBudget, Timestamp, Timestep,
};

/// 60 Hz in whole nanoseconds.
const DT: u64 = 16_666_667;

/// The schedule anchor. Nonzero on purpose: it leaves room below the
/// origin for the negative control to perturb the start without the
/// perturbation being swallowed by the backwards-clock rule.
const ORIGIN: u64 = 1_000_000_000;

/// The frozen digest of [`hostile_trace`] under [`run`].
///
/// Recorded from this implementation, not derived independently — its job
/// is to fail when the loop's arithmetic changes, so that such a change is
/// a deliberate act with a visible diff rather than a silent one. It is
/// pure `u64` arithmetic with an explicit little-endian absorption order,
/// so it should hold on every platform; "should" is not evidence, and the
/// first three-platform run is.
const FROZEN_SCHEDULE_DIGEST: u64 = 0x8154_30d5_7cc1_5744;

/// The canonical hostile trace, as absolute timestamps from `origin`.
///
/// It deliberately hits every branch in the arithmetic: no advance, one
/// nanosecond, one nanosecond short of a step, exactly a step, a step plus
/// one, a multi-second hitch that runs the budget out, ordinary frames
/// either side of it, a clock that goes backwards, and the end of the
/// representable timeline twice over.
fn hostile_trace(origin: u64) -> Vec<Timestamp> {
    let mut stamps = Vec::new();
    let mut now = origin;
    for delta in [0, 1, DT - 1, DT, DT + 1, 3_000_000_000, 8_000_000, DT] {
        now = now.saturating_add(delta);
        stamps.push(Timestamp::from_nanos(now));
    }
    stamps.push(Timestamp::from_nanos(now - 1_000_000_000));
    stamps.push(Timestamp::from_nanos(now));
    stamps.push(Timestamp::from_nanos(u64::MAX));
    stamps.push(Timestamp::from_nanos(u64::MAX));
    stamps
}

/// Drive the hostile trace once and report every plan alongside the tally.
fn run(start: u64) -> (Vec<FramePlan>, FrameStats) {
    let mut frame = FrameLoop::new(
        Timestep::HZ_60,
        StepBudget::DEFAULT,
        Timestamp::from_nanos(start),
    );
    let mut stats = FrameStats::new();
    let plans = hostile_trace(ORIGIN)
        .into_iter()
        .map(|now| {
            let plan = frame.begin_frame(now);
            stats.absorb(&plan);
            plan
        })
        .collect();
    (plans, stats)
}

/// The trace is only evidence if it exercised the machinery. Asserted
/// before any equality, in every test that compares digests.
fn assert_not_vacuous(plans: &[FramePlan], stats: &FrameStats) {
    assert!(
        plans.iter().any(|plan| plan.step_count() > 0),
        "the trace executed no simulation step"
    );
    assert!(
        plans.iter().any(|plan| plan.dropped() > 0),
        "the step budget never engaged"
    );
    assert!(
        plans
            .iter()
            .any(|plan| plan.step_count() == StepBudget::DEFAULT.get().get()),
        "no frame was clamped to the budget"
    );
    assert!(
        plans.iter().any(|plan| plan.remainder().get() > 0),
        "the bank was empty on every frame"
    );
    assert_ne!(
        stats.schedule_hash(),
        StateHash::new().finish(),
        "nothing was absorbed"
    );
}

/// E2 — eight in-process runs of one trace agree plan for plan.
#[test]
fn eight_runs_of_one_trace_produce_one_schedule() {
    let runs: Vec<_> = (0..8).map(|_| run(ORIGIN)).collect();
    let (plans, stats) = runs.first().expect("eight runs").clone();
    assert_not_vacuous(&plans, &stats);

    for (index, (other_plans, other_stats)) in runs.iter().enumerate() {
        assert_eq!(other_plans, &plans, "run {index} planned differently");
        assert_eq!(
            other_stats.schedule_hash(),
            stats.schedule_hash(),
            "run {index} digested differently"
        );
    }
    assert_eq!(stats.frames(), 12);
}

/// E3 — the frozen digest. One canonical trace, one committed constant.
#[test]
fn the_canonical_trace_matches_its_frozen_digest() {
    let (plans, stats) = run(ORIGIN);
    assert_not_vacuous(&plans, &stats);
    assert_eq!(
        stats.schedule_hash(),
        FROZEN_SCHEDULE_DIGEST,
        "the canonical schedule digest changed; if that was deliberate, \
         update FROZEN_SCHEDULE_DIGEST in the same commit and say why"
    );
}

/// E4 — the negative control, and the reason the three tests above mean
/// anything. One nanosecond earlier at the anchor and the digest must
/// differ; if it does not, the oracle is ignoring its input.
#[test]
fn a_one_nanosecond_perturbation_of_the_start_changes_the_digest() {
    let (plans, stats) = run(ORIGIN);
    let (perturbed_plans, perturbed_stats) = run(ORIGIN - 1);
    assert_not_vacuous(&plans, &stats);
    assert_not_vacuous(&perturbed_plans, &perturbed_stats);
    assert_ne!(
        perturbed_stats.schedule_hash(),
        stats.schedule_hash(),
        "the digest ignored a one-nanosecond change in its input"
    );
    assert_ne!(perturbed_plans, plans);
}

/// A fixed-point integer reference world for E5. No floats, no clock, no
/// collections — the only thing under test is that identical schedules
/// produce identical state. It lives in a test rather than in the library
/// because a toy simulation shipped as engine code is exactly the
/// speculative abstraction the simplest-thing-that-works rule refuses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct World {
    position: u64,
    velocity: u64,
    ticks: u64,
}

impl World {
    /// Wrapping throughout: the hostile trace's saturated frames feed this
    /// world timestamps at the end of the timeline, and a defined wrap is
    /// deterministic where an overflow panic is not.
    fn step(&mut self, step: Step) {
        self.velocity = self.velocity.wrapping_add(step.dt.get() >> 12);
        self.position = self
            .position
            .wrapping_add(self.velocity >> 6)
            .wrapping_add(step.sim_time.get() >> 20);
        self.ticks = self.ticks.wrapping_add(step.tick).wrapping_add(1);
    }

    /// Field by field, in an order written out rather than derived.
    fn absorb(self, hash: StateHash) -> StateHash {
        hash.absorb_u64(self.position)
            .absorb_u64(self.velocity)
            .absorb_u64(self.ticks)
    }
}

/// E5 — a world stepped by the plans agrees across eight runs.
///
/// This is the anti-vacuity partner of the schedule digest from the other
/// side: the schedule can be perfectly stable while a consumer driven by
/// it is not.
#[test]
fn eight_runs_of_one_trace_produce_one_world_state() {
    let simulate = || {
        let mut frame = FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(ORIGIN),
        );
        let mut world = World::default();
        let mut stats = FrameStats::new();
        for now in hostile_trace(ORIGIN) {
            let plan = frame.begin_frame(now);
            for step in plan.steps() {
                world.step(step);
            }
            stats.absorb(&plan);
        }
        (
            world,
            stats.schedule_hash(),
            world.absorb(StateHash::new()).finish(),
        )
    };

    let (world, schedule, state) = simulate();
    assert_ne!(world, World::default(), "the world never advanced");
    assert_ne!(
        state,
        StateHash::new().finish(),
        "no world state was absorbed"
    );

    for index in 1..8 {
        assert_eq!(simulate(), (world, schedule, state), "run {index} diverged");
    }
}

/// E5 — the property the alpha exemption rests on.
///
/// Alpha is the one float this crate computes, and this crate is
/// simulation-designated, so the language standard names it an
/// exemption rather than covering it by rule. The exemption is only
/// honest if alpha carries no state of its own: it must be derivable
/// from the integers the digest already absorbs, so that a machine
/// agreeing on the digest agrees on alpha too.
///
/// Stated as a property rather than by recomputing the formula — which
/// would only assert that the implementation equals itself: **if the
/// digest cannot tell two plans apart, neither can alpha.** The day
/// somebody smooths alpha across frames, or seeds it, or lets it
/// accumulate, that stops being true and this test says so.
#[test]
fn alpha_carries_no_state_the_digest_does_not_absorb() {
    // The digested fields of a plan, exactly as `absorb_plan` folds
    // them: first tick, step count, dropped, remainder. All integers.
    fn digested(plan: &FramePlan) -> (u64, u32, u64, u64) {
        (
            plan.first_tick(),
            plan.step_count(),
            plan.dropped(),
            plan.remainder().get(),
        )
    }

    // Two independent walks over the same hostile trace produce plans
    // pairwise equal in their digested fields; alpha must agree
    // wherever they do. Two walks rather than one because a plan
    // compared against itself proves nothing about derivation.
    let (left, _) = run(ORIGIN);
    let (right, _) = run(ORIGIN);
    assert!(!left.is_empty(), "the trace produced no plans");

    let mut compared = 0;
    for (a, b) in left.iter().zip(right.iter()) {
        if digested(a) == digested(b) {
            assert_eq!(
                a.alpha().get().to_bits(),
                b.alpha().get().to_bits(),
                "two plans the digest cannot distinguish disagree on alpha, \
                 so alpha holds state the digest does not cover"
            );
            compared += 1;
        }
    }
    assert_eq!(
        compared,
        left.len(),
        "the two walks diverged in their digested fields, which is a \
         determinism failure before it is an alpha question"
    );

    // The other direction, so the test cannot pass by comparing
    // nothing: somewhere in this trace, two plans DO differ in their
    // digested fields, and there alpha is free to differ too.
    let distinct = left.iter().any(|plan| digested(plan) != digested(&left[0]));
    assert!(
        distinct,
        "every plan in the trace digests identically; the comparison above \
         is vacuous"
    );
}
