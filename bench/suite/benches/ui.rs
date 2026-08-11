//! Widget-tree timings: what a frame pays for layout and for the
//! snapshot copy behind presentation, at a thousand nodes — a tree an
//! order of magnitude past any menu the samples draw, so the numbers
//! bound a real document rather than flatter a toy.
//!
//! Four costs, in the order a running game meets them: solving a tree
//! from cold, re-solving after one node's layout changed, re-solving
//! after a colour-only flip (today identical to the layout re-solve,
//! recorded so the state-table work can show the drop when colour
//! stops dirtying layout), and capturing the solved tree into the
//! presenter's snapshot pair.
//!
//! The allocation gates for these paths do not live here: the tree
//! commits to zero steady-state allocation in its own crate's
//! `tests/zero_alloc.rs` (solve, re-solve, input, digest), and the
//! presenter does the same for `advance` and `frame`. This file only
//! times what those tests already gate.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_ui::{Align, Direction, Fixed, Size, Style, Ui, UiLimits};
use renew_ui_render::UiPresenter;

/// Nodes in the benched tree: one root, `ROWS` rows, the rest split
/// evenly among them.
const NODES: u32 = 1024;
const ROWS: u32 = 31;
/// Children per row: (1024 - 1 - 31) / 31 = 32, exactly.
const PER_ROW: u32 = (NODES - 1 - ROWS) / ROWS;

/// The viewport every solve answers for.
const VIEW: (i32, i32) = (1280, 720);

/// A tree of exactly [`NODES`] nodes, styled so the solver's paths
/// all run: rows grow by weight into the column's leftover height,
/// leaves grow into their row's fixed width, sizes mix fixed and
/// content, gaps and cross-axis centring exercise arrangement. Built
/// dirty; the caller decides when the first solve happens.
fn build_tree() -> Ui {
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
                    height: Size::Px(Fixed::from_int(10)),
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

    // A colour-only flip: geometry is untouched, and today the tree
    // re-solves anyway — one dirty flag, no damage classes. This
    // number exists to be compared against `re_solve_one_dirty_1024`
    // now (they should match) and against itself when styling stops
    // dirtying layout (it should collapse).
    c.bench_function("ui_state_flip_colour_1024", |b| {
        let mut ui = build_tree();
        solve(&mut ui);
        let leaf = last_leaf(&ui);
        let mut lit = false;
        b.iter(|| {
            lit = !lit;
            let background = if lit {
                [90, 120, 200, 255]
            } else {
                [40, 44, 52, 230]
            };
            ui.set_style(
                leaf,
                Style {
                    width: Size::Px(Fixed::from_int(8)),
                    height: Size::Px(Fixed::from_int(10)),
                    background,
                    ..Style::default()
                },
            );
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
}

criterion_group!(benches, ui_benches);
criterion_main!(benches);
