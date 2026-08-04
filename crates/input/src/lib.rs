//! Raw input events to named actions, deterministically.
//!
//! A game asks "is the player jumping", not "is the space bar down". This
//! crate is the layer between: a caller declares which physical inputs
//! mean which action, feeds it window events, and reads action state once
//! per tick.
//!
//! # Contract
//!
//! - **Edges are per tick, not per event.** `just_pressed` is true for the
//!   whole tick in which an action became active and false in the next
//!   one, whatever order the events arrived in or how many of them there
//!   were. A key pressed and released inside one tick still reports both
//!   edges, because a game that misses a fast tap is worse than one that
//!   sees it late.
//! - **Nothing here reads a clock.** [`InputMap::advance`] is called by
//!   the caller's fixed-timestep loop; the crate has no idea what time it
//!   is and cannot behave differently on a slow frame.
//! - **Binding order does not affect state.** Two inputs bound to one
//!   action are an OR, and the action is held while any of them is.
//! - **Unbound input is ignored, not an error.** A keyboard has more keys
//!   than any game binds, and refusing the others would make the common
//!   case noisy.
//!
//! # Example
//!
//! ```
//! use renew_input::{Binding, InputMap};
//! use renew_event::{KeyCode, WindowEvent};
//!
//! #[derive(Clone, Copy, PartialEq, Eq, Debug)]
//! enum Action { Jump, Left }
//!
//! let mut input = InputMap::new();
//! input.bind(Binding::key(KeyCode::Space), Action::Jump);
//! input.bind(Binding::key(KeyCode::ArrowLeft), Action::Left);
//!
//! input.handle(WindowEvent::Key { code: KeyCode::Space, pressed: true, repeat: false });
//! assert!(input.just_pressed(Action::Jump));
//! assert!(input.held(Action::Jump));
//!
//! // A new tick: the edge is gone, the hold remains.
//! input.advance();
//! assert!(!input.just_pressed(Action::Jump));
//! assert!(input.held(Action::Jump));
//! ```

// This layer resolves state; it never reports. Diagnostics about input
// belong to whoever is driving the loop.
// The determinism rule in the language standard: a simulation crate does not
// perform floating-point arithmetic whose result can reach digested state.
// Denied here rather than left to review — the lint covers operators only, so
// it is necessary and not sufficient, but what it does cover it covers with
// teeth.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::float_arithmetic)]

use renew_event::{KeyCode, PointerButton, WindowEvent};

/// One physical input that can be bound to an action.
///
/// A small closed enum rather than a general "any event" matcher: the
/// inputs a game binds are keys and buttons, and admitting resize or
/// focus events here would invite bindings that make no sense and then
/// need rules about what they mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Binding {
    /// A physical key, identified by position rather than by label.
    Key(KeyCode),
    /// A pointer button.
    Pointer(PointerButton),
}

impl Binding {
    /// Bind a key.
    #[must_use]
    pub const fn key(code: KeyCode) -> Self {
        Self::Key(code)
    }

    /// Bind a pointer button.
    #[must_use]
    pub const fn pointer(button: PointerButton) -> Self {
        Self::Pointer(button)
    }
}

/// What an action is doing this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActionState {
    /// Held down as of now.
    pub held: bool,
    /// Became held during this tick.
    pub just_pressed: bool,
    /// Stopped being held during this tick.
    pub just_released: bool,
}

/// Bindings and the state they produce.
///
/// Generic over the action type so a game names its own actions and the
/// compiler catches a typo, rather than passing strings that fail at
/// runtime. `A` need only be `Copy + Eq`: no hashing, because a hash map
/// would order iteration by a per-process seed and this crate promises
/// its behaviour does not vary between runs.
#[derive(Debug)]
pub struct InputMap<A> {
    /// Sorted by binding, so lookup is a binary search and iteration is
    /// stable. A `Vec` beats a map at this size and cannot surprise
    /// anyone with its order.
    bindings: Vec<(Binding, A)>,
    /// One entry per distinct action, in first-bound order.
    actions: Vec<(A, ActionState)>,
    /// Bindings currently physically down, sorted.
    down: Vec<Binding>,
}

