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

use renew_sample_cube_world::grid::{AIR, BRICK, Block, Cell, Grid, STONE};
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
        let corner = |(su, sv): (i8, i8)| {
            let along = |axis: usize| {
                f32::from(normal[axis])
                    + f32::from(su) * f32::from(u[axis])
                    + f32::from(sv) * f32::from(v[axis])
            };
            [
                centre[0] + along(0) * HALF,
                centre[1] + along(1) * HALF,
                centre[2] + along(2) * HALF,
            ]
        };
        CORNER_SIGNS.map(corner)
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
pub(crate) fn world_units(coordinate: i32) -> f32 {
    coordinate as f32
}

/// A face's outward normal and its two in-plane axes, chosen so that
/// `u × v == normal` — which is what makes the corner order above
/// counter-clockwise from outside without a per-face special case.
const fn basis(face: Face) -> ([i8; 3], [i8; 3], [i8; 3]) {
    match face {
        Face::East => ([1, 0, 0], [0, 0, -1], [0, 1, 0]),
        Face::West => ([-1, 0, 0], [0, 0, 1], [0, 1, 0]),
        Face::Top => ([0, 1, 0], [1, 0, 0], [0, 0, -1]),
        Face::Bottom => ([0, -1, 0], [1, 0, 0], [0, 0, 1]),
        Face::North => ([0, 0, 1], [1, 0, 0], [0, 1, 0]),
        Face::South => ([0, 0, -1], [-1, 0, 0], [0, 1, 0]),
    }
}

/// The four corner offsets, as signs on the two in-plane axes, in the
/// same order [`Quad::corners`] emits them.
const CORNER_SIGNS: [(i8, i8); 4] = [(-1, -1), (1, -1), (1, 1), (-1, 1)];

