//! The UI atlas, generated: a file would be a dependency with a
//! licence, and a function has a test.
//!
//! Two eight-texel tiles side by side. The **fill tile** is solid
//! white — every background is this tile under a premultiplied tint,
//! and the sampled region sits two texels inside it so linear
//! filtering can never blend an edge in. The **chrome tile** is a
//! white one-texel border over transparency, reserved for the panel
//! and button borders that arrive with the compiled style tables; it
//! ships now so the atlas layout is settled before anything samples
//! it.

use renew_render2d::Region;

use crate::glyphs;

/// The tile row's height: the fill and chrome tiles live above the
/// glyph strip.
const TILE_ROW: u32 = 8;

/// Atlas width in texels: the glyph strip is far wider than the two
/// tiles, so it sets the width.
pub const WIDTH: u32 = glyphs::STRIP_WIDTH;
/// Atlas height in texels: the tile row, then the glyph strip.
pub const HEIGHT: u32 = TILE_ROW + glyphs::LINE_HEIGHT;

/// The atlas bytes, **authored** RGBA with straight alpha, row-major: the
/// tile row on top, the baked glyph strip below it — glyph alpha becomes
/// white ink at that coverage, so a text tint multiplies through the same
/// way a background tint does.
///
/// **Straight alpha rather than premultiplied**, since the sprite stage
/// premultiplies after the hardware decodes. Nothing about the result
/// moved: white and full alpha are both fixed points of the transfer
/// curve, so `(255, 255, 255, a)` decoded and then multiplied by `a` is
/// exactly the `(a, a, a, a)` this used to emit.
#[must_use]
pub fn pixels() -> Vec<u8> {
    let mut bytes = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..TILE_ROW {
        for x in 0..WIDTH {
            let texel: [u8; 4] = if x < 8 {
                // The fill tile: white everywhere.
                [255, 255, 255, 255]
            } else if x < 16 {
                // The chrome tile: a one-texel white border over
                // nothing.
                let inner_x = x - 8;
                let border = inner_x == 0 || inner_x == 7 || y == 0 || y == TILE_ROW - 1;
                if border {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 0]
                }
            } else {
                // The rest of the tile row is padding: nothing samples
                // it, and nothing ever should.
                [0, 0, 0, 0]
            };
            bytes.extend_from_slice(&texel);
        }
    }
    for y in 0..glyphs::LINE_HEIGHT {
        for x in 0..WIDTH {
            let ink = glyphs::STRIP_ALPHA[(y * glyphs::STRIP_WIDTH + x) as usize];
            // White at that coverage, straight: the sprite stage does the
            // multiply. Writing `[ink; 4]` here would premultiply twice.
            bytes.extend_from_slice(&[255, 255, 255, ink]);
        }
    }
    bytes
}

/// A character's baked glyph and its atlas region. Characters outside
/// the baked range answer as the question mark, which is also how the
/// advance table measured them — width and picture agree.
#[must_use]
pub fn glyph_of(character: char) -> (glyphs::Glyph, Region) {
    let code = character as u32;
    let index = if (glyphs::GLYPH_FIRST..=glyphs::GLYPH_LAST).contains(&code) {
        code - glyphs::GLYPH_FIRST
    } else {
        u32::from(b'?') - glyphs::GLYPH_FIRST
    };
    let glyph = glyphs::GLYPHS[index as usize];
    let region = Region {
        x: glyph.x,
        y: TILE_ROW,
        width: glyph.width,
        height: glyphs::LINE_HEIGHT,
    };
    (glyph, region)
}

/// The region every background samples: the middle of the fill tile,
/// two texels in from every edge, so filtering never reaches a
/// neighbour.
#[must_use]
pub fn white() -> Region {
    Region {
        x: 2,
        y: 2,
        width: 4,
        height: 4,
    }
}

