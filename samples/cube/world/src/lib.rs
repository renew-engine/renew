//! A voxel world: blocks, a player who walks on them, and the ray that decides
//! which one is being looked at.
//!
//! A pure fixed-step function of per-tick input. Everything is fixed-point and
//! every value that can change future behaviour reaches the digest.
//!
//! # What this exists to prove
//!
//! The three-dimensional collision crate was written against a vocabulary, and
//! a vocabulary cannot be run. This is the first thing that uses it the way a
//! game does — a player who has to stand on blocks, stop at walls, and pick out
//! the one block in front of them from thousands.
//!
//! # Why blocks are not physics bodies
//!
//! A chunk of sixteen cubed is four thousand cells. Creating a body per solid
//! cell would make the collider set the largest thing in the world and the
//! broadphase's cost a function of terrain rather than of moving things.
//!
//! So the grid stays the source of truth and movement asks it directly: gather
//! the cells the swept path could touch, and sweep against each one's box with
//! the collision crate's own geometry. The crate supplies the arithmetic; the
//! world supplies the acceleration structure, which for a uniform grid is the
//! grid itself.

// The determinism rule in the language standard: a simulation crate performs
// no floating-point arithmetic whose result can reach digested state.
#![deny(clippy::float_arithmetic)]

pub mod grid;
pub mod ray;

use renew_fixed::{Fixed, Vec3};
use renew_frame::StateHash;
use renew_physics3d::{Shape, Transform, sweep};

pub use grid::{AIR, Block, Cell, Grid, STONE, block_half_extent};
pub use ray::{Face, Pick, pick};

/// What the player is asking for this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Intent {
    /// East–west, clamped to −1, 0 or +1.
    pub walk_x: i32,
    /// North–south, clamped the same way.
    pub walk_z: i32,
    /// Whether jump is held. Edge detection is the world's job.
    pub jump: bool,
    /// Break the block being looked at.
    pub dig: bool,
    /// Place a block against the face being looked at.
    pub place: bool,
}

impl Intent {
    /// Doing nothing.
    pub const IDLE: Self = Self {
        walk_x: 0,
        walk_z: 0,
        jump: false,
        dig: false,
        place: false,
    };

    /// Walking in a direction.
    #[must_use]
    pub const fn walking(x: i32, z: i32) -> Self {
        Self {
            walk_x: x,
            walk_z: z,
            ..Self::IDLE
        }
    }
}

/// The tuning a caller may not change mid-run.
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    /// Downward acceleration per tick.
    pub gravity: Fixed,
    /// Horizontal speed while walking.
    pub walk_speed: Fixed,
    /// Upward speed applied at the moment of a jump.
    pub jump_speed: Fixed,
    /// Fastest the player may fall.
    pub terminal_speed: Fixed,
    /// How far short of a surface the player stops.
    pub skin: Fixed,
    /// How many sweep-and-slide iterations one move may take.
    pub slide_iterations: u32,
    /// How far the player can reach to break or place.
    pub reach: Fixed,
    /// Half the player's width and depth.
    pub half_width: Fixed,
    /// Half the player's height.
    pub half_height: Fixed,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            gravity: Fixed::from_ratio(-1, 40),
            walk_speed: Fixed::from_ratio(1, 5),
            jump_speed: Fixed::from_ratio(1, 2),
            terminal_speed: Fixed::from_int(-1),
            skin: Fixed::from_bits(64),
            slide_iterations: 3,
            reach: Fixed::from_int(5),
            half_width: Fixed::from_ratio(3, 10),
            half_height: Fixed::from_ratio(9, 10),
        }
    }
}

/// A normal must lean this far toward vertical to count as ground.
fn is_ground(normal: Vec3) -> bool {
    normal.y >= Fixed::from_ratio(1, 2)
}

/// The world.
#[expect(
    clippy::struct_excessive_bools,
    reason = "three of the four are input latches — jump, dig and place each               need their own previous state for edge detection, and merging               any two of them would make a player who digs while jumping               behave differently from one who does not"
)]
pub struct Cube {
    grid: Grid,
    tuning: Tuning,
    position: Vec3,
    velocity: Vec3,
    /// Which way the player is looking. Unit length.
    look: Vec3,
    grounded: bool,
    jump_was_held: bool,
    dig_was_held: bool,
    place_was_held: bool,
    tick: u64,
    /// How many blocks have been broken and placed, so a test can tell a world
    /// that did nothing from one that did the same amount of everything.
    broken: u32,
    placed: u32,
}

