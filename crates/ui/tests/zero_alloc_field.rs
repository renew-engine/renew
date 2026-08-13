//! The field pool's allocation contract, in its own file.
//!
//! **One test per file, and this is why.** The `#[global_allocator]` is
//! process-wide and cargo runs a file's tests in parallel, so two
//! measured windows in one binary see each other's allocations. The
//! tree's gate says so in its own header; this gate was written into
//! that file first and failed exactly as predicted — passing alone,
//! failing beside its neighbour. The convention was already written
//! down, and reading one screen further would have saved the round
//! trip.

use renew_memory::{CountingAllocator, counters};
use renew_ui::{EditOp, Fixed, Size, Style, Ui, UiEvent, UiLimits};

/// **Without this the gate measures nothing.** Integration tests are
/// separate binaries, so a sibling file's allocator does not apply here:
/// `counters` would be written by nobody, every snapshot would read
/// zero, and the window would pass on its first attempt whatever the
/// code did. The first version of this file omitted it, and that
/// proved the point by allocating four kilobytes inside the measured
/// closure — still green.
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Claiming a field, typing into it, and editing it are heap-silent too.
///
/// The pool is an inline array and the encoder's scratch is four bytes
/// on the stack, so by inspection nothing here can allocate. That is
/// exactly the argument this file exists to distrust: a gate written
/// after the code measures what the code grew into rather than what it
/// promised, and text entry arrived after the gate did.
#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn text_entry_stays_heap_silent() {
    let mut ui = Ui::new(UiLimits { nodes: 16 });
    let root = ui.root();
    let node = ui.insert(root).expect("room");
    ui.set_style(
        node,
        Style {
            width: Size::Px(Fixed::from_int(40)),
            height: Size::Px(Fixed::from_int(20)),
            ..Style::default()
        },
    );
    ui.solve(Fixed::from_int(100), Fixed::from_int(100));

    // Focus by clicking, and warm every path once outside the window so
    // a first-call allocation cannot be mistaken for a steady-state one.
    let rect = ui.rect(node).expect("a solved node has a box");
    let x = i32::try_from(rect.x.trunc_int() + 1).unwrap_or(0);
    let y = i32::try_from(rect.y.trunc_int() + 1).unwrap_or(0);
    ui.handle(UiEvent::PointerMoved { x, y });
    ui.handle(UiEvent::PointerPressed);
    ui.handle(UiEvent::PointerReleased);
    let _ = ui.drain_outputs().count();
    ui.make_field(node).expect("a free slot");
    ui.handle(UiEvent::TextEntered { ch: u32::from('w') });
    ui.handle(UiEvent::Edit {
        op: EditOp::Backspace,
    });

    let verdict = counters::quiet_window(5, || {
        for round in 0..16u32 {
            // A multi-byte scalar on some rounds, because the encoder
            // and the character walks are the parts with any chance of
            // reaching for memory.
            let ch = if round % 3 == 0 { 'é' } else { 'w' };
            ui.handle(UiEvent::TextEntered { ch: u32::from(ch) });
            // **The window must work, and be seen to.** The first
            // version typed and then deleted, so the field was empty
            // every round and the gate would have gone quiet rather than
            // red if focus or the pool broke. Its sibling states the
            // policy: the measured window works something that genuinely
            // churns, and the test asserts the churn happened.
            assert!(
                !ui.field_text(node).unwrap_or_default().is_empty(),
                "round {round}: the keystroke did not reach the field"
            );
            ui.handle(UiEvent::Edit { op: EditOp::Left });
            ui.handle(UiEvent::Edit { op: EditOp::Delete });
            ui.handle(UiEvent::Edit { op: EditOp::End });
        }
        // Reclaiming a slot walks the pool and asks the arena who is
        // live; neither should touch the heap either. **Re-runnable on
        // purpose:** a loud attempt makes `quiet_window` call this
        // closure again, so a body that consumed the tree would report
        // its own broken setup instead of the allocation it measured. A
        // is exactly what happened. The node is re-made rather than
        // assumed to survive.
        let scratch = ui.insert(root).expect("room");
        ui.make_field(scratch).expect("a free slot");
        assert_eq!(
            ui.field_text(scratch),
            Some(&[][..]),
            "a freshly claimed slot must be readable and empty"
        );
        assert!(ui.remove(scratch));
        assert_eq!(ui.field_text(scratch), None, "and gone once removed");
    });
    verdict.expect("text entry stays heap-silent");
}
