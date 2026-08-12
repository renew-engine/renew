//! The state-patch half of the tree's allocation contract: wearing a
//! patch — the per-event refresh that swaps worn styles as hover flips —
//! reaches the heap as little as bare interaction does, which is not at
//! all.
//!
//! **Its own file, deliberately.** The `#[global_allocator]` is
//! process-wide and cargo runs one file's tests concurrently, so a
//! second counting test beside another races it: one opens its measured
//! window while the other is still freeing its fixtures, and the loser
//! reports a delta it did not cause. Separate files are separate
//! binaries and separate processes, which is the cheapest way to make
//! the counters mean what they say.

use renew_fixed::Fixed;
use renew_memory::{CountingAllocator, counters};
use renew_ui::{
    NO_PATCH, STATE_COMBINATIONS, STATE_HOVER, Size, StatePatch, Style, Ui, UiEvent, UiLimits,
};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn wearing_state_patches_allocates_nothing() {
    // Load-time allocation happens out here: the tree, the pool, the
    // table. The measured window is the per-event refresh — hover
    // flips swapping worn styles — which must stay as heap-silent as
    // bare interaction. Asserted moving so the window cannot pass by
    // wearing nothing.
    let mut ui = Ui::new(UiLimits { nodes: 8 });
    let root = ui.root();
    let wide = ui.insert(root).expect("room");
    ui.set_style(
        wide,
        Style {
            width: Size::Px(Fixed::from_int(20)),
            height: Size::Px(Fixed::from_int(10)),
            ..Style::default()
        },
    );
    assert!(ui.set_patch_pool(vec![StatePatch {
        style: Style {
            width: Size::Px(Fixed::from_int(20)),
            height: Size::Px(Fixed::from_int(10)),
            background: [9, 9, 9, 255],
            ..Style::default()
        },
        touches_layout: false,
    }]));
    let mut table = [NO_PATCH; STATE_COMBINATIONS];
    for (bits, entry) in table.iter_mut().enumerate() {
        if u8::try_from(bits).unwrap_or(0) & STATE_HOVER != 0 {
            *entry = 0;
        }
    }
    assert!(ui.set_state_table(wide, table));
    ui.solve(Fixed::from_int(640), Fixed::from_int(360));
    let verdict = counters::quiet_window(5, || {
        for _ in 0..8 {
            ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
            assert_eq!(
                ui.style(wide).expect("live").background,
                [9, 9, 9, 255],
                "the hover patch must really be worn"
            );
            ui.handle(UiEvent::PointerMoved { x: 600, y: 300 });
            assert_ne!(
                ui.style(wide).expect("live").background,
                [9, 9, 9, 255],
                "leaving must really shed it"
            );
        }
    });
    verdict.expect("wearing state patches stays heap-silent");
}
