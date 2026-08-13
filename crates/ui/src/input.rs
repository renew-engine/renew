//! Input and interaction state: the integer events a tree hears, the
//! decisions it makes, and the digest those decisions feed.
//!
//! **The surface is integer-only, deliberately.** Pointer coordinates
//! arrive as `i32` physical pixels; the one documented float-to-integer
//! seam (`quantize_pointer`) lives in the maths crate, applied by
//! windowed drivers and the replay harness before events reach
//! [`crate::Ui::handle`]. No float enters this crate, so nothing here
//! can put one where a digest sees it.
//!
//! **The interaction vocabulary** is hover, press/release activation,
//! focus, and — for whichever node holds focus and is a text field —
//! typed characters and a closed set of editing operations. Focus
//! follows activation, because the first consumer was a menu and a
//! clicked button is the focused one; text arrived when a consumer
//! needed a typed address and had nowhere to put it. Keyboard traversal
//! and scroll are still cut until a consumer needs them; each returns
//! with its own quantization rule where floats are involved.
//!
//! A typed character arrives as a Unicode scalar, never a key code.
//! What a keystroke means — shift, layout, dead keys, composition — is
//! the window system's answer and differs per platform and per person,
//! so a tree deriving a character from a key would be wrong differently
//! everywhere. It is the same line the pointer draws.
//!
//! **Hit-testing walks the retained rectangles** of the last
//! [`crate::Ui::solve`], topmost first: children draw over their
//! parents and later siblings draw over earlier ones, so the test
//! descends to the deepest, latest node containing the point. A tree
//! that was never solved hit-tests against zero-sized rectangles,
//! which contain no point at all under half-open edges — solve first,
//! or every event lands on nothing.

use renew_frame::StateHash;

use crate::{NIL, NodeId, Ui};

/// One event the tree can hear. Integers only — see the module doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEvent {
    /// The pointer moved to these physical pixels, in the same space
    /// the solver's rectangles live in.
    PointerMoved { x: i32, y: i32 },
    /// The primary button went down at the current pointer position.
    PointerPressed,
    /// The primary button came up at the current pointer position.
    PointerReleased,
    /// One character was typed, as a Unicode scalar value.
    ///
    /// **A scalar, not a key.** Shift, layout, dead keys and IME
    /// composition are the window system's answer and differ per
    /// platform and per person; a tree deriving a character from a key
    /// code would be wrong differently everywhere. The driver decides
    /// what a keystroke means, exactly as it decides what a pointer
    /// position is. A value that is not a scalar is ignored rather than
    /// refused — this event carries no failure back to anyone.
    TextEntered { ch: u32 },
    /// One editing operation, from the closed set the tree understands.
    Edit { op: crate::EditOp },
}

/// One decision the tree made, queued for the host to drain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiOutput {
    /// A press and release landed on the same node: the click
    /// happened, and the node now holds focus.
    Activated(NodeId),
}

/// Which kind of event a folded token came from.
///
/// **Without this the two share a namespace and collide.** An edit
/// operation's code is a small integer and so is a control character's
/// scalar, so typing U+0003 and pressing the key bound to `Left` folded
/// the same number — and Windows delivers U+0001..U+001A for Ctrl with a
/// letter, so it is a keystroke a player can produce by accident. Two
/// materially different fields, different bytes *and* different cursor,
/// shared a fingerprint.
///
/// A review found it after the affine collision was fixed: routing
/// through the hash made the arithmetic sound and left the domain
/// unexamined.
const KIND_TEXT: u64 = 1;
const KIND_EDIT: u64 = 2;

