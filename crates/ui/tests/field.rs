//! Text fields: the pool, the editing, and the fingerprint.

use renew_ui::{
    EditOp, Fixed, MAX_FIELD_BYTES, MAX_FIELDS, Size, Style, Ui, UiEvent, UiLimits, UiRefused,
};

fn tree(nodes: u32) -> Ui {
    Ui::new(UiLimits { nodes })
}

/// A field that holds focus, so typing reaches it.
///
/// **Focus is taken by clicking, because the tree has no other way to
/// give it.** There is no test-only door here on purpose: a fixture that
/// set focus directly would stop exercising the one path a real caller
/// has, and would keep passing if that path broke.
#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn focused_field(ui: &mut Ui) -> renew_ui::NodeId {
    let root = ui.root();
    let node = ui.insert(root).expect("room for one node");
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
    let x = rect.x.trunc_int() + 1;
    let y = rect.y.trunc_int() + 1;
    let (x, y) = (i32::try_from(x).unwrap_or(0), i32::try_from(y).unwrap_or(0));
    ui.handle(UiEvent::PointerMoved { x, y });
    ui.handle(UiEvent::PointerPressed);
    ui.handle(UiEvent::PointerReleased);
    assert_eq!(ui.focus(), Some(node), "the click must have given it focus");
    // The activation it produced is not this test's subject; drained so
    // it cannot be mistaken for one later.
    let _ = ui.drain_outputs().count();
    node
}

#[test]
fn a_field_starts_empty_and_a_plain_node_is_not_one() {
    let mut ui = tree(4);
    let root = ui.root();
    let node = ui.insert(root).expect("room");
    assert_eq!(
        ui.field_text(node),
        None,
        "a node is not a field until asked"
    );
    ui.make_field(node).expect("a free slot");
    assert_eq!(ui.field_text(node), Some(&[][..]));
    assert_eq!(ui.field_cursor(node), Some(0));
}

#[test]
fn making_a_field_twice_keeps_what_was_typed() {
    // A caller rebuilding a screen must not wipe a half-typed address.
    let mut ui = tree(4);
    let node = focused_field(&mut ui);
    ui.handle(UiEvent::TextEntered { ch: u32::from('a') });
    ui.make_field(node).expect("idempotent");
    assert_eq!(ui.field_text(node), Some(&b"a"[..]));
}

#[test]
fn the_pool_fills_and_then_refuses_by_name() {
    let mut ui = tree(32);
    let root = ui.root();
    for _ in 0..MAX_FIELDS {
        let node = ui.insert(root).expect("room");
        ui.make_field(node).expect("a free slot");
    }
    let extra = ui.insert(root).expect("room");
    assert_eq!(ui.make_field(extra), Err(UiRefused::Full));
}

#[test]
fn typing_lands_in_the_focused_field_and_nowhere_else() {
    let mut ui = tree(8);
    let root = ui.root();
    let other = ui.insert(root).expect("room");
    ui.make_field(other).expect("slot");
    let node = focused_field(&mut ui);

    for ch in "hi".chars() {
        ui.handle(UiEvent::TextEntered { ch: u32::from(ch) });
    }
    assert_eq!(ui.field_text(node), Some(&b"hi"[..]));
    assert_eq!(
        ui.field_text(other),
        Some(&[][..]),
        "an unfocused field must not hear a keystroke"
    );
}

#[test]
fn editing_moves_and_removes_whole_characters() {
    let mut ui = tree(4);
    let node = focused_field(&mut ui);
    for ch in "abc".chars() {
        ui.handle(UiEvent::TextEntered { ch: u32::from(ch) });
    }
    ui.handle(UiEvent::Edit { op: EditOp::Left });
    ui.handle(UiEvent::TextEntered { ch: u32::from('X') });
    assert_eq!(ui.field_text(node), Some(&b"abXc"[..]));

    ui.handle(UiEvent::Edit {
        op: EditOp::Backspace,
    });
    assert_eq!(ui.field_text(node), Some(&b"abc"[..]));
    ui.handle(UiEvent::Edit { op: EditOp::Home });
    ui.handle(UiEvent::Edit { op: EditOp::Delete });
    assert_eq!(ui.field_text(node), Some(&b"bc"[..]));
}

