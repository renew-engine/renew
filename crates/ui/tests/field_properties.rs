//! The field pool's invariants, over inputs nobody chose.
//!
//! A container with editing operations wants these, and the field pool
//! shipped its first revision without them. Walking forty thousand
//! pseudo-random operations by hand establishes that the invariants held
//! that once; only a property test keeps establishing it.

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

/// An independent model of what a field should hold.
///
/// **The point of writing it twice.** The first version of this file
/// asserted invariants — valid text, cursor in range, cursor on a
/// boundary — and all four passed against an `insert`
/// gutted to do nothing at all: an empty field satisfies every one of
/// them. Invariants describe a shape, and the empty field has the right
/// shape. A model describes the *content*, and nothing vacuous agrees
/// with it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Model {
    text: String,
    cursor: usize,
}

impl Model {
    fn apply(&mut self, act: Act) {
        match act {
            Act::Type(ch) => {
                if self.text.len() + ch.len_utf8() <= MAX_FIELD_BYTES {
                    self.text.insert(self.cursor, ch);
                    self.cursor += ch.len_utf8();
                }
            }
            Act::Edit(EditOp::Home) => self.cursor = 0,
            Act::Edit(EditOp::End) => self.cursor = self.text.len(),
            Act::Edit(EditOp::Left) => self.cursor = self.prev(),
            Act::Edit(EditOp::Right) => self.cursor = self.next(),
            Act::Edit(EditOp::Backspace) => {
                let from = self.prev();
                if from != self.cursor {
                    self.text.replace_range(from..self.cursor, "");
                    self.cursor = from;
                }
            }
            Act::Edit(EditOp::Delete) => {
                let to = self.next();
                if to != self.cursor {
                    self.text.replace_range(self.cursor..to, "");
                }
            }
        }
    }

    fn prev(&self) -> usize {
        let mut at = self.cursor;
        while at > 0 {
            at -= 1;
            if self.text.is_char_boundary(at) {
                break;
            }
        }
        at
    }

    fn next(&self) -> usize {
        let mut at = self.cursor;
        while at < self.text.len() {
            at += 1;
            if self.text.is_char_boundary(at) {
                break;
            }
        }
        at
    }
}

proptest! {
    /// The field agrees with the model, byte for byte and cursor for
    /// cursor, after every single operation.
    ///
    /// This is the property the other three were meant to imply and did
    /// not. It fails immediately against an implementation that drops an
    /// insertion, ignores an edit, or moves a cursor differently.
    #[test]
    fn the_field_agrees_with_an_independent_model(script in acts()) {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let node = focused_field(&mut ui);
        let mut model = Model::default();
        for (step, act) in script.into_iter().enumerate() {
            apply(&mut ui, act);
            model.apply(act);
            let bytes = ui.field_text(node).unwrap_or_default();
            prop_assert_eq!(
                core::str::from_utf8(bytes).unwrap_or("<not text>"),
                model.text.as_str(),
                "diverged at step {} on {:?}", step, act
            );
            prop_assert_eq!(
                usize::from(ui.field_cursor(node).unwrap_or(0)),
                model.cursor,
                "cursor diverged at step {} on {:?}", step, act
            );
        }
    }

    /// Two scripts that produce different text must produce different
    /// fingerprints.
    ///
    /// Replaces a reflexivity check — `run(s) == run(s)` — that no
    /// deterministic implementation could fail and which therefore said
    /// nothing. The model decides when the two runs really differ, so
    /// this only demands a difference where one exists.
    #[test]
    fn different_text_means_different_fingerprints(left in acts(), right in acts()) {
        use renew_frame::StateHash;
        let run = |script: &[Act]| {
            let mut ui = Ui::new(UiLimits { nodes: 8 });
            let node = focused_field(&mut ui);
            let mut model = Model::default();
            for act in script {
                apply(&mut ui, *act);
                model.apply(*act);
            }
            let text = ui.field_text(node).unwrap_or_default().to_vec();
            (text, model, ui.absorb(StateHash::new()).finish())
        };
        let (text_l, model_l, digest_l) = run(&left);
        let (text_r, model_r, digest_r) = run(&right);
        prop_assert_eq!(&text_l, model_l.text.as_bytes());
        prop_assert_eq!(&text_r, model_r.text.as_bytes());
        if model_l != model_r {
            prop_assert_ne!(digest_l, digest_r, "two different fields shared a fingerprint");
        }
    }
}
