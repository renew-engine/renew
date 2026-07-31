//! The driver's half of input: physical keys in, resolved intent out.
//!
//! **This lives in the driver, not in the world, and that placement is
//! the point.** A binding table and the set of keys physically down are
//! facts about the machine someone is sitting at. They are not
//! simulation state: they do not belong in a recording, they do not
//! belong in a digest, and they must not have to be `Copy` merely
//! because the world is.
//!
//! What crosses into the simulation is [`Intent`] — which way the player
//! is asking to go, already resolved. Two keys meaning the same
//! direction have already been OR-ed together; opposite directions have
//! already cancelled. The world receives a decision, not a keyboard.

use renew_input::{Binding, InputMap};
use renew_platform::event::{KeyCode, WindowEvent};

/// The four directions this sample understands.
///
/// A caller-defined action type rather than strings: a typo is a compile
/// error instead of a binding that silently never fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// What the player is asking for this tick, resolved to two axes.
///
/// Each field is `-1`, `0`, or `1`. **Deliberately not a speed:** how far
/// that intent moves anything is the simulation's business, chosen by its
/// seed, and the input layer has no opinion about it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Intent {
    /// Right is positive.
    pub horizontal: i64,
    /// Down is positive, matching screen coordinates.
    pub vertical: i64,
}

/// The binding table and the keys currently down.
///
/// Not `Copy`, and it does not need to be — nothing here reaches the
/// world except through [`Input::intent`].
#[derive(Debug)]
pub struct Input {
    map: InputMap<Direction>,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    /// Every direction bound to both an arrow and a letter.
    #[must_use]
    pub fn new() -> Self {
        let mut map = InputMap::new();
        for (binding, direction) in [
            (KeyCode::ArrowUp, Direction::Up),
            (KeyCode::KeyW, Direction::Up),
            (KeyCode::ArrowDown, Direction::Down),
            (KeyCode::KeyS, Direction::Down),
            (KeyCode::ArrowLeft, Direction::Left),
            (KeyCode::KeyA, Direction::Left),
            (KeyCode::ArrowRight, Direction::Right),
            (KeyCode::KeyD, Direction::Right),
        ] {
            map.bind(Binding::key(binding), direction);
        }
        Self { map }
    }

    /// Feed one window event. Anything unbound is ignored.
    pub fn handle(&mut self, event: WindowEvent) {
        self.map.handle(event);
    }

    /// End the tick, clearing press and release edges.
    ///
    /// This sample reads only holds, so nothing it does today depends on
    /// this being called. It is called anyway, because a map whose edges
    /// are never retired reports a stale `just_pressed` forever, and the
    /// first consumer to read one would find a bug that predates it.
    pub fn advance(&mut self) {
        self.map.advance();
    }

    /// Forget every held key, for focus loss.
    ///
    /// The OS stops delivering key-up while another window has focus, so
    /// without this a player who alt-tabs mid-move comes back still
    /// moving.
    pub fn release_all(&mut self) {
        self.map.release_all();
    }

    /// The resolved intent: opposite directions cancel, and a direction
    /// counts as held while either of its keys is down.
    #[must_use]
    pub fn intent(&self) -> Intent {
        Intent {
            horizontal: self.axis(Direction::Right, Direction::Left),
            vertical: self.axis(Direction::Down, Direction::Up),
        }
    }

    fn axis(&self, positive: Direction, negative: Direction) -> i64 {
        i64::from(self.map.held(positive)) - i64::from(self.map.held(negative))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, pressed: bool) -> WindowEvent {
        WindowEvent::Key {
            code,
            pressed,
            repeat: false,
        }
    }

    #[test]
    fn nothing_held_is_no_intent() {
        let input = Input::new();
        assert_eq!(input.intent(), Intent::default());
        assert_eq!(Input::default().intent(), Intent::default());
    }

