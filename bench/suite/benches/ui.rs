//! Widget-tree timings: what a frame pays for layout and for the
//! snapshot copy behind presentation, at a thousand nodes — a tree an
//! order of magnitude past any menu the samples draw, so the numbers
//! bound a real document rather than flatter a toy.
//!
//! Five costs, in the order a running game meets them: solving a tree
//! from cold, re-solving after one node's layout changed, wearing a
//! colour-only state patch, capturing the solved tree into the
//! presenter's snapshot pair, and blending that pair into the frame's
//! quads.
//!
//! **The frame blend had no number until now**, which mattered:
//! presentation is the half a visual vocabulary grows — a node that
//! can carry an image, a nine-slice or a label emits more quads and
//! cuts a source rectangle as well as a destination one — and a change
//! is only measurable against a before. `ui_frame_1024` is that
//! before, taken while a node is still one white quad.
//!
//! **`emit` is deliberately not benched.** It pushes into a
//! `SpriteRenderer`, which uploads an atlas and builds a pipeline, so
//! timing it means a GPU device and a dependency this suite does not
//! have (section 11 presumes a new dependency rejected). `frame` is
//! the whole of the CPU work `emit` does before the push, so the
//! number that moves when presentation grows is the one here.
//!
//! The allocation gates for these paths do not live here: the tree
//! commits to zero steady-state allocation in its own crate's
//! `tests/zero_alloc.rs` (solve, re-solve, input, digest), and the
//! presenter does the same for `advance` and `frame`. This file only
//! times what those tests already gate.

use std::hint::black_box;
use std::num::NonZeroU64;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_math::Alpha;
use renew_ui::{
    Align, Direction, Fixed, NO_PATCH, STATE_COMBINATIONS, Size, StatePatch, Style, Ui, UiEvent,
    UiLimits,
};
use renew_ui_render::UiPresenter;

/// Nodes in the benched tree: one root, `ROWS` rows, the rest split
/// evenly among them.
const NODES: u32 = 1024;
const ROWS: u32 = 31;
/// Children per row: (1024 - 1 - 31) / 31 = 32, exactly.
const PER_ROW: u32 = (NODES - 1 - ROWS) / ROWS;

/// The viewport every solve answers for.
const VIEW: (i32, i32) = (1280, 720);

/// A two-tick step, for the halfway blend. Built up from `MIN`, which
/// is one, so the constant carries no panic path.
const TWO: NonZeroU64 = NonZeroU64::MIN.saturating_add(1);

/// A tree of exactly [`NODES`] nodes, styled so the solver's paths
/// all run: rows grow by weight into the column's leftover height,
/// leaves grow into their row's fixed width, sizes mix fixed and
/// content, gaps and cross-axis centring exercise arrangement. Built
/// dirty; the caller decides when the first solve happens.
fn build_tree() -> Ui {
    build_tree_with(OverflowS::Contained)
}

/// Whether the leaves fit inside their row, or stand out of it.
///
/// A leaf taller than its row is clipped top and bottom by the row's
/// own box, which is how the presenter's cut path is reached without
/// inventing a scroll container the solver does not have yet.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OverflowS {
    Contained,
    LeavesOverflowTheirRow,
}

