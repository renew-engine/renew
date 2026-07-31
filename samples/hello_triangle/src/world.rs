//! The sample's whole simulation: one integer value, walked one fixed
//! step at a time.

use renew_frame::{StateHash, Step};

/// The number of distinct strides a seed can select. Prime, so no seed
/// picks a stride that divides the byte range evenly and hides a
/// miscount behind a repeating colour.
const STRIDES: u64 = 7;

/// A value advanced by a fixed stride per simulation step, plus a
/// running fingerprint of the steps that advanced it.
///
/// Integer-only, deliberately. Bit-determinism is scoped to one platform
///, and the transcendental functions differ between platform math
/// libraries — so a world holding an angle would quietly make the state
/// hash a cross-platform promise the engine does not make. Anything that
/// wants to rotate does it in the shader, which is render, not
/// simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct World {
    seed: u64,
    /// How far one step moves the value. Never zero, so the clear colour
    /// changes on every tick and a loop that stopped stepping shows up
    /// in the pixels, not only in the counters.
    stride: u64,
    ticks: u64,
    value: u64,
    /// Absorbs every step as it arrives, so a repeated or reordered tick
    /// changes the digest even when the final value does not.
    trace: StateHash,
}

impl World {
    /// The world at rest, before any step.
    ///
    /// The seed selects the stride and nothing else: there is no random
    /// number service until the simulation layer has one, and a seed
    /// that fed nothing would be a flag pretending to be an axis.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            seed,
            stride: 1 + seed % STRIDES,
            ticks: 0,
            value: 0,
            trace: StateHash::new(),
        }
    }

    /// Advance one fixed step — the only way this world ever changes.
    pub fn step(&mut self, step: Step) {
        self.ticks = self.ticks.saturating_add(1);
        self.value = self.value.wrapping_add(self.stride);
        self.trace = self
            .trace
            .absorb_u64(step.tick)
            .absorb_u64(step.sim_time.get())
            .absorb_u64(self.value);
    }

    /// Steps executed so far.
    #[must_use]
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }

    /// The clear colour this world paints: the low three bytes of the
    /// value, as 8-bit channels.
    ///
    /// An integer function of the tick count, which is the whole point.
    /// The renderer converts each channel with `k / 255`, a conversion
    /// every conformant adapter performs exactly, so the headless test
    /// can compute the expected pixels from the tick count instead of
    /// comparing against a committed image — no artifact, no refresh
    /// ritual, and a one-step-off loop changes the bytes.
    #[must_use]
    pub const fn clear_rgb8(&self) -> [u8; 3] {
        let [red, green, blue, ..] = self.value.to_le_bytes();
        [red, green, blue]
    }

    /// The colour the next step will paint. The renderer interpolates
    /// toward it by the frame's alpha; nothing in the simulation reads
    /// it.
    #[must_use]
    pub const fn next_clear_rgb8(&self) -> [u8; 3] {
        let [red, green, blue, ..] = self.value.wrapping_add(self.stride).to_le_bytes();
        [red, green, blue]
    }

    /// The run's fingerprint: every step absorbed in order, closed with
    /// the final state.
    ///
    /// The seed is deliberately NOT absorbed, though it is the obvious
    /// thing to close with. A digest that absorbs an input cannot be
    /// used to prove that input had an effect: every seed would produce
    /// its own digest even if the seed were parsed, printed, and then
    /// ignored by the simulation entirely. Leaving it out makes the
    /// digest a fingerprint of BEHAVIOUR, so two seeds that move the
    /// world differently are told apart on the evidence, and two that
    /// move it identically are honestly reported as identical. The run's
    /// configuration is not lost — the digest line and the stats
    /// document both print the seed beside this number.
    #[must_use]
    pub const fn state_hash(&self) -> u64 {
        self.trace
            .absorb_u64(self.ticks)
            .absorb_u64(self.value)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::World;
    use renew_frame::{Nanos, Step};

    fn step(tick: u64) -> Step {
        Step {
            tick,
            dt: Nanos::from_nanos(16_666_667),
            sim_time: Nanos::from_nanos(tick.saturating_mul(16_666_667)),
        }
    }

    fn walk(seed: u64, steps: u64) -> World {
        let mut world = World::new(seed);
        for tick in 0..steps {
            world.step(step(tick));
        }
        world
    }

    #[test]
    fn a_fresh_world_has_run_no_steps_and_paints_black() {
        let world = World::new(0);
        assert_eq!(world.ticks(), 0);
        assert_eq!(world.clear_rgb8(), [0, 0, 0]);
        assert_eq!(world.next_clear_rgb8(), [1, 0, 0]);
    }

    #[test]
    fn every_seed_moves_the_colour_on_every_step() {
        for seed in 0..16u64 {
            let before = World::new(seed);
            let after = walk(seed, 1);
            assert_eq!(after.ticks(), 1);
            assert_ne!(
                after.clear_rgb8(),
                before.clear_rgb8(),
                "seed {seed} produced a standing colour"
            );
            // The prediction the renderer interpolates toward is the
            // colour the next step actually paints.
            assert_eq!(before.next_clear_rgb8(), after.clear_rgb8());
        }
    }

    #[test]
    fn the_colour_is_the_low_three_bytes_of_the_walked_value() {
        // Seed 0 strides by one, so the value is the tick count and the
        // channels carry it byte by byte.
        assert_eq!(walk(0, 8).clear_rgb8(), [8, 0, 0]);
        assert_eq!(walk(0, 256).clear_rgb8(), [0, 1, 0]);
        assert_eq!(walk(0, 65_536).clear_rgb8(), [0, 0, 1]);
    }

    #[test]
    fn the_same_seed_and_step_count_reproduce_the_same_state() {
        assert_eq!(walk(3, 40), walk(3, 40));
        assert_eq!(walk(3, 40).state_hash(), walk(3, 40).state_hash());
    }

    #[test]
    fn a_different_seed_or_a_different_step_count_changes_the_digest() {
        let base = walk(3, 40).state_hash();
        // Not because the seed is in the digest — it deliberately is
        // not — but because these two seeds pick different strides and
        // so walk to different values.
        assert_ne!(
            base,
            walk(4, 40).state_hash(),
            "a seed that changes the stride must change the digest"
        );
        assert_ne!(base, walk(3, 41).state_hash(), "the step count is absorbed");
    }

    /// The digest absorbs each step as it happens, so two runs whose
    /// final value agrees but whose tick sequence does not are still
    /// told apart. This is what catches a loop that replays a tick.
    #[test]
    fn a_repeated_tick_changes_the_digest_even_at_the_same_final_value() {
        let mut straight = World::new(0);
        let mut repeated = World::new(0);
        for tick in 0..4 {
            straight.step(step(tick));
        }
        for tick in [0, 1, 1, 2] {
            repeated.step(step(tick));
        }
        assert_eq!(straight.clear_rgb8(), repeated.clear_rgb8());
        assert_eq!(straight.ticks(), repeated.ticks());
        assert_ne!(straight.state_hash(), repeated.state_hash());
    }
}
