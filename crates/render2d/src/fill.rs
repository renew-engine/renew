//! The pure half: sprites in canvas space become instance bytes in NDC.
//!
//! Nothing here touches a device. The ortho map, the UV map, and the
//! packing are plain functions so the unit and property tests below can
//! pin them without bringing up Vulkan — the GPU half consumes exactly
//! these bytes.

use renew_math::Vec2;
use renew_rhi::Extent;

/// One packed instance: seven attributes, sixteen `f32`s, 64 bytes. The
/// shader's `location(0..=6)` list, the layout slice in `gpu.rs`, and
/// [`pack`] describe the same bytes; change one and the others in the
/// same commit or the draw reads garbage.
pub(crate) const INSTANCE_STRIDE: usize = 64;

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

/// One sprite: an atlas region, a place on the canvas, a size, a tint,
/// and — each an identity by default — a mirror on either axis, a turn
/// about a pivot and a scale about the same pivot.
///
/// `#[non_exhaustive]` with a constructor and builders, the descriptor
/// pattern this tree uses everywhere: the fields a later version adds
/// arrive as builders touching no existing caller.
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
    /// one convention end to end. A uniform tint `[a, a, a, a]` is a
    /// fade to `a` of the sprite's opacity: the tint is premultiplied,
    /// so scaling all four channels is what "`a` as opaque" means, and
    /// scaling only the fourth would brighten the sprite as it faded.
    pub tint: [f32; 4],
    /// Mirror the sampled texels left for right. The canvas rectangle
    /// stays where it is; only which texel each pixel reads changes.
    pub flip_x: bool,
    /// Mirror top for bottom, the same way.
    pub flip_y: bool,
    /// The turn about `pivot`, in **turns** (a quarter turn is `0.25`).
    /// Positive turns clockwise as seen on screen — canvas y grows
    /// downward, so the sprite's right edge swings toward the bottom.
    /// `0.0`, the default, is the axis-aligned rectangle exactly: that
    /// value takes a path with no trigonometry and no pivot arithmetic
    /// and packs the bytes an unrotated sprite always packed. Every
    /// other value goes through this crate's own sine and cosine — a
    /// polynomial evaluated in adds, subtracts and multiplies only, so
    /// the same turn packs the same corners on every platform and
    /// toolchain, and a picture of a turned sprite recorded on one
    /// machine is the picture every machine draws. Multiples of a
    /// quarter turn are exact by construction (a whole turn reduces to
    /// zero), but only `0.0` is the arithmetic-free path — a caller
    /// that means "no turn" says `0.0`. Radians divide by `TAU` once.
    ///
    /// A turned edge is rasterised by pixel coverage and sampled by
    /// nearest lookup clamped at the atlas edge, not the region's. A
    /// covered pixel's centre lies inside the quad, so its texel lies
    /// inside the region up to interpolation rounding; the one-texel
    /// transparent gutter every declared region already owes (the UI
    /// atlas's rule) is what makes "up to rounding" harmless, and it
    /// becomes mandatory the day linear filtering exists. The turn is
    /// isotropic in canvas pixels; a consumer that stretches its canvas
    /// onto a surface of another aspect ratio stretches the turned
    /// sprite with everything else, and owns that choice.
    pub rotation: f32,
    /// The point the turn and the scale act about, as fractions of the
    /// sprite's own size from its top-left: `[0.5, 0.5]`, the centre,
    /// by default. Fractions rather than pixels so `size` never leaves
    /// the pivot stale.
    pub pivot: [f32; 2],
    /// A scale about `pivot`, per axis; `[1.0, 1.0]` by default. A
    /// negative factor mirrors the sprite through the pivot — the
    /// geometry's winding reverses and both faces are drawn, so a card
    /// can turn over by scaling its width through zero.
    pub scale: [f32; 2],
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
            flip_x: false,
            flip_y: false,
            rotation: 0.0,
            pivot: [0.5, 0.5],
            scale: [1.0, 1.0],
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

    /// Mirror left for right when `flip` is true; the last call wins.
    #[must_use]
    pub fn flip_x(mut self, flip: bool) -> Self {
        self.flip_x = flip;
        self
    }

    /// Mirror top for bottom when `flip` is true; the last call wins.
    #[must_use]
    pub fn flip_y(mut self, flip: bool) -> Self {
        self.flip_y = flip;
        self
    }

    /// Turn the sprite by `turns` about its pivot, clockwise on screen
    /// for positive values; `0.0` is no turn and the exact axis-aligned
    /// record.
    #[must_use]
    pub fn rotation(mut self, turns: f32) -> Self {
        self.rotation = turns;
        self
    }

    /// The point the turn and the scale act about, as fractions of the
    /// sprite's size from its top-left; `(0.5, 0.5)` is the centre.
    #[must_use]
    pub fn pivot(mut self, x: f32, y: f32) -> Self {
        self.pivot = [x, y];
        self
    }

    /// Scale the sprite about its pivot, per axis; a negative factor
    /// mirrors it through the pivot.
    #[must_use]
    pub fn scale(mut self, x: f32, y: f32) -> Self {
        self.scale = [x, y];
        self
    }

    /// The bytes [`crate::SpriteRenderer::push`] writes for this sprite
    /// on `canvas` over an atlas of `atlas` texels when no batch offset
    /// or fade is set — `push` applies the batch state to the sprite
    /// first and then packs exactly this. The packer, reachable without
    /// a device, so it can be timed and pinned.
    #[must_use]
    pub fn instance(&self, canvas: Canvas, atlas: Extent) -> Instance {
        Instance(pack(self, canvas, atlas))
    }
}