fn build_tree_with(overflow: OverflowS) -> Ui {
    let mut ui = Ui::new(UiLimits { nodes: NODES });
    let root = ui.root();
    ui.set_style(
        root,
        Style {
            direction: Direction::Column,
            gap: Fixed::from_int(2),
            ..Style::default()
        },
    );
    // Successes are counted and asserted below rather than each
    // insert unwrapped: one guard, at the size the name claims.
    let mut built = 1;
    for row in 0..ROWS {
        let Ok(panel) = ui.insert(root) else { continue };
        built += 1;
        ui.set_style(
            panel,
            Style {
                direction: Direction::Row,
                // Alternating weights so the largest-remainder split
                // has remainders to hand out, and a width wide enough
                // past the leaves' content that their own growers get
                // leftover to split too.
                width: Size::Px(Fixed::from_int(1000)),
                // Fixed and short in the overflow shape, so the taller
                // leaves inside genuinely stand out of it.
                height: if overflow == OverflowS::Contained {
                    Size::Auto
                } else {
                    Size::Px(Fixed::from_int(20))
                },
                grow: 1 + row % 3,
                gap: Fixed::from_int(1),
                align_cross: Align::Center,
                background: [20, 20, 30, 255],
                ..Style::default()
            },
        );
        for child in 0..PER_ROW {
            let Ok(leaf) = ui.insert(panel) else { continue };
            built += 1;
            ui.set_style(
                leaf,
                Style {
                    width: Size::Px(Fixed::from_int(8 + i32::try_from(child % 5).unwrap_or(0))),
                    height: Size::Px(Fixed::from_int(if overflow == OverflowS::Contained {
                        10
                    } else {
                        40
                    })),
                    grow: child % 2,
                    background: [40, 44, 52, 230],
                    ..Style::default()
                },
            );
        }
    }
    assert_eq!(built, NODES, "the tree must be the size the name claims");
    ui
}

fn solve(ui: &mut Ui) {
    ui.solve(Fixed::from_int(VIEW.0), Fixed::from_int(VIEW.1));
}

/// The deepest, last-inserted leaf: the one whose restyle dirties the
/// whole tree today.
fn last_leaf(ui: &Ui) -> renew_ui::NodeId {
    let root = ui.root();
    // The fallbacks are unreachable past build_tree's size assert;
    // root merely keeps this total without unwrapping.
    let row = ui.children(root).last().unwrap_or(root);
    ui.children(row).last().unwrap_or(root)
}

