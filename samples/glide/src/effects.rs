//! Sparks on the crash: the game's one particle effect, and the first
//! consumer that turns a particle pool into sprites.
//!
//! **Presentation, not simulation.** Nothing here reaches the world or
//! its digest — the pool is stepped from an observation of the world
//! after the world has already moved, and a replay reproduces it only
//! because the observation it is seeded from is itself reproducible.
//! The digest is unchanged by this module existing, which
//! [`the_effects_never_reach_the_digest`] asserts rather than assumes.
//!
//! The pool is read back through [`ParticleSystem::particles`] and
//! turned into [`SceneSprite`]s in the caller's own draw order, which is
//! the whole of the seam between the particle crate and a 2D picture:
//! no GPU type crosses it, and the sample's headless build carries the
//! pool without carrying a renderer.

use renew_particles::{EffectDesc, ParticleSystem, Shape, VelocityCone};
use renew_rng::{Seed, StreamId};
use renew_sample_glide_world::World;

use crate::scene::{SceneSprite, Tile};

/// How many sparks one crash throws.
///
/// Two dozen fills the corpse's body without saturating the pool, whose
/// capacity is [`crate::scene::SPARK_CAPACITY`]: a second burst cannot
/// happen — a bird dies once — so the headroom is for nothing but
/// arithmetic comfort.
const BURST: u32 = 24;

/// The crash effect: a short upward spray of light, falling back under
/// gravity and fading as it goes.
///
/// **Everything is in canvas units per second**, the same space the
/// scene is in, so a spark's position is a scene coordinate with no
/// conversion at the draw site.
///
/// **The colours are premultiplied with a zero alpha, which makes the
/// sparks light rather than ink.** The sprite renderer's Contract spells
/// out why that works out of one pipeline: the premultiplied blend is
/// `src + dst·(1 − α_src)`, so at `α_src = 0` it adds the colour and
/// leaves the destination's alpha alone. Sparks brighten what they cross
/// and occlude nothing, which is what a spark is; ink would have needed
/// a second blend state and would have punched holes in the pipes.
#[must_use]
fn effect() -> EffectDesc {
    EffectDesc {
        capacity: crate::scene::SPARK_CAPACITY,
        // Long enough to clear the corpse and fall back into it, short
        // enough that the frame six ticks after the crash still has
        // every spark in the air.
        lifetime: (0.3, 0.6),
        velocity: VelocityCone {
            // Up the screen: canvas y grows downward, so a negative y
            // is up.
            axis: [0.0, -1.0, 0.0],
            spread: 0.9,
            speed: (40.0, 120.0),
        },
        // Down the screen, three times the world's own pull, so the
        // spray arcs inside its own lifetime rather than leaving frame.
        gravity: [0.0, 180.0, 0.0],
        drag_per_step: 0.97,
        size: (3.0, 1.0),
        // Hot yellow-white to a dim ember, both with alpha zero.
        color: ([1.0, 0.9, 0.4, 0.0], [0.3, 0.1, 0.0, 0.0]),
        // Unused: the sprite path takes its rectangle from the atlas
        // region the tile names, not from this field, which exists for
        // the crate's own instance packer.
        tile: [0.0; 4],
        angle: (0.0, 1.0),
        spin: (-2.0, 2.0),
    }
}

/// The game's presentation-side effects: one pool, and the liveness it
/// watches for the edge that fires it.
///
/// A struct rather than a bare pool because the burst is an **edge**,
/// not a state: it fires on the step where the bird stops being alive,
/// and nothing else in the sample knows that edge has passed.
#[derive(Debug)]
pub struct Effects {
    sparks: ParticleSystem,
    was_alive: bool,
}

