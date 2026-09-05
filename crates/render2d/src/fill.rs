//! The pure half: sprites in canvas space become instance bytes in NDC.
//!
//! Nothing here touches a device. The ortho map, the UV map, and the
//! packing are plain functions so the unit and property tests below can
//! pin them without bringing up Vulkan — the GPU half consumes exactly
//! these bytes.

use renew_math::Vec2;
use renew_rhi::Extent;

/// One packed instance: five attributes, twelve `f32`s, 48 bytes. The
/// shader's `location(0..=4)` list, the layout slice in `gpu.rs`, and
/// [`pack`] describe the same bytes; change one and the others in the
/// same commit or the draw reads garbage.
pub(crate) const INSTANCE_STRIDE: usize = 48;

/// The drawing surface's logical size in pixels. Sprites are placed in
/// this space — y grows downward from the top-left, matching both how
/// 2D art is authored and the NDC orientation the targets render with —
/// and the ortho map turns it into NDC at push time.
///
/// `new` refuses zero by construction (`Option`, the `NonZeroU32::new`
/// idiom): a zero-sized canvas is not a failed environment or a broken
/// contract mid-frame, it is a value that cannot exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Canvas {
    width: core::num::NonZeroU32,
    height: core::num::NonZeroU32,
}

impl Canvas {
    /// A canvas of `width × height` logical pixels, or `None` if either
    /// is zero.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            width: core::num::NonZeroU32::new(width)?,
            height: core::num::NonZeroU32::new(height)?,
        })
    }

    /// Width in logical pixels.
    #[must_use]
    pub fn width(self) -> u32 {
        self.width.get()
    }

    /// Height in logical pixels.
    #[must_use]
    pub fn height(self) -> u32 {
        self.height.get()
    }
}

/// A rectangle of atlas texels: which part of the atlas a sprite shows.
///
/// Plain data with public fields — every combination is meaningful to
/// construct, and the UV map is total over it. A region reaching past
/// the atlas edge samples clamped edge texels rather than failing;
/// that is visible art, not unsoundness, and the golden tests pin the
/// in-bounds behavior that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// Left edge, in texels from the atlas's left.
    pub x: u32,
    /// Top edge, in texels from the atlas's top.
    pub y: u32,
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
}

/// One sprite: an atlas region, a place on the canvas, a size, a tint.
///
/// `#[non_exhaustive]` with a constructor and builders, the descriptor
/// pattern this tree uses everywhere: the fields a later version adds
/// (rotation, origin) arrive as builders touching no existing caller.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Sprite {
    /// The atlas texels this sprite shows.
    pub region: Region,
    /// Left edge on the canvas, logical pixels.
    pub x: f32,
    /// Top edge on the canvas, logical pixels.
    pub y: f32,
    /// Width on the canvas, logical pixels.
    pub width: f32,
    /// Height on the canvas, logical pixels.
    pub height: f32,
    /// Premultiplied RGBA tint, multiplied into every sampled texel.
    /// Opaque white — the default — is the identity. Like the atlas
    /// bytes themselves, the tint carries its alpha multiplied in;
    /// one convention end to end.
    pub tint: [f32; 4],
}

impl Sprite {
    /// `region` drawn with its top-left corner at (`x`, `y`), at the
    /// region's own size, untinted.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "past 2^24 texels the placement degrades visibly, never unsafely; real atlases sit far below it"
    )]
    pub fn new(region: Region, x: f32, y: f32) -> Self {
        Self {
            region,
            x,
            y,
            width: region.width as f32,
            height: region.height as f32,
            tint: [1.0, 1.0, 1.0, 1.0],
        }
    }

    /// Draw at `width × height` canvas pixels instead of the region's
    /// own size.
    #[must_use]
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Multiply every sampled texel by `tint` (premultiplied RGBA).
    #[must_use]
    pub fn tint(mut self, tint: [f32; 4]) -> Self {
        self.tint = tint;
        self
    }
}

/// Canvas pixels to NDC: `2c/extent - 1` per axis, no flip — canvas y
/// grows downward and so does the targets' NDC y (both viewports are
/// positive-height). The unit tests pin all four canvas corners so a
/// sign mistake cannot survive.
#[allow(
    clippy::cast_precision_loss,
    reason = "past 2^24 pixels the map degrades visibly, never unsafely; real canvases sit far below it"
)]
pub(crate) fn to_ndc(point: Vec2, canvas: Canvas) -> Vec2 {
    Vec2::new(
        2.0 * point.x / canvas.width() as f32 - 1.0,
        2.0 * point.y / canvas.height() as f32 - 1.0,
    )
}