impl Cube {
    /// A world with the given terrain and the player at `start`.
    #[must_use]
    pub fn new(tuning: Tuning, grid: Grid, start: Vec3) -> Self {
        Self {
            grid,
            tuning,
            position: start,
            velocity: Vec3::ZERO,
            look: Vec3::new(Fixed::ZERO, Fixed::ZERO, Fixed::ONE),
            grounded: false,
            jump_was_held: false,
            dig_was_held: false,
            place_was_held: false,
            tick: 0,
            broken: 0,
            placed: 0,
        }
    }

    /// Point the player somewhere. Ignored if the direction has no length,
    /// since a zero direction names no block and would otherwise silently
    /// leave the player looking along whatever they last chose.
    pub fn look_at(&mut self, direction: Vec3) {
        if let Some(unit) = direction.normalize() {
            self.look = unit;
        }
    }

    /// The player's shape.
    fn body(&self) -> Shape {
        Shape::Box {
            half_extents: Vec3::new(
                self.tuning.half_width,
                self.tuning.half_height,
                self.tuning.half_width,
            ),
        }
    }

    /// Advance one tick.
    pub fn step(&mut self, intent: Intent) {
        self.tick += 1;

        self.act(intent);

        let walk_x = Fixed::from_int(intent.walk_x.clamp(-1, 1)) * self.tuning.walk_speed;
        let walk_z = Fixed::from_int(intent.walk_z.clamp(-1, 1)) * self.tuning.walk_speed;

        let pressed = intent.jump && !self.jump_was_held;
        let mut vertical = if pressed && self.grounded {
            self.tuning.jump_speed
        } else {
            self.velocity.y + self.tuning.gravity
        };
        vertical = vertical.max(self.tuning.terminal_speed);
        self.jump_was_held = intent.jump;

        self.velocity = Vec3::new(walk_x, vertical, walk_z);
        self.move_and_slide();
    }

    /// Break or place, on the tick the button goes down.
    fn act(&mut self, intent: Intent) {
        let dig = intent.dig && !self.dig_was_held;
        let place = intent.place && !self.place_was_held;
        self.dig_was_held = intent.dig;
        self.place_was_held = intent.place;

        if !dig && !place {
            return;
        }
        let Some(picked) = pick(&self.grid, self.eye(), self.look, self.tuning.reach) else {
            return;
        };
        if dig {
            if self.grid.set(picked.cell, AIR) {
                self.broken += 1;
            }
        } else {
            // Placing, because the guard above established that one of the two
            // is happening. Written as `else if place` this had a third path
            // nothing could take, and dig wins a tick where both are pressed.
            // Against the face, not into the block — placing into the block
            // being looked at would replace it, which is what digging is for.
            let target = picked.neighbour();
            // And never inside the player: a block placed where the body
            // stands would trap it with no way out, and the check has to
            // happen here because the grid does not know where anybody is.
            if !self.body_overlaps(target) && self.grid.set(target, STONE) {
                self.placed += 1;
            }
        }
    }

    /// Whether the player's box overlaps a cell.
    fn body_overlaps(&self, cell: Cell) -> bool {
        renew_physics3d::collide(
            self.body(),
            Transform::at(self.position),
            Shape::Box {
                half_extents: block_half_extent(),
            },
            Transform::at(cell.centre()),
        )
        .is_some()
    }

