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
fn making_a_field_twice_claims_one_slot() {
    // The documented idempotence, tested for *contents* and not for the
    // slot. Deleting the early return leaves every test green —
    // `field_slot` finds the first duplicate, so the text looks right
    // while a slot has quietly gone. A caller that rebuilds its screen
    // calls `make_field` on the same node each pass and exhausts the
    // pool on the eighth rebuild with one field on screen.
    let mut ui = tree(32);
    let root = ui.root();
    let node = ui.insert(root).expect("room");
    for _ in 0..MAX_FIELDS * 2 {
        ui.make_field(node).expect("idempotent, so always free");
    }
    for round in 1..MAX_FIELDS {
        let other = ui.insert(root).expect("room");
        ui.make_field(other)
            .unwrap_or_else(|_| panic!("re-claiming ate slot {round}"));
    }
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

#[test]
fn removing_a_parent_releases_its_childrens_field_slots() {
    // The direct release missed this case:
    // removing a panel ends the field inside it, and the field's slot
    // has to go back too. Chasing every path that can end a node's life
    // is a losing game — removing a parent is one, and the next kind of
    // removal will be another — so the pool reclaims by asking the arena
    // who is still alive.
    let mut ui = tree(64);
    let root = ui.root();
    for round in 0..MAX_FIELDS * 3 {
        let panel = ui.insert(root).expect("room");
        let inner = ui.insert(panel).expect("room");
        ui.make_field(inner)
            .unwrap_or_else(|_| panic!("the pool leaked through a parent by round {round}"));
        assert!(
            ui.remove(panel),
            "the panel must go away, and the field with it"
        );
    }
}

#[test]
fn a_field_whose_parent_went_away_is_not_readable() {
    let mut ui = tree(16);
    let root = ui.root();
    let panel = ui.insert(root).expect("room");
    let inner = ui.insert(panel).expect("room");
    ui.make_field(inner).expect("slot");
    assert!(ui.remove(panel));
    assert_eq!(
        ui.field_text(inner),
        None,
        "a field inside a removed panel must not still answer"
    );
}

#[test]
fn two_different_texts_never_share_a_fingerprint() {
    // **The collision a review found, pinned.** The first fold rotated
    // and added, which is affine in the token: `rot7(x + 1)` is
    // `rot7(x) + 128`, so a token one larger and a later token 128
    // smaller cancelled exactly. Typing "aÈ" and typing "bH" produced
    // one digest — and a fingerprint two different texts share is worse
    // than no fingerprint, because everything downstream trusts it.
    use renew_frame::StateHash;
    let digest_of = |text: &str| {
        let mut ui = tree(4);
        let _ = focused_field(&mut ui);
        for ch in text.chars() {
            ui.handle(UiEvent::TextEntered { ch: u32::from(ch) });
        }
        ui.absorb(StateHash::new()).finish()
    };
    assert_ne!(
        digest_of("aÈ"),
        digest_of("bH"),
        "the exact pair the affine fold collided"
    );
    assert_ne!(
        digest_of("ab"),
        digest_of("ba"),
        "order is part of the record"
    );
    // **The chain, not just the last token.** Dropping the previous fold
    // from the chain leaves the whole workspace green, because a digest
    // then records only the final keystroke — and the leg above cannot
    // see it, since "ab" and "ba" end in different characters, so it
    // merely repeats the check below. Two strings ending alike are what
    // test the chain.
    assert_ne!(
        digest_of("ab"),
        digest_of("cb"),
        "a fold that forgets its history keeps only the last keystroke"
    );
    assert_ne!(digest_of("a"), digest_of("b"), "so is what was typed");
    assert_eq!(
        digest_of("hello"),
        digest_of("hello"),
        "and it is a function"
    );
}

#[test]
fn one_keystroke_into_two_different_fields_is_two_histories() {
    // **Both runs end focused on the same node**, which is the whole
    // difficulty: `absorb` folds the focus too, so a naive comparison
    // passes whether or not the edit fold knows which field was typed
    // into. The first version of this test did exactly that and could
    // not fail. Here the only difference is *where the keystroke went*.
    use renew_frame::StateHash;
    let digest_of = |into_second: bool| {
        let mut ui = tree(8);
        let first = focused_field(&mut ui);
        let root = ui.root();
        let second = ui.insert(root).expect("room");
        ui.make_field(second).expect("slot");
        ui.set_style(
            second,
            Style {
                width: Size::Px(Fixed::from_int(40)),
                height: Size::Px(Fixed::from_int(20)),
                ..Style::default()
            },
        );
        ui.solve(Fixed::from_int(100), Fixed::from_int(100));
        let click_second = |ui: &mut Ui| {
            let rect = ui.rect(second).expect("a box");
            let x = i32::try_from(rect.x.trunc_int() + 1).unwrap_or(0);
            let y = i32::try_from(rect.y.trunc_int() + 1).unwrap_or(0);
            ui.handle(UiEvent::PointerMoved { x, y });
            ui.handle(UiEvent::PointerPressed);
            ui.handle(UiEvent::PointerReleased);
            let _ = ui.drain_outputs().count();
        };
        if into_second {
            click_second(&mut ui);
            ui.handle(UiEvent::TextEntered { ch: u32::from('a') });
        } else {
            assert_eq!(ui.focus(), Some(first));
            ui.handle(UiEvent::TextEntered { ch: u32::from('a') });
            click_second(&mut ui);
        }
        assert_eq!(ui.focus(), Some(second), "both runs must end alike");
        ui.absorb(StateHash::new()).finish()
    };
    assert_ne!(
        digest_of(false),
        digest_of(true),
        "the same keystroke into two different fields must be two histories"
    );
}

/// Every editing operation, in the state where it does nothing.
const ALL_OPS: [EditOp; 6] = [
    EditOp::Backspace,
    EditOp::Delete,
    EditOp::Left,
    EditOp::Right,
    EditOp::Home,
    EditOp::End,
];

#[test]
fn an_operation_that_changes_nothing_moves_no_fingerprint() {
    // **An empty field makes all six no-ops at once**: the cursor is at
    // zero and at the end simultaneously, so every arrow has nowhere to
    // go and both deletions have nothing to reach.
    //
    // The rule this pins is not tidiness. Two runs that reached the same
    // text — one of them bumping a cursor against the start a few times
    // on the way — must fingerprint alike, or the fold is recording the
    // player's fidgeting instead of the field's contents. Five of these
    // six determinations were untested; making any of them return true
    // unconditionally now moves a digest that must not move.
    use renew_frame::StateHash;
    for op in ALL_OPS {
        let mut ui = tree(4);
        let _ = focused_field(&mut ui);
        let before = ui.absorb(StateHash::new()).finish();
        ui.handle(UiEvent::Edit { op });
        assert_eq!(
            ui.absorb(StateHash::new()).finish(),
            before,
            "{op:?} did nothing to an empty field but moved the fingerprint"
        );
    }
}

#[test]
fn an_operation_that_changes_something_moves_the_fingerprint() {
    // The other half, so the test above cannot pass by the tree having
    // stopped listening: each op is put where it genuinely does
    // something, and then it has to show.
    use renew_frame::StateHash;
    // `Home` first for the ops that need the cursor away from the end.
    let setups: [(EditOp, bool); 6] = [
        (EditOp::Backspace, false),
        (EditOp::Delete, true),
        (EditOp::Left, false),
        (EditOp::Right, true),
        (EditOp::Home, false),
        (EditOp::End, true),
    ];
    for (op, from_start) in setups {
        let mut ui = tree(4);
        let _ = focused_field(&mut ui);
        for ch in "ab".chars() {
            ui.handle(UiEvent::TextEntered { ch: u32::from(ch) });
        }
        if from_start {
            ui.handle(UiEvent::Edit { op: EditOp::Home });
        }
        let before = ui.absorb(StateHash::new()).finish();
        ui.handle(UiEvent::Edit { op });
        assert_ne!(
            ui.absorb(StateHash::new()).finish(),
            before,
            "{op:?} changed the field but the fingerprint did not notice"
        );
    }
}

#[test]
fn the_pool_costs_what_the_documentation_says() {
    // The first figure in the docs was 512, which counted the bytes
    // alone and forgot that a field also carries an owner, a length and
    // a cursor. The number is now the compiler's, and this is where a
    // reader can see it.
    assert_eq!(
        renew_ui::POOL_BYTES,
        768,
        "the pool size moved; update the module doc with it"
    );
}

#[test]
fn a_reclaimed_slot_arrives_empty() {
    // **This must reach the reclaim path, and the first version did
    // not.** It removed the field's own node, which used to clear the
    // slot eagerly, so the reclaim's dead-owner branch never fired and
    // weakening that branch left the test green — vacuous, in the file
    // added to close a vacuous-test finding.
    //
    // Removing an *ancestor* is what leaves a slot owned by a dead node
    // for the reclaim to find, so that is what this does.
    let mut ui = tree(64);
    let root = ui.root();
    let panel = ui.insert(root).expect("room");
    let inner = ui.insert(panel).expect("room");
    ui.make_field(inner).expect("slot");
    // Focus it and type, so the slot holds bytes worth inheriting.
    ui.set_style(
        inner,
        Style {
            width: Size::Px(Fixed::from_int(40)),
            height: Size::Px(Fixed::from_int(20)),
            ..Style::default()
        },
    );
    ui.solve(Fixed::from_int(100), Fixed::from_int(100));
    let rect = ui.rect(inner).expect("a box");
    let x = i32::try_from(rect.x.trunc_int() + 1).unwrap_or(0);
    let y = i32::try_from(rect.y.trunc_int() + 1).unwrap_or(0);
    ui.handle(UiEvent::PointerMoved { x, y });
    ui.handle(UiEvent::PointerPressed);
    ui.handle(UiEvent::PointerReleased);
    let _ = ui.drain_outputs().count();
    ui.handle(UiEvent::TextEntered { ch: u32::from('s') });
    ui.handle(UiEvent::TextEntered { ch: u32::from('e') });
    assert_eq!(
        ui.field_text(inner),
        Some(&b"se"[..]),
        "the fixture must type"
    );

    // The ancestor goes, taking the field's node with it and leaving the
    // slot owned by something no longer alive.
    assert!(ui.remove(panel));

    // Every slot the pool hands out from here must be empty, and the
    // first of them is the reclaimed one.
    for round in 0..MAX_FIELDS {
        let node = ui.insert(root).expect("room");
        ui.make_field(node)
            .unwrap_or_else(|_| panic!("the pool leaked by round {round}"));
        assert_eq!(
            ui.field_text(node),
            Some(&[][..]),
            "round {round} inherited a dead field's typing"
        );
    }
}

#[test]
fn left_and_right_fold_different_tokens() {
    // What this catches is the two folding *alike*. What it cannot catch
    // is the two being **exchanged**, and that is worth stating rather
    // than attempting again: a swap preserves distinctness, so no test
    // comparing digests to each other can see one. Exchanging two tokens
    // changes a fingerprint only against a previously recorded value —
    // which nothing here catches: the cross-target
    // lane compares its legs to each other, not to a recorded value, so
    // a swap moves all of them alike. Stated in the codes themselves. Two attempts were spent here before the shape of
    // the mutation was read properly.
    use renew_frame::StateHash;
    let digest_after = |op: EditOp| {
        let mut ui = tree(4);
        let _ = focused_field(&mut ui);
        for ch in "abc".chars() {
            ui.handle(UiEvent::TextEntered { ch: u32::from(ch) });
        }
        ui.handle(UiEvent::Edit { op: EditOp::Home });
        ui.handle(UiEvent::Edit { op: EditOp::Right });
        // Cursor is at 1 of 3: both directions are a real change, and
        // each folds exactly one token more than the other.
        ui.handle(UiEvent::Edit { op });
        ui.absorb(StateHash::new()).finish()
    };
    assert_ne!(
        digest_after(EditOp::Left),
        digest_after(EditOp::Right),
        "a replay cannot tell the two apart if they fold alike"
    );
}

#[test]
fn a_control_character_and_an_edit_key_never_fold_alike() {
    // **The second collision a review found**, after the first was
    // fixed. An edit operation's code is a small integer and so is a
    // control character's scalar, so the two shared a namespace: typing
    // U+0003 and pressing Left folded the same number. Windows delivers
    // U+0001..U+001A for Ctrl with a letter, so it is a keystroke a
    // player produces by accident, not an exotic input.
    //
    // The exact pair from the report: both runs accept both events, so
    // every other counter matches and only the fold could tell them
    // apart. It could not.
    use renew_frame::StateHash;
    let mut typed = tree(4);
    let node_a = focused_field(&mut typed);
    typed.handle(UiEvent::TextEntered { ch: u32::from('a') });
    typed.handle(UiEvent::TextEntered { ch: 3 });

    let mut edited = tree(4);
    let node_b = focused_field(&mut edited);
    edited.handle(UiEvent::TextEntered { ch: u32::from('a') });
    edited.handle(UiEvent::Edit { op: EditOp::Left });

    // Materially different: different bytes and a different cursor, both
    // of which change what happens next.
    assert_eq!(typed.field_text(node_a).map(<[u8]>::len), Some(2));
    assert_eq!(edited.field_text(node_b).map(<[u8]>::len), Some(1));
    assert_ne!(typed.field_cursor(node_a), edited.field_cursor(node_b));

    assert_ne!(
        typed.absorb(StateHash::new()).finish(),
        edited.absorb(StateHash::new()).finish(),
        "a typed control character and an edit key shared a fingerprint"
    );
}

#[test]
fn a_stale_node_cannot_become_a_field() {
    // The `# Errors` contract of a new public API, which nothing checked
    // — deleting the liveness guard entirely left every test green, and
    // the arm was uncovered besides. A caller holding an id across a
    // rebuild is the ordinary way to reach this.
    let mut ui = tree(32);
    let root = ui.root();
    let node = ui.insert(root).expect("room");
    assert!(ui.remove(node));
    assert_eq!(
        ui.make_field(node),
        Err(UiRefused::MissingParent),
        "a removed node must not be able to claim a slot"
    );
    // And it claimed nothing on the way out: the pool is untouched, so a
    // refusal cannot leak a slot.
    for _ in 0..MAX_FIELDS {
        let live = ui.insert(root).expect("room");
        ui.make_field(live).expect("every slot is still free");
    }
}