/// The chrome tile, whole: reserved for borders; nothing samples it
/// in v0, and the region exists so the layout is a published fact
/// rather than a magic number in a later change.
#[must_use]
pub fn chrome() -> Region {
    Region {
        x: 8,
        y: 0,
        width: 8,
        height: 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two generated halves agree glyph by glyph: the advance the
    /// simulation measures with is the advance the picture draws with,
    /// for every character, or a partial re-bake has torn them apart.
    #[test]
    fn the_advance_table_and_the_glyphs_are_one_bake() {
        assert_eq!(glyphs::GLYPH_FIRST, renew_ui::text::TEXT_FIRST);
        assert_eq!(glyphs::GLYPH_LAST, renew_ui::text::TEXT_LAST);
        assert_eq!(glyphs::LINE_HEIGHT, renew_ui::text::LINE_HEIGHT);
        assert_eq!(glyphs::GLYPHS.len(), renew_ui::text::ADVANCES.len());
        for (index, glyph) in glyphs::GLYPHS.iter().enumerate() {
            assert_eq!(
                glyph.advance,
                u32::from(renew_ui::text::ADVANCES[index]),
                "glyph {index} measures one width and draws another"
            );
        }
    }

    /// The composed atlas reproduces to the bit: one hash pins the
    /// tiles and the baked strip together, so an accidental edit to
    /// either — or a silent re-bake — reddens here before it reaches
    /// a picture.
    #[test]
    fn the_atlas_reproduces_to_the_committed_hash() {
        let bytes = pixels();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in &bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(
            // Re-pinned when the glyph strip became straight alpha: ink
            // is now white at the baked coverage rather than white
            // premultiplied by it, because the sprite stage does the
            // multiply. The BYTES moved; the picture did not, since white
            // and full alpha are fixed points of the transfer curve.
            hash,
            0x0169_61ff_3e57_0b0a,
            "the atlas bytes moved; if this was a deliberate re-bake, re-pin \
             with the value the failure names"
        );
    }

    /// Every character answers a glyph whose region sits inside the
    /// strip, below the tile row, and the fallback is the question
    /// mark it was measured as.
    #[test]
    fn glyph_regions_stay_inside_the_strip() {
        for code in glyphs::GLYPH_FIRST..=glyphs::GLYPH_LAST {
            let character = char::from_u32(code).expect("printable ascii");
            let (glyph, region) = glyph_of(character);
            assert_eq!(region.x, glyph.x);
            assert!(region.x + region.width <= WIDTH);
            assert_eq!(region.y, TILE_ROW);
            assert_eq!(region.height, glyphs::LINE_HEIGHT);
        }
        let (fallback, _) = glyph_of('\u{00e9}');
        let (question, _) = glyph_of('?');
        assert_eq!(fallback.x, question.x, "outside the range is the stand-in");
    }

    /// The atlas is the declared size, the fill tile is uniformly
    /// white, the sampled region sits strictly inside it, and the
    /// chrome tile is a border over transparency.
    #[test]
    fn the_atlas_is_what_the_regions_promise() {
        let bytes = pixels();
        assert_eq!(bytes.len(), (WIDTH * HEIGHT * 4) as usize);
        let texel = |x: u32, y: u32| {
            let at = ((y * WIDTH + x) * 4) as usize;
            [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
        };
        for y in 0..TILE_ROW {
            for x in 0..8 {
                assert_eq!(
                    texel(x, y),
                    [255; 4],
                    "the fill tile is white at ({x}, {y})"
                );
            }
        }
        let white = white();
        assert!(white.x >= 1 && white.x + white.width <= 7);
        assert!(white.y >= 1 && white.y + white.height <= 7);
        let chrome = chrome();
        assert_eq!(texel(chrome.x, chrome.y), [255; 4], "the border is ink");
        assert_eq!(
            texel(chrome.x + 3, chrome.y + 3),
            [0; 4],
            "the middle is nothing"
        );
    }
}