/// One packed instance record, exactly as the pipeline reads it.
///
/// Opaque on purpose: the stride and the lane order belong to the
/// pipeline, and a caller holding one of these can hand its bytes to a
/// benchmark, a hash or a buffer without the layout becoming a promise
/// this crate has to keep. [`Sprite::instance`] makes one without a
/// device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instance([u8; INSTANCE_STRIDE]);

impl Instance {
    /// The record's bytes, in the order the pipeline declares them.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.0
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

/// The UV corners after the flips: the axis that mirrors swaps its two
/// edges, so the vertex stage's interpolation from the first corner to
/// the second runs backwards along it.
///
/// A mirror is a UV swap and nothing else. The geometry — and with it
/// the winding — is untouched, so a flipped sprite costs the pipeline
/// nothing and cannot be culled by a facing rule.
pub(crate) fn mirrored(uv_min: Vec2, uv_max: Vec2, flip_x: bool, flip_y: bool) -> (Vec2, Vec2) {
    let (x0, x1) = if flip_x {
        (uv_max.x, uv_min.x)
    } else {
        (uv_min.x, uv_max.x)
    };
    let (y0, y1) = if flip_y {
        (uv_max.y, uv_min.y)
    } else {
        (uv_min.y, uv_max.y)
    };
    (Vec2::new(x0, y0), Vec2::new(x1, y1))
}

// Minimax coefficients for the folded octant `|x| <= pi/4`, fitted in
// double precision by iteratively reweighted least squares on relative
// error. With these values rounded to `f32` the polynomials sit within
// 1e-8 of the true functions in double precision, so what bounds the
// error is the single-precision evaluation's own rounding.
const S1: f32 = -0.166_666_55;
const S2: f32 = 0.008_332_16;
const S3: f32 = -0.000_195_152_82;
const C1: f32 = -0.5;
const C2: f32 = 0.041_666_62;
const C3: f32 = -0.001_388_668_3;
const C4: f32 = 0.000_024_383_655;

/// Sine and cosine of a turn count — this crate's own, so that a turn
/// packs the same corners on every platform.
///
/// The standard library's `sin_cos` is the platform C library's, which
/// is neither correctly rounded nor the same library on every machine;
/// a picture of a turned sprite recorded through it would attest a
/// library the record does not name. This is a fixed polynomial
/// evaluated in adds, subtracts and multiplies only — each correctly
/// rounded by IEEE 754, and never fused — so the bits are the same
/// everywhere.
///
/// Exact at every multiple of a quarter turn by construction: the
/// reduction lands there with a residual of exactly zero, and the
/// polynomials are written so that a zero residual gives `(0.0, 1.0)`
/// without rounding. NaN propagates and draws nothing recognisable;
/// past 2^23 turns every `f32` is an integer and the residual is zero.
pub(crate) fn turn_sin_cos(turns: f32) -> (f32, f32) {
    // Whole turns are the identity: `t - round(t)` in [-0.5, 0.5],
    // exact — below 2^23 the two operands are within a factor of two
    // of each other (Sterbenz), above it every value is an integer.
    let within_turn = turns - turns.round();
    // Quarters: a power-of-two scale, exact, in [-2, 2].
    let quarters = within_turn * 4.0;
    // The nearest quarter and the residual in [-0.5, 0.5] quarters —
    // exact by the same argument (the nearest quarter is 0, or the
    // value is within a factor of two of it).
    let nearest = quarters.round();
    let residual = quarters - nearest;
    // Radians on the folded octant, at most pi/4 either way: ONE
    // rounding.
    let radians = residual * core::f32::consts::FRAC_PI_2;
    let squared = radians * radians;
    let sine = radians * (1.0 + squared * (S1 + squared * (S2 + squared * S3)));
    let cosine = 1.0 + squared * (C1 + squared * (C2 + squared * (C3 + squared * C4)));
    // Unfold. The nearest quarter is a small integer in {-2, ..., 2},
    // so its Euclidean remainder by four is exact and the cast is a
    // value in 0..4.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the remainder of a small integer-valued float by four is one of 0.0, 1.0, 2.0, 3.0"
    )]
    let quadrant = nearest.rem_euclid(4.0) as u8;
    let (sine, cosine) = match quadrant {
        0 => (sine, cosine),
        1 => (cosine, -sine),
        2 => (-sine, -cosine),
        _ => (-cosine, sine),
    };
    // `+ 0.0` turns any `-0.0` into `+0.0` and changes nothing else,
    // so the quarter-turn results are bit-exactly (0, 1), (1, 0),
    // (0, -1), (-1, 0).
    (sine + 0.0, cosine + 0.0)
}

