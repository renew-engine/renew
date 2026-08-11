//! Presentation for the widget tree: retained snapshots, blended at
//! display rate, clipped on the CPU, emitted as sprites.
//!
//! **The split this crate lives on.** The tree solves at the fixed
//! timestep; frames arrive faster. The presenter captures a snapshot
//! of the solved rectangles each tick and blends the last two by the
//! ratified interpolation factor, keyed by (slot, generation) so a
//! node is only ever blended with *itself*: a recycled slot's new
//! tenant never inherits the old tenant's motion. Nodes with one known
//! tick draw unlerped at it — a newborn at the current tick, a dying
//! node at the previous — so nothing vanishes mid-blend. (Paint order
//! puts the dying underneath the living, so what CAN change abruptly
//! is stacking, never existence.)
//!
//! **Clipping is CPU rectangle intersection.** Every node clips to the
//! intersection of its ancestors' boxes, computed at capture and
//! blended with the rest. The one atlas region v0 samples is a uniform
//! white texel, so clipping the rectangle *is* clipping the image —
//! the proportional-UV half of the job arrives with the first
//! non-uniform region, where texel granularity has to be answered.
//!
//! **Emission is sprites.** One generated atlas (a white fill tile and
//! a chrome tile beside it, reserved for borders), one premultiplied
//! tint per node from its integer background colour, pushed through
//! the 2D renderer's own preallocated buffer. Quads are in the
//! solver's pixel space: the sprite renderer's canvas must match the
//! viewport the tree solves at, and a frame can hold up to twice the
//! node limit in quads — a bulk replace legitimately draws every old
//! node once more under every new one — so size the sprite capacity
//! by [`UiPresenter::max_quads`], not by the node count. After
//! construction the presenter allocates nothing: both snapshots and
//! the capture stack are sized at construction, and `advance` asserts
//! the tree fits them.

// A presenter draws pictures, not terminal lines: the standard
// output macros are banned by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use renew_math::Alpha;
use renew_render2d::{Sprite, SpriteRenderer};
use renew_ui::{NodeId, Ui};

pub mod atlas;

/// One captured node: everything a frame needs to draw it.
#[derive(Clone, Copy, Debug, Default)]
struct Entry {
    present: bool,
    generation: u64,
    /// Solved box, in canvas pixels: x, y, width, height.
    rect: [f32; 4],
    /// The ancestor intersection this node may not draw outside:
    /// left, top, right, bottom.
    clip: [f32; 4],
    /// Premultiplied tint from the node's integer background.
    tint: [f32; 4],
}

/// One tick's captured tree: slot-indexed entries plus paint order.
#[derive(Debug)]
struct Snapshot {
    entries: Vec<Entry>,
    /// Live slots in paint order — parents before children, siblings
    /// in document order, exactly the walk the solver promises.
    order: Vec<u32>,
}

impl Snapshot {
    fn with_capacity(nodes: u32) -> Self {
        Self {
            entries: vec![Entry::default(); nodes as usize],
            order: Vec::with_capacity(nodes as usize),
        }
    }

    /// Forget everything, keeping the allocations.
    fn clear(&mut self) {
        self.entries.fill(Entry::default());
        self.order.clear();
    }
}

/// The presenter: two snapshots and the blend between them.
#[derive(Debug)]
pub struct UiPresenter {
    previous: Snapshot,
    current: Snapshot,
    /// Capture scratch: the walk stack of (node, inherited clip).
    stack: Vec<(NodeId, [f32; 4])>,
}

impl UiPresenter {
    /// A presenter sized for trees of up to `nodes` nodes — normally
    /// the same limit the tree itself was built with.
    #[must_use]
    pub fn new(nodes: u32) -> Self {
        let nodes = nodes.max(1);
        Self {
            previous: Snapshot::with_capacity(nodes),
            current: Snapshot::with_capacity(nodes),
            stack: Vec::with_capacity(nodes as usize),
        }
    }