/// The interaction state one [`Ui`] carries between events.
#[derive(Debug, Default)]
pub(crate) struct Interaction {
    /// The last pointer position, in physical pixels. Kept because a
    /// press decides against the position of the moment, not the
    /// position of the last move event.
    pub pointer: (i32, i32),
    /// The node the primary button went down on, until release.
    pub pressed: Option<NodeId>,
    /// The node that holds focus: the last activated one.
    pub focus: Option<NodeId>,
    /// How many activations this tree has ever made — the ordinal the
    /// digest folds, so a replayed run that clicks twice can never
    /// digest like one that clicked once.
    pub activations: u64,
    /// A running fold of every activated id, in order — the sequence
    /// record the ordinal alone cannot be. Two runs that activated
    /// different nodes, or the same nodes in a different order, hold
    /// different folds even after their queues drain.
    pub decisions: u64,
    /// Accepted edits, ever — the ordinal a replay compares.
    pub edits: u64,
    /// A running fold of every accepted edit, in order.
    ///
    /// **The stream, not the contents.** Folding a field's bytes on
    /// every keystroke is linear in its length for a property this
    /// already has: two runs that typed different things, or the same
    /// things in a different order, differ here.
    pub edit_fold: u64,
    /// Decisions dropped because the output queue was full. Absorbed
    /// into the digest: a dropped decision is a behaviour difference,
    /// and a counter is how it stays visible rather than silent.
    pub overflowed: u64,
}

impl Ui {
    /// Feed one event through the tree's interaction state.
    ///
    /// Hover is refreshed against the retained rectangles of the last
    /// [`Self::solve`] on every event — a moved pointer, a press, and
    /// a release all decide against where things are *now*. A press
    /// remembers the node it landed on; a release on that same node
    /// activates it, queues [`UiOutput::Activated`], and moves focus
    /// there. A release anywhere else abandons the press, which is how
    /// every toolkit lets a mis-click be dragged off a button.
    pub fn handle(&mut self, event: UiEvent) {
        match event {
            // Typing reaches the focused node and nothing else. A tree
            // with no focus, or a focus that is not a field, hears the
            // event and does nothing — which is what a keystroke into a
            // page with no cursor in it should do.
            UiEvent::TextEntered { ch } => {
                if let Some(ch) = char::from_u32(ch) {
                    self.edit_focused(KIND_TEXT, |field| field.insert(ch), u64::from(ch as u32));
                }
            }
            UiEvent::Edit { op } => {
                self.edit_focused(KIND_EDIT, |field| field.edit(op), op.code());
            }
            UiEvent::PointerMoved { x, y } => {
                self.interaction.pointer = (x, y);
            }
            UiEvent::PointerPressed => {
                let (x, y) = self.interaction.pointer;
                self.interaction.pressed = self.hit_test(x, y);
            }
            UiEvent::PointerReleased => {
                let (x, y) = self.interaction.pointer;
                let released_on = self.hit_test(x, y);
                if let Some(node) = self.interaction.pressed
                    && released_on == Some(node)
                {
                    self.interaction.activations += 1;
                    // Chain the id into the running decision fold: the
                    // previous fold seeds the next, so the sequence —
                    // not just the count and the last — is the record.
                    self.interaction.decisions = StateHash::new()
                        .absorb_u64(self.interaction.decisions)
                        .absorb_u32(node.index())
                        .absorb_u64(node.generation())
                        .finish();
                    self.interaction.focus = Some(node);
                    // Bounded by the declared limit, not by whatever
                    // the allocator happened to hand the queue.
                    if self.outputs.len() < self.limits.nodes as usize {
                        self.outputs.push(UiOutput::Activated(node));
                    } else {
                        self.interaction.overflowed += 1;
                    }
                }
                self.interaction.pressed = None;
            }
        }
        // Bits may have moved for the old holders and the new alike:
        // re-derive them and swap patches where they changed. One
        // lookup per candidate, no matcher, no allocation.
        self.refresh_states();
    }

