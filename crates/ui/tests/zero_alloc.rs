//! Mechanical enforcement of the tree's allocation contract: after
//! construction, the steady state — insert, remove, walk — performs no
//! heap allocation through the global allocator.
//!
//! Shipped with the crate's first commit rather than after it, because
//! a gate that arrives later measures whatever the code has grown into
//! rather than what it promised. Non-vacuous by construction: the
//! measured window works a tree that genuinely churns, and the test
//! asserts the churn happened.

use renew_memory::{CountingAllocator, counters};
use renew_ui::{Ui, UiLimits};

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
}
