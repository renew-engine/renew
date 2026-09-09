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

use renew_particles::{EffectDesc, Emitter, ParticleSystem, Shape, VelocityCone};
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

/// How many sparks a second the living bird sheds.
///
/// Forty-five a second against a sixtieth-of-a-second step is three
/// sparks every four ticks, which reads as a stream rather than as a
/// row of separate dots, and leaves the trail well inside its pool.
const TRAIL_PER_SECOND: f32 = 45.0;

/// The trail effect: a thin stream of embers shed backwards by a bird
/// that is still flying.
///
/// **Its own description, not the crash's.** The two are the same kind
/// of light and nothing else: an explosion throws hard and wide and
/// falls fast, a trail is dropped gently and lags behind. Reusing
/// [`effect`] here would have shed the crash's spray — up the screen at
/// forty to a hundred and twenty units a second under three times the
/// world's gravity — off a bird in level flight, which is not a trail,
/// it is a bird on fire.
///
/// Alpha is zero at both ends of the ramp, for the same reason and with
/// the same consequence as the crash: these are light, added to the sky
/// rather than painted over it.
#[must_use]
fn trail_effect() -> EffectDesc {
    EffectDesc {
        capacity: crate::scene::TRAIL_CAPACITY,
        // Short: an ember is meant to be behind the bird, not halfway
        // across the canvas. The longest life here is what decides
        // whether the trail is still in the air at the crash
        // checkpoint, six ticks after the death.
        lifetime: (0.2, 0.45),
        velocity: VelocityCone {
            // Backwards. The bird's column is fixed and the world
            // scrolls past it, so "behind" is screen-left.
            axis: [-1.0, 0.0, 0.0],
            // Narrow, so the stream stays a stream.
            spread: 0.25,
            speed: (10.0, 30.0),
        },
        // Gentle, about the world's own pull: an ember sags, it does
        // not dive.
        gravity: [0.0, 60.0, 0.0],
        drag_per_step: 0.97,
        size: (2.0, 0.5),
        // Dimmer than the crash at both ends — a trail that matched the
        // burst would make the death invisible, because the frame would
        // already be full of that light.
        color: ([0.6, 0.5, 0.25, 0.0], [0.15, 0.05, 0.0, 0.0]),
        // Unused on the sprite path, as in `effect`.
        tile: [0.0; 4],
        angle: (0.0, 1.0),
        spin: (-1.0, 1.0),
    }
}

/// The game's presentation-side effects: two pools, and the liveness
/// that decides which of them is running.
///
/// A struct rather than a bare pool because the burst is an **edge**,
/// not a state: it fires on the step where the bird stops being alive,
/// and nothing else in the sample knows that edge has passed. The trail
/// is the complement — a **state**, running for exactly as long as the
/// bird is alive.
///
/// **Why two pools rather than one bigger one.** [`ParticleSystem`]
/// saturates at capacity: `burst_in` spawns `count.min(room)` and drops
/// the rest without a word. Emitting the trail into the crash's pool
/// would therefore let a trail that happened to be full on the tick the
/// bird died silently shorten the crash burst — the single moment in
/// this game that has to look right. It would also cost the crash tests
/// their meaning, since `live` could no longer answer "how much of the
/// burst is in the air" without knowing how much of the trail was.
/// Two pools cost one extra system and one extra stream, and remove the
/// coupling entirely rather than sizing around it.
#[derive(Debug)]
pub struct Effects {
    sparks: ParticleSystem,
    trail: ParticleSystem,
    emitter: Emitter,
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
            // The same seed, a different stream. Two pools drawing from
            // one stream would interleave: how many numbers the trail
            // happened to have taken would decide what the crash looked
            // like, so the burst would change with the length of the
            // flight that preceded it. Separate streams make each pool a
            // function of the seed alone.
            trail: ParticleSystem::new(
                &trail_effect(),
                Seed::from_u64(world.tick()),
                StreamId::from_name("trail"),
            ),
            emitter: Emitter::new(TRAIL_PER_SECOND),
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
        let half = BIRD_HALF as f32;
        let centre_x = BIRD_X as f32;
        let centre_y = world.bird_y_units() as f32;

        if self.was_alive && !world.alive() {
            self.sparks.burst_in(
                Shape::Box {
                    min: [centre_x - half, centre_y - half, 0.0],
                    max: [centre_x + half, centre_y + half, 0.0],
                },
                [0.0, -1.0, 0.0],
                BURST,
            );
        }