    /// The queued decisions, drained oldest first. The queue holds as
    /// many decisions as the tree holds nodes; past that, decisions
    /// are dropped and counted, never silently lost — the digest folds
    /// the count.
    pub fn drain_outputs(&mut self) -> impl Iterator<Item = UiOutput> + '_ {
        self.outputs.drain(..)
    }

    /// Apply an edit to the focused field, folding it if it changed.
    ///
    /// **Only a change is folded.** A left arrow at the start of a field
    /// does nothing, and an event that does nothing must not move the
    /// fingerprint — otherwise two runs that reached the same field
    /// contents by different amounts of cursor-bumping would disagree.
    fn edit_focused(
        &mut self,
        kind: u64,
        apply: impl FnOnce(&mut crate::field::Field) -> bool,
        token: u64,
    ) {
        let Some(focus) = self.interaction.focus else {
            return;
        };
        let Some(slot) = self.field_slot(focus) else {
            return;
        };
        let Some(field) = self.fields.get_mut(slot) else {
            return;
        };
        if !apply(field) {
            return;
        }
        self.interaction.edits = self.interaction.edits.saturating_add(1);
        // Through `StateHash`, exactly as the decision fold two screens
        // up does, and for a reason found the hard way: the first
        // version rotated and added, which is **affine in the token**.
        // `rot7(x + 1) == rot7(x) + 128`, so a token one larger and a
        // later token 128 smaller cancel exactly — typing "aÈ" and
        // typing "bH" produced one digest. A fingerprint that two
        // different texts share is worse than none, because everything
        // downstream trusts it.
        //
        // The node's generation goes in beside its index, so a slot
        // reused by a later node is a different history, and the
        // previous fold seeds the next so the sequence is the record
        // rather than the count and the last entry.
        self.interaction.edit_fold = StateHash::new()
            .absorb_u64(self.interaction.edit_fold)
            .absorb_u32(focus.index())
            .absorb_u64(focus.generation())
            .absorb_u64(kind)
            .absorb_u64(token)
            .finish();
    }

    /// The node under the pointer, against the rectangles of the last
    /// [`Self::solve`] — computed fresh on every ask, so a re-solve
    /// that moved a box under a stationary pointer answers the new
    /// truth, never a cached one.
    #[must_use]
    pub fn hover(&self) -> Option<NodeId> {
        let (x, y) = self.interaction.pointer;
        self.hit_test(x, y)
    }

    /// The node the primary button is currently down on.
    #[must_use]
    pub fn pressed(&self) -> Option<NodeId> {
        self.interaction.pressed.filter(|&node| self.is_live(node))
    }

    /// The node holding focus: the last activated one, while it lives.
    #[must_use]
    pub fn focus(&self) -> Option<NodeId> {
        self.interaction.focus.filter(|&node| self.is_live(node))
    }

    /// Fold this tree's discrete decisions into `hash`.
    ///
    /// **What is absorbed:** the pointer position (input echo — it
    /// decides the next press), the pressed and focus ids, the
    /// activation ordinal, the running fold of every activated id in
    /// order, and the overflow count. Ids absorb as slot index plus
    /// generation, with the index `NIL` standing for none — a
    /// collision-free sentinel, since no real slot carries `NIL` and
    /// no generation is zero.
    ///
    /// **What is left out, and why — named here because an unstated
    /// exclusion is how a digest goes quietly vacuous:**
    /// - *Hover*: not stored at all. Every decision path re-derives it
    ///   from the absorbed pointer and the current rectangles at the
    ///   moment of the press or release, and the accessor recomputes
    ///   on every ask — there is no cached value to go stale or to
    ///   escape this digest.
    /// - *Solved rectangles and wanted sizes*: geometry. A layout
    ///   change reaches this digest only by changing a decision — a
    ///   press that hits a different node — which is exactly when it
    ///   should.
    /// - *Styles and the tree structure*: authoring data, same road.
    /// - *The output queue*: its contents are exactly the activated
    ///   ids in order, and the decision fold above records that
    ///   sequence — so two states with equal digests hand their hosts
    ///   the same drained decisions.
    /// - *A field's bytes, its cursor, and how much of the pool is
    ///   occupied*: the edit fold above records the stream that
    ///   produced them — which node, which kind of event, which
    ///   character or operation, in order — so two states with equal
    ///   digests were typed into identically and hold identical text.
    ///   The bytes themselves stay out because folding them on every
    ///   keystroke is linear in a field's length for a property the
    ///   stream already has. **Pool occupancy is the weaker half and is
    ///   named as such:** a slot freed by a removal is reclaimed
    ///   lazily, so two states with equal digests can differ in how
    ///   many fields they will accept next. Nothing observable today
    ///   turns on that, and a review measured it rather than assuming
    ///   — but it is a difference the digest does not see, and this is
    ///   where that is said out loud.
    /// - *Worn state patches*: derived from the absorbed pointer and
    ///   press/focus state applied over the excluded geometry and
    ///   authored tables — like geometry, dress reaches this digest
    ///   only by changing a decision. Two *identically authored*
    ///   states with equal digests wear the same patches; authoring
    ///   is excluded here exactly as styles are.
    #[must_use]
    pub fn absorb(&self, hash: StateHash) -> StateHash {
        let hash = hash
            .absorb_u32(self.interaction.pointer.0.cast_unsigned())
            .absorb_u32(self.interaction.pointer.1.cast_unsigned());
        let hash = absorb_id(hash, self.interaction.pressed);
        let hash = absorb_id(hash, self.interaction.focus);
        hash.absorb_u64(self.interaction.activations)
            .absorb_u64(self.interaction.decisions)
            .absorb_u64(self.interaction.overflowed)
            .absorb_u64(self.interaction.edits)
            .absorb_u64(self.interaction.edit_fold)
    }

    /// The deepest, latest node containing the point, or `None` when
    /// even the root does not.
    ///
    /// Paint order is document order — parents under children, earlier
    /// siblings under later ones — so the test starts at the root and
    /// repeatedly descends into the *last* child containing the point.
    pub(crate) fn hit_test(&self, x: i32, y: i32) -> Option<NodeId> {
        if !self.contains(0, x, y) {
            return None;
        }
        let mut at = 0u32;
        loop {
            let mut descended = false;
            let mut child = self.slots[at as usize].first_child;
            let mut topmost = NIL;
            while child != NIL {
                if self.contains(child, x, y) {
                    topmost = child;
                }
                child = self.slots[child as usize].next_sibling;
            }
            if topmost != NIL {
                at = topmost;
                descended = true;
            }
            if !descended {
                return Some(self.id_at(at));
            }
        }
    }

    /// Whether `index`'s solved rectangle contains the point.
    /// Half-open on both axes: a point on the right or bottom edge is
    /// outside, so adjacent rectangles never both claim it.
    fn contains(&self, index: u32, x: i32, y: i32) -> bool {
        let rect = self.layout[index as usize].rect;
        let x = renew_fixed::Fixed::from_int(x);
        let y = renew_fixed::Fixed::from_int(y);
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    }
}