impl Effects {
    /// A fresh set of effects, seeded from the world as it stands.
    ///
    /// **Seeded from the tick the world stands at now** — a digested
    /// observable, so two runs of the same replay build the pool at the
    /// same tick and draw the same sparks. Note it is the tick at
    /// CONSTRUCTION, not the tick the burst later fires on: a pool
    /// built at tick 0 and one built at tick 10 draw different sparks
    /// from the same crash, which is what pins the seed to the tick
    /// rather than to a constant. A wall-clock or an unseeded source
    /// would make the picture unreproducible while leaving the digest
    /// green, which is the failure this seeding exists to prevent.
    #[must_use]
    pub fn new(world: &World) -> Self {
        Self {
            sparks: ParticleSystem::new(
                &effect(),
                Seed::from_u64(world.tick()),
                StreamId::from_name("sparks"),
            ),
            was_alive: world.alive(),
        }
    }

    /// Watch the world for one **executed** step and advance the pool.
    ///
    /// Call once per step, after the step — a frame that runs three
    /// catch-up steps observes three times, the same rule
    /// `Presentation::capture` follows and for the same reason: the
    /// pool's age is measured in the world's steps, not the display's
    /// frames.
    ///
    /// **This reads the world and never writes it.** The burst fires on
    /// the falling edge of liveness only, so a world that was already
    /// dead when the effects were built never bursts at all.
    #[allow(
        clippy::cast_precision_loss,
        reason = "canvas units are bounded by the view constants, far below f32's exact range — the same allowance the scene module carries for the same numbers"
    )]
    pub fn observe(&mut self, world: &World) {
        if self.was_alive && !world.alive() {
            let half = BIRD_HALF as f32;
            let centre_x = BIRD_X as f32;
            let centre_y = world.bird_y_units() as f32;
            self.sparks.burst_in(
                Shape::Box {
                    min: [centre_x - half, centre_y - half, 0.0],
                    max: [centre_x + half, centre_y + half, 0.0],
                },
                [0.0, -1.0, 0.0],
                BURST,
            );
        }
        // A fixed step, matching the world's own cadence: the pool ages
        // by the simulation's clock, never by the frame's.
        self.sparks.step(1.0 / 60.0);
        self.was_alive = world.alive();
    }

    /// How many sparks are in the air.
    ///
    /// Public so a test can assert the burst happened before it looks
    /// for it in a picture — a golden over an empty pool would pass
    /// vacuously.
    #[must_use]
    pub fn live(&self) -> u32 {
        self.sparks.live()
    }

    /// Append every live spark to `out`, in pool order, as sprites.
    ///
    /// Draw order is append order, so sparks land **on top of** whatever
    /// the caller already filled — which is what light over a scene
    /// means. The caller decides that by calling this after
    /// [`crate::scene::scene`], not by anything decided here.
    ///
    /// A spark's rectangle is centred on its position, because a
    /// particle's position is its centre and a sprite's is its
    /// top-left corner.
    pub fn fill(&self, out: &mut Vec<SceneSprite>) {
        for particle in self.sparks.particles() {
            let size = particle.size;
            out.push(SceneSprite {
                tile: Tile::Spark,
                x: particle.position[0] - size * 0.5,
                y: particle.position[1] - size * 0.5,
                width: size,
                height: size,
                // A spark has no colour of its own to lose: its tint
                // carries the whole of it.
                saturation: 1.0,
                rotation: particle.rotation,
                smear: [0.0, 0.0],
                // Alpha zero: the sprite renderer adds this and
                // occludes nothing. The particle's colour is already
                // premultiplied, which is the convention the tint wants.
                tint: [particle.color[0], particle.color[1], particle.color[2], 0.0],
            });
        }
    }
}

/// The bird's fixed column and half-extent, in canvas units — the same
/// numbers `scene` derives its body from, named here so the burst box
/// and the body cannot drift apart.
const BIRD_X: i32 = renew_sample_glide_world::BIRD_X_UNITS;
const BIRD_HALF: i32 = renew_sample_glide_world::BIRD_HALF_UNITS;

#[cfg(test)]
mod tests {
    use super::*;

    /// Fly a fresh fall for `ticks`, observing every executed step.
    fn fall(ticks: u64) -> (World, Effects) {
        let mut world = World::new(7);
        let mut effects = Effects::new(&world);
        for _ in 0..ticks {
            world.step(false);
            effects.observe(&world);
        }
        (world, effects)
    }