/// How much light each corner of `quad` is denied by the blocks touching
/// it, as a multiplier in the same order [`Quad::corners`] emits them.
///
/// **The cue a flat-coloured world has none of.** Two faces of one colour
/// meeting at an inner corner are indistinguishable from a single flat
/// face: nothing in the picture says the geometry turns. Face-direction
/// shading does not help, because it varies between faces of *different*
/// orientation and an inner corner is where two same-facing walls meet a
/// third. Darkening a corner by how enclosed it is puts the information
/// back, and it is a property of the geometry rather than of the surface —
/// so it survives whatever textures arrive later rather than being
/// replaced by them.
///
/// For each corner, three cells can enclose it: the two that share an
/// edge with it and the one diagonally across. The two edge-sharers
/// meeting is the darkest case regardless of the diagonal, because they
/// close the corner off between them and whether anything sits behind
/// that is not visible from here.
#[must_use]
pub fn corner_shades(grid: &Grid, quad: Quad) -> [f32; 4] {
    let (normal, u, v) = basis(quad.face);
    let occupied = |offset: [i8; 3]| {
        let at = Cell::new(
            quad.cell.x + i32::from(offset[0]),
            quad.cell.y + i32::from(offset[1]),
            quad.cell.z + i32::from(offset[2]),
        );
        // Outside the grid is open, for the same reason the mesher treats
        // it as open: a player who walks out of the world falls, so the
        // void is not something that can shade anything.
        grid.get(at).is_some_and(|block| block != AIR)
    };
    let add = |a: [i8; 3], b: [i8; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    let scale = |a: [i8; 3], k: i8| [a[0] * k, a[1] * k, a[2] * k];

    CORNER_SIGNS.map(|(su, sv)| {
        let out = |axes: [i8; 3]| add(normal, axes);
        let side_u = occupied(out(scale(u, su)));
        let side_v = occupied(out(scale(v, sv)));
        let diagonal = occupied(out(add(scale(u, su), scale(v, sv))));
        let level = if side_u && side_v {
            3
        } else {
            u8::from(side_u) + u8::from(side_v) + u8::from(diagonal)
        };
        1.0 - f32::from(level) * OCCLUSION_STEP
    })
}

/// How much one enclosing neighbour costs a corner.
///
/// Three of them take a corner to a little over half brightness. Chosen
/// to be plainly visible without reading as a painted-on shadow: the
/// point is to say "the geometry turns here", not to imitate a light.
const OCCLUSION_STEP: f32 = 0.16;

/// The colour a renderer should draw this face in.
///
/// **Shaded by direction, because a flat-coloured cube is a silhouette.**
/// Nothing lights the scene — there is no light, no normal in the vertex
/// format, and no shading in the built-in shader — so a world drawn in
/// one colour per block type reads as a single blob with an outline.
/// Varying the colour by which way a face points is the cheapest thing
/// that makes edges visible, and it costs nothing at runtime because the
/// colour is baked into the vertex.
///
/// This is the *face* half of the shading. It cannot say anything about
/// an inner corner, where two faces pointing the same way meet a third:
/// they get the same colour and the corner disappears. That is what
/// [`corner_shades`] is for, and the two multiply.
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
        // Warmer and a little darker, so a placed block reads as placed
        // against stone from any distance and in any light this world
        // has — which is none, so the colour is the whole of the signal.
        BRICK => [0.55, 0.40, 0.34, 1.0],
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

#[cfg(test)]
mod tests {
    use super::*;

    /// **A placed block is not the colour of the stone it sits against.**
    /// The whole point of a second kind is that a player can see what
    /// they built, and nothing but this comparison says so — the tile is
    /// shared, so the hue carries all of the signal.
    #[test]
    fn a_placed_block_is_a_different_colour_from_the_world() {
        for face in [Face::Top, Face::North, Face::Bottom] {
            let stone = colour(STONE, face);
            let brick = colour(BRICK, face);
            assert!(
                stone
                    .iter()
                    .zip(brick)
                    .any(|(a, b)| (a - b).abs() > f32::EPSILON),
                "{face:?}: brick reads as stone"
            );
            // Not merely different: different in hue, so it survives the
            // face shading that darkens both by the same factor.
            let stone_warmth = stone[0] - stone[2];
            let brick_warmth = brick[0] - brick[2];
            assert!(
                brick_warmth > stone_warmth + 0.05,
                "{face:?}: brick should be warmer than stone, got {brick_warmth} against                  {stone_warmth}"
            );
        }
    }

    /// Neither is the magenta that means "nobody gave this a colour".
    #[test]
    fn both_kinds_have_a_colour_of_their_own() {
        let unknown = base_colour(200);
        for block in [STONE, BRICK] {
            assert!(
                base_colour(block)
                    .iter()
                    .zip(unknown)
                    .any(|(a, b)| (a - b).abs() > f32::EPSILON),
                "block {block} falls through to the unknown-block colour"
            );
        }
    }

    /// A block alone in the air is denied nothing: every corner of every
    /// face is at full brightness.
    #[test]
    fn a_lone_block_has_no_darkened_corner() {
        let mut grid = Grid::new(Cell::new(-2, -2, -2), (5, 5, 5));
        grid.set(Cell::new(0, 0, 0), STONE);

        for quad in faces(&grid) {
            for (corner, shade) in corner_shades(&grid, quad).into_iter().enumerate() {
                assert!(
                    (shade - 1.0).abs() < f32::EPSILON,
                    "{:?} corner {corner} is shaded {shade} with nothing around it",
                    quad.face
                );
            }
        }
    }

    /// **Two neighbours meeting close a corner off**, and that is the
    /// darkest case whatever sits diagonally behind them — which is why
    /// the two-sides test comes before the count.
    #[test]
    fn a_corner_between_two_neighbours_is_the_darkest_case() {
        // An L above the block under test: one neighbour along +x and one
        // along -z, so exactly one corner of its top face is enclosed on
        // both sides.
        let mut grid = Grid::new(Cell::new(-2, -2, -2), (5, 5, 5));
        for cell in [Cell::new(0, 0, 0), Cell::new(1, 1, 0), Cell::new(0, 1, -1)] {
            grid.set(cell, STONE);
        }

        let top = faces(&grid)
            .into_iter()
            .find(|quad| quad.cell == Cell::new(0, 0, 0) && quad.face == Face::Top)
            .expect("the block's top face should be drawn");
        let shades = corner_shades(&grid, top);

        // Top's axes are u = +x and v = -z, so the corner at (+1, +1) is
        // the one touching both neighbours.
        let enclosed = shades[2];
        assert!(
            (enclosed - (1.0 - 3.0 * OCCLUSION_STEP)).abs() < 1e-6,
            "the enclosed corner should be at the darkest level, got {enclosed} in {shades:?}"
        );
        assert!(
            shades[0] > enclosed,
            "the opposite corner touches nothing and must be brighter: {shades:?}"
        );
    }

    /// The void outside the grid shades nothing, for the same reason the
    /// mesher does not draw faces against it: it is not there.
    #[test]
    fn the_void_outside_the_grid_shades_nothing() {
        // One block filling a one-cell grid: every neighbour of every
        // corner is outside it.
        let mut grid = Grid::new(Cell::new(0, 0, 0), (1, 1, 1));
        grid.set(Cell::new(0, 0, 0), STONE);

        // No face is drawn at all here — every neighbour is outside
        // rather than air — so the shading is asked for directly.
        for face in [
            Face::East,
            Face::West,
            Face::Top,
            Face::Bottom,
            Face::North,
            Face::South,
        ] {
            let quad = Quad {
                cell: Cell::new(0, 0, 0),
                face,
                block: STONE,
            };
            let shades = corner_shades(&grid, quad);
            assert!(
                shades
                    .iter()
                    .all(|shade| (shade - 1.0).abs() < f32::EPSILON),
                "{face:?} is shaded by the void: {shades:?}"
            );
        }
    }

    /// Shading is a pure function of the grid: the same world answers the
    /// same way, which is what lets a picture of it be committed.
    #[test]
    fn shading_is_reproducible() {
        let grid = crate::arena();
        let once: Vec<[f32; 4]> = faces(&grid)
            .into_iter()
            .map(|quad| corner_shades(&grid, quad))
            .collect();
        let again: Vec<[f32; 4]> = faces(&grid)
            .into_iter()
            .map(|quad| corner_shades(&grid, quad))
            .collect();
        assert_eq!(once, again);
    }

    /// The arena really does have enclosed corners — a guard against a
    /// formula that returns full brightness everywhere and passes every
    /// test above without doing anything.
    #[test]
    fn the_arena_has_darkened_corners() {
        let grid = crate::arena();
        let darkened = faces(&grid)
            .into_iter()
            .flat_map(|quad| corner_shades(&grid, quad))
            .filter(|shade| *shade < 1.0)
            .count();
        assert!(
            darkened > 100,
            "only {darkened} darkened corners in a closed box with a mound in it"
        );
    }
}