#[test]
fn a_multi_byte_character_is_removed_whole() {
    // Backspacing one byte out of a multi-byte scalar would leave the
    // field holding something that is not text, which the accessor
    // promises it never does.
    let mut ui = tree(4);
    let node = focused_field(&mut ui);
    ui.handle(UiEvent::TextEntered {
        ch: u32::from('é')
    });
    assert_eq!(ui.field_text(node).map(<[u8]>::len), Some(2));
    ui.handle(UiEvent::Edit {
        op: EditOp::Backspace,
    });
    assert_eq!(ui.field_text(node), Some(&[][..]));

    // And the same for stepping over it.
    ui.handle(UiEvent::TextEntered {
        ch: u32::from('é')
    });
    ui.handle(UiEvent::Edit { op: EditOp::Left });
    assert_eq!(
        ui.field_cursor(node),
        Some(0),
        "left steps the whole character"
    );

    // Forward over it too. That is a separate walk in the code, and it
    // was uncovered until this line existed — the earlier attempt to add
    // it silently edited nothing, and the coverage report is what said
    // so.
    ui.handle(UiEvent::Edit { op: EditOp::Right });
    assert_eq!(
        ui.field_cursor(node),
        Some(2),
        "right steps the whole character too"
    );
    ui.handle(UiEvent::Edit { op: EditOp::Home });
    ui.handle(UiEvent::Edit { op: EditOp::Delete });
    assert_eq!(
        ui.field_text(node),
        Some(&[][..]),
        "delete removes it whole as well"
    );
}

#[test]
fn a_full_field_refuses_rather_than_truncating() {
    let mut ui = tree(4);
    let node = focused_field(&mut ui);
    for _ in 0..MAX_FIELD_BYTES + 8 {
        ui.handle(UiEvent::TextEntered { ch: u32::from('x') });
    }
    assert_eq!(ui.field_text(node).map(<[u8]>::len), Some(MAX_FIELD_BYTES));

    // A two-byte character with one byte free must not take half.
    ui.handle(UiEvent::Edit {
        op: EditOp::Backspace,
    });
    ui.handle(UiEvent::TextEntered {
        ch: u32::from('é')
    });
    let text = ui.field_text(node).expect("a field");
    assert_eq!(
        text.len(),
        MAX_FIELD_BYTES - 1,
        "half a character was taken"
    );
    assert!(
        core::str::from_utf8(text).is_ok(),
        "a field must always be text"
    );
}

#[test]
fn text_reaches_the_fingerprint_and_a_no_op_does_not() {
    use renew_frame::StateHash;
    let mut ui = tree(4);
    focused_field(&mut ui);
    let before = ui.absorb(StateHash::new());

    ui.handle(UiEvent::TextEntered { ch: u32::from('a') });
    let typed = ui.absorb(StateHash::new());
    assert_ne!(
        before, typed,
        "a replay that typed a different address must not digest the same"
    );

    // A left arrow at the start moves nothing, so it must move no
    // fingerprint either — otherwise two runs reaching one field's
    // contents by different amounts of cursor-bumping would disagree.
    ui.handle(UiEvent::Edit { op: EditOp::Home });
    let idle = ui.absorb(StateHash::new());
    ui.handle(UiEvent::Edit { op: EditOp::Left });
    assert_eq!(idle, ui.absorb(StateHash::new()));
}

#[test]
fn typing_with_no_focus_is_heard_and_ignored() {
    let mut ui = tree(4);
    let root = ui.root();
    let node = ui.insert(root).expect("room");
    ui.make_field(node).expect("slot");
    ui.handle(UiEvent::TextEntered { ch: u32::from('a') });
    ui.handle(UiEvent::Edit {
        op: EditOp::Backspace,
    });
    assert_eq!(ui.field_text(node), Some(&[][..]));
}

#[test]
fn a_value_that_is_not_a_scalar_is_ignored() {
    let mut ui = tree(4);
    let node = focused_field(&mut ui);
    // A surrogate half. It is not a character and never will be.
    ui.handle(UiEvent::TextEntered { ch: 0xD800 });
    assert_eq!(ui.field_text(node), Some(&[][..]));
}