fn ui_benches(c: &mut Criterion) {
    // Solving a freshly built tree: the cost of showing a document
    // for the first time. The build is setup, not measurement.
    c.bench_function("ui_solve_cold_1024", |b| {
        b.iter_batched_ref(
            build_tree,
            |ui| {
                solve(ui);
                // The root is pinned to the viewport whatever the tree
                // does; a leaf's rectangle is an answer the solve had
                // to compute.
                let leaf = last_leaf(ui);
                black_box(ui.rect(leaf));
            },
            criterion::BatchSize::LargeInput,
        );
    });

    // One node's layout changes, the tree re-solves. Two widths
    // alternate so every iteration provably lays out rather than
    // early-outing on a clean flag.
    c.bench_function("ui_re_solve_one_dirty_1024", |b| {
        let mut ui = build_tree();
        solve(&mut ui);
        let leaf = last_leaf(&ui);
        let mut wide = false;
        b.iter(|| {
            wide = !wide;
            let width = if wide { 24 } else { 8 };
            ui.set_style(
                leaf,
                Style {
                    width: Size::Px(Fixed::from_int(width)),
                    height: Size::Px(Fixed::from_int(10)),
                    background: [40, 44, 52, 230],
                    ..Style::default()
                },
            );
            solve(&mut ui);
            black_box(ui.rect(leaf));
        });
    });

    // A colour-only flip, through the mechanism that exists for it.
    //
    // **This benchmark used to call `set_style`**, which replaces the
    // authored base and dirties layout unconditionally — so the line
    // measured a full re-solve and could never show the drop it was
    // written to watch, whatever the state tables did. It wears a
    // pooled patch now, flipped by moving the pointer on and off the
    // leaf, which is what a hovered button actually does: `wear` swaps
    // one resolved style, and a patch whose `touches_layout` is false
    // must leave `layout_passes` where it was.
    c.bench_function("ui_state_flip_colour_1024", |b| {
        let mut ui = build_tree();
        solve(&mut ui);
        let leaf = last_leaf(&ui);
        let base = ui.base_style(leaf).unwrap_or_default();
        // One patch: the same box, a different colour. Geometry is
        // untouched, so it declares itself layout-free.
        let lit = StatePatch {
            style: Style {
                background: [90, 120, 200, 255],
                ..base
            },
            touches_layout: false,
        };
        assert!(ui.set_patch_pool(vec![lit]), "the pool was refused");
        let mut table = [NO_PATCH; STATE_COMBINATIONS];
        // Every combination carrying the hover bit wears the patch.
        for (bits, entry) in table.iter_mut().enumerate() {
            if u8::try_from(bits).unwrap_or(0) & renew_ui::STATE_HOVER != 0 {
                *entry = 0;
            }
        }
        assert!(ui.set_state_table(leaf, table), "the table was refused");
        solve(&mut ui);
        // On the leaf, and far away from it: the two pointer positions
        // the flip alternates between.
        // Pointer coordinates are whole physical pixels, so the centre
        // is taken in the integer space `contains` compares in.
        let rect = ui.rect(leaf).unwrap_or_default();
        let mid = |edge: Fixed, span: Fixed| {
            let low = edge.trunc_int();
            let high = (edge + span).trunc_int();
            i32::try_from(i64::midpoint(low, high)).unwrap_or(0)
        };
        let on = (mid(rect.x, rect.width), mid(rect.y, rect.height));
        let off = (-1, -1);
        let mut hovered = false;
        b.iter(|| {
            hovered = !hovered;
            let (x, y) = if hovered { on } else { off };
            ui.handle(UiEvent::PointerMoved { x, y });
            solve(&mut ui);
            black_box(ui.rect(leaf));
        });
    });

    // The snapshot copy: capturing a solved tree into the presenter's
    // pair. This is presentation's per-tick cost, paid whether or not
    // anything moved.
    c.bench_function("ui_snapshot_advance_1024", |b| {
        let mut ui = build_tree();
        solve(&mut ui);
        let mut presenter = UiPresenter::new(NODES);
        presenter.advance(&ui);
        b.iter(|| {
            presenter.advance(black_box(&ui));
        });
    });

    // The frame blend: the snapshot pair interpolated, clipped to
    // ancestor boxes and turned into quads, at the display rate rather
    // than the tick rate — so a 144 Hz screen pays this more often than
    // anything else in this file.
    //
    // Drained, because the work is in the iterator: `frame` returns a
    // lazy walk and a caller that does not consume it measures a
    // function call. The half-alpha is the honest case — every position
    // and tint is a live blend of two snapshots rather than a copy of
    // one.
    c.bench_function("ui_frame_1024", |b| {
        let mut ui = build_tree();
        solve(&mut ui);
        let mut presenter = UiPresenter::new(NODES);
        // Two advances, so the pair holds two distinct snapshots and
        // the blend has something to interpolate between.
        presenter.advance(&ui);
        presenter.advance(&ui);
        // Exactly halfway between the pair: one tick's remainder over a
        // two-tick step, so the blend is a real interpolation rather
        // than a copy of either snapshot.
        let alpha = Alpha::new(1, TWO);
        b.iter(|| {
            let mut quads = 0u32;
            for quad in presenter.frame(black_box(alpha)) {
                black_box(&quad);
                quads += 1;
            }
            black_box(quads);
        });
    });
}

fn ui_clipped_benches(c: &mut Criterion) {
    // The same blend, with the cut path taken.
    //
    // Every leaf is forty pixels tall inside a twenty-pixel row, so
    // the row's own box cuts it top and bottom: 992 of the 1,024
    // nodes take the proportional path while the 32 rows and the root
    // take the early-out. Subtracting `ui_frame_1024` and dividing by
    // 992 gives the added cost of a cut, which is the number the
    // clipping work is answerable for.
    c.bench_function("ui_frame_clipped_1024", |b| {
        let mut ui = build_tree_with(OverflowS::LeavesOverflowTheirRow);
        solve(&mut ui);
        let mut presenter = UiPresenter::new(NODES);
        presenter.advance(&ui);
        presenter.advance(&ui);
        let alpha = Alpha::new(1, TWO);
        b.iter(|| {
            let mut quads = 0u32;
            for quad in presenter.frame(black_box(alpha)) {
                black_box(&quad);
                quads += 1;
            }
            black_box(quads);
        });
    });
}

criterion_group!(benches, ui_benches, ui_clipped_benches);
criterion_main!(benches);