/// Region texels to UV: `texel / atlas_extent` per axis. Region edges
/// land on texel boundaries; nearest sampling then picks interior
/// texels, so no half-texel inset exists to get wrong.
#[allow(
    clippy::cast_precision_loss,
    reason = "past 2^24 texels the map degrades visibly, never unsafely; real atlases sit far below it"
)]
pub(crate) fn to_uv(texel: Vec2, atlas: Extent) -> Vec2 {
    Vec2::new(texel.x / atlas.width as f32, texel.y / atlas.height as f32)
}

/// `alpha` as an opacity: clamped to `0.0..=1.0`.
///
/// # Panics
///
/// On NaN. `f32::clamp` passes NaN straight through, so an unguarded
/// clamp would multiply it into all four channels of every later
/// sprite and draw nothing, frame after frame, with no error anywhere
/// — a silent wrong picture rather than a named refusal. Infinities
/// need no such guard: they clamp.
///
/// Split from the renderer so the refusal can be exercised without a
/// device, which is the only way it gets exercised at all.
pub(crate) fn fade(alpha: f32) -> f32 {
    assert!(!alpha.is_nan(), "a NaN fade has no opacity to mean");
    alpha.clamp(0.0, 1.0)
}

/// `sprite` moved by `offset` and faded to `alpha` of its opacity.
///
/// The whole of what a renderer's batch offset and fade do to one
/// sprite, in one place, so it can be checked without a device.
///
/// **The tint is premultiplied, so the fade scales all four channels
/// and not just the fourth.** In premultiplied RGBA the colour already
/// carries its own alpha; halving only the alpha leaves the colour
/// arriving at full strength while occluding less, so the sprite
/// BRIGHTENS as it fades. Scaling the whole tuple is what "half as
/// opaque" means under this convention.
pub(crate) fn placed(sprite: &Sprite, offset: (f32, f32), alpha: f32) -> Sprite {
    let mut moved = *sprite;
    moved.x += offset.0;
    moved.y += offset.1;
    for channel in &mut moved.tint {
        *channel *= alpha;
    }
    moved
}

