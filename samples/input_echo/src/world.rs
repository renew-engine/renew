//! Input in, fixed steps out: the sample's whole simulation.
//!
//! Events arrive whenever the OS feels like sending them; the world they
//! land in advances only in fixed steps. That separation is the point of
//! the sample — a key held for three ticks moves the same distance on
//! every machine, whatever the frame rate was while it was held.

use renew_frame::{StateHash, Step};
use renew_platform::window::{KeyCode, WindowEvent};

/// The eight movement keys, one bit each.
///
/// **Keys, not directions, and the distinction is a bug fix.** The mask
/// used to hold four direction bits, so releasing either key bound to a
/// direction cleared it: press Up, press W, release Up, and the sample
/// stopped moving up while W was still down. A direction is held while
/// *any* of its keys is, which is an OR over the keys — and that can only
/// be expressed by tracking the keys.
const K_ARROW_UP: u8 = 1 << 0;
const K_W: u8 = 1 << 1;
const K_ARROW_DOWN: u8 = 1 << 2;
const K_S: u8 = 1 << 3;
const K_ARROW_LEFT: u8 = 1 << 4;
const K_A: u8 = 1 << 5;
const K_ARROW_RIGHT: u8 = 1 << 6;
const K_D: u8 = 1 << 7;

/// A direction is held while any of its keys is.
const UP: u8 = K_ARROW_UP | K_W;
const DOWN: u8 = K_ARROW_DOWN | K_S;
const LEFT: u8 = K_ARROW_LEFT | K_A;
const RIGHT: u8 = K_ARROW_RIGHT | K_D;

/// How many speeds a seed can select.
const SPEEDS: u64 = 4;

/// Physical pixels as whole units.
///
/// The fractional part of a pointer position is a pointing device's
/// business; the simulation records where the pointer was, not where it
/// was to the nearest hundredth.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a pointer position outside the i64 range is not a position"
)]
fn whole(value: f64) -> i64 {
    value as i64
}

/// Units per tick for a seed: one to four, never zero, so every seed
/// produces a world that visibly moves.
const fn speed_for(seed: u64) -> i64 {
    #[expect(
        clippy::cast_possible_wrap,
        reason = "the modulus bounds the value to 1..=SPEEDS"
    )]
    let units = (1 + seed % SPEEDS) as i64;
    units
}

/// What the input adds up to: a position that only fixed steps move, and
/// a tally of everything that arrived on the way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EchoWorld {
    seed: u64,
    /// Units per tick, selected by the seed.
    speed: i64,
    position: (i64, i64),
    /// Which movement keys are physically down, one bit each.
    held: u8,
    ticks: u64,
    events: u64,
    keys_pressed: u64,
    keys_released: u64,
    repeats: u64,
    pointer: (i64, i64),
    pointer_moves: u64,
    buttons: u64,
    wheel: i64,
    extent: (u32, u32),
    focused: bool,
    close_requested: bool,
    /// Absorbs every step as it happens, so a repeated or reordered tick
    /// changes the digest even when the final position does not.
    trace: StateHash,
}

