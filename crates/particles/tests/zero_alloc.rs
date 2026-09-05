//! Mechanical enforcement of the crate's allocation contract: after
//! construction, the steady state — burst, step, pack, view — performs no
//! heap allocation through the global allocator.
//!
//! Shipped with the crate's first commit rather than after it, because
//! a gate that arrives later measures whatever the code has grown into
//! rather than what it promised; the register records exactly that
//! lesson from a sibling crate. Non-vacuous by construction: the
//! measured window works a pool that is genuinely alive, and the test
//! asserts so.

use renew_memory::{CountingAllocator, counters};
use renew_particles::{EffectDesc, INSTANCE_STRIDE, ParticleSystem, Seed, StreamId, VelocityCone};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_steady_state_allocates_nothing() {
    let desc = EffectDesc {
        capacity: 256,
        lifetime: (0.2, 0.6),
        velocity: VelocityCone {
            axis: [0.0, 1.0, 0.0],
            spread: 0.75,
            speed: (1.0, 4.0),
        },
        gravity: [0.0, -9.8, 0.0],
        drag_per_step: 0.97,
        size: (0.3, 0.02),
        color: ([1.0, 0.9, 0.5, 1.0], [0.1, 0.05, 0.02, 0.0]),
        tile: [0.0, 0.0, 1.0, 1.0],
    };
    // Everything that may allocate happens out here: the pool and the
    // packing buffer, once.
    let mut system = ParticleSystem::new(
        &desc,
        Seed::from_u64(20_260_811),
        StreamId::from_name("gate"),
    );
    let mut bytes = vec![0u8; desc.capacity as usize * INSTANCE_STRIDE];

    // Warmup: reach a steady mix of spawning, aging and dying.
    for round in 0u8..8 {
        system.burst([f32::from(round), 0.0, 0.0], 24);
        for _ in 0..8 {
            system.step(1.0 / 60.0);
        }
        system.write_instances(&mut bytes);
    }

    let verdict = counters::quiet_window(5, || {
        for round in 0u8..16 {
            system.burst([f32::from(round), 0.5, 0.0], 24);
            for _ in 0..8 {
                system.step(1.0 / 60.0);
            }
            let live = system.write_instances(&mut bytes);
            // The window must measure real work: a pool that emptied
            // would pass vacuously, packing nothing.
            assert!(
                live > 0,
                "the measured window went vacuous at round {round}"
            );
            // The view inside the window too: a walk that folds every
            // size into a sum, fenced so nothing folds the walk away,
            // and asserted positive so the walk provably visited a
            // live particle.
            let total: f32 = system
                .particles()
                .map(|particle| std::hint::black_box(particle).size)
                .sum();
            let total = std::hint::black_box(total);
            assert!(
                total.is_finite() && total > 0.0,
                "the view walked nothing at round {round}"
            );
        }
    });
    if let Err(activity) = verdict {
        panic!("the particle steady state was loud in every window (last: {activity})");
    }
}
