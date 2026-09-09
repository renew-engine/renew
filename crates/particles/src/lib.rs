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
//!   runs per particle anywhere in this crate. A [`Shape`]'s draw count
//!   is fixed per variant and stated on the type; an [`Emitter`] draws
//!   nothing, and an effect whose angle and spin ranges are both
//!   `(0.0, 0.0)` draws nothing for them.
//! - **All allocation happens at construction.** `step`,
//!   [`ParticleSystem::burst`], [`ParticleSystem::write_instances`] and
//!   the [`ParticleSystem::particles`] view allocate nothing,
//!   each gate-tested from its own first commit; a burst past capacity
//!   saturates rather than growing.
//!
//! The GPU-facing half — the billboard pipeline, the atlas, the draw —
//! lives behind the `render` feature as its own module with its own
//! dependency; this half never touches a device, which is what makes
//! every claim above testable on any machine. Which blend mode an
//! effect draws with belongs to that half too: it is a property of a
//! pipeline, not of arithmetic. A consumer with its own atlas and its
//! own draw order — a 2D sprite batch — reads the pool through
//! [`ParticleSystem::particles`] instead and draws each particle as one
//! of its own sprites.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

#[cfg(feature = "render")]
mod render;
#[cfg(feature = "render")]
pub use render::{CameraPush, ParticleBlend, ParticleRenderError, ParticleRenderer};

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
    /// Angle at birth, in turns, drawn uniformly from this range.
    /// `(0.0, 0.0)` — no turn — is what every effect written before this
    /// field existed means.
    pub angle: (f32, f32),
    /// Angular velocity, in turns per second, drawn uniformly from this
    /// range. `(0.0, 0.0)` is no spin.
    ///
    /// **An effect that does not turn draws nothing for either.** The
    /// angle and the spin are drawn, in that order, after the lifetime
    /// and only when one of the two ranges is not `(0.0, 0.0)`, so every
    /// effect authored before these fields existed replays the same
    /// bytes bit for bit and the committed hash stands.
    pub spin: (f32, f32),
}

impl EffectDesc {
    /// Whether a particle of this effect can have an angle at all: an
    /// exact `(0.0, 0.0)` for both ranges is the documented "no turn",
    /// and is the case that draws nothing.
    fn turns(&self) -> bool {
        self.angle != (0.0, 0.0) || self.spin != (0.0, 0.0)
    }
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

/// Where a burst spawns: a point, a segment or a box.
///
/// **The draw count is pinned per variant** — a point draws nothing
/// from the generator, a segment one unit, a box three (in x, y, z
/// order) — and a particle's shape draws come before its cone draws.
/// That is what keeps [`ParticleSystem::burst`] and
/// [`ParticleSystem::burst_along`] bit-identical to
/// [`ParticleSystem::burst_in`] at a point, and the committed hash
/// standing: a shape changes how many values the generator gives out
/// by a fixed, stated amount, never by an accident of load.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Shape {
    /// Every particle at this one position. Draws nothing.
    Point([f32; 3]),
    /// Uniform along the segment: `from + u · (to − from)`, one unit
    /// `u` per particle, the same `u` on every axis.
    Segment {
        /// One end.
        from: [f32; 3],
        /// The other.
        to: [f32; 3],
    },
    /// Uniform in the box: `min + u · (max − min)` per axis, one unit
    /// per axis in x, y, z order. A flat box (`min[2] == max[2]`) keeps
    /// its z exactly, because the lerp of equal endpoints is
    /// `z + 0 · u`.
    Box {
        /// The corner with the smallest coordinates.
        min: [f32; 3],
        /// The corner with the largest.
        max: [f32; 3],
    },
}

impl Shape {
    /// One position in the shape, drawing `unit` exactly as many times
    /// as the variant states, in the stated order — count and order are
    /// contract, held by test.
    fn sample(&self, mut unit: impl FnMut() -> f32) -> [f32; 3] {
        match *self {
            Shape::Point(at) => at,
            Shape::Segment { from, to } => {
                let u = unit();
                [
                    lerp(from[0], to[0], u),
                    lerp(from[1], to[1], u),
                    lerp(from[2], to[2], u),
                ]
            }
            Shape::Box { min, max } => {
                let x = unit();
                let y = unit();
                let z = unit();
                [
                    lerp(min[0], max[0], x),
                    lerp(min[1], max[1], y),
                    lerp(min[2], max[2], z),
                ]
            }
        }
    }
}

/// Spawn-at-a-rate: how many particles fall due each step, the
/// fraction carried to the next.
///
/// Kept outside the pool on purpose: several emitters may feed one
/// pool, and an emitter never touches the generator, so adding one to a
/// scene cannot move any committed hash — it only decides how many
/// times a burst is asked for. The arithmetic is an add, a multiply and
/// a floor, all correctly rounded, so the count sequence is the same on
/// every platform; the carry stays in `[0, 1)` and the long-run total
/// never drifts from `per_second · elapsed` by more than one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emitter {
    per_second: f32,
    carry: f32,
}

