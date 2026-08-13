//! Compiled state styling: per-node tables over a shared patch pool.
//!
//! **No selector matcher runs in the frame loop.** Every combination
//! of the four interaction states was resolved ahead of time — by the
//! document compiler, or by a host building tables directly — into a
//! complete style per combination, deduplicated into one pool. At
//! runtime a node's state is four bits, the table lookup is one index,
//! and applying a patch is one style swap. The pool entry carries the
//! one fact the swap needs beyond the style: whether its layout fields
//! differ from the node's base, so a colour-only flip never dirties
//! layout and the exact-damage claim is a counter, not a hope.
//!
//! **The state bits are derived, not authored.** Hover is the node
//! under the pointer, pressed and focus are the interaction state the
//! tree already tracks, and disabled is reserved — the bit exists in
//! every table so the format never bumps for it, and nothing sets it
//! yet. Bits refresh per event: between a solve that moves geometry
//! and the next event, worn dress can lag the pointer, exactly as the
//! freshly-computed hover answer does not. Because every bit derives
//! from state the digest already covers (the pointer and the decision
//! fold) or deliberately excludes (hover and geometry), applied
//! patches add nothing to [`crate::Ui::absorb`]: like geometry, dress
//! reaches the digest only by changing a decision.
//!
//! **Not that equal digests mean identical dress.** This module said so
//! until it was checked. Two trees authored by the same code, differing
//! only in whether the solve ran before or after the first pointer
//! move, hold one digest and wear different patches — the lag named
//! above, seen from outside. What holds is the exclusion: the next
//! event re-derives the bits from the pointer the digest already
//! carries, the two converge, and dress never decides anything on its
//! own. `a_lagging_patch_catches_up_and_the_digest_never_saw_it` is
//! that pair.

use crate::layout::Style;

/// The hover bit: the pointer sits on this node.
pub const STATE_HOVER: u8 = 1;
/// The pressed bit: this node took the press that is still down.
pub const STATE_PRESSED: u8 = 2;
/// The focus bit: this node was activated most recently.
pub const STATE_FOCUS: u8 = 4;
/// The disabled bit, reserved: present in every table, set by nothing
/// in v0.
pub const STATE_DISABLED: u8 = 8;

/// How many state combinations a table holds: two to the four bits.
pub const STATE_COMBINATIONS: usize = 16;

/// The table entry meaning "no patch: wear the base style".
pub const NO_PATCH: u16 = u16::MAX;

/// One pooled patch: a complete resolved style, and whether wearing it
/// moves geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatePatch {
    /// The whole style a node wears while this patch applies — a
    /// resolved answer, not a diff, so applying is one swap.
    pub style: Style,
    /// Whether any layout-feeding field differs from the base this
    /// patch was compiled against. Computed where the resolution
    /// happened; trusted here, because recomputing it per flip would
    /// put the comparison back in the frame loop.
    pub touches_layout: bool,
}

/// Per-slot state styling: the combination table, the base style the
/// patches were resolved against, and what is currently worn.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StateSlot {
    /// Patch per state combination; [`NO_PATCH`] wears the base.
    pub table: [u16; STATE_COMBINATIONS],
    /// The style [`crate::Ui::set_style`] authored, kept whole so
    /// un-applying a patch restores it exactly.
    pub base: Style,
    /// The patch currently worn; [`NO_PATCH`] when the base is.
    pub applied: u16,
    /// The state bits last applied, so a refresh is a comparison.
    pub bits: u8,
}

impl Default for StateSlot {
    fn default() -> Self {
        Self {
            table: [NO_PATCH; STATE_COMBINATIONS],
            base: Style::default(),
            applied: NO_PATCH,
            bits: 0,
        }
    }
}

impl StateSlot {
    /// Whether the given patch index moves geometry relative to base.
    /// [`NO_PATCH`] never does: the base IS the geometry.
    pub fn touches_layout(patch: u16, pool: &[StatePatch]) -> bool {
        usize::from(patch) < pool.len() && pool[usize::from(patch)].touches_layout
    }
}