    /// Where the player is looking from.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        // A little below the top of the head, which is where eyes are and,
        // more usefully, means a player standing under a one-block ceiling can
        // still look at it.
        self.position
            + Vec3::new(
                Fixed::ZERO,
                Fixed::from_bits(self.tuning.half_height.to_bits() / 2),
                Fixed::ZERO,
            )
    }

    /// Sweep the player through the world, sliding along what stops them.
    fn move_and_slide(&mut self) {
        let mut remaining = self.velocity;
        let mut grounded = false;

        for _ in 0..self.tuning.slide_iterations {
            if remaining.length_squared().to_bits() <= 1 {
                break;
            }
            let Some((time, normal)) = self.first_hit(remaining) else {
                // Nothing in the way: spend what is left and finish.
                self.position = self.position + remaining;
                break;
            };
            if is_ground(normal) {
                grounded = true;
            }
            self.position = self.position + remaining * time;
            let unspent = remaining * (Fixed::ONE - time);
            remaining = unspent.slide_along(normal);
        }

        // Landing kills downward speed and a ceiling kills upward, or gravity
        // accumulates while standing still and the player launches on the next
        // step off.
        if grounded && self.velocity.y < Fixed::ZERO {
            self.velocity = Vec3::new(self.velocity.x, Fixed::ZERO, self.velocity.z);
        }
        self.grounded = grounded;
    }

    /// The earliest block the player's box meets along a displacement.
    ///
    /// Candidates come from the grid rather than a broadphase: the cells a
    /// swept box could touch are exactly the ones its bounding box covers, and
    /// for a uniform grid that is a triple loop rather than a search.
    fn first_hit(&self, displacement: Vec3) -> Option<(Fixed, Vec3)> {
        let half = Vec3::new(
            self.tuning.half_width,
            self.tuning.half_height,
            self.tuning.half_width,
        );
        let start = self.position;
        let end = self.position + displacement;
        let low = Cell::containing(Vec3::new(
            start.x.min(end.x) - half.x,
            start.y.min(end.y) - half.y,
            start.z.min(end.z) - half.z,
        ));
        let high = Cell::containing(Vec3::new(
            start.x.max(end.x) + half.x,
            start.y.max(end.y) + half.y,
            start.z.max(end.z) + half.z,
        ));

        let mut best: Option<(Fixed, Vec3, Cell)> = None;
        for x in low.x..=high.x {
            for y in low.y..=high.y {
                for z in low.z..=high.z {
                    let cell = Cell::new(x, y, z);
                    if !self.grid.is_solid(cell) {
                        continue;
                    }
                    let Some(hit) = sweep(
                        self.body(),
                        Transform::at(start),
                        displacement,
                        Shape::Box {
                            half_extents: block_half_extent(),
                        },
                        Transform::at(cell.centre()),
                        self.tuning.skin,
                    ) else {
                        continue;
                    };
                    // Earliest wins; a tie goes to the lower cell, so a player
                    // meeting a seam between two blocks resolves the same way
                    // on every machine.
                    let earlier = best.as_ref().is_none_or(|(time, _, at)| {
                        hit.time < *time || (hit.time == *time && cell < *at)
                    });
                    if earlier {
                        best = Some((hit.time, hit.normal, cell));
                    }
                }
            }
        }
        best.map(|(time, normal, _)| (time, normal))
    }

    /// Where the player is.
    #[must_use]
    pub const fn position(&self) -> Vec3 {
        self.position
    }

    /// How fast they are moving.
    #[must_use]
    pub const fn velocity(&self) -> Vec3 {
        self.velocity
    }

    /// Whether they are standing on something.
    #[must_use]
    pub const fn grounded(&self) -> bool {
        self.grounded
    }

    /// The terrain.
    #[must_use]
    pub const fn grid(&self) -> &Grid {
        &self.grid
    }

    /// Which block the player is looking at, if any is in reach.
    #[must_use]
    pub fn looking_at(&self) -> Option<Pick> {
        pick(&self.grid, self.eye(), self.look, self.tuning.reach)
    }

    /// How many ticks have run.
    #[must_use]
    pub const fn tick(&self) -> u64 {
        self.tick
    }

    /// How many blocks have been broken, and how many placed.
    #[must_use]
    pub const fn edits(&self) -> (u32, u32) {
        (self.broken, self.placed)
    }

    /// A hash over every value that can change future behaviour.
    ///
    /// **The terrain is in here**, walked in the grid's stated order, because
    /// a world whose blocks differ plays differently from the next tick on.
    /// So are the three input latches: two players standing in the same place
    /// diverge immediately if one is holding a button and the other is not.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hash = StateHash::new().absorb_u64(self.tick);
        for (cell, block) in self.grid.solids() {
            hash = hash
                .absorb_u32(cell.x.cast_unsigned())
                .absorb_u32(cell.y.cast_unsigned())
                .absorb_u32(cell.z.cast_unsigned())
                .absorb_u32(u32::from(block));
        }
        for value in [
            self.position.x,
            self.position.y,
            self.position.z,
            self.velocity.x,
            self.velocity.y,
            self.velocity.z,
            self.look.x,
            self.look.y,
            self.look.z,
        ] {
            hash = hash.absorb_u64(value.to_bits().cast_unsigned());
        }
        hash.absorb_u32(u32::from(self.grounded))
            .absorb_u32(u32::from(self.jump_was_held))
            .absorb_u32(u32::from(self.dig_was_held))
            .absorb_u32(u32::from(self.place_was_held))
            .absorb_u32(self.broken)
            .absorb_u32(self.placed)
            .finish()
    }
}
