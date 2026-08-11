//! Presentation-side particles: a fixed-capacity pool stepped at the
//! simulation's cadence, seeded so a replayed trace reproduces the same
//! picture bit for bit.
//!
//! # Contract
//!
//! - **Nothing here is simulation state.** No particle value is
//!   digested, and no simulation system reads one back — the data flow
//!   is strictly one way, from digested observables (a tick, a cell, an
//!   event ordinal) into the seed and the bursts, and from there to
//!   pixels. That is what lets this crate compute in floats while the
//!   worlds it decorates stay fixed-point.
//! - **Reproducible anyway, as a tested property.** The same effect,
//!   seed and burst sequence stepped the same number of times packs the
//!   same bytes — on this machine by a repeated-run test, and across
//!   every platform the engine builds for by a committed hash the
//!   ordinary suite asserts on every lane. The update restricts itself
//!   to IEEE correctly-rounded operations: add, subtract, multiply,
//!   divide, min, max, and square root. No transcendental function
//!   runs per particle anywhere in this crate.
//! - **All allocation happens at construction.** `step`,
//!   [`ParticleSystem::burst`] and [`ParticleSystem::write_instances`]
//!   allocate nothing, gate-tested from the crate's first commit; a
//!   burst past capacity saturates rather than growing.
//!
//! The GPU-facing half — the billboard pipeline, the atlas, the draw —
//! arrives as its own module with its own dependency when the renderer
//! lands; this half never touches a device, which is what makes every
//! claim above testable on any machine. Which blend mode an effect
//! draws with belongs to that half too: it is a property of a pipeline,
//! not of arithmetic.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use renew_rng::Rng;
pub use renew_rng::{Seed, StreamId};

/// Bytes per packed instance record: centre and size in one four-float
/// group, a premultiplied colour, and an atlas rectangle.
pub const INSTANCE_STRIDE: usize = 48;

/// What an effect is: pure data a generic pool interprets. No file, no
/// parser — an effect is authored in code in v0, and a file format is a
/// deliberate later step with its own obligations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectDesc {
    /// The pool and buffer size, fixed at creation. A burst past it
    /// saturates.
    pub capacity: u32,
    /// How long a particle lives, in seconds — each particle draws a
    /// lifetime uniformly from this range.
    pub lifetime: (f32, f32),
    /// Where new particles fly: a direction and how tightly they hold
    /// it.
    pub velocity: VelocityCone,
    /// Acceleration applied every second, in world units.
    pub gravity: [f32; 3],
    /// The factor a velocity is multiplied by once per step.
    ///
    /// **Per step, not per second, deliberately**: the step length is
    /// fixed at the simulation's cadence, so the caller states the
    /// factor it means and no power function ever runs — `powf` is not
    /// correctly rounded and differs between platform maths libraries,
    /// which would quietly break the cross-platform hash this crate
    /// commits to.
    pub drag_per_step: f32,
    /// Size at birth and at death, lerped over the particle's life.
    pub size: (f32, f32),
    /// Premultiplied colour at birth and at death, lerped over life.
    pub color: ([f32; 4], [f32; 4]),
    /// The atlas rectangle every particle of this effect samples:
    /// minimum u, minimum v, maximum u, maximum v. One tile per effect
    /// in v0; ranges arrive with a consumer that varies them.
    pub tile: [f32; 4],
}

/// A direction and a spread: where particles fly.
///
/// The spread is the radius of a jitter ball added to the axis before
/// normalizing — zero flies every particle exactly along the axis, one
/// jitters by as much as the axis itself, and larger values approach
/// uniform-in-a-sphere. **A jitter radius rather than an angle**,
/// because turning an angle into a direction costs trigonometry, which
/// is not correctly rounded and would break the cross-platform hash;
/// a caller that thinks in angles converts once, at authoring time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VelocityCone {
    /// The direction particles fly, before jitter. Need not be unit
    /// length; the jitter scales with it.
    pub axis: [f32; 3],
    /// The jitter ball's radius, as a fraction of the axis length.
    pub spread: f32,
    /// Speed at the slow and fast ends, drawn uniformly.
    pub speed: (f32, f32),
}

