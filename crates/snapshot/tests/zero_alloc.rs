//! Mechanical enforcement of the pair's allocation contract: after
//! construction, capturing and framing allocate exactly nothing.
//!
//! Shipped with the crate's first commit rather than after it. The
//! measured window recycles a slot every other step, so the branch that
//! produces a dying entry beside a newborn — the one path that yields
//! more rows than were put — is inside the window rather than beside it.

use renew_math::Alpha;
use renew_memory::{CountingAllocator, counters};
use renew_snapshot::{Fate, Key, Snapshots};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_steady_state_allocates_nothing() {
    // Everything that may allocate happens out here: the pair itself and
    // one warmup capture-and-frame.
    let mut pair = Snapshots::<f32>::new(16);
    let half = Alpha::new(1, core::num::NonZeroU64::new(2).expect("two"));
    {
        let mut capture = pair.capture();
        for slot in 0u16..8 {
            capture.put(Key::new(u32::from(slot), 0), f32::from(slot));
        }
    }
    assert_eq!(pair.frame(half).count(), 8, "the warmup really drew");

    // The measured window: a capture and a full frame walk, repeatedly,
    // with slot 3's tenant replaced on every other step so the recycle
    // path is measured too — that is the branch that emits a dying row
    // beside a newborn, and it is the one a naive implementation would
    // service with a temporary.
    let mut generation = 0u64;
    let verdict = counters::quiet_window(5, || {
        for step in 0u16..8 {
            if step % 2 == 0 {
                generation += 1;
            }
            {
                let mut capture = pair.capture();
                for slot in 0u16..8 {
                    let generation = if slot == 3 { generation } else { 0 };
                    capture.put(
                        Key::new(u32::from(slot), generation),
                        f32::from(slot) + f32::from(step),
                    );
                }
            }
            let mut drawn = 0;
            let mut dying = 0;
            for row in pair.frame(half) {
                drawn += 1;
                if row.fate == Fate::Dying {
                    dying += 1;
                }
            }
            // Eight live slots always; on a step that replaced slot 3's
            // tenant, its predecessor draws once more underneath. Step 0
            // counts: the warmup left slot 3 at generation 0 and the
            // first step bumps it, so the very first window frame already
            // walks the recycle path.
            let expected_dying = usize::from(step % 2 == 0);
            assert_eq!(
                dying, expected_dying,
                "the recycle path must really be exercised"
            );
            assert_eq!(
                drawn,
                8 + expected_dying,
                "every windowed frame must really draw"
            );
        }
    });
    verdict.expect("the pair's steady state stays heap-silent");
}
