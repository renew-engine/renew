//! A platformer's rules: run, jump, land, and nothing else.
//!
//! A pure fixed-step function of per-tick input, which is what lets a replay
//! be an assertion rather than a demonstration. Every quantity is fixed-point
//! and every value that can change future behaviour reaches the digest.
//!
//! # What this exists to prove
//!
//! The collision crate was written against a specification, and a
//! specification cannot be run. This world is the first thing that uses it the
//! way a game does — a character that has to stand on something, slide along
//! walls rather than stick to them, and know the difference between the two.
//! One defect in the sweep was already found that way, and it had passed every
//! test written against the geometry in isolation.

// The determinism rule in the language standard: a simulation crate performs
// no floating-point arithmetic whose result can reach digested state. Denied
// here rather than left to review — the lint covers operators only, so it is
// necessary and not sufficient, but what it covers it covers with teeth.
#![deny(clippy::float_arithmetic)]

use renew_ecs::{Entities, Entity, Store};
use renew_fixed::{Angle, Fixed, Vec2};
use renew_frame::StateHash;
use renew_physics2d::{
    BodyKind, Collider, Filter, Shape, ShapeIndex, SlideEnd, SlideHit, Transform, World,
};
use renew_scene::{Global, Local, Parent, Scratch, propagate};

/// What the player is asking for this tick.
///
/// Deliberately not a key state: a world that took keys would have to know
/// about keyboards, and a replay would have to reproduce one. Intent is the
/// smaller thing, and it is what a recording stores.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Intent {
    /// −1, 0 or +1. Anything else is clamped, because a caller that means
    /// "hard left" and a caller with a stuck analogue stick must not produce
    /// different worlds.
    pub run: i32,
    /// Whether jump is held. Edge detection is the world's job, not the
    /// caller's — otherwise two drivers disagree about what a tap is.
    pub jump: bool,
}

impl Intent {
    /// Standing still.
    pub const IDLE: Self = Self {
        run: 0,
        jump: false,
    };

    /// Running in a direction, not jumping.
    #[must_use]
    pub const fn running(run: i32) -> Self {
        Self { run, jump: false }
    }

    /// Holding jump while running.
    #[must_use]
    pub const fn jumping(run: i32) -> Self {
        Self { run, jump: true }
    }

    fn direction(self) -> i32 {
        self.run.clamp(-1, 1)
    }
}

/// How the character is touching the world.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Footing {
    /// Standing on something this tick.
    pub grounded: bool,
    /// Touching a wall on the left or right.
    pub against_wall: bool,
    /// The move ran out of slide iterations with displacement unspent.
    ///
    /// **Its own bit rather than folded into `against_wall`**, which is what
    /// it was at first. The two are different facts: a character running along
    /// a wall is against one and moving fine, and a character wedged in a
    /// crevice has stopped for a reason the caller may want to act on. Merging
    /// them loses the distinction exactly where it matters.
    pub wedged: bool,
    /// Ticks since the character last stood on something.
    ///
    /// **This is what coyote time is made of**, and it is state rather than a
    /// derived value: a character that walks off a ledge is not grounded, and
    /// a jump in the few ticks after that has to still work or the game feels
    /// like it is ignoring the player.
    pub ticks_airborne: u32,
}

/// The tuning a caller may not change mid-run.
///
/// Fixed at construction because every one of these reaches the digest through
/// the character's position. A world built with different numbers is a
/// different world, and restoring a save into one would be a silent divergence
/// rather than an error.
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    /// Downward acceleration per tick.
    pub gravity: Fixed,
    /// Horizontal speed while running.
    pub run_speed: Fixed,
    /// Upward speed applied at the moment of a jump.
    pub jump_speed: Fixed,
    /// Fastest the character may fall, so a long drop stays swept accurately.
    pub terminal_speed: Fixed,
    /// Ticks after leaving the ground during which a jump still works.
    pub coyote_ticks: u32,
    /// How far short of a surface the character stops.
    pub skin: Fixed,
    /// How many sweep-and-slide iterations one move may take.
    pub slide_iterations: u32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            gravity: Fixed::from_ratio(-1, 40),
            run_speed: Fixed::from_ratio(1, 5),
            jump_speed: Fixed::from_ratio(3, 5),
            terminal_speed: Fixed::from_int(-1),
            coyote_ticks: 6,
            skin: Fixed::from_bits(64),
            slide_iterations: 4,
        }
    }
}

