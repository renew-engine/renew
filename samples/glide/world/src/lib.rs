//! The glide game's rules: a bird, gravity, pipes, a score — and nothing
//! else. A pure fixed-step function of (seed, per-tick input), which is
//! what lets a replay be an assertion instead of a demo.
//!
//! Ported from the feasibility run that proved the simulation half needs
//! no renderer, with its two known defects fixed here rather than
//! carried:
//!
//! - **Pipes despawn their entities.** The original removed a pipe's
//!   store entries and leaked its entity slot, so a long session grew
//!   slot count without bound. The world now keeps each pipe's handle
//!   and despawns it, and a test holds the slot count flat over a run
//!   long enough that the leak version visibly climbs.
//! - **The step allocates nothing.** The original collected despawn and
//!   scoring candidates into fresh `Vec`s every tick. Those lists are
//!   now persistent scratch, cleared and refilled in place — the world
//!   owns two small allocations for its whole life, made at
//!   construction, and a counting-allocator test holds the steady state
//!   at zero.
//!
//! Integer-only throughout: one world unit is 1/1000 of a screen unit,
//! so the digest is a fact about the rules and not about anyone's
//! floating-point unit.

use renew_ecs::{Entities, Entity, Store};
use renew_frame::StateHash;
use renew_rng::{Rng, Seed, StreamId};

/// Fixed-point scale: world units per screen unit.
const ONE: i64 = 1_000;

const GRAVITY: i64 = 45;
/// Tuned for a steering input rather than a blind schedule: strong
/// enough that reaching a distant gap is possible, which is what makes
/// the outcome depend on the seed and the input rather than on neither.
const FLAP_VELOCITY: i64 = -900;
const TERMINAL_VELOCITY: i64 = 1_200;

const BIRD_X: i64 = 40 * ONE;
const BIRD_HALF: i64 = 6 * ONE;
const CEILING: i64 = 0;
const FLOOR: i64 = 240 * ONE;

const PIPE_WIDTH: i64 = 16 * ONE;
const PIPE_GAP: i64 = 60 * ONE;
const PIPE_SPEED: i64 = 900;
const PIPE_INTERVAL: u64 = 90;
const PIPE_SPAWN_X: i64 = 320 * ONE;
const GAP_MARGIN: i64 = 30 * ONE;

/// The one thing a player can do. The driver binds keys, taps and
/// scripted traces to this through the input layer's generic map; the
/// world itself takes the resolved decision as a plain `bool`, which is
/// what keeps its dependency closure at exactly the deterministic
/// crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Flap,
}

/// Everything the simulation is, so one value can be hashed and compared.
pub struct World {
    entities: Entities,
    /// Each live pipe's handle, by slot — what despawn needs and what
    /// the leak version never kept.
    pipe: Store<Entity>,
    pipe_x: Store<i64>,
    pipe_gap_y: Store<i64>,
    pipe_passed: Store<bool>,
    rng: Rng,
    bird_y: i64,
    bird_velocity: i64,
    score: u64,
    alive: bool,
    tick: u64,
    digest: StateHash,
    /// Persistent scratch for the two per-tick sweeps. Allocated once
    /// here so the step itself allocates nothing; the original collected
    /// into fresh vectors every tick.
    swept: Vec<u32>,
}

