//! The action state machine, including the cases that make it worth
//! having a layer here at all rather than reading keys directly.

use renew_input::{ActionState, Binding, InputMap};
use renew_platform::event::{KeyCode, PointerButton, WindowEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Jump,
    Fire,
    Left,
}

fn key(code: KeyCode, pressed: bool) -> WindowEvent {
    WindowEvent::Key {
        code,
        pressed,
        repeat: false,
    }
}

fn mapped() -> InputMap<Action> {
    let mut input = InputMap::new();
    input.bind(Binding::key(KeyCode::Space), Action::Jump);
    input.bind(Binding::key(KeyCode::ArrowLeft), Action::Left);
    input.bind(Binding::pointer(PointerButton::Left), Action::Fire);
    input
}

#[test]
fn an_unbound_map_reports_nothing() {
    let input: InputMap<Action> = InputMap::new();
    assert!(input.is_empty());
    assert_eq!(input.len(), 0);
    assert_eq!(input.state(Action::Jump), ActionState::default());
    assert!(!input.held(Action::Jump));
    assert!(input.action_for(Binding::key(KeyCode::Space)).is_none());
}

#[test]
fn a_press_sets_the_edge_and_the_hold() {
    let mut input = mapped();
    input.handle(key(KeyCode::Space, true));
    assert_eq!(
        input.state(Action::Jump),
        ActionState {
            held: true,
            just_pressed: true,
            just_released: false
        }
    );
    // Unrelated actions are untouched.
    assert_eq!(input.state(Action::Left), ActionState::default());
}

#[test]
fn advancing_clears_the_edge_and_keeps_the_hold() {
    let mut input = mapped();
    input.handle(key(KeyCode::Space, true));
    input.advance();
    assert!(input.held(Action::Jump));
    assert!(!input.just_pressed(Action::Jump));

    input.handle(key(KeyCode::Space, false));
    assert!(!input.held(Action::Jump));
    assert!(input.just_released(Action::Jump));
    input.advance();
    assert!(!input.just_released(Action::Jump));
}

/// The case the whole per-tick design exists for: a tap inside one tick
/// must not be lost. A game that misses a fast input is worse than one
/// that sees it a tick late.
#[test]
fn a_tap_within_one_tick_reports_both_edges() {
    let mut input = mapped();
    input.handle(key(KeyCode::Space, true));
    input.handle(key(KeyCode::Space, false));
    let state = input.state(Action::Jump);
    assert!(state.just_pressed, "the press must survive the release");
    assert!(state.just_released);
    assert!(!state.held, "and it must not still be held");
}

/// A repeat is the OS restating what is already known. Letting one
/// through would fire `just_pressed` again in the middle of a hold.
#[test]
fn a_key_repeat_does_not_re_fire_the_press_edge() {
    let mut input = mapped();
    input.handle(key(KeyCode::Space, true));
    input.advance();
    input.handle(WindowEvent::Key {
        code: KeyCode::Space,
        pressed: true,
        repeat: true,
    });
    assert!(input.held(Action::Jump));
    assert!(!input.just_pressed(Action::Jump), "a repeat is not a press");
}

/// Two bindings for one action are an OR: it stays held while either is
/// down, and only releases when the last one goes.
#[test]
fn two_bindings_for_one_action_or_together() {
    let mut input = InputMap::new();
    input.bind(Binding::key(KeyCode::Space), Action::Jump);
    input.bind(Binding::key(KeyCode::ArrowUp), Action::Jump);
    assert_eq!(input.len(), 2);

    input.handle(key(KeyCode::Space, true));
    input.advance();
    input.handle(key(KeyCode::ArrowUp, true));
    assert!(input.held(Action::Jump));
    assert!(
        !input.just_pressed(Action::Jump),
        "already held; the second binding is not a new press"
    );

    input.advance();
    input.handle(key(KeyCode::Space, false));
    assert!(input.held(Action::Jump), "the other binding is still down");
    assert!(!input.just_released(Action::Jump));

    input.handle(key(KeyCode::ArrowUp, false));
    assert!(!input.held(Action::Jump));
    assert!(input.just_released(Action::Jump));
}

#[test]
fn rebinding_an_input_replaces_the_action_it_drives() {
    let mut input = mapped();
    let before = input.len();
    input.bind(Binding::key(KeyCode::Space), Action::Fire);
    assert_eq!(input.len(), before, "rebinding must not add a binding");
    assert_eq!(
        input.action_for(Binding::key(KeyCode::Space)),
        Some(Action::Fire)
    );

    input.handle(key(KeyCode::Space, true));
    assert!(input.held(Action::Fire));
    assert!(!input.held(Action::Jump));
}

