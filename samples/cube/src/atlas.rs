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
//! # Deterministic, because the pictures are committed
//!
//! The speckle comes from an integer hash of the texel's own coordinates,
//! so the atlas is the same bytes on every machine and every run. No
//! clock, no randomness, nothing to seed. The renders in this sample's
//! README are compared byte-for-byte in review, and an atlas that drifted
//! would make every one of them a false alarm.

/// One tile's edge, in texels.
///
/// Sixteen is small enough that the whole atlas is a few kilobytes and
/// large enough for a border and some variation to read at the sizes a
/// block is drawn.
pub const TILE: u32 = 16;

/// Which tile a face should sample.
///
/// **Two, not six.** Orientation is already carried by face shading; what
/// a tile adds is the *edge*, and an edge looks the same whichever way a
/// face points. The top is separate because a floor is the surface a
/// player looks at most and reads better slightly coarser than a wall.
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
pub const COUNT: u32 = 2;

/// The atlas width in texels.
pub const WIDTH: u32 = TILE * COUNT;

/// The atlas height in texels.
pub const HEIGHT: u32 = TILE;

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
fn shade(column: u32, (x, y): (u32, u32)) -> u8 {
    /// The middle of a tile.
    const BASE: i32 = 168;
    /// How much darker the outermost ring is, so blocks are countable.
    const EDGE: i32 = 34;
    /// How much darker the ring inside that is, so the edge is a bevel
    /// rather than a drawn-on line.
    const BEVEL: i32 = 12;

    let last = TILE - 1;
    let border = x == 0 || y == 0 || x == last || y == last;
    let inner = x == 1 || y == 1 || x == last - 1 || y == last - 1;
    // The top tile is coarser: its speckle swings twice as far, which
    // reads as grain from above without changing the colour.
    let swing = if column == 1 { 2 } else { 1 };

    let mut level = BASE + speckle(column, x, y) * swing;
    if border {
        level -= EDGE;
    } else if inner {
        level -= BEVEL;
    }
    u8::try_from(level.clamp(0, 255)).unwrap_or(0)
}

/// A small deterministic variation for one texel, in `-4..=3`.
///
/// An integer hash rather than a random number generator: there is
/// nothing to seed, nothing to thread through, and the same texel is the
/// same shade on every machine — which is what lets the renders that
/// sample it be committed.
fn speckle(column: u32, x: u32, y: u32) -> i32 {
    let mut hash = column.wrapping_mul(0x9E37_79B9);
    hash ^= x.wrapping_mul(0x85EB_CA6B);
    hash = hash.rotate_left(13);
    hash ^= y.wrapping_mul(0xC2B2_AE35);
    hash ^= hash >> 15;
    i32::try_from(hash & 0x7).unwrap_or(0) - 4
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The same atlas every time, on every machine: no clock, no seed.
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
}
