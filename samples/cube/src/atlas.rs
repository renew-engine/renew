//! The block textures, generated rather than loaded.
//!
//! **Generated because a file would be a dependency with a licence.** An
//! image in the tree is third-party content whether or not anyone thinks
//! of it that way: it needs provenance, a licence entry and a reason it
//! cannot be built instead. A function needs a test.
//!
//! # What a texture buys that shading cannot
//!
//! Faces are already shaded by which way they point, and corners are
//! already darkened by how enclosed they are. Neither says anything about
//! a **flat** surface: forty floor blocks in a row have the same
//! orientation and no enclosed corners between them, so they draw as one
//! unbroken plane. A tile with a darker edge is what makes the blocks
//! countable — which is the whole of the complaint that everything is one
//! grey.
//!
//! # Flat colours and an edge, measured against the alternative
//!
//! The first version speckled each texel by a hash of its own
//! coordinates, on the reasoning that a flat tile reads as synthetic.
//! Rendered and compared, it was worse on both counts that matter:
//!
//! * **the picture** — at the size a block is drawn a 16-texel tile is
//!   minified, so the speckle became high-frequency noise competing with
//!   the very edges it was meant to complement. The grid is easier to
//!   read without it.
//! * **the repository** — the room render went from 208 kB to 592 kB.
//!   Noise is what a deflate stream cannot compress, and this sample's
//!   pictures are committed.
//!
//! So the tiles are flat colours with a bevelled edge, and the two are
//! told apart by how wide that edge is rather than by their grain. There
//! is no randomness here at all now, which also makes "the same atlas on
//! every machine" true by construction rather than by careful hashing.

use renew_sample_cube_world::ray::Face;

/// One tile's edge, in texels.
///
/// Sixteen is small enough that the whole atlas is under two kilobytes,
/// and large enough for a joint and a bevel inside it to survive the
/// minification a block gets at the sizes it is drawn.
const TILE: u32 = 16;

/// Which tile a face should sample.
///
/// **Two, not six.** Orientation is already carried by face shading; what
/// a tile adds is the *edge*, and an edge looks the same whichever way a
/// face points. The top is separate because a floor is the surface a
/// player looks at most, and a wider joint there reads as flagstones
/// rather than as the same wall laid down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    /// Sides and undersides.
    Stone,
    /// Upward-facing surfaces.
    StoneTop,
}

impl Tile {
    /// Which column of the atlas this tile occupies.
    const fn column(self) -> u32 {
        match self {
            Self::Stone => 0,
            Self::StoneTop => 1,
        }
    }
}

/// How many tiles the atlas holds, laid out in one row.
const COUNT: u32 = 2;

/// The atlas width in texels.
pub const WIDTH: u32 = TILE * COUNT;

/// The atlas height in texels.
pub const HEIGHT: u32 = TILE;

/// The tile a face of that orientation samples.
///
/// Upward faces get the coarser tile; everything else gets the plain
/// one. The choice is orientation only — what a block is made of would
/// choose the *set* of tiles, and this world has one material.
#[must_use]
pub fn tile_for(face: Face) -> Tile {
    match face {
        Face::Top => Tile::StoneTop,
        _ => Tile::Stone,
    }
}

/// The atlas, RGBA8, row-major from the top-left.
///
/// One row of tiles, so a tile's column is its only coordinate and the
/// mapping below is a division rather than a lookup table.
#[must_use]
pub fn pixels() -> Vec<u8> {
    let mut bytes = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let column = x / TILE;
            let within = (x % TILE, y);
            let texel = shade(column, within);
            bytes.extend_from_slice(&[texel, texel, texel, 255]);
        }
    }
    bytes
}

/// The four texture coordinates of `tile`, in the corner order
/// `Quad::corners` emits: bottom-left, bottom-right, top-right, top-left
/// of the tile.
///
/// **A half-texel inset on every side.** Sampling exactly at a tile's
/// boundary lets a filter reach into the neighbour, which shows as a
/// one-pixel fringe of the wrong tile along every block edge — the
/// classic atlas artefact, and one that only appears at some distances,
/// so a still can look perfect while the game does not.
#[must_use]
pub fn tile_uv(tile: Tile) -> [[f32; 2]; 4] {
    let column = tile.column();
    // Half a texel, in the atlas's own coordinates.
    let inset = 0.5 / f32::from(u16::try_from(WIDTH).unwrap_or(u16::MAX));
    let span = 1.0 / f32::from(u16::try_from(COUNT).unwrap_or(u16::MAX));
    let left = f32::from(u16::try_from(column).unwrap_or(0)) * span + inset;
    let right = left + span - 2.0 * inset;
    let top = inset;
    let bottom = 1.0 - inset;
    [[left, bottom], [right, bottom], [right, top], [left, top]]
}