/// A rectangle of solid ground.
#[derive(Clone, Copy, Debug)]
pub struct Platform {
    /// Centre.
    pub centre: Vec2,
    /// Half-extents.
    pub half_extents: Vec2,
}

impl Platform {
    /// A platform, in whole units.
    #[must_use]
    pub fn new(x: i32, y: i32, half_width: i32, half_height: i32) -> Self {
        Self {
            centre: Vec2::new(Fixed::from_int(x), Fixed::from_int(y)),
            half_extents: Vec2::new(Fixed::from_int(half_width), Fixed::from_int(half_height)),
        }
    }
}

/// A platform carried around a turning pivot.
///
/// **Where the deck is, is never written down.** A hub sits at `pivot` and
/// turns by `turn_per_tick`; the deck hangs off it at a fixed `arm` and its
/// world placement is whatever composing those two gives. That is the whole
/// reason this exists in a platformer sample rather than a unit test: a deck
/// that only slid back and forth could be driven by adding two vectors, and
/// would be evidence for nothing. Rotating the arm is the case addition
/// cannot express.
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    /// Centre of the turn, in world space.
    pub pivot: Vec2,
    /// Offset from the hub to the deck, measured in the hub's frame.
    pub arm: Vec2,
    /// Half-extents of the deck.
    pub half_extents: Vec2,
    /// How far the hub turns each tick.
    pub turn_per_tick: Angle,
}

/// One built orbit: the two nodes it became, and the rate it turns at.
#[derive(Clone, Copy, Debug)]
struct Orbiting {
    hub: Entity,
    deck: Entity,
    turn_per_tick: Angle,
}

/// Everything the terrain collides with.
/// How large the character is, measured from its centre.
///
/// **Public because anything drawing the world needs it.** A drawing that
/// guesses the size puts the character somewhere it is not — one cell tall
/// when it is two makes a character standing on the floor look like a
/// character hovering above it, and the picture then contradicts the
/// `grounded` flag printed beside it.
pub const CHARACTER_HALF_EXTENTS: Vec2 = Vec2::new(Fixed::from_ratio(1, 2), Fixed::ONE);

const TERRAIN_LAYER: u32 = 0b01;
/// Everything the character is.
const CHARACTER_LAYER: u32 = 0b10;

/// How many slide surfaces one tick may record.
///
/// Two is the useful number: a corner is a floor and a wall, and a character
/// that met more than two distinct surfaces in one tick is wedged rather than
/// moving.
const MAX_SLIDE_HITS: usize = 4;

/// A normal must lean this far toward vertical to count as ground.
///
/// Comparing the normal's *y* against a fraction of one is the whole slope
/// test. At one half this is a forty-five degree limit: anything steeper is a
/// wall the character slides down rather than a floor it stands on.
fn is_ground(normal: Vec2) -> bool {
    normal.y >= Fixed::from_ratio(1, 2)
}

/// A normal this close to horizontal is a wall.
fn is_wall(normal: Vec2) -> bool {
    normal.x.abs() >= Fixed::from_ratio(1, 2)
}

/// The world: a character, some platforms, and the rules between them.
pub struct Leap {
    physics: World,
    entities: Entities,
    character: Entity,
    tuning: Tuning,
    velocity: Vec2,
    footing: Footing,
    /// Whether jump was held last tick, so a held button does not re-fire.
    jump_was_held: bool,
    tick: u64,
    hits: [SlideHit; MAX_SLIDE_HITS],
    /// The hierarchy behind every moving platform. Three stores and a buffer
    /// rather than one list of positions, because the positions are the
    /// *output*: nothing here may be written by hand, or the sample would be
    /// asserting against numbers it had itself supplied.
    locals: Store<Local>,
    parents: Store<Parent>,
    globals: Store<Global>,
    scratch: Scratch,
    orbits: Vec<Orbiting>,
}