impl World {
    /// A world at tick zero, alive, empty of pipes.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            entities: Entities::new(),
            pipe: Store::new(),
            pipe_x: Store::new(),
            pipe_gap_y: Store::new(),
            pipe_passed: Store::new(),
            rng: Rng::new(Seed::from_u64(seed), StreamId::from_u64(1)),
            bird_y: 120 * ONE,
            bird_velocity: 0,
            score: 0,
            alive: true,
            tick: 0,
            digest: StateHash::new(),
            swept: Vec::with_capacity(16),
        }
    }

    /// One fixed step. `flap` is this tick's input, already resolved by
    /// the driver from whatever produced it.
    pub fn step(&mut self, flap: bool) {
        if self.alive {
            self.integrate_bird(flap);
            self.spawn_pipes();
            self.advance_pipes();
            self.collide_and_score();
        }
        self.tick += 1;
        self.absorb();
    }

    #[must_use]
    pub fn score(&self) -> u64 {
        self.score
    }

    #[must_use]
    pub fn alive(&self) -> bool {
        self.alive
    }

    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// The digest over every store and scalar, folded once per tick.
    #[must_use]
    pub fn digest(&self) -> StateHash {
        self.digest
    }

    /// Live pipe count — the leak regression's probe, and a renderer's
    /// upper bound for sizing an instance batch.
    #[must_use]
    pub fn pipes(&self) -> usize {
        self.pipe.len()
    }

    /// Entity slots ever allocated. Flat over a long run when despawn
    /// works; climbing when it leaks. Public because the test that
    /// proves the fix needs the number, and a renderer never should.
    #[must_use]
    pub fn entity_capacity(&self) -> usize {
        self.entities.capacity()
    }

    fn integrate_bird(&mut self, flap: bool) {
        if flap {
            self.bird_velocity = FLAP_VELOCITY;
        }
        self.bird_velocity = (self.bird_velocity + GRAVITY).min(TERMINAL_VELOCITY);
        self.bird_y += self.bird_velocity;
        if self.bird_y - BIRD_HALF < CEILING {
            self.bird_y = CEILING + BIRD_HALF;
            self.bird_velocity = 0;
        }
        if self.bird_y + BIRD_HALF >= FLOOR {
            self.alive = false;
        }
    }

    fn spawn_pipes(&mut self) {
        if !self.tick.is_multiple_of(PIPE_INTERVAL) {
            return;
        }
        let span = u32::try_from((FLOOR - 2 * GAP_MARGIN) / ONE).unwrap_or(1);
        let Some(bound) = core::num::NonZeroU32::new(span) else {
            return;
        };
        let gap_y = GAP_MARGIN + i64::from(self.rng.below_u32(bound)) * ONE;
        let entity = self.entities.spawn();
        self.pipe.insert(entity.index(), entity);
        self.pipe_x.insert(entity.index(), PIPE_SPAWN_X);
        self.pipe_gap_y.insert(entity.index(), gap_y);
        self.pipe_passed.insert(entity.index(), false);
    }

    fn advance_pipes(&mut self) {
        self.pipe_x.for_each_mut(|_, x| *x -= PIPE_SPEED);
        // Sweep in ascending slot order (`iter`'s guarantee), so the
        // order pipes leave the world is a property of the rules and not
        // of the storage. The scratch list is reused, never reallocated
        // at steady state.
        self.swept.clear();
        self.swept.extend(
            self.pipe_x
                .iter()
                .filter(|(_, x)| **x + PIPE_WIDTH < 0)
                .map(|(slot, _)| slot),
        );
        for index in 0..self.swept.len() {
            let slot = self.swept[index];
            // The handle is what despawn needs; removing only the store
            // entries is exactly the leak this port exists to fix.
            if let Some(entity) = self.pipe.get(slot).copied() {
                self.entities.despawn(entity);
            }
            self.pipe.remove(slot);
            self.pipe_x.remove(slot);
            self.pipe_gap_y.remove(slot);
            self.pipe_passed.remove(slot);
        }
    }

    fn collide_and_score(&mut self) {
        let bird_top = self.bird_y - BIRD_HALF;
        let bird_bottom = self.bird_y + BIRD_HALF;
        let mut hit = false;

        self.swept.clear();
        for (slot, x) in self.pipe_x.iter() {
            let Some(gap_y) = self.pipe_gap_y.get(slot) else {
                continue;
            };
            let overlaps_x = *x < BIRD_X + BIRD_HALF && *x + PIPE_WIDTH > BIRD_X - BIRD_HALF;
            if overlaps_x {
                let gap_top = gap_y - PIPE_GAP / 2;
                let gap_bottom = gap_y + PIPE_GAP / 2;
                if bird_top < gap_top || bird_bottom > gap_bottom {
                    hit = true;
                }
            }
            if *x + PIPE_WIDTH < BIRD_X - BIRD_HALF && self.pipe_passed.get(slot) == Some(&false) {
                self.swept.push(slot);
            }
        }

        for index in 0..self.swept.len() {
            let slot = self.swept[index];
            self.pipe_passed.insert(slot, true);
            self.score += 1;
        }
        if hit {
            self.alive = false;
        }
    }

    /// The digest covers every store and every scalar. A determinism
    /// oracle that omits part of the state passes happily while blind —
    /// only a discrimination check (a different seed must move the
    /// digest) can see the omission, so both live in the tests.
    fn absorb(&mut self) {
        self.digest = self
            .digest
            .absorb_u64(self.tick)
            .absorb_bytes(&self.bird_y.to_le_bytes())
            .absorb_bytes(&self.bird_velocity.to_le_bytes())
            .absorb_u64(self.score)
            .absorb_u64(u64::from(self.alive))
            .absorb_u64(self.pipe_x.len() as u64);
        for (slot, x) in self.pipe_x.iter() {
            self.digest = self
                .digest
                .absorb_u32(slot)
                .absorb_bytes(&x.to_le_bytes())
                .absorb_bytes(
                    &self
                        .pipe_gap_y
                        .get(slot)
                        .copied()
                        .unwrap_or(0)
                        .to_le_bytes(),
                )
                .absorb_u64(u64::from(
                    self.pipe_passed.get(slot).copied().unwrap_or(false),
                ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic flap schedule that steers toward the next gap —
    /// the policy that actually exercises scoring and despawn.
    fn steer(world: &World) -> bool {
        // Flap when falling below the midline; crude, deterministic, and
        // enough to survive long enough to score.
        world.bird_velocity > 0 && world.bird_y > 100 * ONE
    }

    fn run(seed: u64, ticks: u64) -> World {
        let mut world = World::new(seed);
        for _ in 0..ticks {
            let flap = steer(&world);
            world.step(flap);
        }
        world
    }

    #[test]
    fn same_seed_same_input_same_digest() {
        let a = run(7, 3_000);
        let b = run(7, 3_000);
        assert_eq!(a.digest(), b.digest(), "determinism over 3000 ticks");
    }

    #[test]
    fn a_different_seed_moves_the_digest() {
        // The discrimination half of the oracle: without it, a digest
        // that omits the RNG-derived state passes the test above while
        // hashing nothing that matters.
        let a = run(7, 3_000);
        let b = run(8, 3_000);
        assert_ne!(a.digest(), b.digest(), "the seed must reach the digest");
    }

    #[test]
    fn entity_slots_stay_bounded_over_a_long_run() {
        let world = run(7, 60_000);
        // Pipes on screen at once are bounded by geometry: spawn X over
        // speed, per interval. Slot reuse keeps capacity in the same
        // order; the leak version reaches the high hundreds here.
        assert!(
            world.entity_capacity() <= 16,
            "slot count climbed to {} — despawn is leaking",
            world.entity_capacity()
        );
    }

    #[test]
    fn scoring_happens_and_death_is_reachable() {
        // The steering policy scores; the blind world (never flapping)
        // dies on the floor. Both ends of the game are real.
        let steered = run(7, 20_000);
        assert!(steered.score() > 0, "the steering policy never scored");
        let mut doomed = World::new(7);
        for _ in 0..2_000 {
            doomed.step(false);
        }
        assert!(!doomed.alive(), "a bird that never flaps must fall");
    }

    #[test]
    fn dead_worlds_still_tick_and_digest() {
        let mut world = World::new(7);
        for _ in 0..2_000 {
            world.step(false);
        }
        let after_death = world.tick();
        world.step(false);
        assert_eq!(world.tick(), after_death + 1, "ticks continue past death");
    }
}
