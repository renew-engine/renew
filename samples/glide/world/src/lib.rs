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
//! - **The live step allocates nothing.** The original collected sweep
//!   candidates into fresh vectors every tick, and the store's own
//!   `for_each_mut` allocates once per call besides. Both are gone: one
//!   scratch list, allocated at construction, does every walk in place,
//!   and a counting-allocator test measures a window it proves is alive
//!   with pipes on screen — a gate over a dead world's ticks holds
//!   nothing.
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
/// The farthest one gap's centre may sit from the previous gap's.
///
/// A rules constraint, not a nicety: between adjacent pipes the bird has
/// ~58 ticks of transit, its climb tops out near 50 screen units in that
/// window, and an unconstrained generator can demand 180 — some seeds
/// were unwinnable by ANY input, measured when the pilot died at the
/// second pipe. Bounding the delta makes every seed playable while the
/// gap sequence stays exactly as deterministic as before.
const GAP_MAX_STEP: i64 = 35 * ONE;

/// One draw's worth of gap movement: every step in
/// `[-GAP_MAX_STEP, +GAP_MAX_STEP]`, in screen units, as the generator
/// bound. A compile-time fact about the constants, checked there.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
const STEP_SPAN: core::num::NonZeroU32 = {
    // Range-checked before the cast: a future negative GAP_MAX_STEP
    // must refuse the build, not wrap into a huge valid-looking bound.
    assert!(GAP_MAX_STEP > 0 && GAP_MAX_STEP / ONE <= 1_000);
    match core::num::NonZeroU32::new((2 * (GAP_MAX_STEP / ONE) + 1) as u32) {
        Some(span) => span,
        None => panic!("the gap step span is empty"),
    }
};

/// The visible playfield in whole screen units — the render view's
/// vocabulary. Deliberately a dedicated constant rather than an alias
/// of the spawn position: spawning at the right screen edge is a design
/// choice, not a rule, and the unit test asserting the identity makes a
/// future divergence break loudly instead of silently widening the view
/// and invalidating every committed image with a misleading diff.
pub const VIEW_WIDTH: u32 = 320;
/// The floor line, in whole screen units; the view's height.
pub const VIEW_HEIGHT: u32 = 240;
/// The bird's fixed horizontal centre, whole units.
pub const BIRD_X_UNITS: i32 = 40;
/// Half the bird's square body, whole units.
pub const BIRD_HALF_UNITS: i32 = 6;
/// A pipe's width, whole units.
pub const PIPE_WIDTH_UNITS: i32 = 16;
/// The gap's half-height: a gap spans its centre ± this, whole units.
pub const PIPE_GAP_HALF_UNITS: i32 = 30;

/// The one thing a player can do. The driver binds keys, taps and
/// scripted traces to this through the input layer's generic map; the
/// world itself takes the resolved decision as a plain `bool`, which is
/// what keeps its dependency closure at exactly the deterministic
/// crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Flap,
}

/// One pipe's whole body. One store, not three parallel ones: parallel
/// stores force defensive arms for a desync that construction forbids,
/// and an arm no test can reach is a hole in the coverage gate.
#[derive(Clone, Copy)]
struct Pipe {
    x: i64,
    gap_y: i64,
    passed: bool,
}

