//! Mechanical enforcement of the presenter's allocation contract:
//! after construction, capturing and framing allocate exactly nothing.
//!
//! Shipped with the crate's first commit rather than after it. The
//! measured window advances the presenter across a real style change
//! and walks every frame quad, asserting the quads really flowed —
//! non-vacuous by construction.

use renew_math::Alpha;
use renew_memory::{CountingAllocator, counters};
use renew_ui::{Fixed, Size, Style, Ui, UiLimits};
use renew_ui_render::UiPresenter;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_steady_state_allocates_nothing() {
    // Everything that may allocate happens out here: the tree, the
    // presenter, and one warmup pass through capture and frame.
    let mut ui = Ui::new(UiLimits { nodes: 64 });
    let root = ui.root();
    let mut nodes = Vec::new();
    for nth in 1..32 {
        let node = ui.insert(root).expect("room under the limit");
        ui.set_style(
            node,
            Style {
                width: Size::Px(Fixed::from_int(4 + nth)),
                height: Size::Px(Fixed::from_int(6)),
                background: [200, 180, 160, 255],
                ..Style::default()
            },
        );
        nodes.push(node);
    }
    ui.solve(Fixed::from_int(640), Fixed::from_int(360));
    let mut presenter = UiPresenter::new(64);
    presenter.advance(&ui);
    let half = Alpha::new(1, core::num::NonZeroU64::new(2).expect("two"));
    assert_eq!(presenter.frame(half).count(), 31, "the warmup really drew");

    // The measured window: a style change, a re-solve, a capture, and
    // a full frame walk, repeatedly — the presenter's whole per-tick
    // and per-frame life, heap-silent.
    let mut wide = false;
    let verdict = counters::quiet_window(5, || {
        for _ in 0..8 {
            wide = !wide;
            let width = if wide { 40 } else { 20 };
            let mut style = ui.style(nodes[0]).expect("live");
            style.width = Size::Px(Fixed::from_int(width));
            assert!(ui.set_style(nodes[0], style));
            ui.solve(Fixed::from_int(640), Fixed::from_int(360));
            presenter.advance(&ui);
            let drawn = presenter.frame(half).count();
            assert_eq!(drawn, 31, "every windowed frame must really draw");
        }
    });
    verdict.expect("the presenter's steady state stays heap-silent");
}
