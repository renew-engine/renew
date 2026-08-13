//! The widget tree's headless determinism scenario: one scripted menu
//! session, digested, printed as a single JSON line.
//!
//! This binary exists for the cross-platform comparison: three targets
//! run the same script and their digests are held against each other,
//! never against a committed constant — agreement between machines is
//! the only evidence that the machine did not matter. The script is
//! fixed in this file; changing it changes the digest on every target
//! at once, which the comparison forgives, and on one target alone,
//! which it exists to catch.
//!
//! The session exercises the surfaces a divergence could hide in: the
//! solver (mixed pixel and content sizes, growth, a mid-script restyle
//! whose re-solve moves a later click onto a different button),
//! hit-testing (moves across every button, a boundary-exact corner, a
//! miss outside the widget tree), and the decision fold (activations, an
//! abandoned press, a click on nothing). Unlike the tree's own
//! `absorb` — whose geometry exclusion serves gameplay gates — this
//! witness folds the solved rectangles too, to the raw fixed-point
//! bit: a lane scenario wants maximum sensitivity, and a one-unit
//! solver divergence must move this digest even though it would not
//! move a game's.

use renew_fixed::Fixed;
use renew_frame::StateHash;
use renew_ui::{Align, Direction, Edges, EditOp, NodeId, Size, Style, Ui, UiEvent, UiLimits};

fn main() {
    // The scenario takes no arguments; being handed one means the
    // pinned-run table drifted, and a drifted invocation must fail
    // rather than run a different scenario under the same name.
    if std::env::args().len() > 1 {
        eprintln!("ui_digest takes no arguments; the scenario is fixed in its source");
        std::process::exit(2);
    }
    let Some((digest, events, activations)) = run() else {
        // Unreachable while the file's own limits hold; if it ever
        // fires, a nonzero exit makes the lane leg fail by name
        // rather than contribute a vacuous digest that all targets
        // would agree on.
        eprintln!("the scenario could not build its own tree; nothing was digested");
        std::process::exit(1);
    };
    // One line, machine-readable, schema-versioned: the lane's whole
    // interface to this binary.
    println!(
        "{{\"schema_version\":1,\"sample\":\"ui\",\"script\":\"menu\",\"events\":{events},\"activations\":{activations},\"digest\":\"0x{digest:016x}\"}}"
    );
}

/// The typed half of the scenario, lifted out so `run` stays inside the
/// line limit the canonical lint enforces.
///
/// **This is the only part of the scenario that exercises text**, and it
/// covers both event kinds and the whole editing vocabulary. U+0003 is
/// here on purpose: Windows delivers it for Ctrl and a letter, and a
/// typed scalar once shared a token namespace with an operation code.
///
/// **What it does not do is catch an exchanged digest token**, and an
/// earlier version of this comment claimed it did. This lane holds its
/// legs against *each other*, never against a committed constant, so
/// swapping two operation codes moves every target's digest by the same
/// amount and the comparison still agrees. The claim was written into
/// two files and a commit message before anyone checked it against what
/// the lane compares. Nothing in this repository catches that mutation
/// today; saying so is worth more than a guard that is not one.
fn type_into(ui: &mut Ui, node: NodeId, mut hash: StateHash) -> Option<StateHash> {
    // An error rather than a skip: silently dropping these would leave
    // the reported event count claiming them.
    ui.make_field(node).ok()?;
    for &event in &TYPING {
        ui.handle(event);
        hash = ui.absorb(hash);
    }
    // **The typed events must have landed somewhere.** Without this the
    // whole block is decoration: replace the claim above with a no-op
    // and all ten events reach a node that is not a field, doing
    // nothing, while the reported count still says twenty-seven and
    // every cross-target leg still agrees. Anything that moves focus off
    // this node before the typing — a restyle, a layout change, a change
    // to focus-follows-activation — would empty the only part of this
    // scenario that exercises text, silently.
    //
    // The script leaves "hi" less its last character plus an inserted
    // one, so a non-empty field is the evidence the events were taken.
    let typed = ui.field_text(node)?;
    if typed.is_empty() {
        return None;
    }
    Some(hash)
}

/// The typed events, as a constant so `run` can count them without
/// holding the array.
const TYPING: [UiEvent; 10] = [
    UiEvent::TextEntered { ch: 0x68 },
    UiEvent::TextEntered { ch: 0x69 },
    UiEvent::TextEntered { ch: 3 },
    UiEvent::Edit { op: EditOp::Left },
    UiEvent::TextEntered { ch: 0xe9 },
    UiEvent::Edit { op: EditOp::Home },
    UiEvent::Edit { op: EditOp::Right },
    UiEvent::Edit { op: EditOp::Delete },
    UiEvent::Edit { op: EditOp::End },
    UiEvent::Edit {
        op: EditOp::Backspace,
    },
];

