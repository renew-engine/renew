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

/// Atlas width in texels: two tiles of eight.
pub const WIDTH: u32 = 16;
/// Atlas height in texels: one tile row.
pub const HEIGHT: u32 = 8;

/// The atlas bytes, premultiplied RGBA, row-major.
#[must_use]
pub fn pixels() -> Vec<u8> {
    let mut bytes = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let texel: [u8; 4] = if x < 8 {
                // The fill tile: white everywhere.
                [255, 255, 255, 255]
            } else {
                // The chrome tile: a one-texel white border over
                // nothing.
                let inner_x = x - 8;
                let border = inner_x == 0 || inner_x == 7 || y == 0 || y == HEIGHT - 1;
                if border {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 0]
                }
            };
            bytes.extend_from_slice(&texel);
        }
    }
    bytes
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
        for y in 0..HEIGHT {
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
