//! The world's picture, as data: which block faces a renderer would draw,
//! with no renderer in sight.
//!
//! Pure on purpose, and in the driver crate rather than the world crate.
//! The world is fixed-point and declares `simulation = true`; face corners
//! are `f32`, because that is what a vertex buffer holds. Putting them
//! next to the simulation would mix float geometry into the one crate
//! whose whole claim is bit-determinism, for a consumer the simulation
//! does not have. The sibling game draws the same line in the same place.
//!
//! # The rule, and the trap it exists to avoid
//!
//! A face is emitted when a solid cell's neighbour is **air inside the
//! grid** — not merely "not solid".
//!
//! `Grid::is_solid` answers `false` for cells outside the grid, and it is
//! right to: a player who walks out of the world should fall, so outside
//! is not something to stand on. But a mesher written against that answer
//! emits a face wherever the neighbour is not solid, which includes every
//! cell of the surrounding void — so it also emits the box's entire outer
//! skin. For this arena that is **5330 extra faces against 4642 real
//! ones**, more than doubling the mesh, and because the pipeline culls
//! nothing those backfaces rasterize rather than being free.
//!
//! `Grid::get` answers with three cases where `is_solid` answers with two,
//! and the third is the one that matters: `None` means outside. Meshing
//! against `get` is what makes the difference structural rather than a
//! condition somebody has to remember.

use renew_sample_cube_world::grid::{AIR, Block, Cell, Grid, STONE};
use renew_sample_cube_world::ray::Face;

/// One block face a renderer would draw.
///
/// The cell and the direction rather than four corners, because that is
/// the smallest thing that names the face — corners are derived, and a
/// mesher that returned them would decide the winding for every consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quad {
    /// The solid cell this face belongs to.
    pub cell: Cell,
    /// Which side of that cell it is.
    pub face: Face,
    /// What the cell is made of, which decides the colour.
    pub block: Block,
}

/// Half a block, in world units. A cell spans one unit and is centred on
/// its integer coordinate, so every face sits exactly half a unit out.
const HALF: f32 = 0.5;

/// Every visible face in `grid`, in a fixed order.
///
/// **The order is part of the contract.** `Grid::solids` walks in
/// ascending cell index and this walks the six faces in enum order within
/// each cell, so the same grid always produces the same sequence — which
/// is what makes the mesh reproducible, and what lets the renderer's
/// submission order mean anything.
#[must_use]
pub fn faces(grid: &Grid) -> Vec<Quad> {
    const SIDES: [Face; 6] = [
        Face::East,
        Face::West,
        Face::Top,
        Face::Bottom,
        Face::North,
        Face::South,
    ];

    let mut quads = Vec::new();
    for (cell, block) in grid.solids() {
        for face in SIDES {
            let (dx, dy, dz) = face.step();
            // `Some(AIR)`, not `!is_solid()`. See the module note: the
            // difference is the outer skin of the world.
            if grid.get(cell.offset(dx, dy, dz)) == Some(AIR) {
                quads.push(Quad { cell, face, block });
            }
        }
    }
    quads
}

impl Quad {
    /// The four corners, in world space, counter-clockwise seen from
    /// outside the block.
    ///
    /// **Wound consistently even though nothing currently reads the
    /// winding.** The pipeline culls no faces in v0, so a reversed quad
    /// draws identically today — which is exactly why getting it right
    /// now is cheap and getting it wrong is invisible until the day
    /// culling is switched on and half the world disappears.
    #[must_use]
    pub fn corners(self) -> [[f32; 3]; 4] {
        let (normal, u, v) = basis(self.face);
        let centre = [
            world_units(self.cell.x),
            world_units(self.cell.y),
            world_units(self.cell.z),
        ];
        let corner = |su: f32, sv: f32| {
            [
                centre[0] + (normal[0] + su * u[0] + sv * v[0]) * HALF,
                centre[1] + (normal[1] + su * u[1] + sv * v[1]) * HALF,
                centre[2] + (normal[2] + su * u[2] + sv * v[2]) * HALF,
            ]
        };
        [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ]
    }
}

/// A cell coordinate as a world-space distance.
///
/// **The precision the lint warns about cannot be reached from here.** An
/// `i32` loses precision as an `f32` beyond 2^24, so a coordinate would
/// have to name a cell sixteen million units from the origin — and a grid
/// that wide would need at least 2^24 cells along that axis, which
/// `Grid::new` allocates one byte apiece for. The allocation fails long
/// before the arithmetic does, so the bound is enforced by the machine
/// rather than by anyone remembering it.
#[expect(
    clippy::cast_precision_loss,
    reason = "a coordinate past 2^24 needs a grid too large to allocate"
)]
fn world_units(coordinate: i32) -> f32 {
    coordinate as f32
}

/// A face's outward normal and its two in-plane axes, chosen so that
/// `u × v == normal` — which is what makes the corner order above
/// counter-clockwise from outside without a per-face special case.
const fn basis(face: Face) -> ([f32; 3], [f32; 3], [f32; 3]) {
    match face {
        Face::East => ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        Face::West => ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        Face::Top => ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        Face::Bottom => ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        Face::North => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        Face::South => ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    }
}

/// The colour a renderer should draw this face in.
///
/// **Shaded by direction, because a flat-coloured cube is a silhouette.**
/// Nothing lights the scene in v0 — there is no light, no normal in the
/// vertex format, and no shading in the built-in shader — so a world drawn
/// in one colour per block type reads as a single blob with an outline.
/// Varying the colour by which way a face points is the cheapest thing
/// that makes edges visible, and it costs nothing at runtime because the
/// colour is baked into the vertex.
///
/// The factors imitate a sky: brightest up, dimmest down, sides between,
/// with the two horizontal axes distinguished so a corner between them
/// reads as an edge.
#[must_use]
pub fn colour(block: Block, face: Face) -> [f32; 4] {
    let base = base_colour(block);
    let shade = shade(face);
    [base[0] * shade, base[1] * shade, base[2] * shade, base[3]]
}

/// The same face, lit as the one being aimed at.
///
/// **Without this the game is played blind.** Every block is the same
/// grey, so a player cannot tell which one `enter` will break until it
/// is gone. Brightening the aimed-at block is the smallest thing that
/// turns digging from a guess into an action.
///
/// Brightened rather than tinted: a hue would compete with block colours
/// once there is more than one kind of block, where brightness composes
/// with whatever the block already is.
#[must_use]
pub fn aimed_colour(block: Block, face: Face) -> [f32; 4] {
    let base = colour(block, face);
    [
        (base[0] + 0.35).min(1.0),
        (base[1] + 0.35).min(1.0),
        (base[2] + 0.30).min(1.0),
        base[3],
    ]
}

/// The unshaded colour of a block type.
fn base_colour(block: Block) -> [f32; 4] {
    match block {
        STONE => [0.62, 0.60, 0.57, 1.0],
        // Every other value, including `AIR`, which should never reach
        // here because only solid cells are meshed. Magenta on purpose:
        // a block type nobody gave a colour is a bug, and the picture
        // should say so rather than quietly picking grey.
        _ => [1.0, 0.0, 1.0, 1.0],
    }
}

/// The brightness factor for a face's direction.
fn shade(face: Face) -> f32 {
    match face {
        Face::Top => 1.0,
        Face::Bottom => 0.55,
        Face::North | Face::South => 0.82,
        Face::East | Face::West => 0.68,
    }
}