impl Emitter {
    /// An emitter spawning `per_second` particles per second, nothing
    /// carried yet.
    ///
    /// `per_second` must be finite and non-negative — asserted in dev
    /// builds, the pool's own rule for the same reason: a NaN rate
    /// spawns nothing forever with no error anywhere.
    #[must_use]
    pub fn new(per_second: f32) -> Self {
        debug_assert!(
            per_second.is_finite() && per_second >= 0.0,
            "an emitter's rate must be finite and non-negative: a NaN here spawns nothing \
             forever rather than failing anywhere visible"
        );
        Self {
            per_second,
            carry: 0.0,
        }
    }

    /// How many particles fall due in a step of `dt_seconds`:
    /// `carry += per_second · dt; due = floor(carry); carry −= due`.
    /// Saturates at `u32::MAX`. A step of zero seconds is due nothing
    /// and moves nothing.
    ///
    /// `dt_seconds` must be finite and non-negative — asserted in dev
    /// builds, because a NaN step poisons the carry and the emitter
    /// falls silent for good.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a float-to-integer `as` saturates, which is the stated ceiling, and the \
                  floor of a non-negative carry is never negative"
    )]
    pub fn advance(&mut self, dt_seconds: f32) -> u32 {
        debug_assert!(
            dt_seconds.is_finite() && dt_seconds >= 0.0,
            "an emitter's step must be finite and non-negative: a NaN here silences the \
             emitter for good"
        );
        self.carry += self.per_second * dt_seconds;
        let due = self.carry.floor();
        self.carry -= due;
        due as u32
    }

    /// The rate this emitter was made with, particles per second.
    #[must_use]
    pub fn per_second(self) -> f32 {
        self.per_second
    }
}

/// One live particle as the pool sees it this step — what a consumer
/// with its own atlas and its own draw order turns into a sprite.
///
/// A read-side record: `#[non_exhaustive]` and no constructor, because
/// only the pool makes one. Every value is what the packed record
/// carries or what it was computed from — `size` and `color` are the
/// same lerp at the same `progress` — so a caller drawing through this
/// view and one drawing the packed bytes see the same particle. The
/// billboard's atlas rectangle is deliberately absent: a caller with its
/// own atlas names its own source.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Particle {
    /// Centre, in the effect's own units.
    pub position: [f32; 3],
    /// The velocity `step` last left it, units per second: at birth the
    /// cone direction times the drawn speed, before any drag, and after
    /// every step the post-drag velocity the next step integrates from.
    pub velocity: [f32; 3],
    /// Size at this moment of its life — the packed record's fourth
    /// value.
    pub size: f32,
    /// Premultiplied colour at this moment — the packed record's fifth
    /// to eighth values.
    pub color: [f32; 4],
    /// Age over lifetime, clamped to `0.0..=1.0` — the progress the
    /// packer lerps size and colour by, so a caller lerping its own
    /// quantity lands where the colour did.
    pub progress: f32,
    /// The particle's angle now, in turns: its birth angle, advanced by
    /// `spin · dt` once per step — one multiply and one add, accumulated,
    /// which is what the cross-platform hash pins (not
    /// `birth + spin · age`, which rounds differently). Never wrapped:
    /// it is bounded by the lifetime times the spin, and a consumer's
    /// turn arithmetic reduces it itself. Exactly `0.0` for an effect
    /// that does not turn.
    pub rotation: f32,
}

/// The live particles in pool order — the order
/// [`ParticleSystem::write_instances`] packs. Borrows the pool,
/// allocates nothing, and knows its length exactly, so a caller can
/// size a sprite batch before pushing.
#[derive(Clone, Debug)]
pub struct Particles<'a> {
    pool: &'a ParticleSystem,
    index: usize,
}

impl Iterator for Particles<'_> {
    type Item = Particle;

    fn next(&mut self) -> Option<Particle> {
        let particle = self.pool.particle(self.index)?;
        self.index += 1;
        Some(particle)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.pool.position.len().saturating_sub(self.index);
        (left, Some(left))
    }
}

impl ExactSizeIterator for Particles<'_> {}