/// How bright one texel of a tile is.
///
/// Three levels: the interior, a bevel, and the joint at the edge. The
/// bevel exists so the joint reads as a recess rather than as a line
/// drawn on a flat surface.
fn shade(column: u32, (x, y): (u32, u32)) -> u8 {
    /// The interior of a tile.
    const BASE: i32 = 168;
    /// How much darker the joint is, so blocks are countable.
    const JOINT: i32 = 34;
    /// How much darker the ring inside it is.
    const BEVEL: i32 = 12;

    let last = TILE - 1;
    let edge = x == 0 || y == 0 || x == last || y == last;
    let inner = x == 1 || y == 1 || x == last - 1 || y == last - 1;
    // The top tile's joint is two texels wide rather than one, which is
    // what tells the two apart at a glance and what makes a floor read as
    // flagstones.
    let wide_joint = column == 1;

    let level = if edge || (inner && wide_joint) {
        BASE - JOINT
    } else if inner {
        BASE - BEVEL
    } else {
        BASE
    };
    u8::try_from(level.clamp(0, 255)).unwrap_or(0)
}

/// The particle tile: one soft square, premultiplied, generated the
/// same way the block tiles are and for the same reason — a file would
/// be a dependency with a licence, and a function has a test.
///
/// White at full alpha in the middle, fading premultiplied toward the
/// edge, so an instance colour multiplies through cleanly under either
/// blend mode. Eight texels square: particles draw small, and a soft
/// edge at that size needs gradient, not resolution.
#[must_use]
pub fn particle_pixels() -> (u32, Vec<u8>) {
    const SIDE: u32 = 8;
    // Distances squared in doubled-texel units, so the centre sits on
    // an integer and every value below is a small exact integer. Signed
    // constants written as the literals they are, because 8 fits in an
    // i32 without a cast to argue about.
    const SIDE_I: i32 = 8;
    const CENTRE: i32 = SIDE_I - 1; // doubled: texel centre x2 - 1
    const FULL: i32 = (SIDE_I - 2) * (SIDE_I - 2); // full ink inside
    const EDGE: i32 = 2 * (SIDE_I - 1) * (SIDE_I - 1); // zero by here
    let mut pixels = Vec::with_capacity((SIDE * SIDE * 4) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let dx = 2 * i32::try_from(x).unwrap_or(0) - CENTRE;
            let dy = 2 * i32::try_from(y).unwrap_or(0) - CENTRE;
            let distance_squared = dx * dx + dy * dy;
            let ink_256 = if distance_squared <= FULL {
                255
            } else {
                (255 * (EDGE - distance_squared).max(0)) / (EDGE - FULL)
            };
            let ink = u8::try_from(ink_256.clamp(0, 255)).unwrap_or(255);
            // Premultiplied: colour channels carry the alpha already.
            pixels.extend_from_slice(&[ink, ink, ink, ink]);
        }
    }
    (SIDE, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upward faces sample the coarser tile; everything else the plain
    /// one. Orientation is the only thing that chooses, because this
    /// world has one material.
    #[test]
    fn only_upward_faces_take_the_top_tile() {
        assert_eq!(tile_for(Face::Top), Tile::StoneTop);
        for face in [
            Face::Bottom,
            Face::North,
            Face::South,
            Face::East,
            Face::West,
        ] {
            assert_eq!(tile_for(face), Tile::Stone, "{face:?}");
        }
    }

    /// The atlas is the size it says it is, and opaque throughout.
    #[test]
    fn the_atlas_is_the_declared_size_and_fully_opaque() {
        let pixels = pixels();
        assert_eq!(pixels.len(), (WIDTH * HEIGHT * 4) as usize);
        assert!(
            pixels.chunks_exact(4).all(|texel| texel[3] == 255),
            "a transparent texel would blend a block with whatever is behind it"
        );
        assert!(
            pixels
                .chunks_exact(4)
                .all(|texel| texel[0] == texel[1] && texel[1] == texel[2]),
            "stone is grey; a channel that drifted would tint the world"
        );
    }

    /// **The edge is darker than the middle**, which is the whole reason
    /// the atlas exists: without it a flat wall of many blocks is one
    /// unbroken plane.
    #[test]
    fn every_tile_has_an_edge_darker_than_its_middle() {
        let pixels = pixels();
        let at = |x: u32, y: u32| pixels[((y * WIDTH + x) * 4) as usize];
        for tile in [Tile::Stone, Tile::StoneTop] {
            let left = tile.column() * TILE;
            let edge = at(left, 0);
            let middle = at(left + TILE / 2, TILE / 2);
            assert!(
                middle > edge,
                "{tile:?}: middle {middle} is not brighter than edge {edge}"
            );
        }
    }

    /// The same atlas every time, on every machine. True by
    /// construction now — there is nothing in here but constants — and
    /// asserted anyway, because that is a property the committed renders
    /// depend on and a future variation could take away silently.
    #[test]
    fn the_atlas_is_reproducible() {
        assert_eq!(pixels(), pixels());
    }

    /// The two tiles differ. Otherwise the atlas is one tile with a
    /// second name, and the mapping below has nothing to choose between.
    #[test]
    fn the_two_tiles_are_not_the_same_picture() {
        let pixels = pixels();
        let column_of = |tile: Tile| {
            let left = tile.column() * TILE;
            (0..TILE)
                .flat_map(|y| (0..TILE).map(move |x| (x, y)))
                .map(|(x, y)| pixels[(((y) * WIDTH + left + x) * 4) as usize])
                .collect::<Vec<u8>>()
        };
        assert_ne!(column_of(Tile::Stone), column_of(Tile::StoneTop));
    }

    /// **Coordinates stay inside their own tile**, inset by half a texel
    /// so a filter cannot reach the neighbour. A fringe of the wrong tile
    /// along every block edge is the classic atlas artefact, and it shows
    /// at some distances and not others.
    #[test]
    fn a_tiles_coordinates_stay_inside_it() {
        let span = 1.0 / f32::from(u16::try_from(COUNT).unwrap_or(1));
        for tile in [Tile::Stone, Tile::StoneTop] {
            let uv = tile_uv(tile);
            let left = f32::from(u16::try_from(tile.column()).unwrap_or(0)) * span;
            let right = left + span;
            for [u, v] in uv {
                assert!(
                    u > left && u < right,
                    "{tile:?}: u {u} is outside [{left}, {right}]"
                );
                assert!(
                    (0.0..=1.0).contains(&v),
                    "{tile:?}: v {v} is outside the atlas"
                );
            }
        }
    }

    /// The mapping is a rectangle in the order the corners come: the
    /// first two share a v, the last two share the other, and the first
    /// and last share a u.
    #[test]
    fn the_mapping_is_a_rectangle_in_corner_order() {
        let [bottom_left, bottom_right, top_right, top_left] = tile_uv(Tile::Stone);
        assert!((bottom_left[1] - bottom_right[1]).abs() < f32::EPSILON);
        assert!((top_left[1] - top_right[1]).abs() < f32::EPSILON);
        assert!((bottom_left[0] - top_left[0]).abs() < f32::EPSILON);
        assert!((bottom_right[0] - top_right[0]).abs() < f32::EPSILON);
        assert!(
            bottom_left[1] > top_left[1],
            "the corner order runs bottom to top"
        );
    }

    /// The particle tile: premultiplied everywhere, opaque in the
    /// middle, transparent at the corners, symmetric under the flips a
    /// square billboard undergoes — the properties the blend modes and
    /// the instance colour rely on.
    #[test]
    fn the_particle_tile_is_a_premultiplied_soft_square() {
        let (side, pixels) = particle_pixels();
        assert_eq!(pixels.len(), (side * side * 4) as usize);
        for texel in pixels.chunks_exact(4) {
            assert!(
                texel.iter().all(|&channel| channel == texel[0]),
                "premultiplied white: every channel carries the same ink: {texel:?}"
            );
        }
        let at = |x: u32, y: u32| pixels[((y * side + x) * 4) as usize];
        assert_eq!(at(3, 3), 255, "the middle is full ink");
        assert_eq!(
            at(4, 4),
            255,
            "the centre has no single texel, so its four share"
        );
        assert_eq!(
            at(0, 0),
            0,
            "the corners are empty, or the quad has corners"
        );
        assert_eq!(at(7, 7), 0, "and every corner, not just one");
        for y in 0..side {
            for x in 0..side {
                assert_eq!(at(x, y), at(side - 1 - x, y), "mirror in x at ({x}, {y})");
                assert_eq!(at(x, y), at(x, side - 1 - y), "mirror in y at ({x}, {y})");
            }
        }
    }
}