    /// The burst is an edge, not a state: liveness rises once and then
    /// only falls.
    ///
    /// Observed rather than asserted from the constant: the fall is
    /// flown one tick at a time and the live count is recorded at each,
    /// so the shape of the sequence is the claim — zero while the bird
    /// flies, a jump to the burst size on one tick, then a monotone
    /// decline as sparks expire. A burst that fired twice would show a
    /// second rise; one that fired every tick while dead would never
    /// decline.
    ///
    /// Probed by dropping `was_alive` from the condition, so the burst
    /// fires on every dead tick: red, because the count rises again.
    #[test]
    fn a_death_bursts_once_and_a_second_step_does_not() {
        let mut world = World::new(7);
        let mut effects = Effects::new(&world);
        let mut counts = Vec::new();
        for _ in 0..160 {
            world.step(false);
            effects.observe(&world);
            counts.push(effects.live());
        }

        let peak = counts.iter().copied().max().unwrap_or(0);
        assert_eq!(peak, BURST, "the one burst must throw its whole count");
        let first_peak = counts
            .iter()
            .position(|&n| n == peak)
            .expect("the peak is in the sequence");
        assert!(
            counts[..first_peak].iter().all(|&n| n == 0),
            "nothing may burst before the bird dies"
        );
        for pair in counts[first_peak..].windows(2) {
            // Bound rather than passed as message arguments: an argument
            // that only evaluates when the assertion fires is a line the
            // coverage gate never sees run.
            let (earlier, later) = (pair[0], pair[1]);
            assert!(
                later <= earlier,
                "the count rose from {earlier} to {later} after the burst — it fired twice"
            );
        }
        assert_eq!(
            counts.last().copied(),
            Some(0),
            "every spark must expire; a pool that never drains would hide a second burst"
        );
    }

    /// Two runs of the same fall draw the same sparks, particle for
    /// particle.
    ///
    /// This is what "seeded from a digested observable" buys: the
    /// picture is a function of the replay, not of when it was drawn.
    /// Compared on bits, because a spark's position is carried into a
    /// sprite unrounded and "close enough" would not catch a generator
    /// that had been reseeded from something else.
    ///
    /// Probed by seeding from a counter that differs between runs: red
    /// on the first particle.
    #[test]
    fn the_burst_is_reproducible_from_the_tick() {
        let (_, first) = fall(120);
        let (_, second) = fall(120);
        assert!(first.live() > 0, "the comparison needs sparks to compare");

        let mut a = Vec::new();
        first.fill(&mut a);
        let mut b = Vec::new();
        second.fill(&mut b);
        assert_eq!(a.len(), b.len(), "the two runs drew different counts");
        for (index, (left, right)) in a.iter().zip(&b).enumerate() {
            assert_eq!(
                (
                    left.x.to_bits(),
                    left.y.to_bits(),
                    left.width.to_bits(),
                    left.rotation.to_bits(),
                    left.tint.map(f32::to_bits)
                ),
                (
                    right.x.to_bits(),
                    right.y.to_bits(),
                    right.width.to_bits(),
                    right.rotation.to_bits(),
                    right.tint.map(f32::to_bits)
                ),
                "spark {index} differs between two runs of the same fall"
            );
        }
    }