    /// Capture the tree's solved state as the new current snapshot,
    /// retiring the old current to previous. Call once per simulation
    /// tick, after the solve; every frame until the next call blends
    /// between the two most recent captures.
    ///
    /// The blend never asks how old a capture is: a host that stops
    /// advancing a hidden tree and resumes on reshow will blend one
    /// interval from wherever it left off. A host that wants a reshow
    /// to cut rather than slide advances twice.
    ///
    /// # Panics
    ///
    /// When the tree's limit exceeds the presenter's capacity — a
    /// construction mismatch, asserted by name rather than left to an
    /// index bound.
    pub fn advance(&mut self, ui: &Ui) {
        assert!(
            ui.limits().nodes as usize <= self.current.entries.len(),
            "the presenter must be sized for the tree it presents: the tree \
             holds up to {} nodes, the presenter {}",
            ui.limits().nodes,
            self.current.entries.len()
        );
        core::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
        self.stack.clear();
        self.stack
            .push((ui.root(), [f32::MIN, f32::MIN, f32::MAX, f32::MAX]));
        while let Some((node, inherited)) = self.stack.pop() {
            let index = node.index();
            let Some(rect) = ui.rect(node) else {
                continue;
            };
            let Some(style) = ui.style(node) else {
                continue;
            };
            let rect = [
                to_f32(rect.x),
                to_f32(rect.y),
                to_f32(rect.width),
                to_f32(rect.height),
            ];
            let clip = intersect(inherited, edges_of(rect));
            let slot = &mut self.current.entries[index as usize];
            slot.present = true;
            slot.generation = node.generation();
            slot.rect = rect;
            slot.clip = clip;
            slot.tint = tint_of(style.background);
            self.current.order.push(index);
            // Children push in reverse document order so the stack
            // pops them forward: paint order is document order.
            let children: &mut Vec<_> = &mut self.stack;
            let before = children.len();
            for child in ui.children(node) {
                children.push((child, clip));
            }
            children[before..].reverse();
        }
    }

    /// One frame's quads: the current snapshot blended `alpha` of the
    /// way from the previous, dying nodes first — underneath the
    /// living, at their last known place — everything clipped to its
    /// ancestors, invisible and empty quads already dropped.
    ///
    /// This is the whole of what a frame draws, as data: every
    /// decision the presenter makes is observable here without a
    /// device, which is where the tests hold it.
    pub fn frame(&self, alpha: Alpha) -> impl Iterator<Item = Quad> + '_ {
        let dying = self.previous.order.iter().filter_map(move |&index| {
            let old = self.previous.entries[index as usize];
            let successor = self.current.entries[index as usize];
            if successor.present && successor.generation == old.generation {
                return None;
            }
            clipped(old.rect, old.clip, old.tint)
        });
        let living = self.current.order.iter().filter_map(move |&index| {
            let entry = self.current.entries[index as usize];
            let old = self.previous.entries[index as usize];
            let (rect, clip) = if old.present && old.generation == entry.generation {
                (
                    lerp4(old.rect, entry.rect, alpha),
                    lerp4(old.clip, entry.clip, alpha),
                )
            } else {
                (entry.rect, entry.clip)
            };
            clipped(rect, clip, entry.tint)
        });
        dying.chain(living)
    }

    /// The most quads one frame can hold: twice the capacity, because
    /// a bulk replace legitimately draws every dying node once more
    /// underneath every newborn. Size the sprite renderer by this,
    /// never by the node count alone.
    #[must_use]
    pub fn max_quads(&self) -> u32 {
        u32::try_from(self.current.entries.len().saturating_mul(2)).unwrap_or(u32::MAX)
    }

    /// Push one frame's quads as sprites — up to [`Self::max_quads`]
    /// of them, in the solver's pixel space, so the renderer's canvas
    /// must match the solve viewport. The caller owns `begin` and
    /// `item`: a presenter is one source of sprites among possibly
    /// several in a frame.
    pub fn emit(&self, alpha: Alpha, sprites: &mut SpriteRenderer) {
        for quad in self.frame(alpha) {
            sprites.push(
                &Sprite::new(atlas::white(), quad.x, quad.y)
                    .size(quad.width, quad.height)
                    .tint(quad.tint),
            );
        }
    }
}

