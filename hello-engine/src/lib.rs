//! Fixed-timestep accumulation.
//!
//! A fixed-timestep loop advances simulation state in constant increments
//! regardless of how long each rendered frame takes. [`Accumulator`] banks
//! elapsed frame time and reports how many whole simulation ticks fit in it,
//! carrying any remainder into future frames.
//!
//! All arithmetic is integer nanoseconds: for a given timestep and sequence
//! of frame times, the tick counts and end state are exactly reproducible.

/// Banks elapsed frame time and converts it into whole simulation ticks.
///
/// Time is measured in integer nanoseconds. The accumulator holds two values:
/// the fixed timestep `dt` and the banked, not-yet-simulated time `acc`.
/// Each call to [`Accumulator::advance`] adds a frame's duration to the bank
/// and drains as many whole timesteps from it as possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accumulator {
    /// Banked time not yet consumed by a tick, in nanoseconds.
    acc: u64,
    /// Fixed simulation timestep, in nanoseconds.
    dt: u64,
}

impl Accumulator {
    /// Creates an accumulator with the given fixed timestep in nanoseconds
    /// and an empty bank.
    ///
    /// # Panics
    ///
    /// Panics if `dt_ns` is zero: a zero-length timestep could never consume
    /// banked time.
    #[must_use]
    pub const fn new(dt_ns: u64) -> Self {
        assert!(dt_ns > 0, "timestep must be non-zero");
        Self { acc: 0, dt: dt_ns }
    }

    /// Banks `frame_time_ns` and returns the number of whole simulation
    /// ticks now due. The remainder stays banked for future frames.
    ///
    /// Two saturation rules keep the arithmetic total (no overflow, no
    /// wraparound), both far outside any realistic frame time:
    ///
    /// - the bank saturates at `u64::MAX` nanoseconds (~584 years);
    /// - every whole timestep in the bank is always consumed, but the
    ///   returned count saturates at `u32::MAX` ticks.
    pub fn advance(&mut self, frame_time_ns: u64) -> u32 {
        self.acc = self.acc.saturating_add(frame_time_ns);
        let ticks = self.acc / self.dt;
        self.acc %= self.dt;
        u32::try_from(ticks).unwrap_or(u32::MAX)
    }

    /// Banked time not yet consumed by a tick, in nanoseconds.
    ///
    /// Always strictly less than [`Accumulator::timestep_ns`] after a call to
    /// [`Accumulator::advance`].
    #[must_use]
    pub const fn pending_ns(&self) -> u64 {
        self.acc
    }

    /// The fixed simulation timestep, in nanoseconds.
    #[must_use]
    pub const fn timestep_ns(&self) -> u64 {
        self.dt
    }
}

#[cfg(test)]
mod tests {
    use super::Accumulator;

    /// 60 Hz timestep, rounded to whole nanoseconds.
    const DT: u64 = 16_666_667;

    #[test]
    fn zero_frame_time_yields_no_ticks() {
        let mut acc = Accumulator::new(DT);
        assert_eq!(acc.advance(0), 0);
        assert_eq!(acc.pending_ns(), 0);
    }

    #[test]
    fn exact_single_timestep_yields_one_tick_and_empty_bank() {
        let mut acc = Accumulator::new(DT);
        assert_eq!(acc.advance(DT), 1);
        assert_eq!(acc.pending_ns(), 0);
    }

    #[test]
    fn exact_multiple_yields_exact_ticks_and_empty_bank() {
        let mut acc = Accumulator::new(DT);
        assert_eq!(acc.advance(3 * DT), 3);
        assert_eq!(acc.pending_ns(), 0);
    }

    #[test]
    fn remainder_carries_across_frames() {
        let mut acc = Accumulator::new(DT);
        assert_eq!(acc.advance(DT + 5), 1);
        assert_eq!(acc.pending_ns(), 5);
    }

    #[test]
    fn sub_timestep_frames_bank_until_a_tick_is_due() {
        let mut acc = Accumulator::new(DT);
        assert_eq!(acc.advance(DT / 2), 0);
        assert_eq!(acc.pending_ns(), DT / 2);
        // DT is odd, so two halves fall one nanosecond short of a tick.
        assert_eq!(acc.advance(DT / 2), 0);
        assert_eq!(acc.pending_ns(), DT - 1);
        assert_eq!(acc.advance(1), 1);
        assert_eq!(acc.pending_ns(), 0);
    }

    #[test]
    fn large_frame_time_saturates_the_bank_without_overflow() {
        let mut acc = Accumulator::new(DT);
        acc.advance(1);
        // Banking u64::MAX on top of existing time must saturate, not wrap.
        // u64::MAX / DT exceeds u32::MAX, so the tick count saturates too.
        assert_eq!(acc.advance(u64::MAX), u32::MAX);
        assert_eq!(acc.pending_ns(), u64::MAX % DT);
        // The accumulator keeps working normally afterwards.
        let pending = acc.pending_ns();
        assert_eq!(acc.advance(DT - pending), 1);
        assert_eq!(acc.pending_ns(), 0);
    }

    #[test]
    fn tick_count_saturates_at_u32_max() {
        let mut acc = Accumulator::new(1);
        assert_eq!(acc.advance(u64::MAX), u32::MAX);
        // All whole timesteps were consumed even though the count saturated.
        assert_eq!(acc.pending_ns(), 0);
    }

    #[test]
    fn identical_input_sequences_produce_identical_results() {
        let frames = [0_u64, 8_000_000, DT, 2 * DT + 1, 1, DT - 1, u64::MAX];
        let run = || {
            let mut acc = Accumulator::new(DT);
            let ticks: Vec<u32> = frames.iter().map(|&f| acc.advance(f)).collect();
            (ticks, acc)
        };
        assert_eq!(run(), run());
    }

    #[test]
    #[should_panic(expected = "timestep must be non-zero")]
    fn zero_timestep_is_rejected() {
        let _ = Accumulator::new(0);
    }
}