/// The four corners in canvas pixels — local top-left, top-right,
/// bottom-left, bottom-right — after the pivot, the scale and the turn.
///
/// The untransformed case takes a path with no trigonometry and no
/// pivot arithmetic, so its bytes are the ones an unrotated sprite
/// always packed. That is a branch and not an identity of the
/// arithmetic: `(q - p) + p` is not `q` in `f32` — with `x = 0.1`, a
/// width of 1000 and the centre pivot it comes back as `0.1000061` —
/// and every committed picture rests on the branch.
#[expect(
    clippy::float_cmp,
    reason = "exactly the untransformed values must take the arithmetic-free path; nearness would send a sprite that means 'no turn' through rounding"
)]
pub(crate) fn corners(sprite: &Sprite) -> [Vec2; 4] {
    let canvas_min = Vec2::new(sprite.x, sprite.y);
    let canvas_max = canvas_min + Vec2::new(sprite.width, sprite.height);
    let rect = [
        canvas_min,
        Vec2::new(canvas_max.x, canvas_min.y),
        Vec2::new(canvas_min.x, canvas_max.y),
        canvas_max,
    ];
    if sprite.rotation == 0.0 && sprite.scale == [1.0, 1.0] {
        return rect;
    }
    let pivot = Vec2::new(
        sprite.x + sprite.pivot[0] * sprite.width,
        sprite.y + sprite.pivot[1] * sprite.height,
    );
    let (s, c) = turn_sin_cos(sprite.rotation);
    rect.map(|q| {
        let d = q - pivot;
        let d = Vec2::new(d.x * sprite.scale[0], d.y * sprite.scale[1]);
        pivot + Vec2::new(d.x * c - d.y * s, d.x * s + d.y * c)
    })
}

