//! The allocation contract, pinned: seeding a generator and drawing from
//! it performs zero heap allocations. Own process on purpose (the counters
//! are process-wide); single test so no sibling allocates alongside.
//!
//! Measurement protocol: warmup first (lazy initialization measured out),
//! then retry windows — the counters see every thread in the process,
//! including the test harness's own output thread, so one-shot neighbor
//! noise rides out while a genuine per-draw allocation reproduces in every
//! window and still fails.
//!
//! This crate has no runtime allocation to remove — a generator is two
//! integers and every draw returns one — so the test is a tripwire rather
//! than a discovery. It fails the day someone adds a shuffle with a
//! scratch buffer, a boxed generator behind a trait, or a formatted
//! message on an error path. Randomness sits in the innermost simulation
//! loop; there is no more expensive place to start allocating.

use core::num::{NonZeroU32, NonZeroU64};

use renew_memory::{CountingAllocator, counters};
use renew_rng::{Rng, Seed, StreamId};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SPAWN: StreamId = StreamId::from_name("spawn");

fn quiet_window(attempts: usize, mut window: impl FnMut()) -> Result<(), String> {
    let mut last = (0u64, 0u64);
    for _ in 0..attempts {
        let before = counters::snapshot();
        window();
        let after = counters::snapshot();
        if after.allocations == before.allocations && after.deallocations == before.deallocations {
            return Ok(());
        }
        last = (
            after.allocations - before.allocations,
            after.deallocations - before.deallocations,
        );
    }
    Err(format!(
        "allocator activity in every window (last deltas: +{} allocations, +{} deallocations)",
        last.0, last.1
    ))
}

/// Everything a simulation does with this crate in one frame: derive a
/// per-entity stream, draw every shape, snapshot, resume. A future
/// allocation anywhere on that path is caught here.
fn draw_body(sink: &mut u64, seed: Seed, entity: u64, narrow: NonZeroU32, wide: NonZeroU64) {
    let mut rng = Rng::new(seed, SPAWN.child(entity));
    *sink = sink
        .wrapping_add(u64::from(rng.next_u32()))
        .wrapping_add(rng.next_u64())
        .wrapping_add(u64::from(rng.next_bool()))
        .wrapping_add(u64::from(rng.below_u32(narrow)))
        .wrapping_add(rng.below_u64(wide));
    let (state, increment) = rng.parts();
    let mut resumed = Rng::from_parts(state, increment);
    *sink = sink.wrapping_add(resumed.next_u64());
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn seeding_and_drawing_allocate_exactly_nothing() {
    let seed = Seed::from_u64(0x5eed_5eed_5eed_5eed);
    // A bound that provokes the rejection loop, so the retry path is
    // inside the measured window rather than beside it.
    let narrow = NonZeroU32::new(3 << 30).expect("non-zero");
    let wide = NonZeroU64::new((3 << 62) + 1).expect("non-zero");
    let mut sink = 0u64;

    for entity in 0..4 {
        draw_body(&mut sink, seed, entity, narrow, wide);
    }

    quiet_window(5, || {
        for entity in 0..4_000 {
            draw_body(&mut sink, seed, entity, narrow, wide);
        }
    })
    .expect("seeding, deriving and drawing stay heap-silent");

    // The window did real work: without this the test would pass on a
    // generator that returned zero forever.
    assert_ne!(sink, 0);
}