impl Leap {
    /// A world with the given platforms and the character at `start`.
    #[must_use]
    pub fn new(tuning: Tuning, start: Vec2, platforms: &[Platform]) -> Self {
        let mut entities = Entities::new();
        let mut physics = World::new();

        let character = entities.spawn();
        physics.create_body(character, BodyKind::Kinematic, Transform::at(start));
        physics.add_shape(
            character,
            Shape::Box {
                half_extents: CHARACTER_HALF_EXTENTS,
            },
            Transform::IDENTITY,
            Filter::new(CHARACTER_LAYER, TERRAIN_LAYER),
        );

        for platform in platforms {
            let handle = entities.spawn();
            physics.create_body(handle, BodyKind::Static, Transform::at(platform.centre));
            physics.add_shape(
                handle,
                Shape::Box {
                    half_extents: platform.half_extents,
                },
                Transform::IDENTITY,
                Filter::new(TERRAIN_LAYER, CHARACTER_LAYER),
            );
        }

        Self {
            physics,
            entities,
            character,
            tuning,
            velocity: Vec2::ZERO,
            footing: Footing {
                grounded: false,
                against_wall: false,
                wedged: false,
                ticks_airborne: u32::MAX,
            },
            jump_was_held: false,
            tick: 0,
            hits: [SlideHit {
                collider: Collider {
                    handle: character,
                    index: ShapeIndex::from_raw(0),
                },
                normal: Vec2::ZERO,
                origin: Vec2::ZERO,
            }; MAX_SLIDE_HITS],
            locals: Store::default(),
            parents: Store::default(),
            globals: Store::default(),
            scratch: Scratch::new(),
            orbits: Vec::new(),
        }
    }

    /// Add a moving platform, and return the entity its deck is.
    ///
    /// Additive rather than a fourth argument to [`Leap::new`]: a level with no
    /// moving parts says so by not calling this, and its digest is byte-for-byte
    /// what it was before moving platforms existed.
    ///
    /// **Construction-only, for the same reason [`Tuning`] is fixed at
    /// construction.** Calling it mid-run adds a body and changes the digest
    /// from that tick onward, so two runs of the same input trace would diverge
    /// on nothing the trace records. Nothing enforces it — a world does not know
    /// when its caller thinks setup ended — which is why it is written down.
    pub fn add_orbit(&mut self, orbit: Orbit) -> Entity {
        let hub = self.entities.spawn();
        self.locals
            .insert(hub.index(), Local::new(orbit.pivot, Angle::ZERO));

        let deck = self.entities.spawn();
        self.locals
            .insert(deck.index(), Local::new(orbit.arm, Angle::ZERO));
        self.parents.insert(deck.index(), Parent(hub));

        // The body is created at the origin on purpose. Seeding it with
        // `pivot + arm` would be the sample writing down an answer it is
        // supposed to derive, and it would agree with the derivation for
        // exactly as long as nobody gave the hub a starting angle.
        self.physics
            .create_body(deck, BodyKind::Kinematic, Transform::IDENTITY);
        self.physics.add_shape(
            deck,
            Shape::Box {
                half_extents: orbit.half_extents,
            },
            Transform::IDENTITY,
            Filter::new(TERRAIN_LAYER, CHARACTER_LAYER),
        );

        self.orbits.push(Orbiting {
            hub,
            deck,
            turn_per_tick: orbit.turn_per_tick,
        });
        self.place_decks();
        deck
    }

    /// Turn every hub by one tick's worth, then move the decks to wherever
    /// that puts them.
    fn advance_orbits(&mut self) {
        for orbiting in &self.orbits {
            if let Some(local) = self.locals.get_mut(orbiting.hub.index()) {
                local.rotation = local.rotation + orbiting.turn_per_tick;
            }
        }
        self.place_decks();
    }