/// One instance record, packed exactly as the layout slice declares:
/// NDC min, NDC max, UV min, UV max — each two `f32`s — then the
/// four-`f32` tint, native-endian, in declaration order.
#[allow(
    clippy::cast_precision_loss,
    reason = "past 2^24 texels the packing degrades visibly, never unsafely; real regions sit far below it"
)]
pub(crate) fn pack(sprite: &Sprite, canvas: Canvas, atlas: Extent) -> [u8; INSTANCE_STRIDE] {
    let canvas_min = Vec2::new(sprite.x, sprite.y);
    let canvas_max = canvas_min + Vec2::new(sprite.width, sprite.height);
    let ndc_min = to_ndc(canvas_min, canvas);
    let ndc_max = to_ndc(canvas_max, canvas);

    let texel_min = Vec2::new(sprite.region.x as f32, sprite.region.y as f32);
    let texel_max = texel_min + Vec2::new(sprite.region.width as f32, sprite.region.height as f32);
    let uv_min = to_uv(texel_min, atlas);
    let uv_max = to_uv(texel_max, atlas);

    let values: [f32; 12] = [
        ndc_min.x,
        ndc_min.y,
        ndc_max.x,
        ndc_max.y,
        uv_min.x,
        uv_min.y,
        uv_max.x,
        uv_max.y,
        sprite.tint[0],
        sprite.tint[1],
        sprite.tint[2],
        sprite.tint[3],
    ];
    let mut bytes = [0u8; INSTANCE_STRIDE];
    for (slot, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        slot.copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    reason = "test dimensions stay far below 2^24, where f32 is exact"
)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn canvas(width: u32, height: u32) -> Canvas {
        Canvas::new(width, height).expect("nonzero test canvas")
    }

    /// Exact float claims compare bits, the math crate's own pattern:
    /// these maps promise identities, not approximations.
    fn bits(v: Vec2) -> (u32, u32) {
        (v.x.to_bits(), v.y.to_bits())
    }

    /// Exact tint claims compare bits, beside `bits` above and for the
    /// same reason: these are identities, not approximations. A fade of
    /// one half is an exponent decrement, so it is bit-exact.
    fn tint_bits(tint: [f32; 4]) -> [u32; 4] {
        tint.map(f32::to_bits)
    }

    fn swatch() -> Sprite {
        Sprite::new(
            Region {
                x: 4,
                y: 8,
                width: 16,
                height: 16,
            },
            10.0,
            20.0,
        )
        .size(32.0, 48.0)
        .tint([0.8, 0.4, 0.2, 0.5])
    }

    /// A fade is clamped, and a NaN fade is refused by name.
    ///
    /// Infinities clamp; NaN does not, which is the whole reason the
    /// guard exists. The asymmetry is asserted rather than left to be
    /// rediscovered.
    #[test]
    fn a_fade_clamps_its_range() {
        assert_eq!(fade(2.0).to_bits(), 1.0f32.to_bits());
        assert_eq!(fade(-1.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(fade(f32::INFINITY).to_bits(), 1.0f32.to_bits());
        assert_eq!(fade(f32::NEG_INFINITY).to_bits(), 0.0f32.to_bits());
        assert_eq!(fade(0.25).to_bits(), 0.25f32.to_bits());
        // The fact the guard rests on: clamp does NOT filter NaN.
        assert!(
            f32::NAN.clamp(0.0, 1.0).is_nan(),
            "clamp began filtering NaN"
        );
    }

    /// Probed by dropping the assert from `fade`: this stops panicking
    /// and goes red.
    #[test]
    #[should_panic(expected = "NaN fade")]
    fn a_nan_fade_is_refused() {
        let _ = fade(f32::NAN);
    }

    /// An offset moves a sprite and changes nothing else about it.
    ///
    /// The batch offset exists so a caller can slide a whole group
    /// without the code that builds each sprite knowing the group is
    /// moving. If it touched the size, the region or the tint it would
    /// stop being a slide.
    #[test]
    fn an_offset_moves_a_sprite_and_leaves_the_rest_alone() {
        let before = swatch();
        let after = placed(&before, (7.0, -3.0), 1.0);
        assert_eq!((after.x, after.y), (17.0, 17.0));
        assert_eq!((after.width, after.height), (before.width, before.height));
        assert_eq!(after.region, before.region);
        assert_eq!(
            tint_bits(after.tint),
            tint_bits(before.tint),
            "an offset recoloured the sprite"
        );
        // Identity really is identity.
        assert_eq!(placed(&before, (0.0, 0.0), 1.0), before);
    }

    /// **A fade scales all four channels, because the tint is
    /// premultiplied.**
    ///
    /// Halving only the fourth would leave the colour arriving at full
    /// strength while occluding less, so the sprite brightens as it
    /// fades - the opposite of the intent, and the classic way to get
    /// premultiplied alpha wrong.
    ///
    /// Probed by fading only `tint[3]`: red on the first channel, which
    /// is exactly the bug.
    #[test]
    fn a_fade_scales_every_channel_because_the_tint_is_premultiplied() {
        let after = placed(&swatch(), (0.0, 0.0), 0.5);
        assert_eq!(
            tint_bits(after.tint),
            tint_bits([0.4, 0.2, 0.1, 0.25]),
            "the fade did not scale the premultiplied colour with its alpha"
        );
        // Fully faded is fully gone, in every channel - a sprite that
        // still carries colour at zero alpha is a sprite that adds
        // light to whatever it is drawn over.
        assert_eq!(
            tint_bits(placed(&swatch(), (0.0, 0.0), 0.0).tint),
            tint_bits([0.0; 4])
        );
    }

    proptest! {
        /// Fading twice is fading once by the product, and a fade never
        /// makes a sprite more opaque than it was.
        ///
        /// The composition law is what lets a caller nest a fading
        /// group inside a fading group without the two multiplying
        /// wrongly, and it is the property a channel-by-channel
        /// implementation can break silently.
        #[test]
        fn fades_compose_and_never_brighten(
            a in 0.0f32..=1.0,
            b in 0.0f32..=1.0,
        ) {
            let once = placed(&placed(&swatch(), (0.0, 0.0), a), (0.0, 0.0), b);
            let twice = placed(&swatch(), (0.0, 0.0), a * b);
            for (x, y) in once.tint.iter().zip(twice.tint) {
                prop_assert!(
                    (x - y).abs() <= 1e-6,
                    "fading by {a} then {b} is not fading by their product"
                );
            }
            for (faded, full) in once.tint.iter().zip(swatch().tint) {
                prop_assert!(*faded <= full + 1e-6, "a fade brightened a channel");
            }
            // **Every channel by the SAME factor**, which is the
            // premultiplied rule stated as a property. Without this the
            // property passes an alpha-only fade: the colour channels
            // stay put, so they still compose and still never brighten
            // - they are simply wrong. Found by probing, when the exact
            // test went red here and this one did not.
            let single = placed(&swatch(), (0.0, 0.0), a);
            for (faded, full) in single.tint.iter().zip(swatch().tint) {
                prop_assert!(
                    (faded - full * a).abs() <= 1e-6,
                    "a fade of {a} scaled a channel to {faded} from {full}"
                );
            }
        }

        /// Offsets add, and they never touch the tint.
        #[test]
        fn offsets_add(
            ax in -4000.0f32..4000.0,
            ay in -4000.0f32..4000.0,
            bx in -4000.0f32..4000.0,
            by in -4000.0f32..4000.0,
        ) {
            let twice = placed(&placed(&swatch(), (ax, ay), 1.0), (bx, by), 1.0);
            let once = placed(&swatch(), (ax + bx, ay + by), 1.0);
            // **Bounded by the INTERMEDIATES, not the result.** Float
            // addition is not associative, and the error in summing
            // three terms scales with the sum of their magnitudes -
            // not with the answer. Two offsets that nearly cancel
            // (2046.2 and -1952.1, which proptest found at four
            // thousand cases) leave a result near a hundred carrying
            // the rounding of an intermediate near two thousand, so a
            // tolerance keyed on the result is off by a factor of
            // twenty. A fixed 1e-3 was worse: it sat exactly on the
            // f32 ulp at this range, so the test would have passed or
            // failed on where the generator happened to land.
            let bound = (swatch().x.abs() + ax.abs() + bx.abs()).max(1.0) * f32::EPSILON * 4.0;
            prop_assert!(
                (twice.x - once.x).abs() <= bound,
                "x differed by {} against a bound of {bound}",
                (twice.x - once.x).abs()
            );
            let bound_y = (swatch().y.abs() + ay.abs() + by.abs()).max(1.0) * f32::EPSILON * 4.0;
            prop_assert!((twice.y - once.y).abs() <= bound_y);
            prop_assert_eq!(tint_bits(twice.tint), tint_bits(swatch().tint));
        }
    }

    #[test]
    fn a_zero_dimension_is_not_a_canvas() {
        assert!(Canvas::new(0, 1).is_none());
        assert!(Canvas::new(1, 0).is_none());
        assert!(Canvas::new(0, 0).is_none());
        let c = canvas(320, 200);
        assert_eq!((c.width(), c.height()), (320, 200));
    }

    #[test]
    fn the_four_canvas_corners_map_to_their_ndc_corners() {
        // The sign-mistake killer: top-left is (-1, -1) — NDC y grows
        // downward like canvas y — and each corner is exact, not close.
        let c = canvas(640, 360);
        assert_eq!(
            bits(to_ndc(Vec2::new(0.0, 0.0), c)),
            bits(Vec2::new(-1.0, -1.0))
        );
        assert_eq!(
            bits(to_ndc(Vec2::new(640.0, 0.0), c)),
            bits(Vec2::new(1.0, -1.0))
        );
        assert_eq!(
            bits(to_ndc(Vec2::new(0.0, 360.0), c)),
            bits(Vec2::new(-1.0, 1.0))
        );
        assert_eq!(
            bits(to_ndc(Vec2::new(640.0, 360.0), c)),
            bits(Vec2::new(1.0, 1.0))
        );
        assert_eq!(
            bits(to_ndc(Vec2::new(320.0, 180.0), c)),
            bits(Vec2::new(0.0, 0.0))
        );
    }

    #[test]
    fn uv_maps_texel_edges_to_unit_fractions() {
        let atlas = Extent {
            width: 8,
            height: 4,
        };
        assert_eq!(
            bits(to_uv(Vec2::new(0.0, 0.0), atlas)),
            bits(Vec2::new(0.0, 0.0))
        );
        assert_eq!(
            bits(to_uv(Vec2::new(8.0, 4.0), atlas)),
            bits(Vec2::new(1.0, 1.0))
        );
        assert_eq!(
            bits(to_uv(Vec2::new(2.0, 1.0), atlas)),
            bits(Vec2::new(0.25, 0.25))
        );
    }

    #[test]
    fn new_takes_the_regions_own_size_and_no_tint() {
        let sprite = Sprite::new(
            Region {
                x: 4,
                y: 0,
                width: 4,
                height: 4,
            },
            10.0,
            20.0,
        );
        assert_eq!(
            (sprite.width.to_bits(), sprite.height.to_bits()),
            (4.0f32.to_bits(), 4.0f32.to_bits())
        );
        assert_eq!(sprite.tint.map(f32::to_bits), [1.0f32; 4].map(f32::to_bits));
        let resized = sprite.size(8.0, 2.0).tint([0.5, 0.5, 0.5, 0.5]);
        assert_eq!(
            (resized.width.to_bits(), resized.height.to_bits()),
            (8.0f32.to_bits(), 2.0f32.to_bits())
        );
        assert_eq!(
            resized.tint.map(f32::to_bits),
            [0.5f32; 4].map(f32::to_bits)
        );
    }

    #[test]
    fn packing_the_same_sprite_twice_is_byte_identical() {
        // The CPU half of the determinism story: the render-twice
        // checks downstream prove the GPU half, this proves the fill.
        let c = canvas(64, 32);
        let atlas = Extent {
            width: 8,
            height: 8,
        };
        let sprite = Sprite::new(
            Region {
                x: 2,
                y: 2,
                width: 3,
                height: 5,
            },
            7.25,
            11.5,
        )
        .tint([0.25, 0.5, 0.75, 1.0]);
        assert_eq!(
            pack(&sprite, c, atlas),
            pack(&sprite, c, atlas),
            "the same fill must produce the same bytes"
        );
    }

    #[test]
    fn packing_is_byte_exact_against_a_hand_computed_record() {
        // A 4×4 region at the top-left of an 8×8 atlas, drawn from
        // (16, 8) to (48, 24) on a 64×32 canvas, half-tinted. Every
        // expected f32 below is written out by hand from the maps'
        // definitions; the test owns the arithmetic, not the code
        // under test.
        let c = canvas(64, 32);
        let atlas = Extent {
            width: 8,
            height: 8,
        };
        let sprite = Sprite::new(
            Region {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            16.0,
            8.0,
        )
        .size(32.0, 16.0)
        .tint([0.5, 0.25, 0.125, 0.5]);

        let expected: [f32; 12] = [
            -0.5, -0.5, // ndc min: 2*16/64-1, 2*8/32-1
            0.5, 0.5, // ndc max: 2*48/64-1, 2*24/32-1
            0.0, 0.0, // uv min
            0.5, 0.5, // uv max: 4/8
            0.5, 0.25, 0.125, 0.5, // tint, verbatim
        ];
        let packed = pack(&sprite, c, atlas);
        for (index, value) in expected.iter().enumerate() {
            let mut raw = [0u8; 4];
            raw.copy_from_slice(&packed[index * 4..index * 4 + 4]);
            assert_eq!(
                f32::from_ne_bytes(raw).to_bits(),
                value.to_bits(),
                "f32 slot {index} disagrees with the hand computation"
            );
        }
    }

    proptest! {
        /// Math earns property tests: over arbitrary canvases and points, the
        /// ortho map is monotone in both axes, exact at the corners,
        /// and inverts back to the input within one part in a million
        /// of the canvas size.
        #[test]
        fn the_ortho_map_is_monotone_exact_at_corners_and_invertible(
            width in 1u32..=16_384,
            height in 1u32..=16_384,
            x in 0.0f32..=16_384.0,
            y in 0.0f32..=16_384.0,
            step in 0.001f32..=64.0,
        ) {
            let c = canvas(width, height);
            let w = width as f32;
            let h = height as f32;

            // Corners are exact — the identity `2w/w - 1 == 1` holds in
            // f32 because the division is by the value itself.
            prop_assert_eq!(bits(to_ndc(Vec2::new(0.0, 0.0), c)), bits(Vec2::new(-1.0, -1.0)));
            prop_assert_eq!(bits(to_ndc(Vec2::new(w, h), c)), bits(Vec2::new(1.0, 1.0)));

            // Monotone: a strictly larger input never maps smaller.
            let here = to_ndc(Vec2::new(x, y), c);
            let there = to_ndc(Vec2::new(x + step, y + step), c);
            prop_assert!(there.x >= here.x);
            prop_assert!(there.y >= here.y);

            // Invertible: canvas = (ndc + 1) * extent / 2, within one
            // part in a million of the extent.
            let back = Vec2::new((here.x + 1.0) * w / 2.0, (here.y + 1.0) * h / 2.0);
            prop_assert!((back.x - x).abs() <= w * 1e-6 + 1e-3);
            prop_assert!((back.y - y).abs() <= h * 1e-6 + 1e-3);
        }
    }
}