/// Past the end it keeps answering `None`: the index never moves once
/// the pool has nothing at it.
impl core::iter::FusedIterator for Particles<'_> {}

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
    angle: Vec<f32>,
    spin: Vec<f32>,
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
    /// pool at capacity forever; a NaN angle reaches a consumer's sprite
    /// and draws nothing, just as silently.
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
                desc.angle.0,
                desc.angle.1,
                desc.spin.0,
                desc.spin.1,
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
            angle: Vec::with_capacity(capacity),
            spin: Vec::with_capacity(capacity),
        }
    }

    /// The pool and instance-buffer size fixed at creation — what a
    /// caller sizing a scratch or a renderer for this pool needs, so
    /// it can size from the pool it was handed rather than guessing
    /// at the effect that built it.
    #[must_use]
    pub fn capacity(&self) -> u32 {
        self.desc.capacity
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
    /// actually spawn and on the shape's variant — a point, which this
    /// is, draws nothing extra; a segment one unit; a box three — which
    /// makes the draw count part of the reproducibility contract rather
    /// than an accident of load. This is [`Self::burst_in`] at a point
    /// along the effect's own axis.
    pub fn burst(&mut self, at: [f32; 3], count: u32) {
        self.burst_in(Shape::Point(at), self.desc.velocity.axis, count);
    }

    /// The same burst, with the cone pointed along `axis` rather than
    /// along the effect's own.
    ///
    /// **An effect says how matter leaves a surface; only the caller
    /// knows which surface.** `EffectDesc` is authored once and a pool
    /// is built once from it, so an axis living there is a per-effect
    /// fact — which is right for a fire, whose sparks rise whatever lit
    /// it, and wrong for anything knocked off a face, whose direction is
    /// a property of the blow and changes with every burst. Without this
    /// such a caller had one axis for all time and had to choose it
    /// blind, which reads as a fountain rather than as debris.
    ///
    /// `axis` need not be normalized: the jitter scales with the axis
    /// length, so the spread means the same shape whatever units it
    /// arrives in — the same rule the effect's own axis follows. A zero
    /// axis falls back exactly as a zero axis in `EffectDesc` does.
    ///
    /// **Reproducibility is untouched.** The draws happen in the same
    /// order and the same number, because the axis is read after the
    /// jitter rather than before it; passing the effect's own axis here
    /// is bit-identical to [`Self::burst`], which is asserted. This is
    /// [`Self::burst_in`] at a point — the general form, which also
    /// takes a segment or a box.
    pub fn burst_along(&mut self, at: [f32; 3], axis: [f32; 3], count: u32) {
        self.burst_in(Shape::Point(at), axis, count);
    }

    /// The general burst: up to `count` particles somewhere in `shape`,
    /// the cone pointed along `axis`, saturating at capacity.
    ///
    /// Per spawned particle the draws are, in order: the shape's (none
    /// for a point, one for a segment, three for a box), then the
    /// cone's rejection loop, then the speed, then the lifetime, then —
    /// only for an effect that turns — the angle and the spin. That is
    /// the order the point-shaped bursts always had, with the shape's
    /// draws in front and the turn's behind, so a point shape on an
    /// effect that does not turn reproduces what [`Self::burst`] and
    /// [`Self::burst_along`] always packed, bit for bit; the committed
    /// hash guard is the proof that neither arrival moved anything.
    pub fn burst_in(&mut self, shape: Shape, axis: [f32; 3], count: u32) {
        let room = self.desc.capacity.saturating_sub(self.live());
        for _ in 0..count.min(room) {
            let at = shape.sample(|| self.unit());
            let direction = self.cone_direction(axis);
            let speed = lerp(
                self.desc.velocity.speed.0,
                self.desc.velocity.speed.1,
                self.unit(),
            );
            let life = lerp(self.desc.lifetime.0, self.desc.lifetime.1, self.unit());
            // Only an effect that turns draws for its angle and spin, so
            // every effect that does not replays what it always did.
            let (angle, spin) = if self.desc.turns() {
                let angle = lerp(self.desc.angle.0, self.desc.angle.1, self.unit());
                let spin = lerp(self.desc.spin.0, self.desc.spin.1, self.unit());
                (angle, spin)
            } else {
                (0.0, 0.0)
            };
            self.position.push(at);
            self.velocity.push([
                direction[0] * speed,
                direction[1] * speed,
                direction[2] * speed,
            ]);
            self.age.push(0.0);
            self.lifetime.push(life);
            self.angle.push(angle);
            self.spin.push(spin);
        }
    }

    /// Advance every particle by `dt_seconds` — once per completed
    /// simulation step, so particle state is a pure function of the
    /// seed, the burst sequence, and the step count.
    ///
    /// The integrator's order is observable, hash-pinned behaviour, so
    /// it is stated: each step, a velocity gains gravity times `dt`, is
    /// multiplied by the drag factor, and the post-drag velocity moves
    /// the position — semi-implicit Euler with drag inside the step. Then,
    /// for an effect that turns, each angle gains its spin times `dt`.
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
        // An angle gains spin times `dt` — one multiply and one add,
        // accumulated, which is the value the hash pins. Skipped whole
        // for an effect that does not turn: its angles are all zero and
        // would stay so, and its step costs what it always cost.
        if self.desc.turns() {
            for (angle, spin) in self.angle.iter_mut().zip(self.spin.iter()) {
                *angle += *spin * dt_seconds;
            }
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
                self.angle.swap_remove(index);
                self.spin.swap_remove(index);
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

    /// Every live particle, in pack order, without a device and without
    /// an allocation — for a consumer that draws particles through its
    /// own atlas and its own draw order rather than the billboard
    /// pipeline. Reads the same arrays [`Self::write_instances`] reads by
    /// the same progress and the same lerps, which a test holds slot by
    /// slot; the two cannot disagree.
    #[must_use]
    pub fn particles(&self) -> Particles<'_> {
        Particles {
            pool: self,
            index: 0,
        }
    }

    /// The particle at `index`, or `None` past the live count — the
    /// second reader of the arrays the packer reads.
    fn particle(&self, index: usize) -> Option<Particle> {
        let position = *self.position.get(index)?;
        let t = progress(self.age[index], self.lifetime[index]);
        Some(Particle {
            position,
            velocity: self.velocity[index],
            size: lerp(self.desc.size.0, self.desc.size.1, t),
            color: lerp4(self.desc.color.0, self.desc.color.1, t),
            progress: t,
            rotation: self.angle[index],
        })
    }

    /// A direction inside a cone: `axis` plus a point in a jitter ball,
    /// normalized.
    ///
    /// The axis is a parameter rather than a read of the effect, because
    /// a burst may be aimed — see [`ParticleSystem::burst_along`]. The
    /// jitter is drawn *before* the axis is used, so which axis arrives
    /// cannot change how many values the generator gives out.
    ///
    /// The ball point comes from rejection sampling — draw a point in
    /// the unit cube, keep it when it lands inside the unit ball — so
    /// the only operations are the generator's integers, the exact
    /// unit-interval conversion, multiplication, and one square root,
    /// every one of them identical on every conformant platform. The
    /// number of draws varies with the rejections, and that variation
    /// is deterministic too: it depends only on the generator's state,
    /// which depends only on the seed and the call sequence.
    fn cone_direction(&mut self, axis: [f32; 3]) -> [f32; 3] {
        let jitter = loop {
            let x = self.unit() * 2.0 - 1.0;
            let y = self.unit() * 2.0 - 1.0;
            let z = self.unit() * 2.0 - 1.0;
            let length_squared = x * x + y * y + z * z;
            if length_squared <= 1.0 && length_squared > 0.0 {
                break [x, y, z];
            }
        };
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
            angle: (0.0, 0.0),
            spin: (0.0, 0.0),
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

    /// **An aimed burst with the effect's own axis is the plain burst,
    /// bit for bit.**
    ///
    /// `burst` is `burst_along` on the effect's own axis, and this is
    /// what holds it to that: the plain entry point must keep meaning
    /// exactly what it meant before the aimed one existed.
    ///
    /// The stream is safe for a different reason, worth writing down
    /// because this test does *not* cover it: the axis is read after the
    /// jitter is drawn, so which axis arrives cannot change how many
    /// values the generator gives out. Both paths here run the same code,
    /// so a draw added inside it would move them together and this would
    /// stay green — what catches that is
    /// `the_packed_bytes_hash_to_the_committed_value_on_every_platform`,
    /// confirmed by inserting a spare draw and watching it, not this, go
    /// red.
    ///
    /// Probed by having `burst` pass a zero axis instead of the effect's:
    /// "an aimed burst on the effect's own axis is not the plain burst".
    #[test]
    fn an_aimed_burst_along_the_effect_s_own_axis_is_the_plain_burst() {
        let desc = burst_effect();
        let bytes_of = |aimed: bool| {
            let mut system =
                ParticleSystem::new(&desc, Seed::from_u64(23), StreamId::from_name("same"));
            for at in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]] {
                if aimed {
                    system.burst_along(at, desc.velocity.axis, 4);
                } else {
                    system.burst(at, 4);
                }
                system.step(DT);
            }
            let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
            let packed = system.write_instances(&mut bytes);
            (packed, bytes)
        };
        let (plain_count, plain) = bytes_of(false);
        let (aimed_count, aimed) = bytes_of(true);
        assert!(plain_count > 0, "the fixture packed nothing");
        assert_eq!(
            plain_count, aimed_count,
            "an aimed burst spawned a different number"
        );
        assert_eq!(
            plain, aimed,
            "an aimed burst on the effect's own axis is not the plain burst"
        );
    }

    /// **And an aimed burst actually goes where it was aimed.** Zero
    /// spread, so the cone is exactly the axis: a burst aimed along +x
    /// flies +x, whatever the effect was authored to do.
    ///
    /// Probed by having `burst_along` ignore its argument and read the
    /// effect: every particle flies +y and this names it.
    #[test]
    fn an_aimed_burst_flies_where_it_was_aimed_and_not_where_the_effect_says() {
        let mut desc = burst_effect();
        desc.velocity.spread = 0.0;
        desc.velocity.axis = [0.0, 1.0, 0.0];
        desc.gravity = [0.0, 0.0, 0.0];
        desc.drag_per_step = 1.0;
        let mut system =
            ParticleSystem::new(&desc, Seed::from_u64(29), StreamId::from_name("aimed"));
        system.burst_along([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 8);
        system.step(DT);
        let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
        system.write_instances(&mut bytes);
        assert!(system.live() > 0, "the aimed burst spawned nothing");
        for index in 0..system.live() as usize {
            let base = index * INSTANCE_STRIDE;
            let at = |offset: usize| {
                f32::from_ne_bytes(bytes[base + offset..base + offset + 4].try_into().unwrap())
            };
            // Bound rather than called inside the message: an argument
            // only evaluated on failure is a line the suite can never
            // cover, and this repository counts those.
            let x = at(0);
            assert!(x > 0.0, "a burst aimed along +x has a particle at x = {x}");
            assert_eq!(
                at(4).to_bits(),
                0.0f32.to_bits(),
                "it flew the effect's axis, not the aim"
            );
            assert_eq!(
                at(8).to_bits(),
                0.0f32.to_bits(),
                "no sideways drift at zero spread"
            );
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
        // Long-lived on purpose: the first version of this test stepped
        // a full second against half-second lifetimes, every particle
        // died, and the assertion loop below passed by never running —
        // the coverage gate is what caught it.
        desc.lifetime = (10.0, 10.0);
        let mut system =
            ParticleSystem::new(&desc, Seed::from_u64(29), StreamId::from_name("fallback"));
        system.burst([0.0, 0.0, 0.0], 4);
        system.step(1.0);
        assert_eq!(
            system.live(),
            4,
            "the assertion loop below must have subjects"
        );
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

    /// The view and the packer read the same particle: over the hash
    /// fixture's schedule, every `position`, `size` and `color` in the
    /// view equals the packed record's slots bit for bit, `progress` is
    /// the packer's own, and `velocity` is the array's.
    ///
    /// Probed by lerping the view's size backwards (`end` to `start`):
    /// red here and nowhere else — the packer is untouched, so the
    /// committed hash still holds.
    #[test]
    fn the_view_and_the_packer_read_the_same_particle() {
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
        let packed = system.write_instances(&mut bytes) as usize;
        assert!(packed > 0, "the fixture must leave something to compare");
        assert_eq!(system.particles().len(), packed, "the view's length");
        for (index, particle) in system.particles().enumerate() {
            let base = index * INSTANCE_STRIDE;
            let slot = |k: usize| {
                f32::from_ne_bytes(bytes[base + k * 4..base + k * 4 + 4].try_into().unwrap())
                    .to_bits()
            };
            assert_eq!(
                particle.position.map(f32::to_bits),
                [slot(0), slot(1), slot(2)],
                "position of particle {index}"
            );
            assert_eq!(particle.size.to_bits(), slot(3), "size of particle {index}");
            assert_eq!(
                particle.color.map(f32::to_bits),
                [slot(4), slot(5), slot(6), slot(7)],
                "colour of particle {index}"
            );
            assert_eq!(
                particle.progress.to_bits(),
                progress(system.age[index], system.lifetime[index]).to_bits(),
                "progress of particle {index}"
            );
            assert_eq!(
                particle.velocity.map(f32::to_bits),
                system.velocity[index].map(f32::to_bits),
                "velocity of particle {index}"
            );
        }
    }

    /// The view is empty when the pool is, its length is the live count
    /// otherwise — before and after expiry — and the length tracks the
    /// walk, which is what lets a caller size a batch from it.
    #[test]
    fn the_view_is_empty_when_the_pool_is_and_exact_size_otherwise() {
        let mut system = ParticleSystem::new(
            &burst_effect(),
            Seed::from_u64(3),
            StreamId::from_name("expiry"),
        );
        assert_eq!(system.particles().len(), 0);
        assert!(
            system.particles().next().is_none(),
            "an empty pool views nothing"
        );
        system.burst([0.0, 0.0, 0.0], 16);
        assert_eq!(system.particles().len(), 16);
        assert_eq!(
            system.particles().count(),
            16,
            "the walk visits every live particle"
        );
        let mut view = system.particles();
        assert!(view.next().is_some());
        assert_eq!(view.len(), 15, "the length tracks the walk");
        // The longest possible lifetime is 1.5 seconds; step past it.
        for _ in 0..120 {
            system.step(DT);
        }
        assert_eq!(system.live(), 0, "every particle should have died");
        assert_eq!(system.particles().len(), 0);
        assert!(system.particles().next().is_none());
    }

    /// FNV-1a 64 over packed bytes — the hash every committed constant
    /// in this module is stated in.
    fn fnv1a(bytes: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// A shape draws exactly what its variant states, in axis order: a
    /// point nothing, a segment one unit, a box three — x, then y, then
    /// z. Fed a counted, known sequence, the position says which unit
    /// went where; a variant that draws more than it states runs off
    /// the end of the sequence.
    #[test]
    fn a_shape_draws_its_pinned_count_in_axis_order() {
        let counted = |units: &[f32], shape: Shape| {
            let mut next = 0usize;
            let at = shape.sample(|| {
                let unit = units[next];
                next += 1;
                unit
            });
            (next, at)
        };
        assert_eq!(
            counted(&[], Shape::Point([1.0, 2.0, 3.0])),
            (0, [1.0, 2.0, 3.0]),
            "a point draws nothing and is where it says"
        );
        assert_eq!(
            counted(
                &[0.25],
                Shape::Segment {
                    from: [0.0, 0.0, 0.0],
                    to: [4.0, 8.0, 12.0],
                }
            ),
            (1, [1.0, 2.0, 3.0]),
            "a segment draws one unit and lerps every axis by it"
        );
        assert_eq!(
            counted(
                &[0.25, 0.5, 0.75],
                Shape::Box {
                    min: [0.0, 0.0, 0.0],
                    max: [4.0, 4.0, 4.0],
                }
            ),
            (3, [1.0, 2.0, 3.0]),
            "a box draws three units, x then y then z"
        );
    }

    /// The packed bytes of the guard fixture after `spawn` and twelve
    /// steps.
    fn packed_after(spawn: impl FnOnce(&mut ParticleSystem)) -> Vec<u8> {
        let mut system = ParticleSystem::new(
            &burst_effect(),
            Seed::from_u64(5),
            StreamId::from_name("point"),
        );
        spawn(&mut system);
        for _ in 0..12 {
            system.step(DT);
        }
        let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
        let count = system.write_instances(&mut bytes);
        assert!(count > 0, "the scenario must leave something to compare");
        bytes
    }

    /// A burst at a point is the plain burst, bit for bit: the general
    /// form given a point and the effect's own axis packs the same
    /// bytes as `burst`, and as `burst_along` with that axis.
    #[test]
    fn a_point_burst_is_the_plain_burst_bit_for_bit() {
        let at = [1.0, 2.0, 3.0];
        let axis = burst_effect().velocity.axis;
        let plain = packed_after(|system| system.burst(at, 30));
        let aimed = packed_after(|system| system.burst_along(at, axis, 30));
        let shaped = packed_after(|system| system.burst_in(Shape::Point(at), axis, 30));
        assert_eq!(
            shaped, plain,
            "a point-shaped burst must be the plain burst byte for byte"
        );
        assert_eq!(
            shaped, aimed,
            "a point-shaped burst must be the aimed burst byte for byte"
        );
    }

    /// The second committed guard, for the shaped bursts: a segment and
    /// a box, each drawing its stated units in front of the cone's,
    /// hash to one value on every platform the ordinary suite runs on.
    /// The first guard cannot see a shape's draws — a point makes none
    /// — so this one exists.
    #[test]
    fn a_shaped_scenario_hashes_to_the_committed_value_on_every_platform() {
        let mut system = ParticleSystem::new(
            &burst_effect(),
            Seed::from_u64(20_260_811),
            StreamId::from_name("guard"),
        );
        let axis = burst_effect().velocity.axis;
        system.burst_in(
            Shape::Segment {
                from: [0.0, 0.0, 0.0],
                to: [4.0, 0.0, 0.0],
            },
            axis,
            20,
        );
        for _ in 0..10 {
            system.step(DT);
        }
        system.burst_in(
            Shape::Box {
                min: [-1.0, -1.0, 0.0],
                max: [1.0, 1.0, 0.0],
            },
            [0.0, -1.0, 0.0],
            20,
        );
        for _ in 0..10 {
            system.step(DT);
        }
        let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
        let count = system.write_instances(&mut bytes);
        assert!(count > 0, "the scenario must leave something to hash");
        assert_eq!(
            fnv1a(&bytes),
            0x3583_f9e4_c90d_3be6,
            "the shaped bytes moved: either a shape's sampling changed (bump this constant \
             in the same change, deliberately) or this platform computes differently \
             (which is the finding)"
        );
    }

    /// An effect that leaves particles where they were born — no
    /// speed, no gravity, unit drag, immortal — so a packed position is
    /// the shape's sample and nothing else.
    fn still_effect() -> EffectDesc {
        EffectDesc {
            capacity: 64,
            lifetime: (1.0e9, 1.0e9),
            velocity: VelocityCone {
                axis: [0.0, 1.0, 0.0],
                spread: 0.5,
                speed: (0.0, 0.0),
            },
            gravity: [0.0, 0.0, 0.0],
            drag_per_step: 1.0,
            size: (1.0, 1.0),
            color: ([1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0]),
            tile: [0.0, 0.0, 1.0, 1.0],
            angle: (0.0, 0.0),
            spin: (0.0, 0.0),
        }
    }

    /// The packed positions of `count` still particles born in `shape`
    /// and stepped once.
    fn packed_positions(shape: Shape, count: u32) -> Vec<[f32; 3]> {
        let mut system = ParticleSystem::new(
            &still_effect(),
            Seed::from_u64(11),
            StreamId::from_name("shape"),
        );
        system.burst_in(shape, [0.0, 1.0, 0.0], count);
        system.step(DT);
        let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
        let packed = system.write_instances(&mut bytes) as usize;
        assert_eq!(
            packed, count as usize,
            "every still particle must be packed"
        );
        bytes
            .as_chunks::<INSTANCE_STRIDE>()
            .0
            .iter()
            .map(|record| {
                let slot =
                    |k: usize| f32::from_ne_bytes(record[k * 4..k * 4 + 4].try_into().unwrap());
                [slot(0), slot(1), slot(2)]
            })
            .collect()
    }

    /// A segment burst lies on its segment: along a diagonal from
    /// `(1, 1, 3)` to `(5, 5, 3)` every particle has `x == y` bit for
    /// bit (one unit lerps both), `z == 3` exactly, `x` inside the
    /// span, and the particles do not all sit at one place.
    #[test]
    fn a_segment_burst_lies_on_its_segment() {
        let positions = packed_positions(
            Shape::Segment {
                from: [1.0, 1.0, 3.0],
                to: [5.0, 5.0, 3.0],
            },
            24,
        );
        for [x, y, z] in &positions {
            assert_eq!(
                x.to_bits(),
                y.to_bits(),
                "one unit must place x and y alike"
            );
            assert_eq!(
                z.to_bits(),
                3.0f32.to_bits(),
                "z is constant along this segment"
            );
            assert!((1.0..=5.0).contains(x), "x = {x} is off the segment");
        }
        let first = positions[0][0];
        assert!(
            positions
                .iter()
                .any(|[x, _, _]| x.to_bits() != first.to_bits()),
            "a segment burst must spread along the segment"
        );
    }

    /// A box burst lies inside its box on every axis, and the axes are
    /// drawn separately: the particles do not all lie on the box's
    /// diagonal, which one shared unit would put them on.
    #[test]
    fn a_box_burst_lies_inside_its_box() {
        let min = [-1.0, -2.0, -3.0];
        let max = [1.0, 2.0, 3.0];
        let positions = packed_positions(Shape::Box { min, max }, 24);
        for position in &positions {
            for axis in 0..3 {
                assert!(
                    (min[axis]..=max[axis]).contains(&position[axis]),
                    "{position:?} is outside the box on axis {axis}"
                );
            }
        }
        let off_diagonal = positions.iter().filter(|[x, y, _]| {
            let along_x = (x - min[0]) / (max[0] - min[0]);
            let along_y = (y - min[1]) / (max[1] - min[1]);
            (along_x - along_y).abs() > 1.0e-3
        });
        assert!(
            off_diagonal.count() > 0,
            "a box burst must draw its axes separately, not lie on one diagonal"
        );
    }

    /// A flat box keeps its z exactly: with `min[2] == max[2] == 0.1`,
    /// every packed z is `0.1` bit for bit, because the lerp of equal
    /// endpoints is `z + 0 · u`. The value has no short binary
    /// expansion on purpose: the `(1 − u) · z + u · z` form of a lerp
    /// keeps a dyadic z such as `2.5` exact by luck and drifts here by
    /// an ulp, so this is the value that tells the two forms apart.
    #[test]
    fn a_flat_box_keeps_its_z_exactly() {
        let positions = packed_positions(
            Shape::Box {
                min: [0.0, 0.0, 0.1],
                max: [4.0, 4.0, 0.1],
            },
            64,
        );
        for [_, _, z] in &positions {
            assert_eq!(
                z.to_bits(),
                0.1f32.to_bits(),
                "a flat box drifted in z: {z}"
            );
        }
    }

    /// A NaN rate is a contract violation, refused where it is made.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "finite and non-negative")]
    fn an_emitter_with_a_nan_rate_is_refused() {
        let _ = Emitter::new(f32::NAN);
    }

    /// A NaN step is the same violation at the other call.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "finite and non-negative")]
    fn an_emitter_with_a_nan_step_is_refused() {
        let mut emitter = Emitter::new(1.0);
        let _ = emitter.advance(f32::NAN);
    }

    /// The guard fixture, turning: a birth angle in `[0, 1)` turns and a
    /// spin in `[−2, 2)` turns per second.
    fn turning_effect() -> EffectDesc {
        EffectDesc {
            angle: (0.0, 1.0),
            spin: (-2.0, 2.0),
            ..burst_effect()
        }
    }

    /// The packed bytes of `system` followed by every particle's
    /// `rotation`, native byte order, in pool order — what the turning
    /// guard hashes, because the record does not carry the angle.
    fn packed_bytes_and_rotations(system: &ParticleSystem) -> Vec<u8> {
        let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
        let count = system.write_instances(&mut bytes);
        assert!(count > 0, "the scenario must leave something to hash");
        for particle in system.particles() {
            bytes.extend_from_slice(&particle.rotation.to_ne_bytes());
        }
        bytes
    }

    /// The third committed guard: the first guard's schedule on an effect
    /// that turns, hashed over the packed bytes and every rotation, so
    /// the angle draws, their order after the lifetime, and the
    /// multiply-add integration are pinned on every platform the
    /// ordinary suite runs on. The first guard cannot see any of it —
    /// its effect does not turn and draws nothing for it — which is why
    /// that guard's constant did not move when this landed.
    #[test]
    fn a_spinning_scenario_hashes_to_the_committed_value_on_every_platform() {
        let mut system = ParticleSystem::new(
            &turning_effect(),
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
        assert_eq!(
            fnv1a(&packed_bytes_and_rotations(&system)),
            0x43d3_d7d4_5cfa_e13b,
            "the turning bytes moved: either the angle draws or the integration changed \
             (bump this constant in the same change, deliberately) or this platform computes \
             differently (which is the finding)"
        );
    }

    /// A newborn wears its birth angle exactly: with an angle range of
    /// one value the lerp is `a + 0 · u`, bit-exact, and before any step
    /// the spin has added nothing.
    #[test]
    fn a_newborn_particle_wears_its_birth_angle_exactly() {
        let desc = EffectDesc {
            angle: (0.3, 0.3),
            spin: (1.0, 1.0),
            ..burst_effect()
        };
        let mut system = ParticleSystem::new(&desc, Seed::from_u64(3), StreamId::from_name("born"));
        system.burst([0.0, 0.0, 0.0], 8);
        for particle in system.particles() {
            assert_eq!(
                particle.rotation.to_bits(),
                0.3f32.to_bits(),
                "a newborn's angle is its birth angle: {}",
                particle.rotation
            );
        }
    }

    /// The spin integrates as one multiply and one add per step, and the
    /// value is the accumulated one: sixty steps of a 1.5-turn spin equal
    /// the f32 sum this test forms by the same loop, bit for bit — and
    /// not `spin · age`, which the mutant that rounds differently would
    /// give.
    #[test]
    fn spin_integrates_one_multiply_add_per_step() {
        let desc = EffectDesc {
            angle: (0.0, 0.0),
            spin: (1.5, 1.5),
            lifetime: (2.0, 2.0),
            ..burst_effect()
        };
        let mut system = ParticleSystem::new(&desc, Seed::from_u64(6), StreamId::from_name("spin"));
        system.burst([0.0, 0.0, 0.0], 4);
        let mut expected = 0.0f32;
        for _ in 0..60 {
            system.step(DT);
            expected += 1.5 * DT;
        }
        assert_eq!(system.live(), 4, "a two-second life outlasts sixty steps");
        for particle in system.particles() {
            assert_eq!(
                particle.rotation.to_bits(),
                expected.to_bits(),
                "the rotation {} is not the accumulated {expected}",
                particle.rotation
            );
        }
    }

    /// A turning effect draws its angle and spin after the lifetime, in
    /// the particle's own draw sequence: the first particle of a turning
    /// burst is the first particle of the non-turning burst bit for bit
    /// (its own draws come first and are the same), and the second
    /// differs (its draws are two further along).
    #[test]
    fn a_turning_effect_keeps_the_generator_in_step() {
        let born = |desc: &EffectDesc| {
            let mut system =
                ParticleSystem::new(desc, Seed::from_u64(9), StreamId::from_name("seq"));
            system.burst([0.0, 0.0, 0.0], 2);
            system.particles().collect::<Vec<Particle>>()
        };
        let plain = born(&burst_effect());
        let turning = born(&turning_effect());
        assert_eq!(
            turning[0].velocity.map(f32::to_bits),
            plain[0].velocity.map(f32::to_bits),
            "the first particle's own draws come before its angle and spin"
        );
        assert_ne!(
            turning[1].velocity.map(f32::to_bits),
            plain[1].velocity.map(f32::to_bits),
            "the second particle's draws follow the first's angle and spin"
        );
        assert_eq!(
            plain[0].rotation.to_bits(),
            0.0f32.to_bits(),
            "an effect that does not turn has no angle"
        );
    }

    proptest::proptest! {
        // Fixed RNG seed: the suite explores the same inputs on every
        // run and every machine, so a property failure anywhere
        // reproduces everywhere.
        #![proptest_config(proptest::prelude::ProptestConfig {
            rng_seed: proptest::test_runner::RngSeed::Fixed(0x2D5A_F010),
            ..proptest::prelude::ProptestConfig::default()
        })]

        /// The view walks pack order: over random burst schedules, the
        /// i-th particle of the view is the i-th packed record in every
        /// slot the record carries from the particle — position, size
        /// and colour — however many bursts and expiries have compacted
        /// the pool. Each burst spawns at its own x, so newborns of one
        /// burst are distinguishable from another's; a schedule that
        /// leaves the pool empty is assumed away rather than passing on
        /// two empty lists.
        #[test]
        fn the_view_walks_pack_order(
            bursts in proptest::collection::vec((0u32..200, 0u32..40), 1..12),
        ) {
            let mut system = ParticleSystem::new(
                &burst_effect(),
                Seed::from_u64(41),
                StreamId::from_name("order"),
            );
            for ((count, steps), origin) in bursts.into_iter().zip(0u8..) {
                system.burst([f32::from(origin), 0.0, 0.0], count);
                for _ in 0..steps {
                    system.step(DT);
                }
            }
            let mut bytes = vec![0u8; system.live() as usize * INSTANCE_STRIDE];
            let packed = system.write_instances(&mut bytes) as usize;
            proptest::prop_assume!(packed > 0);
            let viewed: Vec<Particle> = system.particles().collect();
            proptest::prop_assert_eq!(viewed.len(), packed);
            for (index, particle) in viewed.iter().enumerate() {
                let base = index * INSTANCE_STRIDE;
                let slot = |k: usize| {
                    f32::from_ne_bytes(bytes[base + k * 4..base + k * 4 + 4].try_into().unwrap())
                        .to_bits()
                };
                proptest::prop_assert_eq!(
                    particle.position.map(f32::to_bits),
                    [slot(0), slot(1), slot(2)]
                );
                proptest::prop_assert_eq!(particle.size.to_bits(), slot(3));
                proptest::prop_assert_eq!(
                    particle.color.map(f32::to_bits),
                    [slot(4), slot(5), slot(6), slot(7)]
                );
            }
        }

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

        /// An emitter never drifts: over any rate, step and count, the
        /// particles it says are due sum to within one of the exact
        /// `floor(rate · elapsed)`, the carry is in `[0, 1)` after every
        /// step, a zero step is due nothing, and a zero rate never
        /// spawns.
        #[test]
        fn an_emitter_never_drifts(
            rate in 0.0f32..=1000.0,
            dt in 0.0f32..=0.05,
            steps in 1u32..2000,
        ) {
            let mut emitter = Emitter::new(rate);
            let mut idle = Emitter::new(0.0);
            let mut total: u32 = 0;
            for _ in 0..steps {
                total += emitter.advance(dt);
                proptest::prop_assert!(
                    (0.0..1.0).contains(&emitter.carry),
                    "the carry left [0, 1): {}",
                    emitter.carry
                );
                proptest::prop_assert_eq!(idle.advance(dt), 0, "a zero rate spawned");
            }
            let exact = (f64::from(rate) * f64::from(dt) * f64::from(steps)).floor();
            proptest::prop_assert!(
                (f64::from(total) - exact).abs() <= 1.0,
                "{} particles fell due against an exact {}",
                total,
                exact
            );
            proptest::prop_assert_eq!(emitter.advance(0.0), 0, "a zero step spawned");
            proptest::prop_assert_eq!(emitter.per_second().to_bits(), rate.to_bits());
        }
    }
}
