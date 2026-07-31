//! Frame reporting, split so the gated half cannot touch the timed
//! half.
//!
//! One type tallying everything would absorb measured wall time into the
//! determinism digest — and it would fail silently, because the gate would
//! simply never go green and someone would "fix" it by loosening the
//! comparison. [`FrameStats`] is the deterministic tally and is what gets
//! gated; [`FrameTiming`] is measured and is only ever recorded. The
//! boundary is a type distinction rather than a doc comment, and it
//! is the same line the JSON output draws.

use core::fmt;

use crate::digest::StateHash;
use crate::schedule::FramePlan;
use crate::time::Nanos;

/// The deterministic per-run tally: counts plus the schedule digest.
///
/// Everything here is a function of the plans absorbed, so two runs of the
/// same schedule produce identical statistics on any machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameStats {
    frames: u64,
    ticks: u64,
    steps_dropped: u64,
    hash: StateHash,
}

impl FrameStats {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            frames: 0,
            ticks: 0,
            steps_dropped: 0,
            hash: StateHash::new(),
        }
    }

    /// Tally one frame and fold its plan into the schedule digest.
    ///
    /// Every counter saturates. That is not ceremony for the frame count:
    /// a single frame on a saturated bank drops on the order of 1.1e12
    /// steps, so the *dropped* tally is only about ten million such frames
    /// from the ceiling, and an arithmetic overflow is a panic this crate
    /// does not get to have.
    pub fn absorb(&mut self, plan: &FramePlan) {
        self.frames = self.frames.saturating_add(1);
        self.ticks = self.ticks.saturating_add(u64::from(plan.step_count()));
        self.steps_dropped = self.steps_dropped.saturating_add(plan.dropped());
        self.hash = self.hash.absorb_plan(plan);
    }

    #[must_use]
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    #[must_use]
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }

    #[must_use]
    pub const fn steps_dropped(&self) -> u64 {
        self.steps_dropped
    }

    /// The fingerprint of every plan absorbed, in order.
    #[must_use]
    pub const fn schedule_hash(&self) -> u64 {
        self.hash.finish()
    }

    #[must_use]
    pub const fn json(&self) -> FrameStatsJson<'_> {
        FrameStatsJson(self)
    }
}

impl Default for FrameStats {
    fn default() -> Self {
        Self::new()
    }
}

/// [`FrameStats`] as one JSON object, for the machine-readable half of a
/// tool's output.
///
/// The digest is a hexadecimal *string*: a `u64` exceeds the integer
/// precision of every JSON reader that parses numbers as doubles, and a
/// silently rounded fingerprint is worse than no fingerprint.
#[derive(Clone, Copy, Debug)]
pub struct FrameStatsJson<'a>(&'a FrameStats);

impl fmt::Display for FrameStatsJson<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{\"frames\":{},\"ticks\":{},\"steps_dropped\":{},\"schedule_hash\":\"{:#018x}\"}}",
            self.0.frames,
            self.0.ticks,
            self.0.steps_dropped,
            self.0.schedule_hash()
        )
    }
}

/// The measured per-run timing summary: never gated, only recorded.
///
/// Percentiles are deliberately absent — p50/p99 need a reservoir or a
/// histogram, which is a real design. Count, minimum, maximum and sum are
/// enough for a first baseline; the growth trigger is the first frame
/// budget that needs negotiating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTiming {
    count: u64,
    min: u64,
    max: u64,
    sum: u64,
    drawn: u64,
    skipped: u64,
}

impl FrameTiming {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            count: 0,
            // The ceiling, so the first sample wins the comparison; the
            // JSON reports zero until a sample exists.
            min: u64::MAX,
            max: 0,
            sum: 0,
            drawn: 0,
            skipped: 0,
        }
    }

    /// Record one frame's measured CPU cost, and whether it presented.
    ///
    /// Presented-versus-skipped lives on the measured side deliberately:
    /// its purpose is measurement integrity, not determinism. A dormant
    /// window presenting nothing would otherwise "run at 40,000 fps" and
    /// silently inflate a frame-time baseline.
    pub fn record(&mut self, cpu_frame: Nanos, drawn: bool) {
        let nanos = cpu_frame.get();
        self.count = self.count.saturating_add(1);
        self.min = self.min.min(nanos);
        self.max = self.max.max(nanos);
        self.sum = self.sum.saturating_add(nanos);
        if drawn {
            self.drawn = self.drawn.saturating_add(1);
        } else {
            self.skipped = self.skipped.saturating_add(1);
        }
    }

    #[must_use]
    pub const fn json(&self) -> FrameTimingJson<'_> {
        FrameTimingJson(self)
    }

    /// The reported minimum: zero before the first sample, rather than the
    /// sentinel the comparison starts from.
    const fn reported_min(&self) -> u64 {
        if self.count == 0 { 0 } else { self.min }
    }
}