/// Everything the simulation is, so one value can be hashed and compared.
pub struct World {
    entities: Entities,
    /// Each live pipe's handle, by slot — what despawn needs and what
    /// the leak version never kept.
    pipe: Store<Entity>,
    body: Store<Pipe>,
    rng: Rng,
    /// The previous gap's centre: the next gap is drawn within
    /// [`GAP_MAX_STEP`] of it. State, so the digest absorbs it.
    last_gap_y: i64,
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
            body: Store::new(),
            rng: Rng::new(Seed::from_u64(seed), StreamId::from_u64(1)),
            last_gap_y: 120 * ONE,
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
        self.body.len()
    }

    /// Entity slots ever allocated. Flat over a long run when despawn
    /// works; climbing when it leaks. Public because the test that
    /// proves the fix needs the number, and a renderer never should.
    #[must_use]
    pub fn entity_capacity(&self) -> usize {
        self.entities.capacity()
    }

    /// The bird's centre in whole screen units — a derived read for
    /// renderers, not state: nothing here enters the digest, and the
    /// test beside the accessors holds that sentence true. Division
    /// truncates; the bird's range keeps it non-negative.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "bounded by the ceiling and floor rules to [0, 240] and change"
    )]
    pub fn bird_y_units(&self) -> i32 {
        (self.bird_y / ONE) as i32
    }

    /// Visit every pipe as (left edge x, gap centre y) in whole screen
    /// units, ascending slot order — the store's own guarantee, so draw
    /// order is a property of the rules, not the storage. Allocation-free
    /// by construction: the visitor closes over no collection and the
    /// store walk is lazy. A pipe's x truncates toward zero while it
    /// leaves the screen (one unit of difference from floor for one
    /// visible column, pinned by test).
    #[allow(
        clippy::cast_possible_truncation,
        reason = "pipe coordinates are bounded by spawn position and exit cull to well under i32"
    )]
    pub fn for_each_pipe_units(&self, mut visit: impl FnMut(i32, i32)) {
        for (_, pipe) in self.body.iter() {
            visit((pipe.x / ONE) as i32, (pipe.gap_y / ONE) as i32);
        }
    }

    /// A deterministic pilot: flap when falling below the nearest
    /// oncoming gap's centre. Pure — a function of the state it reads —
    /// so a recorded autopilot run replays exactly.
    ///
    /// Public for three consumers that all need a run that *survives*:
    /// the leak regression (despawn only happens while alive), the
    /// allocation gate (a dead world's tick measures nothing), and the
    /// committed traces (a demo that scores nothing demonstrates
    /// nothing).
    #[must_use]
    pub fn autopilot(&self) -> bool {
        if !self.alive {
            return false;
        }
        let target = self
            .body
            .iter()
            .filter(|(_, pipe)| pipe.x + PIPE_WIDTH >= BIRD_X - BIRD_HALF)
            .min_by_key(|(_, pipe)| pipe.x)
            .map_or(120 * ONE, |(_, pipe)| pipe.gap_y);
        self.bird_velocity > 0 && self.bird_y > target
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
        let delta = (i64::from(self.rng.below_u32(STEP_SPAN)) - GAP_MAX_STEP / ONE) * ONE;
        let gap_y = (self.last_gap_y + delta).clamp(GAP_MARGIN, FLOOR - GAP_MARGIN);
        self.last_gap_y = gap_y;
        let entity = self.entities.spawn();
        self.pipe.insert(entity.index(), entity);
        self.body.insert(
            entity.index(),
            Pipe {
                x: PIPE_SPAWN_X,
                gap_y,
                passed: false,
            },
        );
    }

    fn advance_pipes(&mut self) {
        // Not `for_each_mut`: that method collects its slot list into a
        // fresh vector on every call — one heap allocation per live
        // tick, measured — and this crate's steady state allocates
        // nothing. The world's own scratch list does the same walk for
        // free.
        self.swept.clear();
        self.swept.extend(self.body.iter().map(|(slot, _)| slot));
        for index in 0..self.swept.len() {
            if let Some(pipe) = self.body.get_mut(self.swept[index]) {
                pipe.x -= PIPE_SPEED;
            }
        }
        // Sweep in ascending slot order (`iter`'s guarantee), so the
        // order pipes leave the world is a property of the rules and not
        // of the storage. The scratch list is reused, never reallocated
        // at steady state.
        self.swept.clear();
        self.swept.extend(
            self.body
                .iter()
                .filter(|(_, pipe)| pipe.x + PIPE_WIDTH < 0)
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
            self.body.remove(slot);
        }
    }

    fn collide_and_score(&mut self) {
        let bird_top = self.bird_y - BIRD_HALF;
        let bird_bottom = self.bird_y + BIRD_HALF;
        let mut hit = false;

        self.swept.clear();
        for (slot, pipe) in self.body.iter() {
            let overlaps_x =
                pipe.x < BIRD_X + BIRD_HALF && pipe.x + PIPE_WIDTH > BIRD_X - BIRD_HALF;
            if overlaps_x {
                let gap_top = pipe.gap_y - PIPE_GAP / 2;
                let gap_bottom = pipe.gap_y + PIPE_GAP / 2;
                if bird_top < gap_top || bird_bottom > gap_bottom {
                    hit = true;
                }
            }
            if pipe.x + PIPE_WIDTH < BIRD_X - BIRD_HALF && !pipe.passed {
                self.swept.push(slot);
            }
        }

        for index in 0..self.swept.len() {
            let slot = self.swept[index];
            if let Some(pipe) = self.body.get_mut(slot) {
                pipe.passed = true;
            }
            self.score += 1;
        }
        if hit {
            self.alive = false;
        }
    }

    /// The digest covers every store, every scalar, and the generator.
    /// A determinism oracle that omits part of the state passes happily
    /// while blind — only a discrimination check (a different seed must
    /// move the digest) can see the omission, so both live in the tests.
    /// The generator's own fingerprint rides in because hidden RNG state
    /// is exactly the part everyone forgets: two worlds differing only
    /// there digest identically until the next spawn, then diverge with
    /// nothing to explain it.
    ///
    /// **Nothing here is pointer-width.** Slot counts and store lengths
    /// are `usize` at their source and are narrowed to `u32` before they
    /// are absorbed, so the same run digests identically on a target
    /// with a different pointer size. Both are bounded by `u32::MAX` by
    /// their own types' contracts — a world with four billion pipes has
    /// other problems — so the narrowing loses nothing.
    ///
    /// What remains implicit, stated rather than pretended away: the
    /// entity allocator's free list is not directly observable through
    /// its public surface, so its ORDER is not absorbed. It is not
    /// hidden state in the dangerous sense — the next spawn reflects it
    /// in a slot number this digest does absorb — but a reader auditing
    /// this for totality should know it is reached one tick late rather
    /// than immediately.
    fn absorb(&mut self) {
        let (rng_state, rng_increment) = self.rng.parts();
        let highest_slot = u32::try_from(self.entities.capacity()).unwrap_or(u32::MAX);
        let live_entities = u32::try_from(self.entities.len()).unwrap_or(u32::MAX);
        let live_pipes = u32::try_from(self.body.len()).unwrap_or(u32::MAX);
        self.digest = self
            .digest
            .absorb_u64(rng_state)
            .absorb_u64(rng_increment)
            .absorb_u32(highest_slot)
            .absorb_u32(live_entities)
            .absorb_bytes(&self.last_gap_y.to_le_bytes())
            .absorb_u64(self.tick)
            .absorb_bytes(&self.bird_y.to_le_bytes())
            .absorb_bytes(&self.bird_velocity.to_le_bytes())
            .absorb_u64(self.score)
            .absorb_u64(u64::from(self.alive))
            .absorb_u32(live_pipes);
        for (slot, pipe) in self.body.iter() {
            let generation = self.pipe.get(slot).map_or(0, |entity| entity.generation());
            self.digest = self
                .digest
                .absorb_u32(slot)
                .absorb_u32(generation)
                .absorb_bytes(&pipe.x.to_le_bytes())
                .absorb_bytes(&pipe.gap_y.to_le_bytes())
                .absorb_u64(u64::from(pipe.passed));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(seed: u64, ticks: u64) -> World {
        let mut world = World::new(seed);
        for _ in 0..ticks {
            let flap = world.autopilot();
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
    fn the_view_constants_match_the_rules_they_describe() {
        // Unit test rather than a const block on purpose: const
        // evaluation executes no runtime lines, and the coverage gate
        // once refused exactly that shape. Same claim, visible
        // execution. VIEW_WIDTH is deliberately its own number -- the
        // identity below is where a future spawn-position change breaks
        // loudly instead of silently rescaling every committed image.
        assert_eq!(i64::from(VIEW_WIDTH) * ONE, PIPE_SPAWN_X);
        assert_eq!(i64::from(VIEW_HEIGHT) * ONE, FLOOR);
        assert_eq!(i64::from(BIRD_X_UNITS) * ONE, BIRD_X);
        assert_eq!(i64::from(BIRD_HALF_UNITS) * ONE, BIRD_HALF);
        assert_eq!(i64::from(PIPE_WIDTH_UNITS) * ONE, PIPE_WIDTH);
        assert_eq!(i64::from(PIPE_GAP_HALF_UNITS) * ONE * 2, PIPE_GAP);
    }

    #[test]
    fn the_view_is_a_read_and_truncates_where_it_says_it_does() {
        // Observed pins, the crate's committed-fixture method: seed 7
        // piloted to tick 361 puts the first-visited pipe's left edge
        // one unit past the screen edge at -4 -- where floor division
        // would say -5, so this line is the truncation contract. The
        // rest pins the accessors against the same run.
        let mut world = World::new(7);
        for _ in 0..361 {
            let flap = world.autopilot();
            world.step(flap);
        }
        assert!(world.alive(), "the pilot survives to the pin");
        assert_eq!(world.score(), 1, "observed at tick 361");
        assert_eq!(world.bird_y_units(), 162, "observed at tick 361");
        let before = world.digest();
        let mut pipes = Vec::new();
        world.for_each_pipe_units(|x, gap| pipes.push((x, gap)));
        assert_eq!(
            pipes,
            [(-4, 148), (76, 181), (157, 147), (238, 159), (319, 192)],
            "observed at tick 361; the -4 is truncation, floor would say -5"
        );
        assert_eq!(
            world.digest(),
            before,
            "the view is a read: rendering-side access must not move the digest"
        );
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
        // The oracle is only as good as the run is alive: a dead world
        // neither spawns nor sweeps, so the first version of this test
        // passed with the despawn call deleted — measured, not
        // suspected. So the test now proves its own premises: the pilot
        // survives, hundreds of pipes cross the screen, and the bound is
        // peak concurrency, not a guess. Acceptance bar: delete
        // `entities.despawn` and this must fail on slot growth.
        let mut world = World::new(7);
        let mut peak = 0;
        for _ in 0..60_000 {
            let flap = world.autopilot();
            world.step(flap);
            peak = peak.max(world.pipes());
        }
        assert!(
            world.alive(),
            "the pilot died — everything after would be a frozen corpse"
        );
        assert!(world.score() > 100, "hundreds of pipes must have scored");
        let capacity = world.entity_capacity();
        assert!(
            capacity <= peak + 2,
            "slot count {capacity} exceeds peak concurrency {peak} — despawn is leaking"
        );
    }

    #[test]
    fn scoring_happens_and_death_is_reachable() {
        // The steering policy scores; the blind world (never flapping)
        // dies on the floor. Both ends of the game are real.
        let steered = run(7, 20_000);
        let score = steered.score();
        assert!(score > 0, "the pilot never scored");
        let mut doomed = World::new(7);
        for _ in 0..2_000 {
            doomed.step(false);
        }
        assert!(!doomed.alive(), "a bird that never flaps must fall");
    }

    #[test]
    fn the_ceiling_stops_the_bird_without_killing_it() {
        // Flap every tick: the bird climbs to the clamp and stays alive
        // there — the ceiling is a wall, the floor is the grave.
        let mut world = World::new(7);
        // 200 ticks: the clamp engages within thirty, and the first
        // pipe cannot reach the bird before tick ~287 — dying to a pipe
        // while pinned at the ceiling would test collision, not the wall.
        for _ in 0..200 {
            world.step(true);
        }
        assert!(world.alive(), "the ceiling must not kill");
    }

    #[test]
    fn pipes_exist_at_steady_state() {
        let world = run(7, 1_000);
        assert!(world.pipes() > 0, "pipes spawn and persist on screen");
        assert!(
            world.pipes() <= world.entity_capacity(),
            "live pipes are bounded by slots ever allocated"
        );
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
        assert!(
            !world.autopilot(),
            "a dead world's pilot asks for nothing — the driver may keep polling it"
        );
    }
}