#[test]
fn pointer_buttons_bind_like_keys() {
    let mut input = mapped();
    input.handle(WindowEvent::PointerButton {
        button: PointerButton::Left,
        pressed: true,
    });
    assert!(input.just_pressed(Action::Fire));
    input.handle(WindowEvent::PointerButton {
        button: PointerButton::Right,
        pressed: true,
    });
    assert!(
        input.held(Action::Fire),
        "an unbound button changes nothing"
    );
}

#[test]
fn events_that_are_not_input_are_ignored() {
    let mut input = mapped();
    for event in [
        WindowEvent::CloseRequested,
        WindowEvent::RedrawRequested,
        WindowEvent::Focused(true),
        WindowEvent::Resized {
            width: 1,
            height: 1,
        },
        WindowEvent::ScaleFactorChanged { scale: 2.0 },
        WindowEvent::PointerMoved { x: 1.0, y: 2.0 },
        WindowEvent::Wheel { dx: 0.0, dy: 1.0 },
    ] {
        input.handle(event);
    }
    assert_eq!(input.state(Action::Jump), ActionState::default());
    assert_eq!(input.state(Action::Fire), ActionState::default());
}

/// A duplicate press, and a release of something never pressed. Both are
/// things a real event stream produces and neither is an error.
#[test]
fn redundant_events_change_nothing() {
    let mut input = mapped();
    input.handle(key(KeyCode::Space, true));
    input.advance();
    input.handle(key(KeyCode::Space, true));
    assert!(!input.just_pressed(Action::Jump), "already down");

    input.handle(key(KeyCode::ArrowLeft, false));
    assert_eq!(input.state(Action::Left), ActionState::default());
}

/// Focus loss: the OS stops delivering key-up, so a player who alt-tabs
/// mid-jump would come back still jumping.
#[test]
fn releasing_everything_reports_edges_and_clears_holds() {
    let mut input = mapped();
    input.handle(key(KeyCode::Space, true));
    input.handle(key(KeyCode::ArrowLeft, true));
    input.advance();

    input.release_all();
    assert!(!input.held(Action::Jump));
    assert!(!input.held(Action::Left));
    assert!(
        input.just_released(Action::Jump),
        "a system watching releases must see it"
    );
    assert!(input.just_released(Action::Left));

    // And it is idempotent: nothing was held, so nothing is released.
    input.advance();
    input.release_all();
    assert!(!input.just_released(Action::Jump));
}

/// After a release-all, the physical state is genuinely forgotten — a
/// later release of a key the map thought was down must not resurrect an
/// edge.
#[test]
fn a_release_after_release_all_is_a_no_op() {
    let mut input = mapped();
    input.handle(key(KeyCode::Space, true));
    input.release_all();
    input.advance();
    input.handle(key(KeyCode::Space, false));
    assert_eq!(input.state(Action::Jump), ActionState::default());
}

/// The same event sequence produces the same state, every time. The
/// crate has no clock and no hashing, so this is a statement about the
/// absence of hidden inputs rather than about the state machine.
#[test]
fn the_same_events_produce_the_same_state() {
    let script = [
        key(KeyCode::Space, true),
        key(KeyCode::ArrowLeft, true),
        key(KeyCode::Space, false),
        key(KeyCode::ArrowLeft, false),
        key(KeyCode::Space, true),
    ];
    let run = || {
        let mut input = mapped();
        let mut seen = Vec::new();
        for (index, event) in script.iter().enumerate() {
            input.handle(*event);
            if index % 2 == 1 {
                input.advance();
            }
            seen.push((
                input.state(Action::Jump),
                input.state(Action::Left),
                input.state(Action::Fire),
            ));
        }
        seen
    };
    assert_eq!(run(), run());
}

/// Binding order does not reach the state, which is what the sorted
/// table exists to guarantee.
#[test]
fn binding_order_does_not_change_behaviour() {
    let build = |reverse: bool| {
        let mut input = InputMap::new();
        let pairs = [
            (Binding::key(KeyCode::Space), Action::Jump),
            (Binding::key(KeyCode::ArrowUp), Action::Jump),
            (Binding::key(KeyCode::ArrowLeft), Action::Left),
        ];
        if reverse {
            for (binding, action) in pairs.iter().rev() {
                input.bind(*binding, *action);
            }
        } else {
            for (binding, action) in &pairs {
                input.bind(*binding, *action);
            }
        }
        input.handle(key(KeyCode::ArrowUp, true));
        input.handle(key(KeyCode::ArrowLeft, true));
        (input.state(Action::Jump), input.state(Action::Left))
    };
    assert_eq!(build(false), build(true));
}

/// `Default` is what a struct holding a map will use, so it must agree
/// with `new` rather than merely compile.
#[test]
fn default_and_new_agree() {
    let made: InputMap<Action> = InputMap::default();
    assert!(made.is_empty());
    assert_eq!(made.len(), InputMap::<Action>::new().len());
    assert_eq!(made.state(Action::Jump), ActionState::default());
}