impl Default for FrameTiming {
    fn default() -> Self {
        Self::new()
    }
}

/// [`FrameTiming`] as one JSON object. Everything here varies between
/// runs and machines, which is exactly why it is a separate document
/// section from the digest.
#[derive(Clone, Copy, Debug)]
pub struct FrameTimingJson<'a>(&'a FrameTiming);

impl fmt::Display for FrameTimingJson<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{\"count\":{},\"min_ns\":{},\"max_ns\":{},\"sum_ns\":{},\"drawn\":{},\"skipped\":{}}}",
            self.0.count,
            self.0.reported_min(),
            self.0.max,
            self.0.sum,
            self.0.drawn,
            self.0.skipped
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameStats, FrameTiming};
    use crate::digest::StateHash;
    use crate::schedule::FrameLoop;
    use crate::time::{Nanos, StepBudget, Timestamp, Timestep};

    fn stalling_loop() -> FrameLoop {
        FrameLoop::new(
            Timestep::HZ_60,
            StepBudget::DEFAULT,
            Timestamp::from_nanos(0),
        )
    }

    #[test]
    fn a_fresh_tally_is_empty_and_holds_the_untouched_digest() {
        let stats = FrameStats::new();
        assert_eq!(stats, FrameStats::default());
        assert_eq!(stats.frames(), 0);
        assert_eq!(stats.ticks(), 0);
        assert_eq!(stats.steps_dropped(), 0);
        assert_eq!(stats.schedule_hash(), StateHash::new().finish());
        assert_eq!(
            stats.json().to_string(),
            "{\"frames\":0,\"ticks\":0,\"steps_dropped\":0,\"schedule_hash\":\"0xcbf29ce484222325\"}"
        );
    }

    #[test]
    fn absorbing_frames_tallies_steps_and_drops_and_moves_the_digest() {
        let mut frame = stalling_loop();
        let mut stats = FrameStats::new();
        // Two ordinary frames, then a 200 ms stall the budget refuses.
        for now in [16_666_667_u64, 33_333_334, 233_333_334] {
            stats.absorb(&frame.begin_frame(Timestamp::from_nanos(now)));
        }
        assert_eq!(stats.frames(), 3);
        assert_eq!(stats.ticks(), 7, "one, one, then the budgeted five");
        assert!(stats.steps_dropped() > 0, "the stall was refused");
        assert_ne!(stats.schedule_hash(), StateHash::new().finish());

        // The digest is a quoted, zero-padded hex string, not a JSON
        // number: a u64 exceeds the precision of a double-parsing reader.
        let json = stats.json().to_string();
        let prefix = "{\"frames\":3,\"ticks\":7,\"steps_dropped\":6,\"schedule_hash\":\"0x";
        assert!(json.starts_with(prefix), "{json}");
        assert!(json.ends_with("\"}"), "{json}");
        assert_eq!(
            json.len(),
            prefix.len() + 16 + 2,
            "sixteen hex digits: {json}"
        );
    }

    /// The tally is a function of the plans absorbed and nothing else, so
    /// two independently driven runs of one schedule agree exactly.
    #[test]
    fn two_runs_of_one_schedule_produce_identical_statistics() {
        let run = || {
            let mut frame = stalling_loop();
            let mut stats = FrameStats::new();
            for k in 1..=32u64 {
                stats.absorb(&frame.begin_frame(Timestamp::from_nanos(k * 12_000_000)));
            }
            stats
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn a_fresh_timing_summary_reports_a_zero_minimum_rather_than_the_sentinel() {
        let timing = FrameTiming::new();
        assert_eq!(timing, FrameTiming::default());
        assert_eq!(
            timing.json().to_string(),
            "{\"count\":0,\"min_ns\":0,\"max_ns\":0,\"sum_ns\":0,\"drawn\":0,\"skipped\":0}"
        );
    }

    #[test]
    fn recording_frames_tracks_the_extremes_the_total_and_the_presented_split() {
        let mut timing = FrameTiming::new();
        timing.record(Nanos::from_nanos(4_000_000), true);
        timing.record(Nanos::from_nanos(1_000_000), true);
        timing.record(Nanos::from_nanos(9_000_000), false);
        assert_eq!(
            timing.json().to_string(),
            "{\"count\":3,\"min_ns\":1000000,\"max_ns\":9000000,\"sum_ns\":14000000,\
             \"drawn\":2,\"skipped\":1}"
        );
    }

    #[test]
    fn the_measured_total_saturates_rather_than_wrapping() {
        let mut timing = FrameTiming::new();
        timing.record(Nanos::from_nanos(u64::MAX), true);
        timing.record(Nanos::from_nanos(u64::MAX), true);
        let json = timing.json().to_string();
        assert!(json.contains(&format!("\"sum_ns\":{}", u64::MAX)), "{json}");
        assert!(json.contains(&format!("\"min_ns\":{}", u64::MAX)), "{json}");
    }
}