/// A fixed-capacity particle pool and the generator that feeds it.
///
/// Structure-of-arrays: the update is a linear pass per property, which
/// is the layout the access pattern asks for. Every array is allocated
/// once in [`ParticleSystem::new`] and never grows.
pub struct ParticleSystem {
    desc: EffectDesc,
    rng: Rng,
    position: Vec<[f32; 3]>,
    velocity: Vec<[f32; 3]>,
    age: Vec<f32>,
    lifetime: Vec<f32>,
}

impl ParticleSystem {
    /// A pool for `desc`, seeded for one stream of one run.
    ///
    /// The seed is the caller's statement of *which* burst sequence
    /// this is — derive it from digested observables and a replayed
    /// trace reproduces the identical picture.
    ///
    /// Every float in `desc` must be finite, and the lifetime endpoints
    /// non-negative — asserted in dev builds, because the failure a NaN
    /// buys otherwise is silent and strange: `age >= NaN` is never
    /// true, so a NaN lifetime makes particles immortal and pins the
    /// pool at capacity forever.
    #[must_use]
    pub fn new(desc: &EffectDesc, seed: Seed, stream: StreamId) -> Self {
        debug_assert!(
            [
                desc.lifetime.0,
                desc.lifetime.1,
                desc.velocity.axis[0],
                desc.velocity.axis[1],
                desc.velocity.axis[2],
                desc.velocity.spread,
                desc.velocity.speed.0,
                desc.velocity.speed.1,
                desc.gravity[0],
                desc.gravity[1],
                desc.gravity[2],
                desc.drag_per_step,
            ]
            .iter()
            .all(|value| value.is_finite())
                && desc.lifetime.0 >= 0.0
                && desc.lifetime.1 >= 0.0,
            "an effect's numbers must be finite (and lifetimes non-negative): a NaN here \
             makes particles immortal rather than failing anywhere visible"
        );
        let capacity = desc.capacity as usize;
        Self {
            desc: *desc,
            rng: Rng::new(seed, stream),
            position: Vec::with_capacity(capacity),
            velocity: Vec::with_capacity(capacity),
            age: Vec::with_capacity(capacity),
            lifetime: Vec::with_capacity(capacity),
        }
    }

    /// How many particles are alive.
    #[must_use]
    pub fn live(&self) -> u32 {
        // The pool is bounded by `capacity`, a u32; a length past that
        // is a broken invariant, asserted in dev builds. The saturation
        // is the release build's last defence, unreachable while the
        // assertion above it holds.
        debug_assert!(
            self.position.len() <= self.desc.capacity as usize,
            "the pool exceeded its own capacity"
        );
        u32::try_from(self.position.len()).unwrap_or(u32::MAX)
    }

    /// Spawn up to `count` particles at `at`, saturating at capacity.
    ///
    /// The number of generator draws depends only on how many particles
    /// actually spawn, which makes the draw count part of the
    /// reproducibility contract rather than an accident of load.
    pub fn burst(&mut self, at: [f32; 3], count: u32) {
        let room = self.desc.capacity.saturating_sub(self.live());
        for _ in 0..count.min(room) {
            let direction = self.cone_direction();
            let speed = lerp(
                self.desc.velocity.speed.0,
                self.desc.velocity.speed.1,
                self.unit(),
            );
            let life = lerp(self.desc.lifetime.0, self.desc.lifetime.1, self.unit());
            self.position.push(at);
            self.velocity.push([
                direction[0] * speed,
                direction[1] * speed,
                direction[2] * speed,
            ]);
            self.age.push(0.0);
            self.lifetime.push(life);
        }
    }

