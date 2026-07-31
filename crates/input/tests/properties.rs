//! Property-based tests for the action state machine.
//!
//! The crate's example tests already pin each behaviour once. These say
//! the same things over generated event sequences, which is the shape the
//! interesting claims actually have: *binding order never reaches state*
//! and *an action is held while any of its bindings is* are statements
//! about all inputs, and an example can only ever be consistent with them.
//!
//! **The model is a sorted `Vec`, not a set.** `HashSet` is banned by the
//! crate's `clippy.toml` and the ban applies to tests too — which is
//! correct, and cheap to live with at this size.

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use renew_input::{Binding, InputMap};
use renew_platform::event::{KeyCode, WindowEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Jump,
    Left,
    Right,
}

/// The keys these properties draw from. Deliberately more keys than
/// actions, so several bindings land on one action and the OR matters.
const KEYS: [KeyCode; 6] = [
    KeyCode::Space,
    KeyCode::ArrowUp,
    KeyCode::ArrowLeft,
    KeyCode::KeyA,
    KeyCode::ArrowRight,
    KeyCode::KeyD,
];

const ACTIONS: [Action; 3] = [Action::Jump, Action::Left, Action::Right];

/// A binding table: which action each key drives, by index into `KEYS`.
fn table() -> impl Strategy<Value = Vec<usize>> {
    proptest::collection::vec(0usize..ACTIONS.len(), KEYS.len())
}

/// An event script: (key index, pressed).
fn script() -> impl Strategy<Value = Vec<(usize, bool)>> {
    proptest::collection::vec((0usize..KEYS.len(), any::<bool>()), 0..40)
}

fn build(table: &[usize]) -> InputMap<Action> {
    let mut input = InputMap::new();
    for (key, action) in KEYS.iter().zip(table) {
        input.bind(Binding::key(*key), ACTIONS[*action]);
    }
    input
}

fn key_event(index: usize, pressed: bool) -> WindowEvent {
    WindowEvent::Key {
        code: KEYS[index],
        pressed,
        repeat: false,
    }
}

/// The model: which key indices are physically down, ascending.
fn apply_to_model(model: &mut Vec<usize>, index: usize, pressed: bool) {
    match (pressed, model.binary_search(&index)) {
        (true, Err(at)) => model.insert(at, index),
        (false, Ok(at)) => {
            model.remove(at);
        }
        _ => {}
    }
}

fn states(input: &InputMap<Action>) -> Vec<bool> {
    ACTIONS.iter().map(|a| input.held(*a)).collect()
}

proptest! {
    // Fixed seed, matching every other property suite in the tree: the
    // same inputs are explored on every run and every machine, so a
    // failure reproduces from the message alone. Fresh exploration is a
    // deliberate act -- change the seed -- never an ambient one.
    #![proptest_config(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x0114_9075),
        ..ProptestConfig::default()
    })]

    /// The claim the sorted binding table exists to make: which order the
    /// bindings were registered in never reaches the resulting state.
    #[test]
    fn binding_order_never_reaches_state(table in table(), script in script(), rotate in 0usize..6) {
        let mut forward = build(&table);
        // The same table, registered starting from a different key.
        let mut rotated = InputMap::new();
        for offset in 0..KEYS.len() {
            let index = (offset + rotate) % KEYS.len();
            rotated.bind(Binding::key(KEYS[index]), ACTIONS[table[index]]);
        }

        for (index, pressed) in &script {
            forward.handle(key_event(*index, *pressed));
            rotated.handle(key_event(*index, *pressed));
        }
        prop_assert_eq!(states(&forward), states(&rotated));
    }

    /// An action is held exactly when at least one key bound to it is
    /// down. Checked against an independent model rather than against the
    /// map's own bookkeeping.
    #[test]
    fn an_action_is_held_iff_one_of_its_keys_is(table in table(), script in script()) {
        let mut input = build(&table);
        let mut down: Vec<usize> = Vec::new();

        for (index, pressed) in &script {
            input.handle(key_event(*index, *pressed));
            apply_to_model(&mut down, *index, *pressed);

            for (slot, action) in ACTIONS.iter().enumerate() {
                let expected = down.iter().any(|key| table[*key] == slot);
                prop_assert_eq!(
                    input.held(*action),
                    expected,
                    "action {:?} after {:?}",
                    action,
                    script
                );
            }
        }
    }

    /// Ending a tick retires both edges and disturbs no hold — the whole
    /// difference between an edge and a hold.
    #[test]
    fn advancing_clears_every_edge_and_moves_no_hold(table in table(), script in script()) {
        let mut input = build(&table);
        for (index, pressed) in &script {
            input.handle(key_event(*index, *pressed));
        }
        let before = states(&input);
        input.advance();
        prop_assert_eq!(states(&input), before);
        for action in ACTIONS {
            prop_assert!(!input.just_pressed(action));
            prop_assert!(!input.just_released(action));
        }
    }

    /// Focus loss always lands in the same place, whatever came before.
    #[test]
    fn release_all_leaves_nothing_held(table in table(), script in script()) {
        let mut input = build(&table);
        for (index, pressed) in &script {
            input.handle(key_event(*index, *pressed));
        }
        input.release_all();
        for action in ACTIONS {
            prop_assert!(!input.held(action));
        }
    }

    /// Re-sending an event the map has already absorbed changes nothing:
    /// a real event stream produces duplicates, and they must not
    /// re-fire an edge or disturb a hold.
    #[test]
    fn a_repeated_event_is_absorbed(table in table(), script in script(), again in 0usize..6) {
        let mut input = build(&table);
        for (index, pressed) in &script {
            input.handle(key_event(*index, *pressed));
        }
        input.advance();
        // Whatever the last event for this key said, say it again.
        let last = script.iter().rev().find(|(index, _)| *index == again);
        let Some((index, pressed)) = last.copied() else {
            return Ok(());
        };
        let before = states(&input);
        input.handle(key_event(index, pressed));
        prop_assert_eq!(states(&input), before);
        for action in ACTIONS {
            prop_assert!(!input.just_pressed(action));
            prop_assert!(!input.just_released(action));
        }
    }
}