/// Whether two styles differ anywhere layout can see — everything but
/// the background. The one definition the compiler stamps flags with
/// and the blob reader verifies them against, so the two cannot
/// disagree about what geometry is.
#[must_use]
pub fn moves_geometry(base: &Style, resolved: &Style) -> bool {
    base.direction != resolved.direction
        || base.width != resolved.width
        || base.height != resolved.height
        || base.margin != resolved.margin
        || base.padding != resolved.padding
        || base.gap != resolved.gap
        || base.grow != resolved.grow
        || base.justify != resolved.justify
        || base.align_cross != resolved.align_cross
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Size, Style};
    use crate::{
        Fixed, NO_PATCH, STATE_COMBINATIONS, STATE_HOVER, STATE_PRESSED, Ui, UiEvent, UiLimits,
    };

    const BASE_BG: [u8; 4] = [10, 10, 10, 255];
    const HOVER_BG: [u8; 4] = [40, 40, 40, 255];
    const PRESS_BG: [u8; 4] = [80, 80, 80, 255];

    /// A solved button under a root, with a pool of one colour-only
    /// hover patch and one layout-moving pressed patch, tabled so any
    /// pressed bit outranks hover.
    fn hoverable() -> (Ui, crate::NodeId) {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let button = ui.insert(root).expect("room");
        let base = Style {
            width: Size::Px(Fixed::from_int(40)),
            height: Size::Px(Fixed::from_int(20)),
            background: BASE_BG,
            ..Style::default()
        };
        ui.set_style(button, base);
        assert!(ui.set_patch_pool(vec![
            StatePatch {
                style: Style {
                    background: HOVER_BG,
                    ..base
                },
                touches_layout: false,
            },
            StatePatch {
                style: Style {
                    width: Size::Px(Fixed::from_int(44)),
                    height: Size::Px(Fixed::from_int(20)),
                    background: PRESS_BG,
                    ..Style::default()
                },
                touches_layout: true,
            },
        ]));
        let mut table = [NO_PATCH; STATE_COMBINATIONS];
        for (bits, entry) in table.iter_mut().enumerate() {
            let bits_u8 = u8::try_from(bits).unwrap_or(0);
            if bits_u8 & STATE_PRESSED != 0 {
                *entry = 1;
            } else if bits_u8 & STATE_HOVER != 0 {
                *entry = 0;
            }
        }
        assert!(ui.set_state_table(button, table));
        ui.solve(Fixed::from_int(100), Fixed::from_int(100));
        (ui, button)
    }

    /// Hover on, hover off: the style swaps to the patch and back to
    /// the base, driven by nothing but pointer motion.
    #[test]
    fn hover_wears_the_patch_and_leaving_restores_the_base() {
        let (mut ui, button) = hoverable();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        assert_eq!(ui.style(button).expect("live").background, HOVER_BG);
        ui.handle(UiEvent::PointerMoved { x: 90, y: 90 });
        assert_eq!(ui.style(button).expect("live").background, BASE_BG);
    }

    /// The exact-damage promise as a counter: colour-only hover flips
    /// provoke no layout walk, however often the solve is invited.
    #[test]
    fn a_colour_only_flip_re_solves_nothing() {
        let (mut ui, _button) = hoverable();
        let walked = ui.layout_passes();
        for _ in 0..4 {
            ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
            ui.solve(Fixed::from_int(100), Fixed::from_int(100));
            ui.handle(UiEvent::PointerMoved { x: 90, y: 90 });
            ui.solve(Fixed::from_int(100), Fixed::from_int(100));
        }
        assert_eq!(
            ui.layout_passes(),
            walked,
            "a colour-only patch must never dirty layout"
        );
    }

    /// A layout-moving patch does re-solve, exactly when worn and
    /// when shed, and the rectangle proves the walk happened.
    #[test]
    fn a_layout_patch_re_solves_and_moves_geometry() {
        let (mut ui, button) = hoverable();
        let walked = ui.layout_passes();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        ui.handle(UiEvent::PointerPressed);
        ui.solve(Fixed::from_int(100), Fixed::from_int(100));
        assert_eq!(ui.layout_passes(), walked + 1, "wearing must re-solve");
        assert_eq!(
            ui.rect(button).expect("live").width,
            Fixed::from_int(44),
            "the worn width is what solved"
        );
        ui.handle(UiEvent::PointerReleased);
        ui.solve(Fixed::from_int(100), Fixed::from_int(100));
        assert_eq!(
            ui.rect(button).expect("live").width,
            Fixed::from_int(40),
            "shedding restores the base geometry"
        );
    }

    /// Pressed outranks hover because the table says so — the
    /// precedence lives in authored data, not in this crate.
    #[test]
    fn the_table_owns_precedence() {
        let (mut ui, button) = hoverable();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        ui.handle(UiEvent::PointerPressed);
        assert_eq!(ui.style(button).expect("live").background, PRESS_BG);
        // Releasing activates: focus alone maps to NO_PATCH here, so
        // the button returns to hover dress while still pointed at.
        ui.handle(UiEvent::PointerReleased);
        assert_eq!(ui.style(button).expect("live").background, HOVER_BG);
    }

    /// `set_style` authors the base under a worn patch: the patch
    /// stays on, and the new base shows when it comes off.
    #[test]
    fn a_new_base_waits_under_the_patch() {
        let (mut ui, button) = hoverable();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        let recoloured = Style {
            width: Size::Px(Fixed::from_int(40)),
            height: Size::Px(Fixed::from_int(20)),
            background: [1, 2, 3, 255],
            ..Style::default()
        };
        ui.set_style(button, recoloured);
        assert_eq!(
            ui.style(button).expect("live").background,
            HOVER_BG,
            "the patch stays worn over a new base"
        );
        ui.handle(UiEvent::PointerMoved { x: 90, y: 90 });
        assert_eq!(ui.style(button).expect("live").background, [1, 2, 3, 255]);
    }

    /// The refusals: an out-of-pool table entry, a stale node, an
    /// unaddressable pool.
    #[test]
    fn tables_and_pools_refuse_what_they_cannot_hold() {
        let (mut ui, button) = hoverable();
        let mut beyond = [NO_PATCH; STATE_COMBINATIONS];
        beyond[1] = 7;
        assert!(!ui.set_state_table(button, beyond), "past the pool");
        let stale = {
            let doomed = ui.insert(ui.root()).expect("room");
            ui.remove(doomed);
            doomed
        };
        assert!(!ui.set_state_table(stale, [NO_PATCH; STATE_COMBINATIONS]));
        let huge = vec![
            StatePatch {
                style: Style::default(),
                touches_layout: false,
            };
            usize::from(u16::MAX)
        ];
        assert!(!ui.set_patch_pool(huge), "unaddressable pool");
    }

    /// Loading a pool strips worn patches back to base: pools load
    /// before tables, and stale dress must not survive the swap.
    #[test]
    fn a_new_pool_strips_worn_patches() {
        let (mut ui, button) = hoverable();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        assert_eq!(ui.style(button).expect("live").background, HOVER_BG);
        assert!(ui.set_patch_pool(Vec::new()));
        assert_eq!(ui.style(button).expect("live").background, BASE_BG);
    }

    /// Reloading a smaller pool cannot strand a table into it: the
    /// swap clears every table, so the once-hoverable button wears
    /// its base and nothing indexes past the new pool — the sequence
    /// that would otherwise panic in the frame loop.
    #[test]
    fn a_smaller_pool_cannot_be_indexed_by_an_old_table() {
        let (mut ui, button) = hoverable();
        // Point away first, then shrink the pool under the old table.
        ui.handle(UiEvent::PointerMoved { x: 90, y: 90 });
        assert!(ui.set_patch_pool(vec![StatePatch {
            style: Style::default(),
            touches_layout: false,
        }]));
        // The old table would have worn entry 1 here; the swap
        // cleared it, so pointing at the button wears the base.
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        assert_eq!(
            ui.style(button).expect("live").background,
            BASE_BG,
            "a cleared table wears the base, never a stale index"
        );
    }

    /// Removing a node that is wearing dress leaves no ghost: the
    /// dead slot is reset as it is freed, and the event after the
    /// removal provokes no spurious layout walk shedding it.
    #[test]
    fn removing_a_worn_node_leaves_no_ghost() {
        let (mut ui, button) = hoverable();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        ui.handle(UiEvent::PointerPressed);
        assert_eq!(ui.style(button).expect("live").background, PRESS_BG);
        assert!(ui.remove(button));
        ui.solve(Fixed::from_int(100), Fixed::from_int(100));
        let walked = ui.layout_passes();
        // The next event pops the dead slot; a ghost patch would
        // dirty layout here and the counter would move.
        ui.handle(UiEvent::PointerMoved { x: 6, y: 6 });
        ui.solve(Fixed::from_int(100), Fixed::from_int(100));
        assert_eq!(
            ui.layout_passes(),
            walked,
            "a freed slot must not shed a ghost patch into a layout walk"
        );
    }

    /// The digest does not see dress: hover flips wearing a patch
    /// leave absorb where it was, exactly as bare hover always has.
    /// Two identically authored trees with equal digests can wear
    /// different patches — and the difference does not last.
    ///
    /// **The narrow claim, because the wide one is false.** Equal
    /// digests do not mean identical worn dress. Bits refresh on an
    /// event and not on a solve, so a tree
    /// solved before its first pointer move has rects to hit-test
    /// against and wears the patch, while one solved after does not —
    /// same authoring, same digest, different dress.
    ///
    /// What saves the exclusion is that it is transient: the next event
    /// re-derives the bits from the pointer the digest already carries,
    /// and the two converge. So dress still reaches the digest only by
    /// changing a decision, and it cannot change one on its own.
    #[test]
    fn a_lagging_patch_catches_up_and_the_digest_never_saw_it() {
        let build = |move_first: bool| {
            let mut ui = Ui::new(UiLimits { nodes: 4 });
            let root = ui.root();
            let button = ui.insert(root).expect("room");
            let base = Style {
                width: Size::Px(Fixed::from_int(40)),
                height: Size::Px(Fixed::from_int(20)),
                background: BASE_BG,
                ..Style::default()
            };
            ui.set_style(button, base);
            assert!(ui.set_patch_pool(vec![StatePatch {
                style: Style {
                    width: Size::Px(Fixed::from_int(80)),
                    background: HOVER_BG,
                    ..base
                },
                touches_layout: true,
            }]));
            let mut table = [NO_PATCH; STATE_COMBINATIONS];
            for (bits, entry) in table.iter_mut().enumerate() {
                if u8::try_from(bits).unwrap_or(0) & STATE_HOVER != 0 {
                    *entry = 0;
                }
            }
            assert!(ui.set_state_table(button, table));
            // The only difference between the two: which side of the
            // solve the pointer arrived on.
            if move_first {
                ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
                ui.solve(Fixed::from_int(100), Fixed::from_int(100));
            } else {
                ui.solve(Fixed::from_int(100), Fixed::from_int(100));
                ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
            }
            (ui, button)
        };

        let (mut solved_first, one) = build(false);
        let (mut moved_first, other) = build(true);
        let digest = |ui: &Ui| ui.absorb(renew_frame::StateHash::new()).finish();

        assert_eq!(
            digest(&solved_first),
            digest(&moved_first),
            "the two absorbed the same pointer and the same nothing else"
        );
        assert_ne!(
            solved_first.style(one).map(|style| style.width),
            moved_first.style(other).map(|style| style.width),
            "equal digests do not mean identical dress, which this module once promised"
        );

        solved_first.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        moved_first.handle(UiEvent::PointerMoved { x: 5, y: 5 });

        assert_eq!(
            solved_first.style(one).map(|style| style.width),
            moved_first.style(other).map(|style| style.width),
            "one event re-derives the bits and the lag is gone"
        );
        assert_eq!(
            digest(&solved_first),
            digest(&moved_first),
            "and it was never in the digest to begin with"
        );
    }

    #[test]
    fn worn_patches_stay_outside_the_digest() {
        let (mut ui, _button) = hoverable();
        let before = ui.absorb(renew_frame::StateHash::new()).finish();
        ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        let worn = ui.absorb(renew_frame::StateHash::new()).finish();
        // The pointer IS digested; compare against the same pointer
        // without a patch in play by moving to empty space at the same
        // coordinates cannot exist — so the claim is narrower and
        // honest: wearing dress changes nothing beyond what the moved
        // pointer already changed.
        let mut bare = Ui::new(UiLimits { nodes: 4 });
        let root = bare.root();
        let twin = bare.insert(root).expect("room");
        bare.set_style(
            twin,
            Style {
                width: Size::Px(Fixed::from_int(40)),
                height: Size::Px(Fixed::from_int(20)),
                background: BASE_BG,
                ..Style::default()
            },
        );
        bare.solve(Fixed::from_int(100), Fixed::from_int(100));
        bare.handle(UiEvent::PointerMoved { x: 5, y: 5 });
        assert_eq!(
            worn,
            bare.absorb(renew_frame::StateHash::new()).finish(),
            "a worn patch must not reach the digest"
        );
        assert_ne!(before, worn, "the pointer move itself is digested");
    }
}