#[test]
fn right_and_end_move_the_cursor_and_stop_at_the_end() {
    let mut ui = tree(4);
    let node = focused_field(&mut ui);
    for ch in "ab".chars() {
        ui.handle(UiEvent::TextEntered { ch: u32::from(ch) });
    }
    ui.handle(UiEvent::Edit { op: EditOp::Home });
    ui.handle(UiEvent::Edit { op: EditOp::Right });
    assert_eq!(ui.field_cursor(node), Some(1));
    ui.handle(UiEvent::Edit { op: EditOp::End });
    assert_eq!(ui.field_cursor(node), Some(2));
    // Past the end is a no-op, not a wrap and not a panic.
    ui.handle(UiEvent::Edit { op: EditOp::Right });
    assert_eq!(ui.field_cursor(node), Some(2));
    ui.handle(UiEvent::Edit { op: EditOp::Delete });
    assert_eq!(ui.field_text(node), Some(&b"ab"[..]));
}

#[test]
fn backspace_at_the_start_does_nothing() {
    // The guard that stops a cursor at zero walking off the front. It is
    // reachable, unlike its neighbours in that function, and it was
    // uncovered until this test — every other backspace in the file has
    // something in front of it.
    let mut ui = tree(4);
    let node = focused_field(&mut ui);
    ui.handle(UiEvent::Edit {
        op: EditOp::Backspace,
    });
    assert_eq!(ui.field_text(node), Some(&[][..]));

    ui.handle(UiEvent::TextEntered { ch: u32::from('a') });
    ui.handle(UiEvent::Edit { op: EditOp::Home });
    ui.handle(UiEvent::Edit {
        op: EditOp::Backspace,
    });
    assert_eq!(
        ui.field_text(node),
        Some(&b"a"[..]),
        "backspace at the start must not eat the character after the cursor"
    );
}

#[test]
fn typing_into_a_focused_node_that_is_not_a_field_does_nothing() {
    // Focus is given by clicking anything, not only a field, so this is
    // the ordinary case of a player clicking a button and then typing.
    // It must be silent rather than land somewhere.
    let mut ui = tree(8);
    let field = focused_field(&mut ui);
    let root = ui.root();
    let button = ui.insert(root).expect("room");
    ui.set_style(
        button,
        Style {
            width: Size::Px(Fixed::from_int(40)),
            height: Size::Px(Fixed::from_int(20)),
            ..Style::default()
        },
    );
    ui.solve(Fixed::from_int(100), Fixed::from_int(100));
    let rect = ui.rect(button).expect("a solved node has a box");
    let x = i32::try_from(rect.x.trunc_int() + 1).unwrap_or(0);
    let y = i32::try_from(rect.y.trunc_int() + 1).unwrap_or(0);
    ui.handle(UiEvent::PointerMoved { x, y });
    ui.handle(UiEvent::PointerPressed);
    ui.handle(UiEvent::PointerReleased);
    assert_eq!(
        ui.focus(),
        Some(button),
        "the click moved focus off the field"
    );

    ui.handle(UiEvent::TextEntered { ch: u32::from('z') });
    ui.handle(UiEvent::Edit {
        op: EditOp::Backspace,
    });
    assert_eq!(
        ui.field_text(field),
        Some(&[][..]),
        "a keystroke must not reach a field that lost focus"
    );
}

#[test]
fn removing_a_node_releases_its_field_slot() {
    // The pool is eight slots and a screen gets rebuilt. If a removed
    // node kept its slot, a form torn down and rebuilt eight times would
    // refuse the ninth field with no node holding any of them — a leak
    // that looks like a capacity that was too small.
    let mut ui = tree(64);
    let root = ui.root();
    for round in 0..MAX_FIELDS * 3 {
        let node = ui.insert(root).expect("room");
        ui.make_field(node)
            .unwrap_or_else(|_| panic!("the pool leaked by round {round}"));
        assert!(ui.remove(node), "the node must go away");
    }
}

#[test]
fn a_stale_id_never_names_a_live_field() {
    // Removing a node bumps its slot's generation, so every id that
    // named it goes stale. A field slot still holding the old id must
    // not answer to the new tenant, which would hand one node another
    // node's typed text.
    let mut ui = tree(8);
    let root = ui.root();
    let first = ui.insert(root).expect("room");
    ui.make_field(first).expect("slot");
    assert!(ui.remove(first));

    let second = ui.insert(root).expect("room");
    assert_ne!(first, second, "the reused slot must carry a new generation");
    assert_eq!(
        ui.field_text(first),
        None,
        "a stale id must not reach a field"
    );
    assert_eq!(
        ui.field_text(second),
        None,
        "and the new tenant is not a field until it asks"
    );
}
