//! Particle pool timings: the cost of one simulation step (on an effect
//! that turns and one that does not), one
//! instance pack and one walk of the view, at the two pool sizes that
//! bracket what a scene asks for. A thousand particles is a busy scene
//! for the current samples — the block-break burst tops out in the
//! dozens — and four thousand is the stress ceiling the pool should
//! absorb before anyone reaches for a second pool or a coarser effect.
//!
//! Every particle is given an effectively infinite lifetime, so a step
//! updates exactly the full pool on every iteration: a real effect's
//! expiry would drain the pool mid-measurement and the later
//! iterations would time an emptier and emptier update. Uniform work
//! per iteration is what makes two runs comparable.
//!
//! The allocation gate for these kernels does not live here: the pool
//! commits to zero steady-state allocation in its own crate, where
//! `tests/zero_alloc.rs` asserts exact counts over `burst`, `step`,
//! `write_instances` and the `particles()` walk on every push. This
//! file only times what that test already gates.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_particles::{EffectDesc, INSTANCE_STRIDE, ParticleSystem, Seed, StreamId, VelocityCone};

/// Arbitrary but fixed, for the same reason the generator bench pins
/// one: a varying seed makes two runs incomparable for no benefit.
const SEED: Seed = Seed::from_u64(0xBE7C_0000_0000_0001);

/// The simulation cadence the samples actually step at.
const DT: f32 = 1.0 / 60.0;

/// Longer than any benchmark run by orders of magnitude, so no
/// particle expires mid-measurement and every step touches the whole
/// pool.
const IMMORTAL: f32 = 1.0e9;

/// A pool filled to exactly `count` live particles, turning or not.
fn pool(count: u32, angle: (f32, f32), spin: (f32, f32)) -> ParticleSystem {
    let desc = EffectDesc {
        capacity: count,
        lifetime: (IMMORTAL, IMMORTAL),
        velocity: VelocityCone {
            axis: [0.0, 1.0, 0.0],
            spread: 1.0,
            speed: (1.0, 2.0),
        },
        gravity: [0.0, -5.0, 0.0],
        drag_per_step: 0.99,
        size: (0.1, 0.05),
        color: ([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]),
        tile: [0.0, 0.0, 1.0, 1.0],
        angle,
        spin,
    };
    let mut pool = ParticleSystem::new(&desc, SEED, StreamId::from_name("bench"));
    pool.burst([0.0, 0.0, 0.0], count);
    assert_eq!(
        pool.live(),
        count,
        "the pool must start full or the size lies"
    );
    pool
}

/// A full pool of an effect that does not turn — what every line
/// before the turning one measures.
fn full_pool(count: u32) -> ParticleSystem {
    pool(count, (0.0, 0.0), (0.0, 0.0))
}

/// A full pool of an effect that turns: a birth angle in `[0, 1)` turns
/// and a spin in `[−2, 2)` turns per second.
fn turning_pool(count: u32) -> ParticleSystem {
    pool(count, (0.0, 1.0), (-2.0, 2.0))
}

fn particles(c: &mut Criterion) {
    for count in [1024_u32, 4096] {
        c.bench_function(&format!("particle_update_{count}"), |b| {
            let mut pool = full_pool(count);
            b.iter(|| pool.step(black_box(DT)));
        });

        c.bench_function(&format!("particle_update_turning_{count}"), |b| {
            // The same step on an effect that turns: the spin array read
            // and the angle array written on top, which is the whole cost
            // of a spin. An effect that does not turn skips that pass.
            let mut pool = turning_pool(count);
            b.iter(|| pool.step(black_box(DT)));
        });

        c.bench_function(&format!("particle_pack_{count}"), |b| {
            let pool = full_pool(count);
            let mut instances = vec![0u8; count as usize * INSTANCE_STRIDE];
            b.iter(|| {
                // The buffer is the output; observing only the returned
                // count would let a link-time optimizer discard the
                // writes being timed.
                black_box(pool.write_instances(black_box(&mut instances)));
                black_box(&mut instances);
            });
        });

        c.bench_function(&format!("particle_view_{count}"), |b| {
            // The view reads the same arrays the packer reads and
            // writes nothing: every record is fenced whole, so the
            // position, velocity, colour and progress it computes are
            // kept live, and its size is folded into a sum,
            // fenced so the walk cannot be folded away with it.
            let pool = full_pool(count);
            b.iter(|| {
                let total: f32 = black_box(&pool)
                    .particles()
                    .map(|particle| black_box(particle).size)
                    .sum();
                black_box(total)
            });
        });
    }
}

criterion_group!(benches, particles);
criterion_main!(benches);