/// One instance record, packed exactly as the layout slice declares:
/// the four corners in NDC — local top-left, top-right, bottom-left,
/// bottom-right, each two `f32`s — then the UV at the first corner and
/// at the last, then the four-`f32` tint, native-endian, in
/// declaration order. The two UV lanes are the region's min and max
/// unless a flip swapped them.
#[allow(
    clippy::cast_precision_loss,
    reason = "past 2^24 texels the packing degrades visibly, never unsafely; real regions sit far below it"
)]
pub(crate) fn pack(sprite: &Sprite, canvas: Canvas, atlas: Extent) -> [u8; INSTANCE_STRIDE] {
    let [a, b, c, d] = corners(sprite).map(|corner| to_ndc(corner, canvas));

    let texel_min = Vec2::new(sprite.region.x as f32, sprite.region.y as f32);
    let texel_max = texel_min + Vec2::new(sprite.region.width as f32, sprite.region.height as f32);
    let (uv0, uv1) = mirrored(
        to_uv(texel_min, atlas),
        to_uv(texel_max, atlas),
        sprite.flip_x,
        sprite.flip_y,
    );

    let values: [f32; 16] = [
        a.x,
        a.y,
        b.x,
        b.y,
        c.x,
        c.y,
        d.x,
        d.y,
        uv0.x,
        uv0.y,
        uv1.x,
        uv1.y,
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

    /// `Sprite::instance` is the packer's bytes, no more and no less —
    /// the doc's claim that a caller holding no device holds the same
    /// record the renderer would write.
    ///
    /// Probed by having `instance` pack a zeroed record: red.
    #[test]
    fn the_instance_is_the_packers_bytes() {
        let c = canvas(64, 32);
        let atlas = Extent {
            width: 8,
            height: 8,
        };
        let instance = swatch().instance(c, atlas);
        assert_eq!(instance.bytes(), &pack(&swatch(), c, atlas)[..]);
        assert_eq!(instance.bytes().len(), INSTANCE_STRIDE);
    }

    /// A moved sprite packs a moved record: `instance` reads the
    /// placement it is given, so a caller that wants exactly what the
    /// renderer wrote under a batch offset applies `placed` first. That
    /// `push` does so in that order is a device-side fact, pinned by the
    /// batch-offset oracle in the golden suite, not here.
    ///
    /// Probed by having `placed` ignore its offset: the two records
    /// agree and this reds.
    #[test]
    fn a_moved_sprite_packs_a_moved_record() {
        let c = canvas(64, 32);
        let atlas = Extent {
            width: 8,
            height: 8,
        };
        let bare = swatch();
        let moved = placed(&bare, (7.0, -3.0), 1.0);
        assert_ne!(
            moved.instance(c, atlas),
            bare.instance(c, atlas),
            "an offset must move the record"
        );
    }

    /// The sixteen `f32` lanes of a packed record, for tests that name
    /// a lane rather than a byte: 0..8 the four corners, 8..12 the two
    /// UVs, 12..16 the tint.
    fn lanes(record: &[u8; INSTANCE_STRIDE]) -> [u32; 16] {
        let mut out = [0u32; 16];
        for (lane, chunk) in out.iter_mut().zip(record.as_chunks::<4>().0) {
            *lane = f32::from_ne_bytes(*chunk).to_bits();
        }
        out
    }

    /// Distance in ulps between two `f32`s of the same sign; zero for
    /// equal values (either zero counts as equal to the other), the
    /// maximum when the signs differ.
    #[allow(
        clippy::cast_possible_wrap,
        reason = "the bit patterns are compared as signed so a subtraction of same-sign values is their ulp distance"
    )]
    fn ulps(a: f32, b: f32) -> u32 {
        if a.to_bits() == b.to_bits() || (a.abs().to_bits() == 0 && b.abs().to_bits() == 0) {
            return 0;
        }
        if a.is_sign_negative() != b.is_sign_negative() {
            return u32::MAX;
        }
        (a.to_bits() as i32)
            .wrapping_sub(b.to_bits() as i32)
            .unsigned_abs()
    }

    const SQUARE: Region = Region {
        x: 0,
        y: 0,
        width: 4,
        height: 4,
    };

    /// The untransformed sprite packs the rectangle's corners bit for
    /// bit, on the fixture that makes the general path round: `x = 0.1`
    /// with a width of 1000 and the centre pivot puts the pivot at
    /// `500.1`, and `500.1 + (0.1 - 500.1)` is `0.1000061` in `f32`.
    /// The test proves the fixture rounds before it relies on it.
    ///
    /// Probed by deleting the early-out branch in `corners`: red.
    #[test]
    fn an_untransformed_sprite_packs_the_rectangle_bit_for_bit() {
        let sprite = Sprite::new(SQUARE, 0.1, 0.3).size(1000.0, 500.0);
        let pivot = Vec2::new(0.1 + 0.5 * 1000.0, 0.3 + 0.5 * 500.0);
        let round_trip = pivot + (Vec2::new(0.1, 0.3) - pivot);
        assert_ne!(
            bits(round_trip),
            bits(Vec2::new(0.1, 0.3)),
            "the fixture no longer rounds; pick one that does"
        );
        let far = Vec2::new(0.1, 0.3) + Vec2::new(1000.0, 500.0);
        let expected = [
            Vec2::new(0.1, 0.3),
            Vec2::new(far.x, 0.3),
            Vec2::new(0.1, far.y),
            far,
        ];
        for (got, want) in corners(&sprite).into_iter().zip(expected) {
            assert_eq!(bits(got), bits(want));
        }
        // Unit scale, said explicitly, is the same arithmetic-free path.
        for (got, want) in corners(&sprite.scale(1.0, 1.0)).into_iter().zip(expected) {
            assert_eq!(
                bits(got),
                bits(want),
                "an explicit unit scale left the branch"
            );
        }
    }

    /// Bit-exact at every multiple of a quarter turn, whichever way it
    /// is spelled — including the sign of every zero, which the
    /// normalisation at the end of `turn_sin_cos` guarantees.
    ///
    /// Probed by dropping that `+ 0.0`: the quarter turn answers
    /// `(1.0, -0.0)` and the bits differ.
    #[test]
    fn turn_sin_cos_is_exact_at_quarter_turns() {
        let cases: [(f32, (f32, f32)); 9] = [
            (0.0, (0.0, 1.0)),
            (0.25, (1.0, 0.0)),
            (0.5, (0.0, -1.0)),
            (0.75, (-1.0, 0.0)),
            (1.0, (0.0, 1.0)),
            (-0.25, (-1.0, 0.0)),
            (-0.0, (0.0, 1.0)),
            (2.5, (0.0, -1.0)),
            (1e7, (0.0, 1.0)),
        ];
        for (turns, (s, c)) in cases {
            let (got_s, got_c) = turn_sin_cos(turns);
            assert_eq!(
                (got_s.to_bits(), got_c.to_bits()),
                (s.to_bits(), c.to_bits()),
                "at {turns} turns: got ({got_s}, {got_c}), want ({s}, {c})"
            );
        }
    }

    /// The guard on the argument scale: a tenth of a turn is 36°, whose
    /// sine and cosine are written here as constants. A mix-up between
    /// a turn and a radian, or a wrong fold, reddens this; nothing else
    /// in the suite exercises a non-quarter angle exactly.
    #[test]
    fn turn_sin_cos_matches_the_tenth_turn_constants() {
        let (s, c) = turn_sin_cos(0.1);
        assert!(
            ulps(s, 0.587_785_25) <= 2,
            "sin 36° is off by {} ulp",
            ulps(s, 0.587_785_25)
        );
        assert!(
            ulps(c, 0.809_017) <= 2,
            "cos 36° is off by {} ulp",
            ulps(c, 0.809_017)
        );
    }

    /// The fixed sweep the two tests below share: 65,537 arguments
    /// across `[-2, 2]` turns.
    fn sweep() -> impl Iterator<Item = f32> {
        (0..=65_536u32).map(|i| -2.0 + 4.0 * (i as f32) / 65_536.0)
    }

    /// Against double precision over the sweep: every result within
    /// two ulps of the correctly rounded value, or within `1e-7`
    /// absolutely where the reference itself is a rounding of pi at a
    /// zero crossing (the double-precision sine of a whole number of
    /// half turns is not zero, the polynomial's is).
    #[test]
    fn turn_sin_cos_is_within_two_ulp_of_double_precision() {
        let mut worst = 0u32;
        for turns in sweep() {
            let (s, c) = turn_sin_cos(turns);
            let reduced = f64::from(turns) - f64::from(turns).round();
            let (es, ec) = (reduced * core::f64::consts::TAU).sin_cos();
            #[allow(
                clippy::cast_possible_truncation,
                reason = "rounding the double-precision reference to single precision is the comparison"
            )]
            let (es, ec) = (es as f32, ec as f32);
            let error_s = if (s - es).abs() <= 1e-7 {
                0
            } else {
                ulps(s, es)
            };
            let error_c = if (c - ec).abs() <= 1e-7 {
                0
            } else {
                ulps(c, ec)
            };
            worst = worst.max(error_s).max(error_c);
        }
        assert!(worst <= 2, "the worst error over the sweep is {worst} ulp");
    }

    /// The same bits on every platform: FNV-1a 64 over the sweep's
    /// little-endian bytes against a constant measured on one machine
    /// and asserted on every one the engine builds for. A platform that
    /// disagrees fails here by name rather than drawing a turned sprite
    /// nobody compares.
    #[test]
    fn turn_sin_cos_is_the_same_bits_on_every_platform() {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for turns in sweep() {
            let (s, c) = turn_sin_cos(turns);
            for byte in s.to_le_bytes().into_iter().chain(c.to_le_bytes()) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        assert_eq!(
            hash, 0xf027_961a_b5fa_db08,
            "the sweep's bits moved: either the polynomial changed (bump this constant in the \
             same change, deliberately) or this platform computes differently (which is the \
             finding)"
        );
    }

    /// A quarter turn about the centre permutes the corners exactly:
    /// 16×8 at (8, 8) turned by `0.25` about (16, 12) puts the local
    /// top-left at (20, 4), top-right at (20, 20), bottom-left at
    /// (12, 4), bottom-right at (12, 20) — the right edge swung down,
    /// which is what clockwise means on a y-down canvas. Every product
    /// is by zero or one and every sum is of small integers, so the
    /// bits are exact.
    ///
    /// Probed by flipping the sign of `s` in `corners`: the corners
    /// land at (12, 20), (12, 4), (20, 20), (20, 4) — anticlockwise.
    #[test]
    fn a_quarter_turn_about_the_centre_permutes_the_corners_exactly() {
        let wide = Region {
            x: 0,
            y: 0,
            width: 16,
            height: 8,
        };
        let [a, b, c, d] = corners(&Sprite::new(wide, 8.0, 8.0).rotation(0.25));
        assert_eq!(bits(a), bits(Vec2::new(20.0, 4.0)), "top-left");
        assert_eq!(bits(b), bits(Vec2::new(20.0, 20.0)), "top-right");
        assert_eq!(bits(c), bits(Vec2::new(12.0, 4.0)), "bottom-left");
        assert_eq!(bits(d), bits(Vec2::new(12.0, 20.0)), "bottom-right");
    }

    /// A half turn swaps the diagonals exactly: every corner lands on
    /// the one opposite it.
    #[test]
    fn a_half_turn_swaps_diagonals_exactly() {
        let wide = Region {
            x: 0,
            y: 0,
            width: 16,
            height: 8,
        };
        let plain = corners(&Sprite::new(wide, 8.0, 8.0));
        let turned = corners(&Sprite::new(wide, 8.0, 8.0).rotation(0.5));
        for (got, want) in turned
            .into_iter()
            .zip([plain[3], plain[2], plain[1], plain[0]])
        {
            assert_eq!(bits(got), bits(want));
        }
    }

    /// A negative scale mirrors through the pivot exactly: on an
    /// integer fixture `scale(-1, 1)` swaps the left and right corners
    /// bit for bit, which is the geometric mirror beside the sampled
    /// one `flip_x` is.
    #[test]
    fn a_negative_scale_mirrors_through_the_pivot_exactly() {
        let wide = Region {
            x: 0,
            y: 0,
            width: 16,
            height: 8,
        };
        let plain = corners(&Sprite::new(wide, 8.0, 8.0));
        let mirrored = corners(&Sprite::new(wide, 8.0, 8.0).scale(-1.0, 1.0));
        for (got, want) in mirrored
            .into_iter()
            .zip([plain[1], plain[0], plain[3], plain[2]])
        {
            assert_eq!(bits(got), bits(want));
        }
    }

    /// A flip swaps the two UV edges of its axis and touches nothing
    /// else: the placement lanes, the other axis and the tint are the
    /// unflipped record's, bit for bit.
    ///
    /// Probed by having `mirrored` return its inputs: both flipped
    /// records equal the plain one and the swap assertions red.
    #[test]
    fn a_flip_swaps_the_uv_edges_and_nothing_else() {
        let c = canvas(64, 32);
        let atlas = Extent {
            width: 8,
            height: 8,
        };
        let plain = lanes(&pack(&swatch(), c, atlas));
        let across = lanes(&pack(&swatch().flip_x(true), c, atlas));
        let down = lanes(&pack(&swatch().flip_y(true), c, atlas));
        // Lanes 8 and 10 are the two u edges; 9 and 11 the two v edges.
        assert_eq!(
            (across[8], across[10]),
            (plain[10], plain[8]),
            "flip_x swaps u"
        );
        assert_eq!(
            (across[9], across[11]),
            (plain[9], plain[11]),
            "flip_x left v alone"
        );
        assert_eq!((down[9], down[11]), (plain[11], plain[9]), "flip_y swaps v");
        assert_eq!(
            (down[8], down[10]),
            (plain[8], plain[10]),
            "flip_y left u alone"
        );
        for lane in (0..8).chain(12..16) {
            assert_eq!(across[lane], plain[lane], "flip_x moved lane {lane}");
            assert_eq!(down[lane], plain[lane], "flip_y moved lane {lane}");
        }
        // The swap is its own inverse, and the two axes commute.
        let both = lanes(&pack(&swatch().flip_x(true).flip_y(true), c, atlas));
        let both_reversed = lanes(&pack(&swatch().flip_y(true).flip_x(true), c, atlas));
        assert_eq!(both, both_reversed);
        assert_eq!(
            (both[8], both[10], both[9], both[11]),
            (plain[10], plain[8], plain[11], plain[9])
        );
    }

    /// Over all four combinations of flips: a flip set and then unset
    /// packs the unflipped bytes (the last call wins), and the placement
    /// and tint lanes are the unflipped record's, bit for bit. Four
    /// cases, so a loop rather than a generator — exhaustive and
    /// deterministic where a random draw over four values is neither.
    #[test]
    fn every_flip_combination_leaves_placement_and_tint_alone_and_unsets_cleanly() {
        let c = canvas(64, 32);
        let atlas = Extent {
            width: 8,
            height: 8,
        };
        let plain = pack(&swatch(), c, atlas);
        let plain_lanes = lanes(&plain);
        for (x, y) in [(false, false), (true, false), (false, true), (true, true)] {
            let flipped = lanes(&pack(&swatch().flip_x(x).flip_y(y), c, atlas));
            for lane in (0..8).chain(12..16) {
                assert_eq!(
                    flipped[lane], plain_lanes[lane],
                    "flip ({x}, {y}) moved lane {lane}"
                );
            }
            let unset = pack(
                &swatch().flip_x(x).flip_y(y).flip_x(false).flip_y(false),
                c,
                atlas,
            );
            assert_eq!(
                unset, plain,
                "flip ({x}, {y}) then unset is not the plain record"
            );
        }
    }

    /// Turn `point` by `turns` about `pivot` — the test's own copy of
    /// the map, so the composition property below is checked against
    /// arithmetic the test owns.
    fn turned_about(point: Vec2, pivot: Vec2, turns: f32) -> Vec2 {
        let (s, c) = turn_sin_cos(turns);
        let d = point - pivot;
        pivot + Vec2::new(d.x * c - d.y * s, d.x * s + d.y * c)
    }

    proptest! {
        /// A turn is rigid about its pivot: the four corners keep their
        /// pairwise distances, and the pivot — the point at the sprite's
        /// own fractions along its two edges — stays where it was.
        ///
        /// Bounds are keyed on the magnitudes involved, as the offset
        /// property below learned to: a corner's rounding scales with
        /// the pivot's and the size's magnitudes, not with the answer.
        #[test]
        fn a_turn_is_rigid_about_its_pivot(
            rotation in -1.0f32..1.0,
            px in 0.0f32..1.0,
            py in 0.0f32..1.0,
            x in -500.0f32..500.0,
            y in -500.0f32..500.0,
            w in 1.0f32..300.0,
            h in 1.0f32..300.0,
        ) {
            let plain = Sprite::new(SQUARE, x, y).size(w, h).pivot(px, py);
            let before = corners(&plain);
            let after = corners(&plain.rotation(rotation));
            let bound = (x.abs() + y.abs() + w + h).max(1.0) * f32::EPSILON * 8.0;
            for i in 0..4 {
                for j in (i + 1)..4 {
                    let was = (before[i] - before[j]).length();
                    let is = (after[i] - after[j]).length();
                    prop_assert!(
                        (was - is).abs() <= bound,
                        "corners {i} and {j} were {was} apart and are {is} apart"
                    );
                }
            }
            let pivot = Vec2::new(x + px * w, y + py * h);
            let mapped = after[0] + (after[1] - after[0]) * px + (after[2] - after[0]) * py;
            prop_assert!(
                (mapped - pivot).length() <= bound,
                "the pivot moved by {}",
                (mapped - pivot).length()
            );
        }

        /// Turning by `a` and then by `b` about the same pivot is turning
        /// by `a + b`, within the rounding of the two maps and the two
        /// ulps of the trigonometry.
        #[test]
        fn turns_compose(
            a in -1.0f32..1.0,
            b in -1.0f32..1.0,
            x in -500.0f32..500.0,
            y in -500.0f32..500.0,
            w in 1.0f32..300.0,
            h in 1.0f32..300.0,
        ) {
            let sprite = Sprite::new(SQUARE, x, y).size(w, h);
            let pivot = Vec2::new(x + 0.5 * w, y + 0.5 * h);
            let twice = corners(&sprite.rotation(a)).map(|corner| turned_about(corner, pivot, b));
            let once = corners(&sprite.rotation(a + b));
            let bound = (x.abs() + y.abs() + w + h).max(1.0) * f32::EPSILON * 8.0
                + (w + h) * 2.0 * f32::EPSILON;
            for (p, q) in twice.into_iter().zip(once) {
                prop_assert!((p - q).length() <= bound, "corners differ by {}", (p - q).length());
            }
        }

        /// The turn happens in canvas pixels, before the per-axis ortho
        /// map: on a 640×360 canvas the packed NDC corners, read back
        /// into pixels per axis, still span the sprite's own width and
        /// height. The mutant this exists for turns the NDC corners
        /// instead, which shears every non-square canvas by up to the
        /// aspect ratio — an edge-length error of at least ~0.07 px on
        /// this domain against a bound near 0.002 px.
        ///
        /// The bound is derived from what the test reads: each NDC lane
        /// is `2c/extent - 1` rounded once and carries up to an ulp of a
        /// value near one, which the read-back scales by half the extent
        /// — plus the corner arithmetic's own rounding at the pivot's
        /// magnitude.
        #[test]
        fn rotation_keeps_edge_lengths_on_a_non_square_canvas(
            x in 0.0f32..640.0,
            y in 0.0f32..360.0,
            w in 8.0f32..200.0,
            h in 8.0f32..200.0,
            rotation in 0.02f32..0.48,
        ) {
            let c = canvas(640, 360);
            let atlas = Extent { width: 8, height: 8 };
            let record = pack(&Sprite::new(SQUARE, x, y).size(w, h).rotation(rotation), c, atlas);
            let lane = |index: usize| f32::from_bits(lanes(&record)[index]);
            let (a, b, cc) = (
                Vec2::new(lane(0), lane(1)),
                Vec2::new(lane(2), lane(3)),
                Vec2::new(lane(4), lane(5)),
            );
            let to_px = |v: Vec2| Vec2::new(v.x * 320.0, v.y * 180.0);
            let width = to_px(b - a).length();
            let height = to_px(cc - a).length();
            let bound = (640.0 + 360.0 + x.abs() + y.abs() + w + h) * f32::EPSILON * 8.0;
            prop_assert!((width - w).abs() <= bound, "width {width} for {w} (bound {bound})");
            prop_assert!((height - h).abs() <= bound, "height {height} for {h} (bound {bound})");
        }

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

        let expected: [f32; 16] = [
            -0.5, -0.5, // top-left: 2*16/64-1, 2*8/32-1
            0.5, -0.5, // top-right: 2*48/64-1, 2*8/32-1
            -0.5, 0.5, // bottom-left: 2*16/64-1, 2*24/32-1
            0.5, 0.5, // bottom-right: 2*48/64-1, 2*24/32-1
            0.0, 0.0, // uv at the first corner
            0.5, 0.5, // uv at the last corner: 4/8
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