    /// Compose the hierarchy and hand the result to the physics world.
    ///
    /// Every deck transform after construction comes from here — `add_orbit`
    /// creates the body at the origin and then calls this before returning, so
    /// no deck is ever observable at a position the hierarchy did not decide.
    ///
    /// A deck always has a placement: it is created with a `Local` and a live
    /// hub and neither is ever removed. That is asserted rather than papered
    /// over, because the two ways of papering over it disagree — skipping
    /// `set_transform` leaves the collider at the origin while the digest would
    /// read the hierarchy, and a deck whose collision and whose reported
    /// position differ is precisely the bug this whole arrangement prevents.
    fn place_decks(&mut self) {
        let counts = propagate(
            &mut self.scratch,
            &self.entities,
            &self.parents,
            &self.locals,
            &mut self.globals,
        );
        debug_assert_eq!(
            counts.orphaned + counts.cyclic,
            0,
            "a deck hangs off one live hub and nothing else"
        );
        for orbiting in &self.orbits {
            let placed = self.globals.get(orbiting.deck.index()).copied();
            debug_assert!(placed.is_some(), "every deck keeps its placement");
            if let Some(placed) = placed {
                self.physics.set_transform(
                    orbiting.deck,
                    Transform {
                        translation: placed.translation(),
                        rotation: placed.rotation(),
                    },
                );
            }
        }
    }

    /// Advance one tick.
    pub fn step(&mut self, intent: Intent) {
        self.tick += 1;

        // Platforms move first, so the character's sweep this tick meets them
        // where they now are. Moving them afterwards would let a deck pass
        // through a character that had already committed to standing still.
        self.advance_orbits();

        // Horizontal velocity is set, not accumulated: a platformer whose
        // character keeps sliding after the key is released feels broken, and
        // acceleration is a tuning choice this world does not need to make to
        // exercise the collision crate.
        let run = Fixed::from_int(intent.direction()).saturating_mul(self.tuning.run_speed);

        // A jump fires on the tick the button goes down, and only while the
        // character is grounded or inside its coyote window. Reading the held
        // state instead would let a player hold jump and bounce forever.
        let pressed = intent.jump && !self.jump_was_held;
        let may_jump =
            self.footing.grounded || self.footing.ticks_airborne <= self.tuning.coyote_ticks;
        let mut vertical = if pressed && may_jump {
            self.tuning.jump_speed
        } else {
            self.velocity.y + self.tuning.gravity
        };
        vertical = vertical.max(self.tuning.terminal_speed);
        self.jump_was_held = intent.jump;

        self.velocity = Vec2::new(run, vertical);

        let report = self.physics.move_and_slide(
            self.character,
            self.velocity,
            TERRAIN_LAYER,
            self.tuning.skin,
            self.tuning.slide_iterations,
            &mut self.hits,
        );

        // The character is created in `new` and never destroyed, so the slide
        // always finds a body. Written as a branch on `None` this was an arm
        // no test could reach, and one that would have quietly frozen the
        // character rather than saying what went wrong.
        let report =
            report.unwrap_or_else(|| unreachable!("the character body outlives every step"));

        let mut grounded = false;
        let mut against_wall = false;
        let wedged;
        {
            let written = report.hits.written.min(MAX_SLIDE_HITS);
            for hit in self.hits.split_at(written).0 {
                if is_ground(hit.normal) {
                    grounded = true;
                }
                if is_wall(hit.normal) {
                    against_wall = true;
                }
            }
            // Landing kills downward speed; hitting a ceiling kills upward.
            // Without this a character resting on the floor accumulates
            // gravity every tick and launches when it next steps off.
            if grounded && self.velocity.y < Fixed::ZERO {
                self.velocity = Vec2::new(self.velocity.x, Fixed::ZERO);
            }
            // Running into a wall spends the horizontal motion, so the next
            // tick does not keep pressing into it.
            if against_wall {
                self.velocity = Vec2::new(Fixed::ZERO, self.velocity.y);
            }
            // A slide that ran out of tries left displacement unspent, and
            // saying so is what stops that from looking like arrival.
            wedged = report.end == SlideEnd::IterationsExhausted;
        }

        self.footing = Footing {
            grounded,
            against_wall,
            wedged,
            ticks_airborne: if grounded {
                0
            } else {
                self.footing.ticks_airborne.saturating_add(1)
            },
        };
    }

    /// Where the character is.
    #[must_use]
    pub fn position(&self) -> Vec2 {
        self.physics
            .transform(self.character)
            .map_or(Vec2::ZERO, |at| at.translation)
    }

    /// How fast it is moving.
    #[must_use]
    pub const fn velocity(&self) -> Vec2 {
        self.velocity
    }

    /// How it is touching the world.
    #[must_use]
    pub const fn footing(&self) -> Footing {
        self.footing
    }