/// One drawn rectangle: everything presentation decides about a node,
/// after blending and clipping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Premultiplied RGBA.
    pub tint: [f32; 4],
}

/// Clip `rect` to `clip`: the visible quad, or `None` when nothing
/// remains or the tint draws nothing at all.
fn clipped(rect: [f32; 4], clip: [f32; 4], tint: [f32; 4]) -> Option<Quad> {
    if tint == [0.0, 0.0, 0.0, 0.0] {
        return None;
    }
    let [x, y, width, height] = rect;
    let left = x.max(clip[0]);
    let top = y.max(clip[1]);
    let right = (x + width).min(clip[2]);
    let bottom = (y + height).min(clip[3]);
    if right <= left || bottom <= top {
        return None;
    }
    Some(Quad {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
        tint,
    })
}

/// A rectangle's edges: left, top, right, bottom.
fn edges_of(rect: [f32; 4]) -> [f32; 4] {
    [rect[0], rect[1], rect[0] + rect[2], rect[1] + rect[3]]
}

/// The intersection of two edge rectangles. May be empty (right below
/// left); the emitter treats empty as nothing to draw.
fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ]
}

/// Component lerp of two rectangles by the frame's blend factor. At
/// zero this is exactly `from`, because `from + 0 × d` is `from` in
/// IEEE arithmetic for every finite value.
fn lerp4(from: [f32; 4], to: [f32; 4], alpha: Alpha) -> [f32; 4] {
    let t = alpha.get();
    [
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
        from[2] + (to[2] - from[2]) * t,
        from[3] + (to[3] - from[3]) * t,
    ]
}

/// The premultiplied float tint of an integer background.
fn tint_of(background: [u8; 4]) -> [f32; 4] {
    [
        f32::from(background[0]) / 255.0,
        f32::from(background[1]) / 255.0,
        f32::from(background[2]) / 255.0,
        f32::from(background[3]) / 255.0,
    ]
}

/// Canvas pixels from the solver's fixed-point. The i64 raw value
/// casts through f32's 24-bit mantissa, so coordinates past 2^24 raw
/// units — 256 canvas pixels of magnitude — already round to the
/// nearest representable float. That is sub-pixel error at any
/// on-screen magnitude, growing with distance off-screen; the picture
/// degrades, nothing else does.
#[allow(
    clippy::cast_precision_loss,
    reason = "rounding is sub-pixel on screen and only the picture degrades off it"
)]
fn to_f32(value: renew_ui::Fixed) -> f32 {
    value.to_bits() as f32 / 65536.0
}