/// The scripted session: build the menu, solve it, walk the script,
/// folding the tree's digest after every event and the solved geometry
/// after every solve, then hand the typed half to [`type_into`].
///
/// `None` on any of four roads, all of them unreachable under this
/// file's own limits and all of them ending in main's nonzero exit
/// rather than in a digest: the two inserts, the field pool refusing a
/// slot — which would silently empty the only part of this scenario
/// that exercises text, so it ends the run rather than shrinking it —
/// and the style read-back, which answers `None` only for an id the
/// tree does not know.
fn run() -> Option<(u64, usize, u64)> {
    let mut ui = Ui::new(UiLimits { nodes: 16 });
    let root = ui.root();
    ui.set_style(
        root,
        Style {
            direction: Direction::Column,
            padding: Edges::all(Fixed::from_int(8)),
            gap: Fixed::from_int(4),
            align_cross: Align::Center,
            ..Style::default()
        },
    );
    // A title strip and three menu buttons, the middle one growing.
    // A refused insert is impossible under this file's own limits;
    // the `?` road ends in main's nonzero exit, never in a digest.
    let title = ui.insert(root).ok()?;
    ui.set_style(
        title,
        Style {
            width: Size::Px(Fixed::from_int(120)),
            height: Size::Px(Fixed::from_int(24)),
            ..Style::default()
        },
    );
    let mut buttons = [title; 3];
    for (nth, slot) in buttons.iter_mut().enumerate() {
        let button = ui.insert(root).ok()?;
        ui.set_style(
            button,
            Style {
                width: Size::Px(Fixed::from_int(100)),
                height: Size::Px(Fixed::from_int(20)),
                grow: u32::from(nth == 1),
                ..Style::default()
            },
        );
        *slot = button;
    }
    ui.solve(Fixed::from_int(320), Fixed::from_int(200));
    let mut hash = StateHash::new();
    hash = fold_geometry(
        &ui,
        hash,
        &[root, title, buttons[0], buttons[1], buttons[2]],
    );

    // The script: coordinates chosen against the solved layout above —
    // the buttons sit centred at x 110..210, stacked from y 36 with a
    // 4px gap, the middle one grown. Every kind of gesture appears at
    // least once, and one move lands exactly on the first button's
    // top-left corner, where the half-open convention decides the hit.
    let script = [
        UiEvent::PointerMoved { x: 110, y: 36 },
        UiEvent::PointerMoved { x: 160, y: 40 },
        UiEvent::PointerPressed,
        UiEvent::PointerReleased, // activate the first button
        UiEvent::PointerMoved { x: 160, y: 100 },
        UiEvent::PointerPressed,
        UiEvent::PointerMoved { x: 5, y: 5 },
        UiEvent::PointerReleased, // abandoned: dragged off before release
        UiEvent::PointerMoved { x: 400, y: 300 },
        UiEvent::PointerPressed,
        UiEvent::PointerReleased, // click on nothing at all
        UiEvent::PointerMoved { x: 160, y: 180 },
        UiEvent::PointerPressed,
        UiEvent::PointerReleased, // activate the last button
    ];
    for &event in &script {
        ui.handle(event);
        hash = ui.absorb(hash);
    }

    hash = type_into(&mut ui, buttons[2], hash)?;

    // Mid-session restyle: the growing button stops growing, the tree
    // re-solves, and the pixel that hit the middle button before the
    // restyle now hits the last one — the coda click activates a
    // DIFFERENT node than it would have a moment earlier, so a
    // re-solve that diverged or silently never ran changes the
    // decision fold, not just the geometry fold.
    let mut style = ui.style(buttons[1])?;
    style.grow = 0;
    ui.set_style(buttons[1], style);
    ui.solve(Fixed::from_int(320), Fixed::from_int(200));
    hash = fold_geometry(
        &ui,
        hash,
        &[root, title, buttons[0], buttons[1], buttons[2]],
    );
    let coda = [
        UiEvent::PointerMoved { x: 160, y: 100 },
        UiEvent::PointerPressed,
        UiEvent::PointerReleased,
    ];
    for &event in &coda {
        ui.handle(event);
        hash = ui.absorb(hash);
    }
    let activations = ui.drain_outputs().count() as u64;
    Some((
        hash.finish(),
        script.len() + TYPING.len() + coda.len(),
        activations,
    ))
}

/// Fold the solved rectangles of `nodes`, to the raw fixed-point bit.
/// The lane witness wants what the gameplay digest deliberately
/// excludes: a one-raw-unit solver divergence must be visible here.
fn fold_geometry(ui: &Ui, mut hash: StateHash, nodes: &[renew_ui::NodeId]) -> StateHash {
    for rect in nodes.iter().filter_map(|&node| ui.rect(node)) {
        for part in [rect.x, rect.y, rect.width, rect.height] {
            hash = hash.absorb_u64(part.to_bits().cast_unsigned());
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::run;

    /// The scenario is a pure function: two runs in one process agree
    /// exactly. The cross-machine half of the claim belongs to the
    /// comparison lane, which is the point of the binary existing.
    #[test]
    fn the_scenario_reproduces_in_process() {
        assert_eq!(run(), run());
    }

    /// The script really decides things: two activations before the
    /// restyle and one after, counted from the drained queue — a
    /// scenario that stopped deciding could never pass this, however
    /// stable its digest.
    ///
    /// The event count is pinned for the same reason, and it caught
    /// exactly what it is for: typing was added to the scenario while
    /// the reported count still said seventeen, so the machine-readable
    /// line understated what had run. Both halves move together or the
    /// number is decoration.
    #[test]
    fn the_scenario_actually_activates() {
        let (digest, events, activations) = run().expect("the scenario builds its own tree");
        assert_eq!(
            events, 27,
            "fourteen pointer, ten typed or edited, three coda"
        );
        assert_eq!(activations, 3, "the script activates exactly three times");
        assert_ne!(digest, 0);
    }
}
