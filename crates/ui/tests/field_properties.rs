//! The field pool's invariants, over inputs nobody chose.
//!
//! §8's containers row asks for these, and the field pool shipped its
//! first revision without them. A review then walked forty thousand
//! pseudo-random operations by hand and found the invariants held —
//! which established they were true that afternoon and guarded nothing
//! afterwards. This is the guard.

use proptest::prelude::*;
use renew_ui::{EditOp, Fixed, MAX_FIELD_BYTES, NodeId, Size, Style, Ui, UiEvent, UiLimits};

/// One thing a player can do to a field.
#[derive(Clone, Copy, Debug)]
enum Act {
    Type(char),
    Edit(EditOp),
}

fn acts() -> impl Strategy<Value = Vec<Act>> {
    let op = prop_oneof![
        Just(EditOp::Backspace),
        Just(EditOp::Delete),
        Just(EditOp::Left),
        Just(EditOp::Right),
        Just(EditOp::Home),
        Just(EditOp::End),
    ];
    // A spread of widths on purpose: one, two, three and four bytes, so
    // the character walks are exercised rather than the ASCII path only.
    let ch = prop_oneof![
        proptest::char::range('a', 'z'),
        Just('é'),
        Just('☃'),
        Just('𝄞'),
    ];
    prop::collection::vec(
        prop_oneof![ch.prop_map(Act::Type), op.prop_map(Act::Edit)],
        0..200,
    )
}

#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn focused_field(ui: &mut Ui) -> NodeId {
    let root = ui.root();
    let node = ui.insert(root).expect("room");
    ui.make_field(node).expect("a free slot");
    ui.set_style(
        node,
        Style {
            width: Size::Px(Fixed::from_int(40)),
            height: Size::Px(Fixed::from_int(20)),
            ..Style::default()
        },
    );
    ui.solve(Fixed::from_int(100), Fixed::from_int(100));
    let rect = ui.rect(node).expect("a solved field has a box");
    let x = i32::try_from(rect.x.trunc_int() + 1).unwrap_or(0);
    let y = i32::try_from(rect.y.trunc_int() + 1).unwrap_or(0);
    ui.handle(UiEvent::PointerMoved { x, y });
    ui.handle(UiEvent::PointerPressed);
    ui.handle(UiEvent::PointerReleased);
    let _ = ui.drain_outputs().count();
    node
}

fn apply(ui: &mut Ui, act: Act) {
    match act {
        Act::Type(ch) => ui.handle(UiEvent::TextEntered { ch: u32::from(ch) }),
        Act::Edit(op) => ui.handle(UiEvent::Edit { op }),
    }
}

proptest! {
    /// A field holds text, always. The accessor says so in prose and
    /// every consumer that draws or measures one depends on it.
    #[test]
    fn a_field_is_always_valid_text(script in acts()) {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let node = focused_field(&mut ui);
        for act in script {
            apply(&mut ui, act);
            let bytes = ui.field_text(node).unwrap_or_default();
            prop_assert!(core::str::from_utf8(bytes).is_ok());
            prop_assert!(bytes.len() <= MAX_FIELD_BYTES);
        }
    }

    /// The cursor never leaves the buffer, and never lands inside a
    /// character. An insertion at a byte that is not a boundary would
    /// split a scalar in half.
    #[test]
    fn the_cursor_stays_on_a_character_boundary(script in acts()) {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let node = focused_field(&mut ui);
        for act in script {
            apply(&mut ui, act);
            let bytes = ui.field_text(node).unwrap_or_default();
            let cursor = usize::from(ui.field_cursor(node).unwrap_or(0));
            prop_assert!(cursor <= bytes.len(), "cursor {cursor} past {}", bytes.len());
            let text = core::str::from_utf8(bytes).unwrap_or_default();
            prop_assert!(text.is_char_boundary(cursor));
        }
    }

    /// Typing one character and backspacing it is identity — whatever
    /// the field held before, and wherever the cursor was.
    #[test]
    fn typing_then_backspacing_restores_the_field(script in acts(), ch in prop_oneof![
        proptest::char::range('a', 'z'), Just('é'), Just('☃'), Just('𝄞')
    ]) {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let node = focused_field(&mut ui);
        for act in script {
            apply(&mut ui, act);
        }
        let before = ui.field_text(node).unwrap_or_default().to_vec();
        let cursor_before = ui.field_cursor(node).unwrap_or(0);

        ui.handle(UiEvent::TextEntered { ch: u32::from(ch) });
        // Only when it fit: a refused insertion is not an insertion, and
        // a backspace after one would eat a character that was there
        // first. That asymmetry is the behaviour, not a bug in it.
        if ui.field_text(node).unwrap_or_default() != before.as_slice() {
            ui.handle(UiEvent::Edit { op: EditOp::Backspace });
            prop_assert_eq!(ui.field_text(node).unwrap_or_default(), before.as_slice());
            prop_assert_eq!(ui.field_cursor(node).unwrap_or(0), cursor_before);
        }
    }

    /// The same script twice is the same field and the same fingerprint.
    /// Determinism is the whole reason this crate is shaped as it is.
    #[test]
    fn one_script_is_one_outcome(script in acts()) {
        use renew_frame::StateHash;
        let run = |script: &[Act]| {
            let mut ui = Ui::new(UiLimits { nodes: 8 });
            let node = focused_field(&mut ui);
            for act in script {
                apply(&mut ui, *act);
            }
            (
                ui.field_text(node).unwrap_or_default().to_vec(),
                ui.absorb(StateHash::new()).finish(),
            )
        };
        prop_assert_eq!(run(&script), run(&script));
    }
}