#[cfg(test)]
mod tests {
    // Every float comparison below is between values produced by the
    // same IEEE expressions the assertion names — bit equality IS the
    // claim (alpha zero is exactly the previous tick, a capture is
    // exactly its solve), so approximate comparison would weaken the
    // tests into passing when the arithmetic drifts.
    #![expect(
        clippy::float_cmp,
        reason = "bit equality is the claim these tests make"
    )]

    use super::*;
    use renew_ui::{Fixed, Size, Style, UiLimits};

    fn f(value: i32) -> Fixed {
        Fixed::from_int(value)
    }

    fn px(value: i32) -> Size {
        Size::Px(f(value))
    }

    const INK: [u8; 4] = [255, 128, 64, 255];

    fn coloured(width: i32, height: i32) -> Style {
        Style {
            width: px(width),
            height: px(height),
            background: INK,
            ..Style::default()
        }
    }

    /// A solved tree captures into quads at its solved places, in
    /// paint order, with transparent nodes (the unstyled root) absent.
    #[test]
    fn a_capture_draws_what_was_solved() {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let root = ui.root();
        let first = ui.insert(root).expect("room");
        ui.set_style(first, coloured(20, 10));
        let second = ui.insert(root).expect("room");
        ui.set_style(second, coloured(30, 10));
        ui.solve(f(100), f(100));

        let mut presenter = UiPresenter::new(8);
        presenter.advance(&ui);
        let quads: Vec<Quad> = presenter.frame(Alpha::ZERO).collect();
        assert_eq!(quads.len(), 2, "two coloured nodes, no transparent root");
        assert_eq!((quads[0].x, quads[0].width), (0.0, 20.0));
        assert_eq!((quads[1].x, quads[1].width), (20.0, 30.0));
        assert_eq!(quads[0].tint, [1.0, 128.0 / 255.0, 64.0 / 255.0, 1.0]);
    }

    /// The blend moves a surviving node between its two captures: at
    /// zero it stands at the previous tick, midway it stands midway.
    #[test]
    fn a_surviving_node_blends_between_its_ticks() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let node = ui.insert(root).expect("room");
        ui.set_style(
            node,
            Style {
                margin: renew_ui::Edges {
                    left: f(10),
                    ..renew_ui::Edges::default()
                },
                ..coloured(20, 10)
            },
        );
        ui.solve(f(100), f(100));
        let mut presenter = UiPresenter::new(4);
        presenter.advance(&ui);

        let mut style = ui.style(node).expect("live");
        style.margin.left = f(50);
        ui.set_style(node, style);
        ui.solve(f(100), f(100));
        presenter.advance(&ui);

        let at_zero: Vec<Quad> = presenter.frame(Alpha::ZERO).collect();
        assert_eq!(
            at_zero[0].x, 10.0,
            "alpha zero is exactly the previous tick"
        );
        let midway: Vec<Quad> = presenter
            .frame(Alpha::new(1, core::num::NonZeroU64::new(2).expect("two")))
            .collect();
        assert_eq!(midway[0].x, 30.0, "halfway between 10 and 50");
    }

    /// A newborn draws unlerped at its one known tick, and a dying
    /// node draws once more at its last known place, underneath.
    #[test]
    fn newborns_and_dying_nodes_draw_at_their_one_known_tick() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let old = ui.insert(root).expect("room");
        ui.set_style(old, coloured(20, 10));
        ui.solve(f(100), f(100));
        let mut presenter = UiPresenter::new(4);
        presenter.advance(&ui);

        assert!(ui.remove(old));
        let newborn = ui.insert(root).expect("the freed slot returns");
        ui.set_style(newborn, coloured(40, 10));
        ui.solve(f(100), f(100));
        presenter.advance(&ui);

        let half = Alpha::new(1, core::num::NonZeroU64::new(2).expect("two"));
        let quads: Vec<Quad> = presenter.frame(half).collect();
        assert_eq!(quads.len(), 2, "the dying node and the newborn");
        assert_eq!(
            quads[0].width, 20.0,
            "the dying node stands at its last known size, first — underneath"
        );
        assert_eq!(
            quads[1].width, 40.0,
            "the newborn stands unlerped at its only tick — a recycled slot's \
             new tenant never inherits the old tenant's motion"
        );
    }

    /// Children clip to their ancestors: what pokes out is cut, what
    /// is fully outside vanishes.
    #[test]
    fn children_clip_to_their_ancestors() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let panel = ui.insert(root).expect("room");
        ui.set_style(panel, coloured(50, 50));
        let poker = ui.insert(panel).expect("room");
        // Wider than the panel: the overflow spills right and is cut.
        ui.set_style(poker, coloured(80, 10));
        ui.solve(f(200), f(200));
        let mut presenter = UiPresenter::new(4);
        presenter.advance(&ui);
        let quads: Vec<Quad> = presenter.frame(Alpha::ZERO).collect();
        assert_eq!(quads.len(), 2);
        assert_eq!(
            quads[1].width, 50.0,
            "the child is cut at the panel's right edge"
        );
    }

    /// Two captures of the same solved tree produce identical frames
    /// at every alpha: presentation is a pure function of its inputs.
    #[test]
    fn presentation_is_a_pure_function() {
        let build = || {
            let mut ui = Ui::new(UiLimits { nodes: 8 });
            let root = ui.root();
            for nth in 1..4 {
                let node = ui.insert(root).expect("room");
                ui.set_style(node, coloured(10 * nth, 8));
            }
            ui.solve(f(100), f(100));
            let mut presenter = UiPresenter::new(8);
            presenter.advance(&ui);
            presenter.advance(&ui);
            let half = Alpha::new(1, core::num::NonZeroU64::new(2).expect("two"));
            presenter.frame(half).collect::<Vec<Quad>>()
        };
        assert_eq!(build(), build());
    }

    proptest::proptest! {
        /// However rectangles and clips fall, a clipped quad sits
        /// inside both and clipping is stable — to within a few
        /// rounding steps, because the quad stores a width whose
        /// subtraction rounds once and whose `x + width` re-derivation
        /// rounds again, each at the magnitude of the operands rather
        /// than of the edge. The permitted spill is bounded well below
        /// anything a rasterizer can see; the property pins that it
        /// never grows past that.
        #[test]
        fn clipping_contains_and_is_stable(
            rect in proptest::array::uniform4(-100.0f32..100.0),
            clip_a in proptest::array::uniform4(-100.0f32..100.0),
            clip_b in proptest::array::uniform4(-100.0f32..100.0),
        ) {
            let close = |a: f32, b: f32| {
                (a - b).abs() <= f32::EPSILON * 4.0 * (1.0 + a.abs() + b.abs())
            };
            let within = |value: f32, bound: f32| value <= bound || close(value, bound);
            let rect = [rect[0], rect[1], rect[2].abs(), rect[3].abs()];
            let clip = intersect(
                [clip_a[0], clip_a[1], clip_a[0] + clip_a[2].abs(), clip_a[1] + clip_a[3].abs()],
                [clip_b[0], clip_b[1], clip_b[0] + clip_b[2].abs(), clip_b[1] + clip_b[3].abs()],
            );
            let tint = [1.0, 1.0, 1.0, 1.0];
            if let Some(quad) = clipped(rect, clip, tint) {
                proptest::prop_assert!(quad.x >= rect[0]);
                proptest::prop_assert!(quad.x >= clip[0]);
                proptest::prop_assert!(within(quad.x + quad.width, rect[0] + rect[2]));
                proptest::prop_assert!(within(quad.x + quad.width, clip[2]));
                proptest::prop_assert!(quad.y >= clip[1]);
                proptest::prop_assert!(within(quad.y + quad.height, clip[3]));
                proptest::prop_assert!(quad.width > 0.0 && quad.height > 0.0);
                // Stable: clipping the clipped quad moves nothing by
                // more than the one rounding step above.
                let again = clipped([quad.x, quad.y, quad.width, quad.height], clip, tint)
                    .expect("a visible quad stays visible");
                proptest::prop_assert!(close(again.x, quad.x));
                proptest::prop_assert!(close(again.y, quad.y));
                proptest::prop_assert!(close(again.width, quad.width));
                proptest::prop_assert!(close(again.height, quad.height));
            }
        }

        /// A transparent background never reaches the frame.
        #[test]
        fn transparent_draws_nothing(
            rect in proptest::array::uniform4(0.0f32..50.0),
        ) {
            let clip = [f32::MIN, f32::MIN, f32::MAX, f32::MAX];
            proptest::prop_assert_eq!(
                clipped([rect[0], rect[1], rect[2] + 1.0, rect[3] + 1.0], clip, [0.0; 4]),
                None
            );
        }
    }
}