        // The trail runs on liveness as a state, not on an edge: it
        // sheds while the bird is flying and stops on the tick it is
        // not. `alive` is read from the world just stepped, so the tick
        // the bird dies sheds nothing and the burst has that frame to
        // itself.
        if world.alive() {
            // Along the bird's trailing edge — its screen-left side,
            // since the column is fixed and the world scrolls past it.
            // A segment rather than a point so the stream has the
            // bird's height and does not look like it comes out of one
            // spot.
            let trailing_x = centre_x - half;
            self.trail.burst_in(
                Shape::Segment {
                    from: [trailing_x, centre_y - half, 0.0],
                    to: [trailing_x, centre_y + half, 0.0],
                },
                [-1.0, 0.0, 0.0],
                self.emitter.advance(1.0 / 60.0),
            );
        }

        // A fixed step, matching the world's own cadence: the pools age
        // by the simulation's clock, never by the frame's.
        self.sparks.step(1.0 / 60.0);
        self.trail.step(1.0 / 60.0);
        self.was_alive = world.alive();
    }

    /// How many **crash** sparks are in the air.
    ///
    /// Public so a test can assert the burst happened before it looks
    /// for it in a picture — a golden over an empty pool would pass
    /// vacuously.
    ///
    /// **The burst only.** The trail is counted by [`Effects::trail_live`]
    /// and deliberately not folded in here: every test that reasons
    /// about the burst — that it fires once, that it throws its whole
    /// count, that nothing fires before the death — needs a number that
    /// the flight preceding the crash cannot move.
    #[must_use]
    pub fn live(&self) -> u32 {
        self.sparks.live()
    }

    /// How many trail sparks are in the air.
    ///
    /// The trail's counterpart to [`Effects::live`], separate for the
    /// reason given there.
    #[must_use]
    pub fn trail_live(&self) -> u32 {
        self.trail.live()
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
    ///
    /// # Contract
    ///
    /// **The trail is appended first and the burst last**, so the last
    /// [`Effects::live`] sprites of what this call adds are the crash
    /// sparks. Both are additive light, so the order changes no pixel;
    /// it is fixed because a test reads the burst back out of a filled
    /// vector and needs to know where it is, and an order that held only
    /// by accident would make that test quietly wrong the day it
    /// changed.
    pub fn fill(&self, out: &mut Vec<SceneSprite>) {
        Self::fill_from(&self.trail, out);
        Self::fill_from(&self.sparks, out);
    }

    /// Append one pool's live particles to `out`.
    ///
    /// Shared by both pools: a trail spark and a crash spark differ in
    /// the effect that made them, never in how they are drawn.
    fn fill_from(pool: &ParticleSystem, out: &mut Vec<SceneSprite>) {
        for particle in pool.particles() {
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
        // A literal, not `BURST`. Comparing the constant against itself
        // would move with any change to it and pin nothing; two dozen is
        // a decision about how a crash looks, and changing it should
        // have to argue with a red test first.
        assert_eq!(peak, 24, "the one burst must throw its whole count");
        assert_eq!(peak, BURST, "and that count is the one the effect asks for");
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
    ///
    /// **Why it compares the burst alone and not the whole fill.** The
    /// two runs shed trails of different lengths — one has been flying
    /// ten ticks longer — so their trail sparks differ in count and in
    /// age no matter what either pool was seeded with. Comparing every
    /// sprite would therefore pass with the seed hardcoded, which is
    /// exactly the vacuity this test was written to remove. The burst is
    /// the only part of the picture the two runs produce under identical
    /// conditions, so the burst is the only part whose difference means
    /// anything. [`Effects::fill`]'s Contract puts it last, and both
    /// runs' bursts are the same size, which the premise below asserts
    /// rather than assumes.
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

        // The burst only — see the note above. `fill` appends it last,
        // and both bursts are the size the premise just checked.
        let burst = from_zero.live() as usize;
        let mut a = Vec::new();
        from_zero.fill(&mut a);
        let mut b = Vec::new();
        from_ten.fill(&mut b);
        assert!(
            a.len() >= burst && b.len() >= burst,
            "premise: each fill must contain its own burst"
        );
        let a = &a[a.len() - burst..];
        let b = &b[b.len() - burst..];

        let same = a
            .iter()
            .zip(b)
            .all(|(l, r)| l.x.to_bits() == r.x.to_bits() && l.y.to_bits() == r.y.to_bits());
        assert!(
            !same,
            "two pools built at different ticks drew identical sparks, so the seed \
             is not coming from the tick"
        );
    }

    /// The trail runs while the bird is alive, and only then.
    ///
    /// Both halves matter and neither implies the other. A trail that
    /// never started would satisfy "nothing after the death"; one that
    /// never stopped would satisfy "something during the flight". So the
    /// sequence is flown a tick at a time and read in three parts: it
    /// must be shedding well before the death, it must stop adding on
    /// the death tick, and it must drain to nothing and stay there.
    ///
    /// Probed by dropping the `world.alive()` guard from the emission,
    /// so the trail keeps shedding off the corpse: red on the tail,
    /// which never reaches zero.
    #[test]
    fn the_trail_only_runs_while_alive() {
        let mut world = World::new(7);
        let mut effects = Effects::new(&world);
        let mut counts = Vec::new();
        let mut death = None;
        for _ in 0..200 {
            world.step(false);
            effects.observe(&world);
            if death.is_none() && !world.alive() {
                death = Some(counts.len());
            }
            counts.push(effects.trail_live());
        }
        let death = death.expect("premise: the bird must die inside the window");

        assert!(
            counts[..death].iter().any(|&n| n > 0),
            "the living bird shed no trail at all"
        );

        // After the death the trail may only fall: nothing is added to
        // it again. A single rise anywhere in the tail is a trail still
        // emitting off a corpse.
        for (offset, pair) in counts[death..].windows(2).enumerate() {
            let (earlier, later) = (pair[0], pair[1]);
            assert!(
                later <= earlier,
                "the trail grew from {earlier} to {later} at {offset} steps past the death, \
                 so it is still shedding off the corpse"
            );
        }
        assert_eq!(
            counts.last().copied(),
            Some(0),
            "the trail must drain after the death and stay drained"
        );
    }

    /// The trail sheds at the rate the emitter was given.
    ///
    /// Read over the first twelve ticks, which is the window in which
    /// nothing has expired yet: the shortest life the trail effect
    /// allows is 0.2 s — twelve ticks — so the count in the air is
    /// exactly the count emitted, and the emitter's arithmetic is the
    /// whole of what is being measured. Forty-five a second at a
    /// sixtieth of a second a step is three quarters of a spark per
    /// step, and twelve steps of that is nine.
    ///
    /// **A literal nine, not a re-derivation of the rate.** Computing
    /// the expectation from `TRAIL_PER_SECOND` would move with it and
    /// pin nothing — the same defect as comparing a constant against
    /// itself. The rate is a decision about how the game looks, and
    /// changing it should have to argue with a red test first.
    ///
    /// Probed by halving `TRAIL_PER_SECOND`: red, six instead of nine.
    #[test]
    fn the_trail_sheds_at_its_stated_rate() {
        let mut world = World::new(7);
        let mut effects = Effects::new(&world);
        for _ in 0..12 {
            world.step(false);
            effects.observe(&world);
            assert!(
                world.alive(),
                "premise: the bird must still be flying through the whole window"
            );
        }
        assert_eq!(
            effects.trail_live(),
            9,
            "twelve steps at three quarters of a spark each is nine, and none has expired yet"
        );
    }

    /// A trail running right up to the death does not eat into the
    /// crash burst.
    ///
    /// **This is the property the second pool exists for.**
    /// `ParticleSystem::burst_in` spawns `count.min(room)` and drops the
    /// remainder silently, so a trail sharing the crash's pool would
    /// shorten the burst by however much of itself happened to be in the
    /// air — a crash that looked different depending on how long the
    /// bird had flown, with nothing anywhere reporting it.
    ///
    /// Probed by emitting the trail into `self.sparks` instead of
    /// `self.trail` and dropping the second pool: red here, because the
    /// burst arrives short.
    #[test]
    fn the_trail_never_shortens_the_crash() {
        let (_, effects) = fall(120);
        assert!(
            effects.live() > 0,
            "premise: the burst must have fired inside the window"
        );
        assert_eq!(
            effects.live(),
            24,
            "the burst arrived short, so something else had taken the pool's room"
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
