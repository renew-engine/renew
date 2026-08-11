//! Mechanical enforcement of the tree's allocation contract: after
//! construction, the steady state — insert, remove, walk — performs no
//! heap allocation through the global allocator.
//!
//! Shipped with the crate's first commit rather than after it, because
//! a gate that arrives later measures whatever the code has grown into
//! rather than what it promised. Non-vacuous by construction: the
//! measured window works a tree that genuinely churns, and the test
//! asserts the churn happened.

use renew_fixed::Fixed;
use renew_frame::StateHash;
use renew_memory::{CountingAllocator, counters};
use renew_ui::{Size, Style, Ui, UiEvent, UiLimits};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_steady_state_allocates_nothing() {
    // Everything that may allocate happens out here: the arena, once.
    let mut ui = Ui::new(UiLimits { nodes: 256 });
    let root = ui.root();

    // Warmup: one full churn cycle, so any one-time lazy initialization
    // lands before the window opens.
    let first = ui.insert(root).expect("an empty tree has room");
    assert!(ui.remove(first));

    // The measured window: fill a branch to a real depth and width,
    // walk it, tear it down, repeatedly. Insert pops the free list,
    // remove pushes it back, the walk follows intrusive links — none
    // of it may touch the heap.
    let verdict = counters::quiet_window(5, || {
        for _ in 0..8 {
            let branch = ui.insert(root).expect("room for the branch");
            for _ in 0..31 {
                let limb = ui.insert(branch).expect("room under the limit");
                ui.insert(limb).expect("room for a leaf");
            }
            assert_eq!(ui.live(), 64, "the churn must really build the tree");
            let walked = ui.children(branch).count();
            assert_eq!(walked, 31, "the walk must really see the children");
            assert!(ui.remove(branch));
            assert_eq!(ui.live(), 1, "teardown must return every slot");
        }
    });
    verdict.expect("the tree's steady state stays heap-silent");

    // The solver's half of the promise: styling and re-solving a real
    // tree — dirtied every time, so every pass walks — reaches the
    // heap exactly as often as the tree does. Built (and solved once,
    // for the same warmup reason) before the window opens.
    let wide = ui.insert(root).expect("room for the solver's subject");
    for _ in 0..16 {
        ui.insert(wide).expect("room for a row child");
    }
    ui.solve(Fixed::from_int(640), Fixed::from_int(360));
    // Two styles alternated so every pass provably re-solves: the
    // rectangle must flip between the two widths, which a retained or
    // skipped solve could not produce.
    let narrow = Style {
        width: Size::Px(Fixed::from_int(20)),
        height: Size::Px(Fixed::from_int(10)),
        ..Style::default()
    };
    let wide_style = Style {
        width: Size::Px(Fixed::from_int(40)),
        ..narrow
    };
    let verdict = counters::quiet_window(5, || {
        for pass in 0..8u32 {
            let (style, expected) = if pass % 2 == 0 {
                (narrow, Fixed::from_int(20))
            } else {
                (wide_style, Fixed::from_int(40))
            };
            assert!(ui.set_style(wide, style), "the subject must be live");
            ui.solve(Fixed::from_int(640), Fixed::from_int(360));
            assert_eq!(
                ui.rect(wide).expect("the subject must be live").width,
                expected,
                "the re-solve must really lay the tree out"
            );
        }
    });
    verdict.expect("re-solving stays heap-silent");

    // The interaction half: moving, clicking, and draining decisions
    // works the preallocated queue and the retained rectangles, never
    // the heap. One click first as warmup, then the window clicks and
    // drains and asserts the decisions really flowed.
    ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
    ui.handle(UiEvent::PointerPressed);
    ui.handle(UiEvent::PointerReleased);
    assert_eq!(ui.drain_outputs().count(), 1, "the warmup click must land");
    let verdict = counters::quiet_window(5, || {
        for _ in 0..8 {
            ui.handle(UiEvent::PointerMoved { x: 5, y: 5 });
            ui.handle(UiEvent::PointerPressed);
            ui.handle(UiEvent::PointerMoved { x: 6, y: 6 });
            ui.handle(UiEvent::PointerReleased);
            assert_eq!(
                ui.drain_outputs().count(),
                1,
                "every windowed click must decide"
            );
        }
    });
    verdict.expect("interaction stays heap-silent");

    // The digest half: folding the tree's structure and decisions into
    // a hash reads retained state only. Warmed once outside the
    // window; asserted stable inside it, because a fold that read
    // nothing would also be quiet — identical digests of an unchanged
    // tree are what show the fold really ran over real state.
    let baseline = ui.absorb(StateHash::new()).finish();
    let verdict = counters::quiet_window(5, || {
        for _ in 0..8 {
            assert_eq!(
                ui.absorb(StateHash::new()).finish(),
                baseline,
                "an unchanged tree must digest identically"
            );
        }
    });
    verdict.expect("the digest stays heap-silent");
}