impl<A: Copy + Eq> Default for InputMap<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Copy + Eq> InputMap<A> {
    /// A map with no bindings.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bindings: Vec::new(),
            actions: Vec::new(),
            down: Vec::new(),
        }
    }

    /// Bind an input to an action.
    ///
    /// Binding the same input twice replaces the earlier action: a
    /// physical key means one thing at a time, and keeping both would
    /// make the result depend on binding order.
    ///
    /// Binding several inputs to one action is an OR — the action is held
    /// while any of them is.
    pub fn bind(&mut self, binding: Binding, action: A) {
        match self.bindings.binary_search_by(|(b, _)| b.cmp(&binding)) {
            Ok(at) => {
                if let Some(entry) = self.bindings.get_mut(at) {
                    entry.1 = action;
                }
            }
            Err(at) => self.bindings.insert(at, (binding, action)),
        }
        if !self.actions.iter().any(|(known, _)| *known == action) {
            self.actions.push((action, ActionState::default()));
        }
    }

    /// How many bindings are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Whether nothing is bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// The action a physical input is bound to, if any.
    #[must_use]
    pub fn action_for(&self, binding: Binding) -> Option<A> {
        let at = self
            .bindings
            .binary_search_by(|(b, _)| b.cmp(&binding))
            .ok()?;
        self.bindings.get(at).map(|(_, action)| *action)
    }

    /// Feed one window event.
    ///
    /// Events that are not bound input — resize, focus, redraw — are
    /// ignored. Key repeats are ignored too: a repeat is the OS saying
    /// the key is still down, which this crate already knows, and letting
    /// one through would fire `just_pressed` again mid-hold.
    pub fn handle(&mut self, event: WindowEvent) {
        let (binding, pressed) = match event {
            WindowEvent::Key {
                code,
                pressed,
                repeat: false,
            } => (Binding::Key(code), pressed),
            WindowEvent::PointerButton { button, pressed } => (Binding::Pointer(button), pressed),
            _ => return,
        };
        self.set(binding, pressed);
    }

    /// Record a binding's physical state and update the action it drives.
    fn set(&mut self, binding: Binding, pressed: bool) {
        let known = self.down.binary_search(&binding);
        match (pressed, known) {
            (true, Err(at)) => self.down.insert(at, binding),
            (false, Ok(at)) => {
                self.down.remove(at);
            }
            // Already in the state being set: a duplicate press or a
            // release of something that was never down. Neither is an
            // error and neither changes anything.
            _ => return,
        }
        let Some(action) = self.action_for(binding) else {
            return;
        };
        let now_held = self.any_down_for(action);
        // `bind` adds every action it binds, so the entry is always
        // there. `for` over the matching entries rather than `if let`:
        // the absent case is then an empty iteration rather than a branch
        // with a body, so nothing unreachable needs claiming or exempting.
        for (_, state) in self
            .actions
            .iter_mut()
            .filter(|(known, _)| *known == action)
        {
            if now_held && !state.held {
                state.just_pressed = true;
            }
            if !now_held && state.held {
                state.just_released = true;
            }
            state.held = now_held;
        }
    }

    /// Whether any binding for this action is physically down.
    fn any_down_for(&self, action: A) -> bool {
        self.bindings
            .iter()
            .filter(|(_, bound)| *bound == action)
            .any(|(binding, _)| self.down.binary_search(binding).is_ok())
    }

    /// End the tick: edges are cleared, holds are kept.
    ///
    /// **A tap that pressed and released within one tick keeps both edges
    /// until this is called**, so a fast input is seen late rather than
    /// missed. That is the reason edges live on the tick rather than on
    /// the event.
    pub fn advance(&mut self) {
        for (_, state) in &mut self.actions {
            state.just_pressed = false;
            state.just_released = false;
        }
    }

    /// This action's full state.
    #[must_use]
    pub fn state(&self, action: A) -> ActionState {
        self.actions
            .iter()
            .find(|(known, _)| *known == action)
            .map(|(_, state)| *state)
            .unwrap_or_default()
    }

    /// Whether the action is held.
    #[must_use]
    pub fn held(&self, action: A) -> bool {
        self.state(action).held
    }

    /// Whether the action became held this tick.
    #[must_use]
    pub fn just_pressed(&self, action: A) -> bool {
        self.state(action).just_pressed
    }

    /// Whether the action stopped being held this tick.
    #[must_use]
    pub fn just_released(&self, action: A) -> bool {
        self.state(action).just_released
    }

    /// Forget all physical state, keeping bindings.
    ///
    /// For focus loss: the OS stops delivering key-up for keys released
    /// while another window has focus, so without this a player who
    /// alt-tabs mid-jump comes back still jumping. Releases are reported
    /// as edges, so a system watching `just_released` sees them.
    pub fn release_all(&mut self) {
        self.down.clear();
        for (_, state) in &mut self.actions {
            if state.held {
                state.just_released = true;
            }
            state.held = false;
        }
    }
}