/// Absorb an optional id: slot index (`NIL` for none) then generation.
fn absorb_id(hash: StateHash, id: Option<NodeId>) -> StateHash {
    match id {
        Some(node) => hash.absorb_u32(node.index()).absorb_u64(node.generation()),
        None => hash.absorb_u32(NIL).absorb_u64(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edges, Size, Style, UiLimits};
    use renew_fixed::Fixed;

    fn f(value: i32) -> Fixed {
        Fixed::from_int(value)
    }

    fn px(value: i32) -> Size {
        Size::Px(f(value))
    }

    /// A solved two-button column, the fixture most tests share:
    /// buttons at y 0..10 and y 10..20, both x 0..40.
    fn two_buttons() -> (Ui, NodeId, NodeId) {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let root = ui.root();
        ui.set_style(
            root,
            Style {
                direction: crate::Direction::Column,
                ..Style::default()
            },
        );
        let first = ui.insert(root).expect("room");
        ui.set_style(
            first,
            Style {
                width: px(40),
                height: px(10),
                ..Style::default()
            },
        );
        let second = ui.insert(root).expect("room");
        ui.set_style(
            second,
            Style {
                width: px(40),
                height: px(10),
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        (ui, first, second)
    }

    /// The deepest node under the pointer wins, edges are half-open,
    /// and outside the root is nobody.
    #[test]
    fn hit_testing_finds_the_deepest_node_and_respects_edges() {
        let (mut ui, first, second) = two_buttons();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        assert_eq!(ui.hover(), Some(first));
        ui.handle(UiEvent::PointerMoved { x: 5, y: 15 });
        assert_eq!(ui.hover(), Some(second));
        // y = 10 is the second button's first row, not the first's
        // last: half-open edges mean neighbours never share a point.
        ui.handle(UiEvent::PointerMoved { x: 5, y: 10 });
        assert_eq!(ui.hover(), Some(second));
        // x = 40 is past both buttons' right edge but inside the root.
        ui.handle(UiEvent::PointerMoved { x: 40, y: 5 });
        assert_eq!(ui.hover(), Some(ui.root()));
        // Outside the viewport is outside everything.
        ui.handle(UiEvent::PointerMoved { x: 200, y: 5 });
        assert_eq!(ui.hover(), None);
    }

    /// Later siblings draw on top, so an overlapping later sibling
    /// takes the hit.
    #[test]
    fn a_later_sibling_overlapping_an_earlier_one_is_on_top() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let under = ui.insert(root).expect("room");
        ui.set_style(
            under,
            Style {
                width: px(30),
                height: px(30),
                ..Style::default()
            },
        );
        let over = ui.insert(root).expect("room");
        ui.set_style(
            over,
            Style {
                width: px(30),
                height: px(30),
                margin: Edges {
                    left: f(-30),
                    ..Edges::default()
                },
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        assert_eq!(
            ui.rect(under),
            ui.rect(over),
            "the fixture needs the two boxes coincident"
        );
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        assert_eq!(ui.hover(), Some(over), "paint order decides the hit");
    }

    /// Press and release on the same node activates it, queues the
    /// decision, and moves focus; the press then ends.
    #[test]
    fn a_click_activates_and_focuses() {
        let (mut ui, first, _) = two_buttons();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        ui.handle(UiEvent::PointerPressed);
        assert_eq!(ui.pressed(), Some(first));
        ui.handle(UiEvent::PointerReleased);
        assert_eq!(ui.pressed(), None, "release ends the press");
        assert_eq!(ui.focus(), Some(first), "focus follows activation");
        let outputs: Vec<_> = ui.drain_outputs().collect();
        assert_eq!(outputs, vec![UiOutput::Activated(first)]);
        assert_eq!(
            ui.drain_outputs().count(),
            0,
            "draining is consuming: the queue empties"
        );
    }

    /// Dragging off a button abandons the press: no activation, no
    /// focus, nothing queued.
    #[test]
    fn releasing_elsewhere_abandons_the_press() {
        let (mut ui, first, _) = two_buttons();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        ui.handle(UiEvent::PointerPressed);
        assert_eq!(ui.pressed(), Some(first));
        ui.handle(UiEvent::PointerMoved { x: 5, y: 15 });
        ui.handle(UiEvent::PointerReleased);
        assert_eq!(ui.focus(), None, "an abandoned press focuses nothing");
        assert_eq!(ui.drain_outputs().count(), 0);
    }

    /// A release with no press, and a press on nothing, both decide
    /// nothing — the pairing is a pairing.
    #[test]
    fn unpaired_halves_decide_nothing() {
        let (mut ui, _, _) = two_buttons();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        ui.handle(UiEvent::PointerReleased);
        assert_eq!(ui.drain_outputs().count(), 0, "release without press");
        ui.handle(UiEvent::PointerMoved { x: 200, y: 200 });
        ui.handle(UiEvent::PointerPressed);
        ui.handle(UiEvent::PointerReleased);
        assert_eq!(ui.drain_outputs().count(), 0, "press on nothing");
    }

    /// The replay shape: one recorded event sequence, applied to two
    /// identically built trees, digests identically at every step —
    /// and differently from a sequence that decided differently.
    #[test]
    fn a_replayed_sequence_digests_identically() {
        use renew_frame::StateHash;
        let script = [
            UiEvent::PointerMoved { x: 5, y: 5 },
            UiEvent::PointerPressed,
            UiEvent::PointerReleased,
            UiEvent::PointerMoved { x: 5, y: 15 },
            UiEvent::PointerPressed,
            UiEvent::PointerMoved { x: 5, y: 5 },
            UiEvent::PointerReleased,
        ];
        let run = || {
            let (mut ui, _, _) = two_buttons();
            let mut digests = Vec::new();
            for &event in &script {
                ui.handle(event);
                digests.push(ui.absorb(StateHash::new()).finish());
            }
            digests
        };
        assert_eq!(run(), run(), "same trace, same digests, every step");

        // A run that activated a different button — and then parked
        // the pointer on the same final pixel — must digest
        // differently: the decision is the difference.
        let (mut other, _, second) = two_buttons();
        other.handle(UiEvent::PointerMoved { x: 5, y: 15 });
        other.handle(UiEvent::PointerPressed);
        other.handle(UiEvent::PointerReleased);
        other.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        assert_eq!(other.focus(), Some(second), "the fixture decided");
        assert_ne!(
            run().last().copied(),
            Some(other.absorb(StateHash::new()).finish()),
            "different decisions may not share a digest"
        );
    }

    /// The load-bearing exclusion, pinned: geometry that changes no
    /// decision changes no digest. A padding tweak moves rectangles,
    /// but with no press between, the digest stands still.
    #[test]
    fn geometry_reaches_the_digest_only_through_decisions() {
        use renew_frame::StateHash;
        let (mut ui, _, _) = two_buttons();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        let before = ui.absorb(StateHash::new()).finish();
        let root = ui.root();
        let mut style = ui.style(root).expect("root is live");
        style.padding = Edges::all(f(2));
        ui.set_style(root, style);
        ui.solve(f(100), f(100));
        assert_eq!(
            ui.absorb(StateHash::new()).finish(),
            before,
            "a padding tweak with no decision between must not redden a digest"
        );
    }

    /// One button clicked `clicks` times in a tree sized `nodes`, for
    /// the overflow comparison below.
    fn clicked(nodes: u32, clicks: u32) -> Ui {
        let mut ui = Ui::new(UiLimits { nodes });
        let root = ui.root();
        let button = ui.insert(root).expect("room");
        ui.set_style(
            button,
            Style {
                width: px(10),
                height: px(10),
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        for _ in 0..clicks {
            ui.handle(UiEvent::PointerPressed);
            ui.handle(UiEvent::PointerReleased);
        }
        ui
    }

    /// The queue is bounded by the node count; past that, decisions
    /// are counted rather than silently lost — and the count alone
    /// separates the digests, because the comparison runs make the
    /// same activations in the same order and differ only in room.
    #[test]
    fn overflowed_decisions_are_counted_not_lost() {
        use renew_frame::StateHash;
        let mut cramped = clicked(2, 3);
        let mut roomy = clicked(4, 3);
        assert_eq!(cramped.drain_outputs().count(), 2, "two fit, one dropped");
        assert_eq!(roomy.drain_outputs().count(), 3, "all three fit");
        assert_ne!(
            cramped.absorb(StateHash::new()).finish(),
            roomy.absorb(StateHash::new()).finish(),
            "the dropped decision must be visible in the digest, and only \
             the overflow counter distinguishes these two runs"
        );
    }

    /// A second press without a release simply re-decides: the first
    /// press is overwritten, and the release pairs with the newest.
    #[test]
    fn a_second_press_replaces_the_first() {
        let (mut ui, _, second) = two_buttons();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        ui.handle(UiEvent::PointerPressed);
        ui.handle(UiEvent::PointerMoved { x: 5, y: 15 });
        ui.handle(UiEvent::PointerPressed);
        assert_eq!(ui.pressed(), Some(second), "the newest press holds");
        ui.handle(UiEvent::PointerReleased);
        let outputs: Vec<_> = ui.drain_outputs().collect();
        assert_eq!(outputs, vec![UiOutput::Activated(second)]);
    }

    /// A recycled slot under the pointer does not inherit the old
    /// tenant's press: the generation refuses the pairing.
    #[test]
    fn a_recycled_slot_does_not_inherit_a_press() {
        let (mut ui, first, _) = two_buttons();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        ui.handle(UiEvent::PointerPressed);
        assert_eq!(ui.pressed(), Some(first));
        assert!(ui.remove(first));
        let replacement = ui.insert(ui.root()).expect("the freed slot returns");
        ui.set_style(
            replacement,
            Style {
                width: px(40),
                height: px(10),
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        ui.handle(UiEvent::PointerReleased);
        assert_eq!(
            ui.drain_outputs().count(),
            0,
            "the new tenant under the same pixels is not the node that was pressed"
        );
        assert_eq!(ui.focus(), None);
    }

    /// An unsolved tree hits nothing at all: zero-sized rectangles
    /// contain no point under half-open edges, the origin included.
    #[test]
    fn an_unsolved_tree_hits_nothing() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        ui.handle(UiEvent::PointerMoved { x: 0, y: 0 });
        assert_eq!(ui.hover(), None);
        ui.handle(UiEvent::PointerPressed);
        ui.handle(UiEvent::PointerReleased);
        assert_eq!(ui.drain_outputs().count(), 0);
    }

    /// The decision fold records the sequence, not the summary: two
    /// runs with the same activation count, the same final focus, and
    /// the same parked pointer — differing only in WHICH node was
    /// activated first — digest differently.
    #[test]
    fn the_decision_fold_separates_equal_summaries() {
        use renew_frame::StateHash;
        let click = |ui: &mut Ui, y: i32| {
            ui.handle(UiEvent::PointerMoved { x: 5, y });
            ui.handle(UiEvent::PointerPressed);
            ui.handle(UiEvent::PointerReleased);
        };
        let (mut one, _, _) = two_buttons();
        click(&mut one, 5);
        click(&mut one, 15);
        let (mut two, _, _) = two_buttons();
        click(&mut two, 15);
        click(&mut two, 15);
        // Same count, same focus, same pointer; only history differs.
        assert_eq!(one.focus(), two.focus());
        assert_ne!(
            one.absorb(StateHash::new()).finish(),
            two.absorb(StateHash::new()).finish(),
            "the digest must remember which node was activated, not just how many times"
        );
    }

    /// Interaction state follows liveness: focus dies with its node,
    /// and hover — computed fresh — answers what is really under the
    /// pointer, before and after the re-solve.
    #[test]
    fn removed_nodes_leave_the_interaction_state() {
        let (mut ui, first, second) = two_buttons();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        ui.handle(UiEvent::PointerPressed);
        ui.handle(UiEvent::PointerReleased);
        assert_eq!(ui.focus(), Some(first));
        assert!(ui.remove(first));
        assert_eq!(ui.focus(), None, "focus does not outlive its node");
        assert_eq!(
            ui.hover(),
            Some(ui.root()),
            "with the button gone and the tree unsolved, the pointer rests on the root"
        );
        ui.solve(f(100), f(100));
        assert_eq!(
            ui.hover(),
            Some(second),
            "after the re-solve, on the sibling that moved up"
        );
    }
}