    #[test]
    fn each_direction_moves_its_own_axis() {
        for (code, expected) in [
            (
                KeyCode::ArrowRight,
                Intent {
                    horizontal: 1,
                    vertical: 0,
                },
            ),
            (
                KeyCode::ArrowLeft,
                Intent {
                    horizontal: -1,
                    vertical: 0,
                },
            ),
            (
                KeyCode::ArrowDown,
                Intent {
                    horizontal: 0,
                    vertical: 1,
                },
            ),
            (
                KeyCode::ArrowUp,
                Intent {
                    horizontal: 0,
                    vertical: -1,
                },
            ),
        ] {
            let mut input = Input::new();
            input.handle(key(code, true));
            assert_eq!(input.intent(), expected, "{code:?}");
        }
    }

    /// Every direction has both an arrow and a letter, and they are the
    /// same direction rather than two that happen to agree.
    #[test]
    fn every_direction_has_a_letter_and_an_arrow() {
        for (letter, arrow) in [
            (KeyCode::KeyW, KeyCode::ArrowUp),
            (KeyCode::KeyS, KeyCode::ArrowDown),
            (KeyCode::KeyA, KeyCode::ArrowLeft),
            (KeyCode::KeyD, KeyCode::ArrowRight),
        ] {
            let mut by_letter = Input::new();
            by_letter.handle(key(letter, true));
            let mut by_arrow = Input::new();
            by_arrow.handle(key(arrow, true));
            assert_eq!(
                by_letter.intent(),
                by_arrow.intent(),
                "{letter:?} and {arrow:?}"
            );
        }
    }

    /// The case the key-level mask got wrong before this layer existed:
    /// releasing one of two keys meaning one direction must not clear it.
    #[test]
    fn releasing_one_of_two_keys_for_a_direction_keeps_it_held() {
        let mut input = Input::new();
        input.handle(key(KeyCode::ArrowUp, true));
        input.handle(key(KeyCode::KeyW, true));
        assert_eq!(input.intent().vertical, -1, "two keys is not two units");

        input.handle(key(KeyCode::ArrowUp, false));
        assert_eq!(input.intent().vertical, -1, "W is still down");

        input.handle(key(KeyCode::KeyW, false));
        assert_eq!(input.intent().vertical, 0, "and now nothing is");
    }

    #[test]
    fn opposite_directions_cancel_and_uncancel() {
        let mut input = Input::new();
        input.handle(key(KeyCode::KeyA, true));
        input.handle(key(KeyCode::KeyD, true));
        input.handle(key(KeyCode::ArrowUp, true));
        input.handle(key(KeyCode::ArrowDown, true));
        assert_eq!(input.intent(), Intent::default());

        input.handle(key(KeyCode::KeyA, false));
        input.handle(key(KeyCode::ArrowDown, false));
        assert_eq!(
            input.intent(),
            Intent {
                horizontal: 1,
                vertical: -1
            }
        );
    }

    /// A repeat is the OS restating a key that is already down. Acting on
    /// one would make movement depend on the keyboard's repeat rate — the
    /// frame-rate dependence a fixed timestep exists to remove.
    #[test]
    fn a_repeat_changes_nothing() {
        let mut input = Input::new();
        input.handle(key(KeyCode::ArrowRight, true));
        let before = input.intent();
        input.handle(WindowEvent::Key {
            code: KeyCode::ArrowRight,
            pressed: true,
            repeat: true,
        });
        assert_eq!(input.intent(), before);
    }

    #[test]
    fn unbound_input_is_ignored() {
        let mut input = Input::new();
        input.handle(key(KeyCode::Escape, true));
        input.handle(key(KeyCode::Space, true));
        input.handle(WindowEvent::PointerMoved { x: 3.0, y: 4.0 });
        input.handle(WindowEvent::CloseRequested);
        assert_eq!(input.intent(), Intent::default());
    }

    #[test]
    fn focus_loss_releases_everything() {
        let mut input = Input::new();
        input.handle(key(KeyCode::ArrowRight, true));
        input.handle(key(KeyCode::ArrowUp, true));
        input.release_all();
        assert_eq!(input.intent(), Intent::default());
    }

    /// Advancing retires edges and must leave holds alone — this sample
    /// reads only holds, so a regression here would be silent.
    #[test]
    fn advancing_keeps_the_hold() {
        let mut input = Input::new();
        input.handle(key(KeyCode::ArrowRight, true));
        input.advance();
        assert_eq!(input.intent().horizontal, 1);
        input.advance();
        assert_eq!(input.intent().horizontal, 1);
    }
}