    /// Advance every particle by `dt_seconds` — once per completed
    /// simulation step, so particle state is a pure function of the
    /// seed, the burst sequence, and the step count.
    ///
    /// The integrator's order is observable, hash-pinned behaviour, so
    /// it is stated: each step, a velocity gains gravity times `dt`, is
    /// multiplied by the drag factor, and the post-drag velocity moves
    /// the position — semi-implicit Euler with drag inside the step.
    pub fn step(&mut self, dt_seconds: f32) {
        let gravity = self.desc.gravity;
        let drag = self.desc.drag_per_step;
        for (velocity, position) in self.velocity.iter_mut().zip(self.position.iter_mut()) {
            velocity[0] = (velocity[0] + gravity[0] * dt_seconds) * drag;
            velocity[1] = (velocity[1] + gravity[1] * dt_seconds) * drag;
            velocity[2] = (velocity[2] + gravity[2] * dt_seconds) * drag;
            position[0] += velocity[0] * dt_seconds;
            position[1] += velocity[1] * dt_seconds;
            position[2] += velocity[2] * dt_seconds;
        }
        for age in &mut self.age {
            *age += dt_seconds;
        }
        // Backwards, so a swap_remove never skips the element swapped
        // into the hole — it has already been visited.
        let mut index = self.position.len();
        while index > 0 {
            index -= 1;
            if self.age[index] >= self.lifetime[index] {
                self.position.swap_remove(index);
                self.velocity.swap_remove(index);
                self.age.swap_remove(index);
                self.lifetime.swap_remove(index);
            }
        }
    }

    /// Pack every live particle into `out` and answer how many.
    ///
    /// The record is [`INSTANCE_STRIDE`] bytes: centre xyz and size in
    /// one group, premultiplied colour, atlas rectangle — native byte
    /// order, which is what a GPU on every target this engine builds
    /// for expects. The count and the bytes derive from one walk over
    /// one length, so they cannot disagree.
    ///
    /// # Panics
    ///
    /// `out` shorter than `live() * INSTANCE_STRIDE` bytes is a
    /// contract violation, asserted: the length bounds a packing loop,
    /// and truncating instead would be a quiet wrong draw.
    pub fn write_instances(&self, out: &mut [u8]) -> u32 {
        let live = self.position.len();
        assert!(
            out.len() >= live * INSTANCE_STRIDE,
            "the instance buffer holds {} bytes and {live} live particles need {}",
            out.len(),
            live * INSTANCE_STRIDE
        );
        for index in 0..live {
            let t = progress(self.age[index], self.lifetime[index]);
            let size = lerp(self.desc.size.0, self.desc.size.1, t);
            let color = lerp4(self.desc.color.0, self.desc.color.1, t);
            let position = self.position[index];
            let record = [
                position[0],
                position[1],
                position[2],
                size,
                color[0],
                color[1],
                color[2],
                color[3],
                self.desc.tile[0],
                self.desc.tile[1],
                self.desc.tile[2],
                self.desc.tile[3],
            ];
            let base = index * INSTANCE_STRIDE;
            for (slot, value) in record.iter().enumerate() {
                let at = base + slot * 4;
                out[at..at + 4].copy_from_slice(&value.to_ne_bytes());
            }
        }
        self.live()
    }

    /// A direction inside the effect's cone: the axis plus a point in
    /// a jitter ball, normalized.
    ///
    /// The ball point comes from rejection sampling — draw a point in
    /// the unit cube, keep it when it lands inside the unit ball — so
    /// the only operations are the generator's integers, the exact
    /// unit-interval conversion, multiplication, and one square root,
    /// every one of them identical on every conformant platform. The
    /// number of draws varies with the rejections, and that variation
    /// is deterministic too: it depends only on the generator's state,
    /// which depends only on the seed and the call sequence.
    fn cone_direction(&mut self) -> [f32; 3] {
        let jitter = loop {
            let x = self.unit() * 2.0 - 1.0;
            let y = self.unit() * 2.0 - 1.0;
            let z = self.unit() * 2.0 - 1.0;
            let length_squared = x * x + y * y + z * z;
            if length_squared <= 1.0 && length_squared > 0.0 {
                break [x, y, z];
            }
        };
        let axis = self.desc.velocity.axis;
        let spread = self.desc.velocity.spread;
        // The jitter scales with the axis length, so "spread 0.5" means
        // the same shape whatever units the axis is in.
        let axis_length = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
        let scale = axis_length * spread;
        let blended = [
            axis[0] + jitter[0] * scale,
            axis[1] + jitter[1] * scale,
            axis[2] + jitter[2] * scale,
        ];
        let length =
            (blended[0] * blended[0] + blended[1] * blended[1] + blended[2] * blended[2]).sqrt();
        if length > 0.0 {
            [
                blended[0] / length,
                blended[1] / length,
                blended[2] / length,
            ]
        } else {
            // A zero axis with zero spread, or a jitter that exactly
            // cancels the axis: any fixed direction keeps the pool
            // finite, and a caller that meant something else will see
            // every particle fly the same telltale way.
            [0.0, 1.0, 0.0]
        }
    }