    /// The seed really is the tick: effects built at different ticks
    /// draw different sparks from the same crash.
    ///
    /// **Without this, the reproducibility test above proves less than
    /// its name says.** Both of its runs build from a fresh world, whose
    /// tick is zero, so it compares two pools seeded identically — which
    /// would stay green if the seed were a hardcoded constant. It pins
    /// determinism, not the source of the seed. This pins the source.
    ///
    /// The two runs die on the same tick and burst with the same shape;
    /// the only difference is when the pool was created, and therefore
    /// what it was seeded with. A generator draws nothing while the pool
    /// is empty, so the burst is the first draw in both and the seed is
    /// the whole of the difference.
    ///
    /// Probed by seeding from a constant instead of `world.tick()`: red
    /// here, and green in every other test in this crate — which is the
    /// gap this test exists to close.
    #[test]
    fn the_seed_is_the_tick_the_effects_were_built_at() {
        // Built at tick 0, the fresh world's own tick.
        let (_, from_zero) = fall(120);

        // Built at tick 10 instead: fly ten ticks first, and only then
        // create the pool. The bird is alive at both points, so neither
        // has crossed the death edge when it is built.
        let mut world = World::new(7);
        for _ in 0..10 {
            world.step(false);
        }
        assert!(world.alive(), "premise: the pool is built before the death");
        assert_eq!(world.tick(), 10, "premise: built at a different tick");
        let mut from_ten = Effects::new(&world);
        for _ in 10..120 {
            world.step(false);
            from_ten.observe(&world);
        }

        assert!(
            from_zero.live() > 0 && from_ten.live() > 0,
            "premise: both runs must have burst"
        );
        assert_eq!(
            from_zero.live(),
            from_ten.live(),
            "premise: the same burst size, so only the seed differs"
        );

        let mut a = Vec::new();
        from_zero.fill(&mut a);
        let mut b = Vec::new();
        from_ten.fill(&mut b);
        let same = a
            .iter()
            .zip(&b)
            .all(|(l, r)| l.x.to_bits() == r.x.to_bits() && l.y.to_bits() == r.y.to_bits());
        assert!(
            !same,
            "two pools built at different ticks drew identical sparks, so the seed \
             is not coming from the tick"
        );
    }

    /// The effects never reach the digest.
    ///
    /// The rule the whole module rests on: rendering is a read. If
    /// observing the world could move it, a replay would diverge from
    /// the run it replays and every committed hash in the sample would
    /// be wrong — so the digest is taken either side of a burst, of a
    /// plain step's observation, and of a fill.
    ///
    /// **This is a guard, and no mutation of this module can redden
    /// it** — which is worth saying rather than leaving a reader to
    /// wonder. `observe` and `fill` take `&World`, so the borrow checker
    /// already forbids what this asserts; the test would only ever fire
    /// if someone changed those signatures to take the world by mutable
    /// reference, which is exactly the change that should have to argue
    /// with a red test. It costs one digest per tick and holds the seam
    /// still.
    #[test]
    fn the_effects_never_reach_the_digest() {
        let mut world = World::new(7);
        let mut effects = Effects::new(&world);
        for _ in 0..120 {
            world.step(false);
            let tick = world.tick();
            let before = world.digest();
            effects.observe(&world);
            assert_eq!(
                world.digest(),
                before,
                "observing the world moved it at tick {tick}"
            );
        }
        assert!(effects.live() > 0, "the check must span a real burst");
        let before = world.digest();
        let mut out = Vec::new();
        effects.fill(&mut out);
        assert_eq!(world.digest(), before, "filling sprites moved the world");
        assert!(!out.is_empty(), "the fill must have produced sprites");
    }

    /// A spark is light: its tint's alpha is exactly zero, and its
    /// colour is the premultiplied one the pool carries.
    ///
    /// Exact on bits because zero is exact and because this is the one
    /// property that decides whether sparks add or occlude — a spark
    /// that acquired an alpha would punch a hole in the pipe behind it.
    ///
    /// Probed by adding a half to the tint's alpha: red on the first
    /// spark. Note what does NOT work as a probe — copying
    /// `particle.color[3]` in — because this effect's colour is
    /// alpha-zero at **both** ends of its ramp, so that copy writes the
    /// zero that is already there and changes nothing. A probe has to
    /// move the value, not restate it.
    #[test]
    fn every_spark_is_light_and_never_ink() {
        let (_, effects) = fall(120);
        let mut out = Vec::new();
        effects.fill(&mut out);
        assert!(!out.is_empty(), "the check needs sparks");
        for (index, sprite) in out.iter().enumerate() {
            assert_eq!(sprite.tile, Tile::Spark, "fill emits sparks only");
            assert_eq!(
                sprite.tint[3].to_bits(),
                0.0f32.to_bits(),
                "spark {index} carries a tint alpha, so it would occlude"
            );
            assert_eq!(
                (sprite.smear[0].to_bits(), sprite.smear[1].to_bits()),
                (0.0f32.to_bits(), 0.0f32.to_bits()),
                "a spark does not smear"
            );
        }
    }
}
