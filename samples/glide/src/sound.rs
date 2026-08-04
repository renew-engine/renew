//! What the game sounds like, derived from what the world did.
//!
//! The world knows nothing about audio and never will: it exposes a
//! score, an alive flag, and nothing resembling an event queue. So the
//! driver watches it the way the renderer does — take a reading before
//! a tick and another after, and the difference is what happened.
//!
//! **Per tick, not per frame.** A frame that catches up several ticks
//! runs [`tick_sounds`] once per tick, so the ordering of what is heard
//! matches the ordering of what happened: a bird that clears a pipe and
//! then dies in the same frame scores before it dies. Diffing once per
//! frame would fold that into a single ambiguous change.

/// What one simulation tick produced that a player should hear.
///
/// A count rather than a flag for scoring: glide's pipe spacing makes
/// two in one tick impossible today, and a count keeps this honest if
/// that spacing ever changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickSounds {
    /// A flap was consumed by this tick.
    pub flap: bool,
    /// How many pipes were cleared by this tick.
    pub scores: u64,
    /// This tick is the one the bird died on.
    pub death: bool,
}

impl TickSounds {
    /// Whether anything at all should be played.
    #[must_use]
    pub fn silent(self) -> bool {
        !self.flap && self.scores == 0 && !self.death
    }
}

/// Derive one tick's sounds from the readings around it.
///
/// `before_alive` and `before_score` are read before the tick;
/// `after_alive` and `after_score` after it. `flap_passed` is the bool
/// the driver handed to the world.
///
/// **The alive gate on flapping is load-bearing.** The driver passes
/// its flap bool unconditionally and decrements its own counter
/// unconditionally; it is the world that ignores a flap once the bird
/// is dead. Deriving the sound from the bool alone would blip on every
/// buffered press after a death — a sound with no cause on screen.
#[must_use]
pub fn tick_sounds(
    before_alive: bool,
    flap_passed: bool,
    before_score: u64,
    after_alive: bool,
    after_score: u64,
) -> TickSounds {
    TickSounds {
        flap: before_alive && flap_passed,
        scores: after_score.saturating_sub(before_score),
        death: before_alive && !after_alive,
    }
}

#[cfg(test)]
mod tests {
    use super::{TickSounds, tick_sounds};

    #[test]
    fn a_consumed_flap_sounds_once() {
        let sounds = tick_sounds(true, true, 0, true, 0);
        assert_eq!(
            sounds,
            TickSounds {
                flap: true,
                scores: 0,
                death: false
            }
        );
    }

    #[test]
    fn a_tick_with_no_flap_is_silent() {
        assert!(tick_sounds(true, false, 3, true, 3).silent());
    }

    // The case the alive gate exists for: presses buffered before the
    // crash still reach the world after it, and the world ignores them.
    // Anything derived from the raw bool would play a flap over a
    // corpse.
    #[test]
    fn a_press_after_death_makes_no_sound() {
        let sounds = tick_sounds(false, true, 5, false, 5);
        assert!(
            !sounds.flap,
            "a flap the world could not consume must not be heard"
        );
        assert!(sounds.silent(), "and nothing else happened either");
    }

    #[test]
    fn clearing_a_pipe_scores_once() {
        let sounds = tick_sounds(true, false, 2, true, 3);
        assert_eq!(sounds.scores, 1);
        assert!(!sounds.death && !sounds.flap);
    }

    // Not reachable at glide's pipe spacing, but the derivation is a
    // subtraction rather than a flag precisely so that a spacing change
    // does not silently drop the second sound.
    #[test]
    fn two_pipes_in_one_tick_would_score_twice() {
        assert_eq!(tick_sounds(true, false, 7, true, 9).scores, 2);
    }

    #[test]
    fn dying_sounds_exactly_on_the_tick_it_happens() {
        let dying = tick_sounds(true, false, 4, false, 4);
        assert!(dying.death, "the falling edge is the death");
        let after = tick_sounds(false, false, 4, false, 4);
        assert!(!after.death, "and it does not repeat every later tick");
    }

    // A catch-up frame runs several ticks; the point of deriving per
    // tick is that their order survives into what is played.
    #[test]
    fn a_catch_up_frame_keeps_its_ticks_in_order() {
        let ticks = [
            tick_sounds(true, true, 0, true, 0),   // flap
            tick_sounds(true, false, 0, true, 1),  // score
            tick_sounds(true, false, 1, false, 1), // death
        ];
        assert!(ticks[0].flap && ticks[0].scores == 0 && !ticks[0].death);
        assert!(!ticks[1].flap && ticks[1].scores == 1 && !ticks[1].death);
        assert!(!ticks[2].flap && ticks[2].scores == 0 && ticks[2].death);
    }

    // Scores never run backwards in the game, but a saturating
    // subtraction means a reading that somehow did cannot underflow
    // into a few billion sounds.
    #[test]
    fn a_score_that_went_backwards_asks_for_nothing() {
        assert_eq!(tick_sounds(true, false, 9, true, 4).scores, 0);
    }
}