    /// The next value in `[0, 1)`, exactly: the generator's high 24
    /// bits over 2^24, every step of which is exact in f32.
    #[expect(
        clippy::cast_precision_loss,
        reason = "24 bits into f32's 24-bit mantissa is exact by construction"
    )]
    fn unit(&mut self) -> f32 {
        (self.rng.next_u32() >> 8) as f32 / 16_777_216.0
    }
}

impl core::fmt::Debug for ParticleSystem {
    /// The counts, not the contents: sixty thousand floats are not
    /// information.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ParticleSystem")
            .field("live", &self.position.len())
            .field("capacity", &self.desc.capacity)
            .finish_non_exhaustive()
    }
}

/// Where in its life a particle is, clamped to `[0, 1]`.
fn progress(age: f32, lifetime: f32) -> f32 {
    if lifetime > 0.0 {
        (age / lifetime).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// `start + (end - start) * t` — exact at `t == 0` (which the
/// birth-values unit test pins bit-for-bit) and one rounding away from
/// `end` at `t == 1`, which is why nothing pins that endpoint exactly.
fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

fn lerp4(start: [f32; 4], end: [f32; 4], t: f32) -> [f32; 4] {
    [
        lerp(start[0], end[0], t),
        lerp(start[1], end[1], t),
        lerp(start[2], end[2], t),
        lerp(start[3], end[3], t),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst_effect() -> EffectDesc {
        EffectDesc {
            capacity: 64,
            lifetime: (0.5, 1.5),
            velocity: VelocityCone {
                axis: [0.0, 1.0, 0.0],
                spread: 0.5,
                speed: (2.0, 5.0),
            },
            gravity: [0.0, -9.8, 0.0],
            drag_per_step: 0.98,
            size: (0.25, 0.05),
            color: ([1.0, 0.8, 0.3, 1.0], [0.2, 0.1, 0.05, 0.0]),
            tile: [0.0, 0.0, 0.5, 0.5],
        }
    }

    const DT: f32 = 1.0 / 60.0;

    /// The committed cross-platform guard: a fixed scenario's packed
    /// bytes hash to one value, asserted on every platform the ordinary
    /// suite runs on. **This constant is the claim** that the update's
    /// arithmetic is identical everywhere; a platform that disagrees
    /// fails here, by name, rather than drawing subtly different
    /// confetti nobody compares.
    #[test]
    fn the_packed_bytes_hash_to_the_committed_value_on_every_platform() {
        let mut system = ParticleSystem::new(
            &burst_effect(),
            Seed::from_u64(20_260_811),
            StreamId::from_name("guard"),
        );
        system.burst([1.0, 2.0, 3.0], 40);
        for _ in 0..30 {
            system.step(DT);
        }
        system.burst([0.0, 0.5, 0.0], 24);
        for _ in 0..10 {
            system.step(DT);
        }
        let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
        let count = system.write_instances(&mut bytes);
        assert!(count > 0, "the scenario must leave something to hash");
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in &bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(
            hash, 0x8f42_1c8a_4bec_0567,
            "the packed bytes moved: either the update's arithmetic changed (bump this \
             constant in the same change, deliberately) or this platform computes \
             differently (which is the finding)"
        );
    }

    /// Same seed, same calls, same bytes — the repeated-run half of the
    /// reproducibility claim.
    #[test]
    fn the_same_scenario_packs_the_same_bytes_twice() {
        let run = || {
            let mut system = ParticleSystem::new(
                &burst_effect(),
                Seed::from_u64(7),
                StreamId::from_name("twice"),
            );
            system.burst([0.0, 0.0, 0.0], 32);
            for _ in 0..20 {
                system.step(DT);
            }
            let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
            system.write_instances(&mut bytes);
            bytes
        };
        assert_eq!(run(), run(), "a replay must reproduce the picture");
    }

    /// Particles die when their lifetime ends, and the pool compacts.
    #[test]
    fn particles_expire_and_the_pool_compacts() {
        let mut system = ParticleSystem::new(
            &burst_effect(),
            Seed::from_u64(3),
            StreamId::from_name("expiry"),
        );
        system.burst([0.0, 0.0, 0.0], 16);
        assert_eq!(system.live(), 16);
        // The longest possible lifetime is 1.5 seconds; step past it.
        for _ in 0..120 {
            system.step(DT);
        }
        assert_eq!(system.live(), 0, "every particle should have died");
    }

    /// At birth the packed size and colour are exactly the start values
    /// — the lerp's zero endpoint is exact, not close.
    #[test]
    fn a_newborn_particle_wears_its_birth_size_and_colour() {
        let desc = burst_effect();
        let mut system =
            ParticleSystem::new(&desc, Seed::from_u64(11), StreamId::from_name("birth"));
        system.burst([0.0, 0.0, 0.0], 1);
        let mut bytes = [0u8; INSTANCE_STRIDE];
        system.write_instances(&mut bytes);
        let float_at = |index: usize| {
            f32::from_ne_bytes([
                bytes[index * 4],
                bytes[index * 4 + 1],
                bytes[index * 4 + 2],
                bytes[index * 4 + 3],
            ])
        };
        assert_eq!(
            float_at(3).to_bits(),
            desc.size.0.to_bits(),
            "size at birth"
        );
        for channel in 0..4 {
            assert_eq!(
                float_at(4 + channel).to_bits(),
                desc.color.0[channel].to_bits(),
                "colour channel {channel} at birth"
            );
        }
        for corner in 0..4 {
            assert_eq!(
                float_at(8 + corner).to_bits(),
                desc.tile[corner].to_bits(),
                "tile corner {corner}"
            );
        }
    }

    /// A short buffer is refused by name, never truncated into a quiet
    /// wrong draw.
    #[test]
    fn a_short_instance_buffer_is_refused() {
        let mut system = ParticleSystem::new(
            &burst_effect(),
            Seed::from_u64(13),
            StreamId::from_name("short"),
        );
        system.burst([0.0, 0.0, 0.0], 2);
        let mut bytes = [0u8; INSTANCE_STRIDE]; // room for one, two live
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            system.write_instances(&mut bytes);
        }));
        assert!(outcome.is_err(), "a short buffer must refuse, not truncate");
    }

    /// The zero-spread cone is exactly the axis, unit length, for every
    /// particle — the degenerate the jitter design makes exact.
    #[test]
    fn a_zero_spread_cone_flies_straight() {
        let mut desc = burst_effect();
        desc.velocity.spread = 0.0;
        desc.velocity.axis = [0.0, 3.0, 0.0];
        desc.gravity = [0.0, 0.0, 0.0];
        desc.drag_per_step = 1.0;
        let mut system =
            ParticleSystem::new(&desc, Seed::from_u64(17), StreamId::from_name("straight"));
        system.burst([0.0, 0.0, 0.0], 8);
        system.step(DT);
        let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
        system.write_instances(&mut bytes);
        for index in 0..system.live() as usize {
            let base = index * INSTANCE_STRIDE;
            let x = f32::from_ne_bytes(bytes[base..base + 4].try_into().unwrap());
            let z = f32::from_ne_bytes(bytes[base + 8..base + 12].try_into().unwrap());
            assert_eq!(x.to_bits(), 0.0f32.to_bits(), "no sideways drift");
            assert_eq!(z.to_bits(), 0.0f32.to_bits(), "no sideways drift");
        }
    }

    /// A zero axis with zero spread flies the documented fallback —
    /// every particle the same telltale way, asserted rather than
    /// promised in a comment.
    #[test]
    fn a_degenerate_cone_flies_the_documented_fallback() {
        let mut desc = burst_effect();
        desc.velocity.axis = [0.0, 0.0, 0.0];
        desc.velocity.spread = 0.0;
        desc.velocity.speed = (1.0, 1.0);
        desc.gravity = [0.0, 0.0, 0.0];
        desc.drag_per_step = 1.0;
        let mut system =
            ParticleSystem::new(&desc, Seed::from_u64(29), StreamId::from_name("fallback"));
        system.burst([0.0, 0.0, 0.0], 4);
        system.step(1.0);
        let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
        system.write_instances(&mut bytes);
        for index in 0..system.live() as usize {
            let base = index * INSTANCE_STRIDE;
            let y = f32::from_ne_bytes(bytes[base + 4..base + 8].try_into().unwrap());
            assert!(
                y > 0.0,
                "the fallback direction is +y, so a second of flight rises: {y}"
            );
        }
    }

    /// A zero lifetime renders at the death endpoint immediately — the
    /// zero-lifetime arm answers one, not a division by zero.
    #[test]
    fn a_zero_lifetime_particle_wears_its_death_values() {
        let mut desc = burst_effect();
        desc.lifetime = (0.0, 0.0);
        let mut system =
            ParticleSystem::new(&desc, Seed::from_u64(31), StreamId::from_name("instant"));
        system.burst([0.0, 0.0, 0.0], 1);
        let mut bytes = [0u8; INSTANCE_STRIDE];
        system.write_instances(&mut bytes);
        let size = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        // The death endpoint through the same lerp the packer runs:
        // `start + (end - start) * 1.0` is one rounding away from `end`
        // itself, so the pin is the arm's arithmetic, not a value the
        // arithmetic never promises.
        assert_eq!(
            size.to_bits(),
            (desc.size.0 + (desc.size.1 - desc.size.0)).to_bits(),
            "zero lifetime renders at the death endpoint (progress one)"
        );
    }

    /// The Debug form reports counts, not contents — pinned so the
    /// claim cannot rot into a smoke call.
    #[test]
    fn the_debug_form_reports_counts_not_contents() {
        let mut system = ParticleSystem::new(
            &burst_effect(),
            Seed::from_u64(37),
            StreamId::from_name("debug"),
        );
        system.burst([0.0, 0.0, 0.0], 5);
        let shown = format!("{system:?}");
        assert!(shown.contains("live"), "{shown}");
        assert!(shown.contains("live: 5"), "{shown}");
        assert!(shown.contains("capacity"), "{shown}");
        assert!(shown.contains(".."), "{shown}");
    }

    proptest::proptest! {
        /// The pool never exceeds its capacity, however the bursts land.
        #[test]
        fn the_pool_never_exceeds_capacity(
            bursts in proptest::collection::vec((0u32..200, 0u32..40), 1..12),
        ) {
            let mut system = ParticleSystem::new(
                &burst_effect(),
                Seed::from_u64(19),
                StreamId::from_name("bounds"),
            );
            for (count, steps) in bursts {
                system.burst([0.0, 0.0, 0.0], count);
                proptest::prop_assert!(system.live() <= burst_effect().capacity);
                for _ in 0..steps {
                    system.step(DT);
                }
                proptest::prop_assert!(system.live() <= burst_effect().capacity);
            }
        }

        /// Every live particle's age is inside its lifetime — expiry
        /// leaves no zombie behind, whatever the schedule.
        #[test]
        fn no_particle_outlives_its_lifetime(
            count in 1u32..64,
            steps in 0u32..200,
        ) {
            let mut system = ParticleSystem::new(
                &burst_effect(),
                Seed::from_u64(23),
                StreamId::from_name("zombie"),
            );
            system.burst([0.0, 0.0, 0.0], count);
            for _ in 0..steps {
                system.step(DT);
            }
            for index in 0..system.age.len() {
                proptest::prop_assert!(
                    system.age[index] < system.lifetime[index],
                    "a particle at age {} of {} should be gone",
                    system.age[index],
                    system.lifetime[index]
                );
            }
        }
    }
}
