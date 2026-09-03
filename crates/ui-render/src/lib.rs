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
use renew_render2d::{Sprite, SpriteRenderer, SubRegion};
use renew_ui::{NodeId, Ui};

pub mod atlas;
mod glyphs;

pub use glyphs::{BEARING, GLYPH_FIRST, GLYPH_LAST, Glyph, LINE_HEIGHT};

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
            // Both answers are Some for every id this walk can hold —
            // the tree only hands out live children — and the defaults
            // are the no-op answer if that invariant ever bent: a zero
            // rectangle draws nothing.
            let rect = ui.rect(node).unwrap_or_default();
            let style = ui.style(node).unwrap_or_default();
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
    ///
    /// **The same rule is implemented a second time in the tree**, over a
    /// different payload: `Snapshots::frame` in `renew-snapshot` blends an
    /// arbitrary payload under exactly this generation guard. The two were
    /// not merged — the payloads have little in common and this one was
    /// already green — so a correction to the rule has to be made in both,
    /// and each names the other so neither is corrected alone.
    pub fn frame(&self, alpha: Alpha) -> impl Iterator<Item = Quad> + '_ {
        let dying = self.previous.order.iter().filter_map(move |&index| {
            let old = self.previous.entries[index as usize];
            let successor = self.current.entries[index as usize];
            if successor.present && successor.generation == old.generation {
                return None;
            }
            clipped(old.rect, old.clip, atlas::white().into(), old.tint)
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
            clipped(rect, clip, atlas::white().into(), entry.tint)
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
                &Sprite::new(quad.source, quad.x, quad.y)
                    .size(quad.width, quad.height)
                    .tint(quad.tint),
            );
        }
    }
}

/// Push one line of text as glyph sprites, the pen starting at
/// (`x`, `y`) — the line's top-left — advancing by the same integer
/// table the simulation measures with, so a label is exactly as wide
/// as the tree believed. Each bitmap sits at `pen - BEARING`, so ink
/// may reach [`BEARING`] texels past either end of the measured box —
/// bearings and antialiasing live there, exactly as type does.
/// Characters outside the baked range draw as the question mark they
/// were measured as. Budget one sprite per character.
pub fn emit_text(sprites: &mut SpriteRenderer, x: f32, y: f32, text: &str, tint: [f32; 4]) {
    // Glyph dimensions are tens of texels; the widening to f32 is
    // exact for anything a strip could hold.
    #[allow(
        clippy::cast_precision_loss,
        reason = "glyph metrics are tens of texels, exact in an f32"
    )]
    fn wide(value: u32) -> f32 {
        value as f32
    }
    let mut pen = x;
    for character in text.chars() {
        let (glyph, region) = atlas::glyph_of(character);
        sprites.push(
            &Sprite::new(region, pen - wide(glyphs::BEARING), y)
                .size(wide(region.width), wide(region.height))
                .tint(tint),
        );
        pen += wide(glyph.advance);
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
    /// The atlas texels this quad samples, cut by the same proportion
    /// that cut the rectangle. Today every node names the white texel,
    /// so every source here is that one region and the picture is what
    /// it was before quads carried a source at all.
    pub source: SubRegion,
    /// Premultiplied RGBA.
    pub tint: [f32; 4],
}