impl EchoWorld {
    /// The world before anything has happened to it.
    ///
    /// The seed selects the movement speed and nothing else: there is no
    /// random number service until the simulation layer has one, and a
    /// seed that fed nothing would be a flag pretending to be an axis.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            seed,
            speed: speed_for(seed),
            position: (0, 0),
            held: 0,
            ticks: 0,
            events: 0,
            keys_pressed: 0,
            keys_released: 0,
            repeats: 0,
            pointer: (0, 0),
            pointer_moves: 0,
            buttons: 0,
            wheel: 0,
            extent: (0, 0),
            focused: false,
            close_requested: false,
            trace: StateHash::new(),
        }
    }

    /// Consume one input event. Never advances the simulation: events
    /// change what the next step will do, never the state directly.
    pub fn event(&mut self, event: WindowEvent) {
        self.events = self.events.saturating_add(1);
        match event {
            WindowEvent::Key {
                code,
                pressed,
                repeat,
            } => self.key(code, pressed, repeat),
            WindowEvent::PointerMoved { x, y } => {
                self.pointer = (whole(x), whole(y));
                self.pointer_moves = self.pointer_moves.saturating_add(1);
            }
            WindowEvent::PointerButton { pressed: true, .. } => {
                self.buttons = self.buttons.saturating_add(1);
            }
            WindowEvent::Wheel { dy, .. } => {
                self.wheel = self.wheel.saturating_add(whole(f64::from(dy)));
            }
            WindowEvent::Focused(focused) => self.focused = focused,
            WindowEvent::Resized { width, height } => self.extent = (width, height),
            WindowEvent::CloseRequested => self.close_requested = true,
            // Everything else is counted and ignored: a released button,
            // a scale change, a repaint request this sample has nothing
            // to paint for.
            _ => {}
        }
    }

    fn key(&mut self, code: KeyCode, pressed: bool, repeat: bool) {
        if repeat {
            // A repeat is the OS re-sending a key that is already held.
            // Acting on it would make movement depend on the keyboard's
            // repeat rate — the exact frame-rate dependence a fixed
            // timestep exists to remove.
            self.repeats = self.repeats.saturating_add(1);
            return;
        }
        if pressed {
            self.keys_pressed = self.keys_pressed.saturating_add(1);
        } else {
            self.keys_released = self.keys_released.saturating_add(1);
        }
        let key = match code {
            KeyCode::ArrowUp => K_ARROW_UP,
            KeyCode::KeyW => K_W,
            KeyCode::ArrowDown => K_ARROW_DOWN,
            KeyCode::KeyS => K_S,
            KeyCode::ArrowLeft => K_ARROW_LEFT,
            KeyCode::KeyA => K_A,
            KeyCode::ArrowRight => K_ARROW_RIGHT,
            KeyCode::KeyD => K_D,
            // Escape asks to quit, exactly as the window's own close
            // button does; every other key is counted and ignored.
            KeyCode::Escape => {
                self.close_requested = pressed;
                0
            }
            _ => 0,
        };
        if pressed {
            self.held |= key;
        } else {
            self.held &= !key;
        }
    }

    /// Advance one fixed step — the only way this world ever moves.
    pub fn step(&mut self, step: Step) {
        self.ticks = self.ticks.saturating_add(1);
        let horizontal = self.axis(RIGHT, LEFT);
        let vertical = self.axis(DOWN, UP);
        self.position.0 = self.position.0.saturating_add(horizontal);
        self.position.1 = self.position.1.saturating_add(vertical);
        self.trace = self
            .trace
            .absorb_u64(step.tick)
            .absorb_u64(step.sim_time.get())
            .absorb_bytes(&self.position.0.to_le_bytes())
            .absorb_bytes(&self.position.1.to_le_bytes());
    }

    /// One axis of movement: opposite directions held together cancel.
    ///
    /// Each argument is the set of keys meaning that direction, so a
    /// direction counts as held while any one of them is down.
    fn axis(&self, positive: u8, negative: u8) -> i64 {
        let forward = i64::from(self.held & positive != 0);
        let back = i64::from(self.held & negative != 0);
        (forward - back).saturating_mul(self.speed)
    }

    /// Whether the run has been asked to stop — by the close button, or
    /// by the escape key.
    #[must_use]
    pub const fn close_requested(&self) -> bool {
        self.close_requested
    }

    #[must_use]
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }

    #[must_use]
    pub const fn events(&self) -> u64 {
        self.events
    }

    #[must_use]
    pub const fn keys(&self) -> (u64, u64, u64) {
        (self.keys_pressed, self.keys_released, self.repeats)
    }

    #[must_use]
    pub const fn pointer(&self) -> (i64, i64) {
        self.pointer
    }

    #[must_use]
    pub const fn pointer_moves(&self) -> u64 {
        self.pointer_moves
    }

    #[must_use]
    pub const fn buttons(&self) -> u64 {
        self.buttons
    }

    #[must_use]
    pub const fn wheel(&self) -> i64 {
        self.wheel
    }

    #[must_use]
    pub const fn position(&self) -> (i64, i64) {
        self.position
    }

    #[must_use]
    pub const fn extent(&self) -> (u32, u32) {
        self.extent
    }

    #[must_use]
    pub const fn focused(&self) -> bool {
        self.focused
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
            .absorb_u64(self.events)
            .absorb_bytes(&self.position.0.to_le_bytes())
            .absorb_bytes(&self.position.1.to_le_bytes())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::EchoWorld;
    use renew_frame::{Nanos, Step};
    use renew_platform::window::{KeyCode, PointerButton, WindowEvent};

    fn tick(world: &mut EchoWorld, ticks: u64) {
        let first = world.ticks();
        for offset in 0..ticks {
            let index = first + offset;
            world.step(Step {
                tick: index,
                dt: Nanos::from_nanos(16_666_667),
                sim_time: Nanos::from_nanos(index * 16_666_667),
            });
        }
    }

    fn press(code: KeyCode) -> WindowEvent {
        WindowEvent::Key {
            code,
            pressed: true,
            repeat: false,
        }
    }

    fn release(code: KeyCode) -> WindowEvent {
        WindowEvent::Key {
            code,
            pressed: false,
            repeat: false,
        }
    }

    #[test]
    fn a_fresh_world_stands_still_however_long_it_runs() {
        let mut world = EchoWorld::new(0);
        tick(&mut world, 100);
        assert_eq!(world.position(), (0, 0));
        assert_eq!(world.ticks(), 100);
        assert_eq!(world.events(), 0);
    }

    /// The property the whole sample exists to show: distance is a
    /// function of ticks held, never of how many events arrived or how
    /// fast the frames came.
    #[test]
    fn a_held_key_moves_one_speed_per_tick_and_stops_when_released() {
        let mut world = EchoWorld::new(0);
        world.event(press(KeyCode::ArrowRight));
        tick(&mut world, 3);
        assert_eq!(world.position(), (3, 0));
        world.event(release(KeyCode::ArrowRight));
        tick(&mut world, 5);
        assert_eq!(world.position(), (3, 0), "a released key moves nothing");
        assert_eq!(world.keys(), (1, 1, 0));
    }

    #[test]
    fn the_seed_selects_the_speed_and_nothing_else() {
        for seed in 0..8u64 {
            let mut world = EchoWorld::new(seed);
            world.event(press(KeyCode::KeyD));
            tick(&mut world, 1);
            let expected = 1 + i64::try_from(seed % 4).unwrap_or(0);
            assert_eq!(world.position(), (expected, 0), "seed {seed}");
        }
    }

    #[test]
    fn opposite_keys_held_together_cancel() {
        let mut world = EchoWorld::new(0);
        world.event(press(KeyCode::KeyA));
        world.event(press(KeyCode::KeyD));
        world.event(press(KeyCode::ArrowUp));
        world.event(press(KeyCode::ArrowDown));
        tick(&mut world, 10);
        assert_eq!(world.position(), (0, 0));
        // Release one of each pair and the other one wins.
        world.event(release(KeyCode::KeyA));
        world.event(release(KeyCode::ArrowDown));
        tick(&mut world, 2);
        assert_eq!(world.position(), (2, -2));
    }

    /// Both keys for one direction, released one at a time. Releasing
    /// either used to clear the direction outright, so the sample stopped
    /// moving while a key meaning "up" was still physically down.
    #[test]
    fn releasing_one_of_two_keys_for_a_direction_keeps_it_held() {
        let mut world = EchoWorld::new(0);
        world.event(press(KeyCode::ArrowUp));
        world.event(press(KeyCode::KeyW));
        tick(&mut world, 1);
        assert_eq!(
            world.position(),
            (0, -1),
            "two keys for one direction is not two units"
        );

        world.event(release(KeyCode::ArrowUp));
        tick(&mut world, 1);
        assert_eq!(world.position(), (0, -2), "W is still down");

        world.event(release(KeyCode::KeyW));
        tick(&mut world, 3);
        assert_eq!(world.position(), (0, -2), "and now nothing is");
    }

    #[test]
    fn every_direction_key_has_a_letter_and_an_arrow() {
        for (letter, arrow) in [
            (KeyCode::KeyW, KeyCode::ArrowUp),
            (KeyCode::KeyS, KeyCode::ArrowDown),
            (KeyCode::KeyA, KeyCode::ArrowLeft),
            (KeyCode::KeyD, KeyCode::ArrowRight),
        ] {
            let mut with_letter = EchoWorld::new(0);
            with_letter.event(press(letter));
            tick(&mut with_letter, 4);
            let mut with_arrow = EchoWorld::new(0);
            with_arrow.event(press(arrow));
            tick(&mut with_arrow, 4);
            assert_eq!(with_letter.position(), with_arrow.position(), "{letter:?}");
            assert_ne!(with_letter.position(), (0, 0));
        }
    }

    /// Acting on key repeats would make movement depend on the
    /// keyboard's repeat rate, which is the frame-rate dependence a
    /// fixed timestep exists to remove.
    #[test]
    fn key_repeats_are_counted_and_never_acted_on() {
        let mut world = EchoWorld::new(0);
        world.event(WindowEvent::Key {
            code: KeyCode::ArrowRight,
            pressed: true,
            repeat: true,
        });
        tick(&mut world, 4);
        assert_eq!(world.position(), (0, 0));
        assert_eq!(world.keys(), (0, 0, 1));
    }

    #[test]
    fn escape_asks_to_quit_exactly_as_the_close_button_does() {
        let mut escaped = EchoWorld::new(0);
        assert!(!escaped.close_requested());
        escaped.event(press(KeyCode::Escape));
        assert!(escaped.close_requested());
        // Releasing it is not a second request.
        escaped.event(release(KeyCode::Escape));
        assert!(!escaped.close_requested());

        let mut closed = EchoWorld::new(0);
        closed.event(WindowEvent::CloseRequested);
        assert!(closed.close_requested());
    }

    #[test]
    fn an_unmapped_key_is_counted_and_moves_nothing() {
        let mut world = EchoWorld::new(0);
        world.event(press(KeyCode::Space));
        tick(&mut world, 3);
        assert_eq!(world.position(), (0, 0));
        assert_eq!(world.keys(), (1, 0, 0));
    }

    #[test]
    fn the_pointer_the_wheel_and_the_window_are_echoed_as_whole_units() {
        let mut world = EchoWorld::new(0);
        world.event(WindowEvent::PointerMoved { x: 10.75, y: 20.25 });
        world.event(WindowEvent::PointerButton {
            button: PointerButton::Left,
            pressed: true,
        });
        world.event(WindowEvent::PointerButton {
            button: PointerButton::Left,
            pressed: false,
        });
        world.event(WindowEvent::Wheel { dx: 0.0, dy: 32.0 });
        world.event(WindowEvent::Focused(true));
        world.event(WindowEvent::Resized {
            width: 800,
            height: 600,
        });
        world.event(WindowEvent::ScaleFactorChanged { scale: 2.0 });
        assert_eq!(world.pointer(), (10, 20));
        assert_eq!(world.pointer_moves(), 1);
        assert_eq!(world.buttons(), 1, "a release is not a second press");
        assert_eq!(world.wheel(), 32);
        assert!(world.focused());
        assert_eq!(world.extent(), (800, 600));
        assert_eq!(world.events(), 7, "every event is counted, mapped or not");
    }

    #[test]
    fn the_same_input_and_the_same_ticks_reproduce_the_same_digest() {
        let run = || {
            let mut world = EchoWorld::new(2);
            world.event(press(KeyCode::ArrowRight));
            tick(&mut world, 6);
            world.event(release(KeyCode::ArrowRight));
            tick(&mut world, 6);
            world
        };
        assert_eq!(run(), run());
        assert_eq!(run().state_hash(), run().state_hash());
    }

    #[test]
    fn holding_a_key_one_tick_longer_changes_the_digest() {
        let held = |ticks| {
            let mut world = EchoWorld::new(0);
            world.event(press(KeyCode::ArrowRight));
            tick(&mut world, ticks);
            world.state_hash()
        };
        assert_ne!(held(6), held(7));
    }
}
