//! Mechanical enforcement of the pass's allocation contract: once the world
//! has stopped growing, propagating it allocates exactly nothing.
//!
//! The measured window moves the hierarchy rather than just re-running it —
//! locals are rewritten every step and a subtree is re-parented — because a
//! pass that allocated only when the answer *changed* would sail through a
//! window that asked it the same question five times.
//!
//! One counting test per file, always: the allocator is process-global and
//! cargo runs a file's tests concurrently, so a second one here would measure
//! this one's allocations and fail on a defect that does not exist.

use renew_ecs::{Entities, Entity, Store};
use renew_fixed::{Angle, Fixed, Vec2};
use renew_memory::{CountingAllocator, counters};
use renew_scene::{Global, Local, Parent, Scratch, propagate};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const NODES: u32 = 64;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_steady_state_allocates_nothing() {
    // Everything that may allocate happens out here: the stores, the scratch,
    // and one warmup pass that sizes both of them to the world.
    let mut entities = Entities::new();
    let mut parents: Store<Parent> = Store::default();
    let mut locals: Store<Local> = Store::default();
    let mut globals: Store<Global> = Store::default();
    let mut scratch = Scratch::with_capacity(NODES as usize);

    // A chain laid out so that the climb actually climbs. The entity allocator
    // hands recycled slots back newest-first, so burning `NODES` slots and
    // freeing them in order makes the next `NODES` spawns descend — the root
    // takes the highest slot and each child one lower. `Entities::iter()` then
    // seeds at the *leaf*, and one walk runs the whole depth.
    //
    // Spawned in ascending order instead, as this test first was, every node
    // finds its parent already placed and the ancestry buffer never holds more
    // than one entry — so the gate would pass with the buffer removed
    // altogether, which is the opposite of what it is here to prove.
    let burnt: Vec<Entity> = (0..NODES).map(|_| entities.spawn()).collect();
    for handle in burnt {
        entities.despawn(handle);
    }
    let handles: Vec<Entity> = (0..NODES).map(|_| entities.spawn()).collect();
    for (depth, handle) in handles.iter().enumerate() {
        locals.insert(handle.index(), Local::new(unit(1), Angle::from_degrees(5)));
        if depth > 0 {
            parents.insert(handle.index(), Parent(handles[depth - 1]));
        }
    }
    assert!(
        handles
            .windows(2)
            .all(|pair| pair[1].index() < pair[0].index()),
        "premise: every child must hold a lower slot than its parent, or the \
         climb is one deep and this measures nothing"
    );

    let warmup = propagate(&mut scratch, &entities, &parents, &locals, &mut globals);
    assert_eq!(warmup.nodes, NODES, "the warmup really walked the world");

    let mut step = 0i32;
    let verdict = counters::quiet_window(5, || {
        for _ in 0..4 {
            step = step.wrapping_add(1);

            // Move everything: a pass that allocated only on change would
            // otherwise never be asked to.
            for handle in &handles {
                locals.insert(
                    handle.index(),
                    Local::new(unit(step % 7), Angle::from_degrees(step % 360)),
                );
            }

            // And re-shape it, alternating the tail between hanging off the
            // head and hanging off the world — re-parenting is the operation
            // a caller does at runtime, and it changes the climb's depth.
            let tail = handles[NODES as usize / 2];
            if step % 2 == 0 {
                parents.insert(tail.index(), Parent(handles[0]));
            } else {
                parents.remove(tail.index());
            }

            let counts = propagate(&mut scratch, &entities, &parents, &locals, &mut globals);
            assert_eq!(counts.nodes, NODES, "every windowed pass really ran");
            assert_eq!(counts.cyclic, 0);
            assert_eq!(
                counts.roots,
                if step % 2 == 0 { 1 } else { 2 },
                "the re-parenting must really take effect, or the window is \
                 measuring the same shape four times"
            );
        }
    });

    verdict.expect("propagating a settled world stays heap-silent");
}

fn unit(x: i32) -> Vec2 {
    Vec2::new(Fixed::from_int(x), Fixed::ZERO)
}