/// Clip `rect` to `clip`, cutting `source` by the same proportion: the
/// visible quad, or `None` when nothing remains or the tint draws
/// nothing at all.
///
/// **The source is cut by the same linear map that cut the destination,
/// so the map is unchanged and every surviving pixel samples the texel
/// it would have sampled uncut.** Uncut, the shader interpolates between
/// the source edges, so a pixel centre `p` maps to
/// `u(p) = sx + sw*(p - x)/w`. Clipped to `[L, R]` the source runs from
/// `su0 = sx + sw*(L - x)/w` to `su1 = sx + sw*(R - x)/w`, so
/// `su1 - su0 = sw*(R - L)/w` and
///
/// ```text
/// u'(p) = su0 + (p - L)/(R - L) * (su1 - su0)
///       = sx + sw*(L - x)/w + sw*(p - L)/w
///       = sx + sw*(p - x)/w
///       = u(p)
/// ```
///
/// Identically the same function. It holds by construction, and only
/// because both fractions come from the same reciprocal - recomputing
/// per edge is where an implementation loses it.
///
/// Two cases are exact rather than merely close. At 1:1 - every glyph,
/// every nine-slice corner - `source.width / w` is a value divided by
/// itself, exactly `1.0`, so the cut edge is `sx + (L - x)`: a
/// difference and a sum of integers, bit-exact. At a power-of-two scale
/// the reciprocal is exact too. Elsewhere the residue is a few ULPs,
/// orders below the half texel that could move a `floor`.
///
/// Nearest-and-clamped sampling cannot bleed: `0 <= fx0 < fx1 <= 1`, so
/// a cut only shrinks the sampled rectangle strictly inside the
/// original, and the computed edges are additionally clamped into it so
/// a rounding hair cannot reach a neighbouring asset.
#[expect(
    clippy::float_cmp,
    reason = "the early-out fires exactly when no edge moved; a tolerance would take the cut path for a quad that was not cut, which is the one case this must not do"
)]
fn clipped(rect: [f32; 4], clip: [f32; 4], source: SubRegion, tint: [f32; 4]) -> Option<Quad> {
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

    // Nothing was cut: four compares, no division, and the source
    // passes through untouched - which is what carries every existing
    // golden byte for byte.

    // Nothing was cut: four compares, no division, and the source
    // passes through untouched - which is what carries every existing
    // golden byte for byte.

    // Nothing was cut: four compares, no division, and the source
    // passes through untouched - which is what carries every existing
    // golden byte for byte.

    // Nothing was cut: four compares, no division, and the source
    // passes through untouched - which is what carries every existing
    // golden byte for byte.

    // Nothing was cut: four compares, no division, and the source
    // passes through untouched - which is what carries every existing
    // golden byte for byte.
    if left == x && top == y && right == x + width && bottom == y + height {
        return Some(Quad {
            x,
            y,
            width,
            height,
            source,
            tint,
        });
    }

    let inv_w = 1.0 / width;
    let inv_h = 1.0 / height;
    let fx0 = (left - x) * inv_w;
    let fx1 = (right - x) * inv_w;
    let fy0 = (top - y) * inv_h;
    let fy1 = (bottom - y) * inv_h;
    let (sx, sy) = (source.x, source.y);
    let (sw, sh) = (source.width, source.height);
    // Clamped into the original source: the arithmetic above cannot
    // leave it by more than a rounding step, and this makes "cannot" a
    // fact rather than an argument.
    let cut_left = sw.mul_add(fx0, sx).max(sx);
    let cut_top = sh.mul_add(fy0, sy).max(sy);
    let cut_right = sw.mul_add(fx1, sx).min(sx + sw);
    let cut_bottom = sh.mul_add(fy1, sy).min(sy + sh);
    Some(Quad {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
        source: SubRegion {
            x: cut_left,
            y: cut_top,
            width: cut_right - cut_left,
            height: cut_bottom - cut_top,
        },
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
///
/// **Premultiplied means the colour carries its own coverage**, and the
/// pipeline these quads reach depends on it: sprites blend with a source
/// factor of one and a destination factor of one-minus-source-alpha, so
/// the colour is added as it arrives and must already have been scaled.
/// Handing it straight alpha instead draws a translucent panel brighter
/// than it should be, by a factor of one over its own alpha — which for
/// a panel at alpha 230 is about eleven per cent, on every panel, all
/// the time.
///
/// The two spellings agree exactly at alpha 255, which is why an opaque
/// test cannot tell them apart.
/// **The colour channels are decoded, the alpha is not.** A background is
/// authored by choosing bytes that look right, so those bytes are
/// display-encoded and their light is what shading and blending need.
/// Alpha is coverage rather than light, and the transfer function has no
/// business touching it — which is also why the premultiply happens after
/// the decode: multiplying an encoded value by alpha and decoding the
/// product is not the same number, and it is the wrong one.
fn tint_of(background: [u8; 4]) -> [f32; 4] {
    let alpha = f32::from(background[3]) / 255.0;
    [
        renew_rhi::srgb::decode(background[0]) * alpha,
        renew_rhi::srgb::decode(background[1]) * alpha,
        renew_rhi::srgb::decode(background[2]) * alpha,
        alpha,
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

    /// A presenter smaller than its tree refuses by name, not by a
    /// bare slice index.
    #[test]
    #[should_panic(expected = "the presenter must be sized for the tree")]
    fn a_undersized_presenter_refuses_by_name() {
        let ui = Ui::new(UiLimits { nodes: 8 });
        let mut presenter = UiPresenter::new(4);
        presenter.advance(&ui);
    }

    /// A solved tree captures into quads at its solved places, in
    /// paint order, with transparent nodes (the unstyled root) absent —
    /// and one frame can never need more than twice the capacity.
    #[test]
    fn a_capture_draws_what_was_solved() {
        assert_eq!(UiPresenter::new(8).max_quads(), 16);
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
        assert_eq!(
            quads[0].tint,
            [
                renew_rhi::srgb::decode(255),
                renew_rhi::srgb::decode(128),
                renew_rhi::srgb::decode(64),
                1.0
            ]
        );
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

    /// **A translucent background arrives already carrying its coverage.**
    ///
    /// The pipeline these quads reach blends with a source factor of one
    /// and a destination factor of one-minus-source-alpha, so the colour
    /// is added as it arrives. A straight-alpha tint therefore draws a
    /// panel brighter than it should be, by one over its own alpha.
    ///
    /// Every other test in this file uses opaque backgrounds — where the
    /// two spellings agree exactly — which is how this went unnoticed.
    /// The numbers below are the pause menu's own: `#282c34e6`.
    #[test]
    fn a_translucent_background_is_premultiplied_by_its_own_alpha() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let panel = ui.insert(root).expect("room");
        ui.set_style(
            panel,
            Style {
                width: px(20),
                height: px(10),
                background: [0x28, 0x2c, 0x34, 0xe6],
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        let mut presenter = UiPresenter::new(4);
        presenter.advance(&ui);

        let quads: Vec<Quad> = presenter.frame(Alpha::ZERO).collect();
        assert_eq!(quads.len(), 1, "one panel");
        let alpha = 230.0 / 255.0;
        let expect = |channel: u8| (renew_rhi::srgb::decode(channel) * alpha).to_bits();
        assert_eq!(
            quads[0].tint[0].to_bits(),
            expect(0x28),
            "red is not scaled by the panel's own coverage"
        );
        assert_eq!(quads[0].tint[1].to_bits(), expect(0x2c));
        assert_eq!(quads[0].tint[2].to_bits(), expect(0x34));
        assert_eq!(
            quads[0].tint[3].to_bits(),
            alpha.to_bits(),
            "alpha itself is coverage and is never scaled by itself"
        );
        // The straight-alpha spelling this replaced, named so the test
        // fails against it rather than merely differing from it.
        assert_ne!(
            quads[0].tint[0].to_bits(),
            renew_rhi::srgb::decode(0x28).to_bits(),
            "this is the unscaled value the bug produced"
        );
    }

    /// The opaque case both spellings share, kept beside the one that
    /// separates them so neither can drift alone.
    #[test]
    fn an_opaque_background_is_unchanged_by_premultiplying() {
        let mut ui = Ui::new(UiLimits { nodes: 4 });
        let root = ui.root();
        let panel = ui.insert(root).expect("room");
        ui.set_style(
            panel,
            Style {
                width: px(20),
                height: px(10),
                background: [0x28, 0x2c, 0x34, 0xff],
                ..Style::default()
            },
        );
        ui.solve(f(100), f(100));
        let mut presenter = UiPresenter::new(4);
        presenter.advance(&ui);

        let quads: Vec<Quad> = presenter.frame(Alpha::ZERO).collect();
        assert_eq!(
            quads[0].tint[0].to_bits(),
            renew_rhi::srgb::decode(0x28).to_bits()
        );
        assert_eq!(quads[0].tint[3].to_bits(), 1.0f32.to_bits());
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

    /// **An uncut quad passes its source through untouched.**
    ///
    /// The early-out is what carries every existing golden: while no
    /// node names anything but the white texel, a quad inside its clip
    /// must reach the renderer with the identical source it started
    /// with, bit for bit, and no division must have been performed to
    /// get there.
    ///
    /// Probed by deleting the early-out so the general path runs on an
    /// uncut quad: red here, because `1.0/w * w` is not `1.0` for every
    /// `w`.
    #[test]
    fn an_uncut_quad_keeps_its_source_bit_for_bit() {
        let source = SubRegion {
            x: 2.0,
            y: 2.0,
            width: 4.0,
            height: 4.0,
        };
        // Swept, and seeded with widths chosen against the code
        // rather than for it.
        //
        // A single arbitrary width proves nothing here, and neither
        // does a sweep of whole numbers: deleting the early-out leaves
        // both green, because the round trip closes exactly for every
        // integer width.
        //
        // The seeds below are the ones that break the expression the
        // code actually evaluates - `((x + w) - x) * (1.0 / w)`, where
        // the subtraction is the lossy step - rather than the tidier
        // identity `w * (1.0 / w)`. The two disagree about which
        // inputs are adversarial, and only the first is the one under
        // test: 8.2 million f32 widths between 1 and 4096 lose bits
        // this way, the smallest being 1.0000001, and without the
        // early-out an uncut source of width 4 comes back as 3.9999995.
        let clip = [0.0, 0.0, 100_000.0, 100_000.0];
        for span in [
            f32::from_bits(0x3f80_0001),
            f32::from_bits(0x3f80_0002),
            f32::from_bits(0x3f80_0003),
        ] {
            let quad = clipped([10.0, 20.0, span, span + 1.0], clip, source, [1.0; 4])
                .expect("an uncut quad is visible");
            assert_eq!(
                [quad.source.x.to_bits(), quad.source.width.to_bits()],
                [source.x.to_bits(), source.width.to_bits()],
                "an uncut quad of span {span} went through the cut"
            );
        }
        let mut span = 1.0f32;
        while span <= 2000.0 {
            let quad = clipped([10.0, 20.0, span, span + 1.0], clip, source, [1.0; 4])
                .expect("an uncut quad is visible");
            assert_eq!(
                [
                    quad.source.x.to_bits(),
                    quad.source.y.to_bits(),
                    quad.source.width.to_bits(),
                    quad.source.height.to_bits()
                ],
                [
                    source.x.to_bits(),
                    source.y.to_bits(),
                    source.width.to_bits(),
                    source.height.to_bits()
                ],
                "an uncut quad of span {span} went through the cut"
            );
            span += 1.0;
        }
    }

    /// **At 1:1 on whole pixels the cut is bit-exactly integral** - the
    /// case every glyph and every nine-slice corner lands in.
    ///
    /// At 1:1 the scale `source.width / width` is a value divided by
    /// itself, exactly `1.0` in IEEE, so the cut edge is `sx + (L - x)`:
    /// a difference and a sum of small integers, exact. If this were
    /// merely close, text clipped by a scroll viewport would sample
    /// half a neighbouring glyph.
    ///
    /// A 16x16 source drawn at 16x16 pixels, clipped three pixels in
    /// from the left and five from the top, eight wide and eight tall.
    #[test]
    fn a_one_to_one_cut_lands_on_whole_texels() {
        let source = SubRegion {
            x: 32.0,
            y: 48.0,
            width: 16.0,
            height: 16.0,
        };
        let quad = clipped(
            [100.0, 200.0, 16.0, 16.0],
            [103.0, 205.0, 111.0, 213.0],
            source,
            [1.0, 1.0, 1.0, 1.0],
        )
        .expect("visible");
        assert_eq!(quad.source.x.to_bits(), 35.0f32.to_bits(), "cut left texel");
        assert_eq!(quad.source.y.to_bits(), 53.0f32.to_bits(), "cut top texel");
        assert_eq!(quad.source.width.to_bits(), 8.0f32.to_bits(), "cut width");
        assert_eq!(quad.source.height.to_bits(), 8.0f32.to_bits(), "cut height");
    }

    /// **A non-positive span is invisible, and the edge test is what
    /// makes it so** - there is no separate guard, because there is no
    /// room for one to matter.
    ///
    /// The design this implements specified an explicit
    /// `width <= 0.0 || height <= 0.0` guard ahead of the reciprocal.
    /// It was written, and then removed, because a probe would not
    /// redden it: `right > left` requires
    /// `min(x + width, clip.2) > max(x, clip.0) >= x`, hence
    /// `x + width > x`, hence `width > 0`. The division is reached only
    /// when both spans are already positive, so the guard was
    /// unreachable - and it would not have helped against the one case
    /// people reach for it for, since `width <= 0.0` is false for NaN
    /// and `(x + NaN).min(c)` returns `c` rather than NaN.
    ///
    /// What is left is worth pinning on its own: these rectangles draw
    /// nothing. Probed by weakening the edge test to `right < left`,
    /// which lets a zero-width rectangle through to a division by zero:
    /// red here.
    #[test]
    fn a_non_positive_span_draws_nothing() {
        let source = SubRegion {
            x: 2.0,
            y: 2.0,
            width: 4.0,
            height: 4.0,
        };
        let clip = [-1000.0, -1000.0, 1000.0, 1000.0];
        let opaque = [1.0, 1.0, 1.0, 1.0];
        assert_eq!(clipped([5.0, 5.0, 0.0, 10.0], clip, source, opaque), None);
        assert_eq!(clipped([5.0, 5.0, 10.0, 0.0], clip, source, opaque), None);
        assert_eq!(clipped([5.0, 5.0, -3.0, 10.0], clip, source, opaque), None);
    }

    proptest::proptest! {
        /// **A cut samples what the uncut quad would have sampled.**
        ///
        /// Three statements over arbitrary geometry, and the third is
        /// the one that matters. *Contained*: the cut source never
        /// leaves the original, so nearest-and-clamped sampling cannot
        /// reach a neighbour's art. *Proportional*: texels-per-pixel is
        /// unchanged, so nothing is stretched by being clipped. *Same
        /// map*: for pixel centres across the surviving span, the UV
        /// the cut quad interpolates equals the one the uncut quad
        /// would have, which is the actual correctness claim - the
        /// other two are its corollaries.
        ///
        /// The tolerance is relative to the source extent because the
        /// residue is a few ULPs of it; the claim being tested is not
        /// "close enough to look right" but "the same linear function,
        /// evaluated in floating point".
        ///
        /// Probed by recomputing the reciprocal per edge
        /// (`(right - x) / width` in place of `(right - x) * inv_w`):
        /// still close, and still red here at the tolerance below,
        /// which is the point of stating the claim this way.
        #[test]
        fn a_cut_source_samples_what_the_uncut_quad_would_have(
            x in -500.0f32..500.0,
            y in -500.0f32..500.0,
            w in 1.0f32..1000.0,
            h in 1.0f32..1000.0,
            cut_l in 0.0f32..1.0,
            cut_r in 0.0f32..1.0,
            sx in 0.0f32..2000.0,
            sy in 0.0f32..2000.0,
            sw in 0.5f32..512.0,
            sh in 0.5f32..512.0,
        ) {
            // A clip that bites into the rectangle from both sides by a
            // random fraction, so the cut path is the one exercised.
            let lo = cut_l.min(cut_r);
            let hi = cut_l.max(cut_r);
            let clip = [
                w.mul_add(lo * 0.5, x),
                h.mul_add(lo * 0.5, y),
                w.mul_add(1.0 - hi * 0.5, x),
                h.mul_add(1.0 - hi * 0.5, y),
            ];
            let source = SubRegion { x: sx, y: sy, width: sw, height: sh };
            if let Some(q) = clipped([x, y, w, h], clip, source, [1.0, 1.0, 1.0, 1.0]) {
                let slack = 1e-4 * (1.0 + sw.max(sh));

                // Contained.
                proptest::prop_assert!(q.source.x >= source.x);
                proptest::prop_assert!(q.source.y >= source.y);
                proptest::prop_assert!(q.source.x + q.source.width <= source.x + sw + slack);
                proptest::prop_assert!(q.source.y + q.source.height <= source.y + sh + slack);
                proptest::prop_assert!(q.source.width >= 0.0 && q.source.height >= 0.0);

                // Proportional: texels per pixel is what it was.
                proptest::prop_assert!(
                    (q.source.width / q.width - sw / w).abs() <= 1e-3 * (1.0 + sw / w)
                );
                proptest::prop_assert!(
                    (q.source.height / q.height - sh / h).abs() <= 1e-3 * (1.0 + sh / h)
                );

                // The same map, sampled across the surviving span.
                for step in 0u8..=8 {
                    let t = f32::from(step) / 8.0;
                    let p = q.width.mul_add(t, q.x);
                    let uncut = sw.mul_add((p - x) / w, sx);
                    let cut = q.source.width.mul_add((p - q.x) / q.width, q.source.x);
                    proptest::prop_assert!(
                        (cut - uncut).abs() <= slack,
                        "at t={} the cut samples {} where the uncut quad sampled {}",
                        t, cut, uncut
                    );
                }
            }
        }

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
            // The allowance scales with the values a sum passes
            // through, not with the edge that comes out: a small edge
            // derived from large operands carries the operands'
            // rounding error, and a bound scaled only to the edge
            // rejects a legal one-step spill whenever the operands
            // dwarf it — the committed regression seeds are exactly
            // such cases. A real containment defect spills by
            // fractions of the overlap, orders of magnitude past this.
            let step = |magnitude: f32| f32::EPSILON * 4.0 * (1.0 + magnitude);
            let rect = [rect[0], rect[1], rect[2].abs(), rect[3].abs()];
            let clip = intersect(
                [clip_a[0], clip_a[1], clip_a[0] + clip_a[2].abs(), clip_a[1] + clip_a[3].abs()],
                [clip_b[0], clip_b[1], clip_b[0] + clip_b[2].abs(), clip_b[1] + clip_b[3].abs()],
            );
            let tint = [1.0, 1.0, 1.0, 1.0];
            if let Some(quad) = clipped(rect, clip, atlas::white().into(), tint) {
                let mag =
                    quad.x.abs() + quad.y.abs() + quad.width.abs() + quad.height.abs();
                let within = |value: f32, bound: f32| value <= bound + step(mag + bound.abs());
                let close = |a: f32, b: f32| (a - b).abs() <= step(mag);
                proptest::prop_assert!(quad.x >= rect[0]);
                proptest::prop_assert!(quad.x >= clip[0]);
                proptest::prop_assert!(within(quad.x + quad.width, rect[0] + rect[2]));
                proptest::prop_assert!(within(quad.x + quad.width, clip[2]));
                proptest::prop_assert!(quad.y >= clip[1]);
                proptest::prop_assert!(within(quad.y + quad.height, clip[3]));
                proptest::prop_assert!(quad.width > 0.0 && quad.height > 0.0);
                // Stable: clipping the clipped quad moves nothing by
                // more than the one rounding step above.
                let again = clipped(
                    [quad.x, quad.y, quad.width, quad.height],
                    clip,
                    atlas::white().into(),
                    tint,
                )
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
                clipped(
                    [rect[0], rect[1], rect[2] + 1.0, rect[3] + 1.0],
                    clip,
                    atlas::white().into(),
                    [0.0; 4]
                ),
                None
            );
        }
    }
}