    /// Where a deck currently is, for anything drawing the world.
    ///
    /// Reads the composed placement rather than the physics body, so a caller
    /// asking "where is it" and the collision it is about to have cannot
    /// answer differently.
    #[must_use]
    pub fn deck_placement(&self, deck: Entity) -> Option<(Vec2, Angle)> {
        self.globals
            .get(deck.index())
            .map(|placed| (placed.translation(), placed.rotation()))
    }

    /// Where the collision world thinks a deck is.
    ///
    /// The counterpart to [`Leap::deck_placement`], which reads the hierarchy.
    /// Exposed so a test can hold the two against each other: the same formula
    /// is written once here and once in the collision crate, and a deck that
    /// collided somewhere other than where it says it is would be the one
    /// failure this whole arrangement exists to prevent.
    #[must_use]
    pub fn deck_transform(&self, deck: Entity) -> Option<(Vec2, Angle)> {
        self.physics
            .transform(deck)
            .map(|at| (at.translation, at.rotation))
    }

    /// Every moving deck, in the order it was added.
    ///
    /// No `#[must_use]`: the iterator it returns already carries one, and
    /// doubling it is a lint error rather than extra safety.
    pub fn decks(&self) -> impl Iterator<Item = Entity> + '_ {
        self.orbits.iter().map(|orbiting| orbiting.deck)
    }

    /// How many ticks have run.
    #[must_use]
    pub const fn tick(&self) -> u64 {
        self.tick
    }

    /// A hash over every value that can change future behaviour.
    ///
    /// **Position, velocity, footing, the jump latch, and every moving deck's
    /// placement — and nothing else.** The static platform list cannot change,
    /// the tuning is fixed at construction, and the entity allocator's
    /// internals are not observable; including any of them would make the
    /// digest sensitive to things a replay does not reproduce. Leaving out
    /// something that *does* change behaviour is the failure that matters,
    /// which is why the jump latch is here: without it two worlds with
    /// identical positions can diverge on the next tick.
    ///
    /// **What the deck placements stand in for.** The only mutable state in the
    /// hierarchy is each hub's rotation, and it is absorbed *directly* rather
    /// than left to be inferred. It would in fact be recoverable today from the
    /// deck's composed rotation, because `add_orbit` always gives a deck a zero
    /// local rotation — but that is an accident of one constructor, and a
    /// digest resting on it would silently stop covering the hub the day a deck
    /// gained a rotation of its own.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let position = self.position();
        let mut hash = StateHash::new()
            .absorb_u64(self.tick)
            .absorb_u64(position.x.to_bits().cast_unsigned())
            .absorb_u64(position.y.to_bits().cast_unsigned())
            .absorb_u64(self.velocity.x.to_bits().cast_unsigned())
            .absorb_u64(self.velocity.y.to_bits().cast_unsigned())
            .absorb_u32(u32::from(self.footing.grounded))
            .absorb_u32(u32::from(self.footing.against_wall))
            .absorb_u32(self.footing.ticks_airborne)
            .absorb_u32(u32::from(self.jump_was_held));

        // Every hub angle and every deck placement, because a platform
        // somewhere else next tick is a different world. A level with no moving
        // parts absorbs nothing here and digests exactly as it did before they
        // existed, which is what let this arrive without restating a single
        // recorded hash.
        for orbiting in &self.orbits {
            let turned = self
                .locals
                .get(orbiting.hub.index())
                .map_or(Angle::ZERO, |hub| hub.rotation);
            let placed = self
                .globals
                .get(orbiting.deck.index())
                .copied()
                .unwrap_or(Global::IDENTITY);
            hash = hash
                .absorb_u32(turned.to_bits())
                .absorb_u64(placed.translation().x.to_bits().cast_unsigned())
                .absorb_u64(placed.translation().y.to_bits().cast_unsigned())
                .absorb_u32(placed.rotation().to_bits());
        }
        hash.finish()
    }

    /// How many entities the world holds — the character, its static platforms,
    /// and two more for each moving one (a hub and the deck that hangs off it).
    ///
    /// Exposed so a test can hold it flat over a long run: a world that leaked
    /// a slot per tick would still simulate correctly and still digest
    /// consistently, and only this would show it.
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }
}
