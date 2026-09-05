//! The sprite renderer's image oracles, in two tiers.
//!
//! **Computed, exact on every adapter:** opaque sprites at texel-aligned
//! positions over solid-color regions. With alpha 1 the premultiplied
//! blend degenerates to replacement (`src·1 + dst·0`), texel-aligned
//! edges land on pixel boundaries, and solid regions make the nearest
//! lookup color-independent of which texel wins — so the expected image
//! is computable in the test and the comparison is byte-exact with no
//! committed artifact. Weaker than the rendering crate's version of the
//! same argument (blending is *enabled* here); the recorded fallback is
//! scoping it to the software rasterizer, and its trigger is the first
//! divergence report — no debate. Scheduled sunset: the move to a
//! linear working space re-decides this test (convert to a committed
//! golden or retire), as the README's Testing section records.
//!
//! **Computed with an edge margin, exact on every adapter:** a turned
//! opaque square whose every pixel centre sits a fifth of a pixel or
//! more from its diagonal edges, so the covered set is an exact count
//! that no rasteriser's sub-pixel snap can move.
//!
//! **Committed, pinned-lane exact:** semi-transparent overlaps proving
//! the premultiplied compositing convention in committed bytes, with
//! the same candidate/provenance ritual as the rendering crate's
//! goldens. Structure everywhere else.

// The tripwire ban on filesystem access protects engine code; the
// golden harness's entire job is comparing against committed artifacts.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use renew_render2d::{AtlasDesc, Canvas, Region, Sprite, SpriteRenderer};
use renew_rhi::{
    AdapterKind, Color, Device, DeviceDesc, DeviceError, Extent, Pass, RenderDesc, TargetFormat,
    Validation,
};

const SIZE: u32 = 64;
/// 51/255, 102/255, 153/255: unambiguous UNORM conversions.
const CLEAR: Color = Color {
    // The light behind the authored bytes 51, 102, 153. The
    // attachment encodes on write, so handing it these stores the
    // authored bytes back and the picture is the one that was chosen.
    // Passing byte/255 would encode a value that is already encoded,
    // which lifts every pixel -- measured at 51 landing on 124.
    r: renew_rhi::srgb::decode(51),
    g: renew_rhi::srgb::decode(102),
    b: renew_rhi::srgb::decode(153),
    a: 1.0,
};
/// The format every target in this file is created with.
///
/// Named once so the expectations below can be a function of it. When the
/// working space changes, this constant moves and every byte derived from
/// it follows — rather than a scatter of literals each of which is wrong
/// in the same way and none of which says why.
const TARGET: TargetFormat = TargetFormat::Rgba8Srgb;

/// What the attachment stores for the clear above.
///
/// Derived rather than written down. Under UNORM this is exactly
/// `[51, 102, 153, 255]`, which is what it always was — an authored byte
/// survives `round(255 x b/255)` unchanged. The point is what happens when
/// the format changes: these bytes follow it, and the corner assertions
/// that read them do not panic before the golden bootstrap path can write
/// a candidate.
#[allow(
    clippy::expect_used,
    reason = "a colour target that stores no colour is the defect"
)]
fn clear_bytes() -> [u8; 4] {
    let channel = |value: f32| TARGET.stores(value).expect("a color target stores color");
    [
        channel(CLEAR.r),
        channel(CLEAR.g),
        channel(CLEAR.b),
        channel(CLEAR.a),
    ]
}

/// The 4×4 test atlas, four 2×2 solid regions: opaque red, opaque
/// green, opaque blue, and half-alpha red — **authored, straight
/// alpha**, as every byte handed to the renderer now is. The half-alpha
/// red is full red at half coverage; the shader does the multiply.
///
/// **The mid-tone is deliberate.** Opaque red, green and blue are all 0s
/// and 255s, and those are the two values every transfer function fixes —
/// a fixture built only from them cannot test a decode at all, which is
/// exactly how this golden watched an entire colour change go past
/// without noticing anything.
const ATLAS_EXTENT: Extent = Extent {
    width: 4,
    height: 4,
};
const RED: Region = Region {
    x: 0,
    y: 0,
    width: 2,
    height: 2,
};
const GREEN: Region = Region {
    x: 2,
    y: 0,
    width: 2,
    height: 2,
};
const BLUE: Region = Region {
    x: 0,
    y: 2,
    width: 2,
    height: 2,
};
const HALF_RED: Region = Region {
    x: 2,
    y: 2,
    width: 2,
    height: 2,
};

fn atlas_bytes() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4u32 {
        for x in 0..4u32 {
            let texel: [u8; 4] = match (x < 2, y < 2) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 0, 0, 128],
            };
            bytes.extend_from_slice(&texel);
        }
    }
    bytes
}

fn strict() -> bool {
    std::env::var_os("RENEW_GOLDEN").is_some_and(|v| v == "1")
}

/// `Ok(None)` is the graceful skip; other failures surface as `Err`
/// for the calling test to unwrap. Under `RENEW_GOLDEN=1` (the CI
/// rendering lane) a skip is a failure, and the validation layer must
/// actually be active — the lane's oracle can never go silently
/// vacuous. Same harness as the rendering crate's golden tests; the
/// copy is deliberate; a third copy is the cue to extract a shared
/// harness.
fn device_or_skip() -> Result<Option<Device>, DeviceError> {
    match Device::new(&DeviceDesc {
        app_name: "renew-render2d-golden-tests",
        validation: Validation::IfAvailable,
    }) {
        Ok(device) => {
            assert!(
                device.validation_active() || !strict(),
                "RENEW_GOLDEN=1 but the validation layer is not active — \
                 the rendering lane's oracle would be vacuous"
            );
            Ok(Some(device))
        }
        Err(DeviceError::LoaderUnavailable { message }) if !strict() => {
            eprintln!("SKIP: no Vulkan runtime: {message}");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn assert_no_validation_errors(device: &Device) {
    let report = device.validation_report();
    assert_eq!(
        report.errors, 0,
        "validation errors; first messages: {:?}",
        report.first_messages
    );
}

fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

/// FNV-1a 64 over a byte buffer: a cheap content fingerprint for
/// forensics lines and sidecars.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Write RGBA8 pixels as a binary PPM (P6, alpha dropped) beside the
/// goldens — the humanly-viewable form of a mismatch or candidate.
fn write_ppm(path: &Path, pixels: &[u8], width: u32, height: u32) -> std::io::Result<()> {
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for pixel in pixels.as_chunks::<4>().0 {
        ppm.extend_from_slice(&pixel[..3]);
    }
    std::fs::write(path, ppm)
}

/// Fallible on purpose: helpers outside `#[test]` bodies carry no
/// panic allowance, so refusals travel as values and the test unwraps.
fn renderer(device: &Device, atlas: &[u8], max_sprites: u32) -> Result<SpriteRenderer, String> {
    let canvas = Canvas::new(SIZE, SIZE).ok_or("zero canvas dimension")?;
    let capacity = core::num::NonZeroU32::new(max_sprites).ok_or("zero sprite capacity")?;
    SpriteRenderer::new(
        device,
        &AtlasDesc::new(ATLAS_EXTENT, atlas),
        canvas,
        TARGET,
        capacity,
    )
    .map_err(|error| error.to_string())
}

/// Paint `color` over a rectangle of the expected image, the same
/// replacement an opaque sprite performs.
fn paint(image: &mut [u8], x: u32, y: u32, width: u32, height: u32, color: [u8; 4]) {
    for row in y..y + height {
        for column in x..x + width {
            let base = ((row * SIZE + column) * 4) as usize;
            image[base..base + 4].copy_from_slice(&color);
        }
    }
}

/// One pixel of a 64×64 readback, by coordinate.
fn pixel_at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let base = ((y * SIZE + x) * 4) as usize;
    [
        pixels[base],
        pixels[base + 1],
        pixels[base + 2],
        pixels[base + 3],
    ]
}

/// Opaque sprites against a computed expected image, byte-exact on
/// every adapter: placement, region selection, fill-order overwrite,
/// and the instance path end to end — plus the same-frame-twice
/// determinism check.
#[test]
fn opaque_sprites_match_the_computed_image_exactly() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let atlas = atlas_bytes();
    let mut renderer = renderer(&device, &atlas, 8).expect("sprite renderer");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");

    // Three sprites, texel-aligned, the third overlapping the first —
    // push order is draw order, so blue wins the overlap.
    //
    // The blue sprite is authored eight pixels LEFT of where the
    // expected image paints it and reaches its place through the
    // renderer's batch offset, so the offset is proved by the same
    // exact oracle as everything else: drop the `set_offset` call and
    // blue lands at (8, 16), which the computed image refuses. A fade
    // past one clamps to exactly one, which keeps every sprite opaque
    // and the oracle exact — the clamp is asserted through the
    // renderer rather than only through the pure function beneath it.
    renderer.begin();
    renderer.set_alpha(2.0);
    assert_eq!(
        renderer.alpha().to_bits(),
        1.0f32.to_bits(),
        "a fade past one must clamp to one"
    );
    renderer.push(&Sprite::new(RED, 8.0, 8.0).size(16.0, 16.0));
    renderer.push(&Sprite::new(GREEN, 32.0, 8.0).size(16.0, 16.0));
    renderer.set_offset(8.0, 0.0);
    assert_eq!(renderer.offset(), (8.0, 0.0), "the offset must read back");
    renderer.push(&Sprite::new(BLUE, 8.0, 16.0).size(16.0, 16.0));
    let color = [renew_rhi::color_attachment(CLEAR)];
    let items = [renderer.item()];
    let passes = [Pass::new(&color, &items)];
    target
        .render(&RenderDesc::new(&passes))
        .expect("sprite render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    // The Debug surface reports counts, never handles.
    let debug = format!("{renderer:?}");
    assert!(debug.contains("sprites: 3"), "unexpected Debug: {debug}");

    // Determinism self-check: the same frame twice is the same bytes.
    target
        .render(&RenderDesc::new(&passes))
        .expect("second sprite render");
    let mut second = vec![0u8; target.byte_len()];
    target.read_back_into(&mut second);
    assert_eq!(pixels, second, "same frame rendered twice diverged");
    // A new fill forgets the offset and the fade along with the
    // sprites: state that outlives the fill that set it is the kind a
    // caller clears exactly once and then cannot find.
    renderer.begin();
    assert_eq!(renderer.offset(), (0.0, 0.0), "begin must reset the offset");
    assert_eq!(
        renderer.alpha().to_bits(),
        1.0f32.to_bits(),
        "begin must reset the fade"
    );
    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);

    // The expected image, computed by the same painter's algorithm the
    // fill promises: clear, then each sprite's rectangle in push order
    // — blue where the offset put it, not where it was authored.
    let mut expected = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        expected.extend_from_slice(&clear_bytes());
    }
    paint(&mut expected, 8, 8, 16, 16, [255, 0, 0, 255]);
    paint(&mut expected, 32, 8, 16, 16, [0, 255, 0, 255]);
    paint(&mut expected, 16, 16, 16, 16, [0, 0, 255, 255]);

    if pixels != expected {
        let first_diff = pixels
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        let pixel = first_diff / 4;
        panic!(
            "rendered bytes diverge from the computed image on adapter {:?}: first \
             difference at byte {first_diff} (pixel {}, {}), fnv1a {:#018x} vs {:#018x}",
            device.adapter(),
            pixel % SIZE as usize,
            pixel / SIZE as usize,
            fnv1a(&pixels),
            fnv1a(&expected)
        );
    }
}

/// A mirrored sprite reads its texels backwards, exactly: the same
/// computed oracle as above, over regions whose texels differ along
/// the flipped axis, so a flip that did nothing — or flipped the
/// wrong axis — paints the wrong colour into a known pixel.
///
/// The atlas's top row is red, red, green, green and its left column
/// red, red, blue, blue; each texel is drawn over exactly four pixels
/// per axis, texel-aligned, so nearest sampling is exact on every
/// adapter. Probed by having `mirrored` return its inputs: the two
/// flipped sprites paint unflipped and the comparison reds.
#[test]
fn a_mirrored_sprite_reads_its_texels_backwards_exactly() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let atlas = atlas_bytes();
    let mut renderer = renderer(&device, &atlas, 8).expect("sprite renderer");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");

    // The top row of the atlas, RED RED GREEN GREEN, four texels wide.
    let top_row = Region {
        x: 0,
        y: 0,
        width: 4,
        height: 2,
    };
    // The left column, RED over BLUE, four texels tall.
    let left_column = Region {
        x: 0,
        y: 0,
        width: 2,
        height: 4,
    };
    renderer.begin();
    renderer.push(&Sprite::new(top_row, 8.0, 8.0).size(16.0, 8.0));
    renderer.push(&Sprite::new(top_row, 8.0, 24.0).size(16.0, 8.0).flip_x(true));
    renderer.push(
        &Sprite::new(left_column, 32.0, 8.0)
            .size(8.0, 16.0)
            .flip_y(true),
    );
    let color = [renew_rhi::color_attachment(CLEAR)];
    let items = [renderer.item()];
    let passes = [Pass::new(&color, &items)];
    target
        .render(&RenderDesc::new(&passes))
        .expect("mirrored render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);

    let mut expected = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        expected.extend_from_slice(&clear_bytes());
    }
    // Unflipped: red on the left, green on the right.
    paint(&mut expected, 8, 8, 8, 8, [255, 0, 0, 255]);
    paint(&mut expected, 16, 8, 8, 8, [0, 255, 0, 255]);
    // Flipped across: green on the left, red on the right.
    paint(&mut expected, 8, 24, 8, 8, [0, 255, 0, 255]);
    paint(&mut expected, 16, 24, 8, 8, [255, 0, 0, 255]);
    // Flipped down: blue on top, red below.
    paint(&mut expected, 32, 8, 8, 8, [0, 0, 255, 255]);
    paint(&mut expected, 32, 16, 8, 8, [255, 0, 0, 255]);

    if pixels != expected {
        let first_diff = pixels
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        let pixel = first_diff / 4;
        panic!(
            "a mirrored sprite diverges from the computed image on adapter {:?}: first \
             difference at byte {first_diff} (pixel {}, {}), fnv1a {:#018x} vs {:#018x}",
            device.adapter(),
            pixel % SIZE as usize,
            pixel / SIZE as usize,
            fnv1a(&pixels),
            fnv1a(&expected)
        );
    }
}

/// Quarter turns, half turns and the geometric mirror of opaque
/// sprites against a computed image, exactly: every corner lands on an
/// integer pixel boundary, so the argument that makes the axis-aligned
/// oracle exact on every adapter makes this one exact too.
///
/// The row region (red, red, green, green) at (8, 8) size (16, 8),
/// turned a quarter turn about its centre (16, 12): the local x axis
/// now points down the screen, so the footprint is x in [12, 20),
/// y in [4, 20) with red on top and green below. The same row at
/// (8, 24) turned a half turn reads green then red, exactly as the
/// double flip does; at (8, 40) scaled by (-1, 1) it reads green then
/// red, exactly as `flip_x` does — the geometric mirror and the sampled
/// one paint the same pixels.
///
/// Probed by flipping the sign of the sine in the corner map: the
/// quarter turn goes anticlockwise, green lands on top, and the
/// comparison reds at the first row of the footprint.
#[test]
fn quarter_turns_half_turns_and_mirrors_match_the_computed_image_exactly() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let atlas = atlas_bytes();
    let mut renderer = renderer(&device, &atlas, 8).expect("sprite renderer");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");

    let top_row = Region {
        x: 0,
        y: 0,
        width: 4,
        height: 2,
    };
    renderer.begin();
    renderer.push(
        &Sprite::new(top_row, 8.0, 8.0)
            .size(16.0, 8.0)
            .rotation(0.25),
    );
    renderer.push(
        &Sprite::new(top_row, 8.0, 24.0)
            .size(16.0, 8.0)
            .rotation(0.5),
    );
    renderer.push(
        &Sprite::new(top_row, 8.0, 40.0)
            .size(16.0, 8.0)
            .scale(-1.0, 1.0),
    );
    let color = [renew_rhi::color_attachment(CLEAR)];
    let items = [renderer.item()];
    let passes = [Pass::new(&color, &items)];
    target
        .render(&RenderDesc::new(&passes))
        .expect("turned render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);

    let mut expected = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        expected.extend_from_slice(&clear_bytes());
    }
    // A quarter turn clockwise: red on top, green below.
    paint(&mut expected, 12, 4, 8, 8, [255, 0, 0, 255]);
    paint(&mut expected, 12, 12, 8, 8, [0, 255, 0, 255]);
    // A half turn: green then red, as the double flip reads.
    paint(&mut expected, 8, 24, 8, 8, [0, 255, 0, 255]);
    paint(&mut expected, 16, 24, 8, 8, [255, 0, 0, 255]);
    // A negative scale: green then red, as `flip_x` reads.
    paint(&mut expected, 8, 40, 8, 8, [0, 255, 0, 255]);
    paint(&mut expected, 16, 40, 8, 8, [255, 0, 0, 255]);

    if pixels != expected {
        let first_diff = pixels
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        let pixel = first_diff / 4;
        panic!(
            "a turned sprite diverges from the computed image on adapter {:?}: first \
             difference at byte {first_diff} (pixel {}, {}), fnv1a {:#018x} vs {:#018x}",
            device.adapter(),
            pixel % SIZE as usize,
            pixel / SIZE as usize,
            fnv1a(&pixels),
            fnv1a(&expected)
        );
    }
}

/// A diagonal turn stays inside its box and keeps its centre, with an
/// exact pixel count on every adapter: a 16×16 red square at (16, 16)
/// turned an eighth of a turn about (24, 24) is a diamond whose edges
/// sit 8·√2 ≈ 11.31 from the centre along the axes. A pixel centre
/// (i + ½, j + ½) is covered when |i − 23.5| + |j − 23.5| ≤ 11.31; both
/// terms are half-integers, so the sum is an integer at most 11, and
/// per quadrant that is the sum of (11 − m) for m in 0..=10, which is
/// 66 — 264 pixels in all. The nearest pixel centre to any edge is
/// 0.22 px away, more than three times the coarsest sub-pixel snap
/// Vulkan permits (four bits, a sixteenth of a pixel), so no adapter
/// can disagree about a single one of them.
#[test]
fn a_diagonal_turn_stays_inside_its_box_and_keeps_its_centre() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let atlas = atlas_bytes();
    let mut renderer = renderer(&device, &atlas, 8).expect("sprite renderer");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");

    renderer.begin();
    renderer.push(
        &Sprite::new(RED, 16.0, 16.0)
            .size(16.0, 16.0)
            .rotation(0.125),
    );
    let color = [renew_rhi::color_attachment(CLEAR)];
    let items = [renderer.item()];
    let passes = [Pass::new(&color, &items)];
    target
        .render(&RenderDesc::new(&passes))
        .expect("diagonal render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);

    let pixel_at = |x: u32, y: u32| {
        let base = ((y * SIZE + x) * 4) as usize;
        [
            pixels[base],
            pixels[base + 1],
            pixels[base + 2],
            pixels[base + 3],
        ]
    };
    assert_eq!(pixel_at(24, 24), [255, 0, 0, 255], "the centre is red");
    for (x, y) in [(16, 16), (31, 16), (16, 31), (31, 31)] {
        assert_eq!(
            pixel_at(x, y),
            clear_bytes(),
            "box corner ({x}, {y}) is clear"
        );
    }
    let mut covered = 0u32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let pixel = pixel_at(x, y);
            if pixel != clear_bytes() {
                assert_eq!(
                    pixel,
                    [255, 0, 0, 255],
                    "pixel ({x}, {y}) is neither clear nor red"
                );
                covered += 1;
                let inside =
                    (f64::from(x) - 23.5).abs() + (f64::from(y) - 23.5).abs() <= 8.0 * 2f64.sqrt();
                assert!(inside, "pixel ({x}, {y}) is red outside the diamond");
            }
        }
    }
    assert_eq!(covered, 264, "the diamond covers exactly 264 pixel centres");
}

/// One 64×64 target rendered from `push`, cleared to `clear`, read back.
///
/// The six effect oracles below differ only in what they push and what
/// they expect, so the bring-up, the pass and the readback live here
/// once. Returns `None` when there is no device, which is the same
/// graceful skip every test in this file takes.
#[allow(
    clippy::expect_used,
    reason = "a device that cannot bring up a target, or a renderer that cannot be built, \
              is the defect this file reports -- every test below takes the same position"
)]
fn rendered(clear: Color, push: impl FnOnce(&mut SpriteRenderer)) -> Option<(Device, Vec<u8>)> {
    let device = device_or_skip().expect("device bring-up")?;
    let atlas = atlas_bytes();
    let mut renderer = renderer(&device, &atlas, 8).expect("sprite renderer");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");
    renderer.begin();
    push(&mut renderer);
    let color = [renew_rhi::color_attachment(clear)];
    let items = [renderer.item()];
    let passes = [Pass::new(&color, &items)];
    target
        .render(&RenderDesc::new(&passes))
        .expect("sprite render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);
    Some((device, pixels))
}

/// **Additive light needs no second pipeline**, and this is the proof:
/// a tint whose alpha is zero adds the sprite's colour and occludes
/// nothing.
///
/// The blend is premultiplied `src + dst·(1 − α_src)`. With `α_src = 0`
/// that is `src + dst` for colour and `α_dst` for alpha — addition, and
/// no occlusion, out of the one pipeline this crate builds.
///
/// **Computed-exact on every adapter, not within a tolerance**, and the
/// fixture is chosen to make that true: the clear is pure green and the
/// sprite pure red, so every channel of every expectation is 0 or 1 in
/// linear light. Those are the two fixed points of the transfer
/// function — `encode(0) = 0` and `encode(1) = 255` — so no rounding
/// can move a byte, whatever the hardware's encode does in between.
///
/// Probed by setting the tint's alpha to `1.0`: the blend becomes
/// replacement, the interior reads `[255, 0, 0, 255]` instead of
/// yellow, and this test reds while the rest of the file stays green.
#[test]
fn additive_light_is_a_tint_with_no_alpha() {
    let green = Color {
        r: 0.0,
        g: 1.0,
        b: 0.0,
        a: 1.0,
    };
    let Some((_device, pixels)) = rendered(green, |renderer| {
        renderer.push(
            &Sprite::new(RED, 8.0, 8.0)
                .size(16.0, 16.0)
                .tint([1.0, 0.0, 0.0, 0.0]),
        );
    }) else {
        return;
    };

    for y in 0..SIZE {
        for x in 0..SIZE {
            let inside = (8..24).contains(&x) && (8..24).contains(&y);
            let expected = if inside {
                // dst + src: green plus red is yellow, and the alpha is
                // the destination's because the source contributed none.
                [255, 255, 0, 255]
            } else {
                [0, 255, 0, 255]
            };
            assert_eq!(
                pixel_at(&pixels, x, y),
                expected,
                "additive light at ({x},{y}), inside={inside}"
            );
        }
    }
}

/// Light stacks and never occludes: two additive sprites overlapping
/// are brighter where they meet, and nothing they cover goes dark.
///
/// Structural rather than exact, and within one code, because `0.25` is
/// not a fixed point of the transfer function — the hardware encodes it
/// and the byte is one of two neighbours. What the test pins is the
/// three claims that matter: a single sprite stores what `0.25` stores,
/// the overlap is strictly brighter than either sprite alone, and alpha
/// is untouched everywhere, which is what "never occludes" means.
#[test]
fn light_stacks_and_never_occludes() {
    let black = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    let Some((_device, pixels)) = rendered(black, |renderer| {
        renderer.push(
            &Sprite::new(RED, 8.0, 8.0)
                .size(16.0, 16.0)
                .tint([0.25, 0.0, 0.0, 0.0]),
        );
        renderer.push(
            &Sprite::new(RED, 16.0, 8.0)
                .size(16.0, 16.0)
                .tint([0.25, 0.0, 0.0, 0.0]),
        );
    }) else {
        return;
    };

    let stored = TARGET.stores(0.25).expect("a color target stores color");
    let single = pixel_at(&pixels, 10, 12);
    let overlap = pixel_at(&pixels, 20, 12);
    assert!(
        single[0].abs_diff(stored) <= 1,
        "one light should store {stored}, read {}",
        single[0]
    );
    assert!(
        overlap[0] > single[0],
        "two lights ({}) should exceed one ({})",
        overlap[0],
        single[0]
    );
    for y in 0..SIZE {
        for x in 0..SIZE {
            assert_eq!(
                pixel_at(&pixels, x, y)[3],
                255,
                "light must not touch alpha at ({x},{y})"
            );
        }
    }
}

/// Desaturating to zero lands on grey at the luminance it replaced —
/// not on black, which is what a uniform tint would have given.
///
/// Structural within one code: `0.2126` is not a fixed point of the
/// transfer function, so the byte is one of two neighbours. The three
/// claims are that the channels are equal (it is grey at all), that the
/// value is red's luminance rather than red's brightness or nothing,
/// and that alpha is untouched.
#[test]
fn desaturating_to_zero_lands_on_grey() {
    let Some((_device, pixels)) = rendered(CLEAR, |renderer| {
        renderer.push(&Sprite::new(RED, 8.0, 8.0).size(16.0, 16.0).saturation(0.0));
        // Green and blue too: red alone pins one of the three weights,
        // and a shader that dropped or transposed the other two would
        // pass on a red fixture forever.
        renderer.push(
            &Sprite::new(GREEN, 32.0, 8.0)
                .size(16.0, 16.0)
                .saturation(0.0),
        );
        renderer.push(
            &Sprite::new(BLUE, 8.0, 32.0)
                .size(16.0, 16.0)
                .saturation(0.0),
        );
    }) else {
        return;
    };

    // One weight per primary, so all three are pinned. A shader that
    // dropped the blue term, or transposed two of them, passes a
    // red-only fixture forever.
    for (weight, x0, y0, name) in [
        (0.2126f32, 10, 10, "red"),
        (0.7152, 34, 10, "green"),
        (0.0722, 10, 34, "blue"),
    ] {
        let want = TARGET.stores(weight).expect("a color target stores color");
        for y in y0..(y0 + 12) {
            for x in x0..(x0 + 12) {
                let pixel = pixel_at(&pixels, x, y);
                assert_eq!(
                    (pixel[0], pixel[1]),
                    (pixel[1], pixel[2]),
                    "a grey pixel has equal channels at ({x},{y}): {pixel:?}"
                );
                assert!(
                    pixel[0].abs_diff(want) <= 1,
                    "grey should store {name}'s luminance {want}, read {} at ({x},{y})",
                    pixel[0]
                );
                assert_eq!(pixel[3], 255, "the sprite stays opaque at ({x},{y})");
            }
        }
    }
    assert_eq!(
        pixel_at(&pixels, 2, 2),
        clear_bytes(),
        "the clear is untouched outside the sprite"
    );
}

/// A full flash on an opaque texel is white, exactly.
///
/// Exact on every adapter for the same reason the additive oracle is:
/// the flash target is the premultiplied alpha, which is `1` here, and
/// the correction is multiplied by `1.0`, so every channel lands on `1`
/// — a fixed point of the transfer function. `255` is `255` on any
/// hardware that encodes at all.
#[test]
fn a_full_flash_on_an_opaque_texel_is_white() {
    let Some((_device, pixels)) = rendered(CLEAR, |renderer| {
        renderer.push(&Sprite::new(RED, 8.0, 8.0).size(16.0, 16.0).flash(1.0));
    }) else {
        return;
    };

    for y in 10..22 {
        for x in 10..22 {
            assert_eq!(
                pixel_at(&pixels, x, y),
                [255, 255, 255, 255],
                "a full flash is white at ({x},{y})"
            );
        }
    }
}

/// A full flash on a half-transparent texel stays half-transparent: the
/// flash target is the sprite's own alpha, not white.
///
/// **This test exists because a probe found nothing else catching it.**
/// Replacing `vec3(premultiplied.a)` with `vec3(1.0)` in the fragment
/// stage left all eleven other tests in this file green, because every
/// one of them flashes an opaque texel, where the two forms agree. On
/// `HALF_RED` they do not: flashing toward the alpha gives a
/// premultiplied grey at that alpha, and flashing toward white gives
/// `rgb = 1` with `alpha = 0.5` — a premultiplied colour brighter than
/// its own coverage, which is not a colour at all and which reads as a
/// halo around anything fading out.
///
/// Over an opaque black clear the composite is just the source, so the
/// expectation is the alpha itself encoded once. Within one code,
/// because `128/255` is not a fixed point of the transfer function.
#[test]
fn a_full_flash_on_a_half_transparent_texel_stays_half_transparent() {
    let black = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    let Some((_device, pixels)) = rendered(black, |renderer| {
        renderer.push(&Sprite::new(HALF_RED, 8.0, 8.0).size(16.0, 16.0).flash(1.0));
    }) else {
        return;
    };

    // The atlas texel's alpha, which the flash pushes every channel to.
    let alpha = 128.0 / 255.0;
    let want = TARGET.stores(alpha).expect("a color target stores color");
    for y in 10..22 {
        for x in 10..22 {
            let pixel = pixel_at(&pixels, x, y);
            assert_eq!(
                (pixel[0], pixel[1]),
                (pixel[1], pixel[2]),
                "a flashed texel is grey at ({x},{y}): {pixel:?}"
            );
            assert!(
                pixel[0].abs_diff(want) <= 1,
                "the flash should reach the texel's own alpha ({want}), not white; \
                 read {} at ({x},{y})",
                pixel[0]
            );
        }
    }
}

/// A flash fades with the sprite: the tint multiplies *after* the
/// flash, so a half-faded white flash is half white and half whatever
/// is behind it — not a solid white square that ignores the fade.
///
/// This is the claim that makes a flash usable on something dying: were
/// the flash applied after the tint, a fading sprite would flash at full
/// strength to its last frame.
///
/// **The background has to be coloured for this test to mean anything.**
/// Over black, a tint of `[0.5, 0.5, 0.5, 0.5]` and one of
/// `[0.5, 0.5, 0.5, 1.0]` composite to the same bytes — the premultiplied
/// blend adds `dst·(1 − α)` and `dst` is zero — so a fade that never
/// reached alpha would pass. Over the file's own clear the two differ:
/// half the background survives, and the expectation below is a function
/// of the clear rather than a constant.
#[test]
fn a_flash_fades_with_the_sprite() {
    let Some((_device, pixels)) = rendered(CLEAR, |renderer| {
        renderer.push(
            &Sprite::new(RED, 8.0, 8.0)
                .size(16.0, 16.0)
                .flash(1.0)
                .tint([0.5, 0.5, 0.5, 0.5]),
        );
    }) else {
        return;
    };

    // src is premultiplied white at half opacity, so the composite is
    // `0.5 + 0.5·clear` per channel in linear light, encoded once by the
    // attachment. Within one code because none of these values is a
    // fixed point of the transfer function.
    let expected = |clear: f32| {
        TARGET
            .stores(0.5f32.mul_add(clear, 0.5))
            .expect("a color target stores color")
    };
    let want = [expected(CLEAR.r), expected(CLEAR.g), expected(CLEAR.b)];
    for y in 10..22 {
        for x in 10..22 {
            let pixel = pixel_at(&pixels, x, y);
            for channel in 0..3 {
                assert!(
                    pixel[channel].abs_diff(want[channel]) <= 1,
                    "channel {channel} at ({x},{y}) should store {} for a half-faded \
                     flash over the clear, read {}",
                    want[channel],
                    pixel[channel]
                );
            }
            assert_eq!(
                pixel[3], 255,
                "the clear is opaque, so the composite is opaque at ({x},{y})"
            );
        }
    }
    // And the fade is visible as a fade: an unfaded flash is white.
    assert_ne!(
        pixel_at(&pixels, 12, 12),
        [255, 255, 255, 255],
        "a half-faded flash must not reach white"
    );
    let black = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    // And the flash happens BEFORE the tint, which the fixture above
    // cannot see: with a uniform tint, applying the tint first and
    // flashing toward the tinted alpha reaches the same bytes. A tint
    // that reduces coverage without dimming colour separates them —
    // flash-then-tint keeps the flash white and merely makes it cover
    // less, while tint-then-flash would drag the colour down to the
    // tint's alpha.
    let Some((_device, pixels)) = rendered(black, |renderer| {
        renderer.push(
            &Sprite::new(RED, 8.0, 8.0)
                .size(16.0, 16.0)
                .flash(1.0)
                .tint([1.0, 1.0, 1.0, 0.5]),
        );
    }) else {
        return;
    };
    for y in 10..22 {
        for x in 10..22 {
            let pixel = pixel_at(&pixels, x, y);
            assert_eq!(
                [pixel[0], pixel[1], pixel[2]],
                [255, 255, 255],
                "the flash must run before the tint, so a coverage-only tint leaves it \
                 white at ({x},{y})"
            );
        }
    }
}

/// A smear leaves the middle of the sprite exactly where it was.
///
/// The eight taps are averaged, and eight identical opaque samples
/// average back to themselves **exactly** — every step of the tree is a
/// power-of-two scale, `t + t = 2t` through `8t · 0.125 = t`, so no
/// rounding enters and no adapter can disagree. That is the claim this
/// test makes, and it is what says a smear blurs the edges of a moving
/// sprite without touching the art in the middle of it.
///
/// **The geometry is chosen so the claim is provable, not approximate.**
/// A two-pixel smear puts the eight taps within a pixel either side of
/// the fragment, so every pixel two or more pixels inside the sprite has
/// all eight taps in the source region. The margin is 1.5 canvas pixels
/// where the taps sit `2/7` of a pixel apart, and the source's texels
/// are eight canvas pixels wide — so no tap lands anywhere near a texel
/// boundary and the nearest lookup returns the same opaque red eight
/// times over.
///
/// Probed by dropping one tap from the sum and keeping the eighth-scale:
/// the interior falls to seven eighths of the sprite's own opacity, the
/// clear shows through, and this test reds.
#[test]
fn interior_pixels_of_a_smeared_sprite_are_exact() {
    let Some((_device, pixels)) = rendered(CLEAR, |renderer| {
        renderer.push(
            &Sprite::new(RED, 16.0, 16.0)
                .size(16.0, 16.0)
                .smear(2.0, 0.0),
        );
    }) else {
        return;
    };

    for y in 18..30 {
        for x in 18..30 {
            assert_eq!(
                pixel_at(&pixels, x, y),
                [255, 0, 0, 255],
                "the smeared interior at ({x},{y}) must be the texel it always was"
            );
        }
    }
}

/// The smear's band fades outward, punches no hole, and never reads a
/// neighbour's art.
///
/// Eight pixels of smear on a sixteen-pixel sprite: the footprint grows
/// four pixels each side, and across that band fewer and fewer of the
/// eight taps land in the source, so the average fades. Three claims,
/// each pinned along the sprite's middle row:
///
/// - **It fades, monotonically.** The tap window slides out of the
///   source a tap at a time, so the red channel never rises as you walk
///   away from the sprite's centre in either direction — through the
///   band and out past the footprint into the clear.
/// - **It punches no hole.** Alpha stays 255 across the whole row. The
///   premultiplied blend leaves `α_dst` where the source contributes
///   none, so a partly-covered band cannot make an opaque target
///   transparent.
/// - **It reads no neighbour.** The green channel never rises above the
///   clear's own green anywhere on the row. Green can only *fall* here,
///   as the red sprite covers the background — so a green byte above
///   the clear's could only have come out of the atlas, where the green
///   texels sit directly beside the red ones with no gutter between
///   them. That is the bounds mask's whole job: a tap that leaves the
///   source counts as transparent instead of clamping onto whatever is
///   next door.
///
/// Structural rather than byte-exact, because the ramp's values are not
/// fixed points of the transfer function. The band's bytes are pinned by
/// the sample's committed picture of a diving bird instead.
///
/// Probed by clamping outside taps to the source's edge instead of
/// counting them as zero: the right-hand band reads the green texels and
/// the green claim reds.
#[test]
fn the_smear_band_falls_off_and_reads_no_neighbour() {
    // The sprite's middle row: 16 to 32 vertically, so y = 24 is solidly
    // inside and the only thing varying along it is the horizontal
    // smear.
    const ROW: u32 = 24;
    // The plateau: with a four-pixel reach each way, every tap is inside
    // the source between x = 20 and x = 27, so the walk outward starts
    // in the middle of it.
    const CENTRE: u32 = 24;

    let Some((_device, pixels)) = rendered(CLEAR, |renderer| {
        renderer.push(
            &Sprite::new(RED, 16.0, 16.0)
                .size(16.0, 16.0)
                .smear(8.0, 0.0),
        );
    }) else {
        return;
    };

    let clear = clear_bytes();
    let red = |x: u32| pixel_at(&pixels, x, ROW)[0];

    for x in 0..SIZE {
        let pixel = pixel_at(&pixels, x, ROW);
        assert_eq!(
            pixel[3], 255,
            "a partly covered band must not make an opaque target transparent, at x={x}"
        );
        assert!(
            pixel[1] <= clear[1],
            "green rose to {} above the clear's {} at x={x}, which could only have \
             come from the atlas",
            pixel[1],
            clear[1]
        );
    }

    for x in CENTRE..(SIZE - 1) {
        assert!(
            red(x) >= red(x + 1),
            "red rose from {} at x={x} to {} at x={} walking right",
            red(x),
            red(x + 1),
            x + 1
        );
    }
    for x in (1..=CENTRE).rev() {
        assert!(
            red(x) >= red(x - 1),
            "red rose from {} at x={x} to {} at x={} walking left",
            red(x),
            red(x - 1),
            x - 1
        );
    }

    // Past the footprint, nothing was drawn at all. The quad spans
    // x = 12 to x = 36, so it covers the pixels whose centres fall
    // inside that — columns 12 through 35 — and the assertion runs right
    // up to both edges rather than leaving a margin: a footprint one
    // pixel wider than the extension calls for reddens here.
    for x in (0..12).chain(36..SIZE) {
        assert_eq!(
            pixel_at(&pixels, x, ROW),
            clear,
            "the smear must not reach x={x}"
        );
    }
}
/// Semi-transparent overlaps against the committed golden: the
/// premultiplied compositing convention, in bytes, on the pinned lane —
/// structure everywhere else.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "render, structural checks, and the bootstrap ritual are one narrative"
)]
fn blended_sprites_match_structure_and_the_committed_golden() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let atlas = atlas_bytes();
    let mut renderer = renderer(&device, &atlas, 8).expect("sprite renderer");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");

    // An opaque base, a half-alpha region over part of it and part of
    // the clear, and a tinted-translucent sprite alone over the clear:
    // every compositing case the convention has, in one image.
    renderer.begin();
    renderer.push(&Sprite::new(RED, 8.0, 8.0).size(24.0, 24.0));
    renderer.push(&Sprite::new(HALF_RED, 16.0, 16.0).size(24.0, 24.0));
    renderer.push(
        &Sprite::new(GREEN, 32.0, 40.0)
            .size(16.0, 16.0)
            .tint([0.5, 0.5, 0.5, 0.5]),
    );
    let color = [renew_rhi::color_attachment(CLEAR)];
    let items = [renderer.item()];
    let passes = [Pass::new(&color, &items)];
    target
        .render(&RenderDesc::new(&passes))
        .expect("blended render");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    target
        .render(&RenderDesc::new(&passes))
        .expect("second blended render");
    let mut second = vec![0u8; target.byte_len()];
    target.read_back_into(&mut second);
    assert_eq!(pixels, second, "same frame rendered twice diverged");
    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);

    // Structure, adapter-independent: corners hold the exact clear
    // conversion; a pixel under only the half-alpha sprite is neither
    // the clear nor the raw region color, and it is opaque (dst alpha
    // was 1, so composited alpha is 1).
    let pixel_at = |x: u32, y: u32| {
        let base = ((y * SIZE + x) * 4) as usize;
        [
            pixels[base],
            pixels[base + 1],
            pixels[base + 2],
            pixels[base + 3],
        ]
    };
    for (x, y) in [(0, 0), (SIZE - 1, 0), (0, SIZE - 1), (SIZE - 1, SIZE - 1)] {
        assert_eq!(pixel_at(x, y), clear_bytes(), "corner ({x},{y}) not clear");
    }
    let over_clear = pixel_at(36, 36); // half-red over clear only
    assert_ne!(over_clear, clear_bytes(), "half-alpha sprite left no trace");
    assert_ne!(
        over_clear,
        [255, 0, 0, 128],
        "half-alpha replaced instead of blending"
    );
    assert_eq!(
        over_clear[3], 255,
        "compositing over an opaque target must stay opaque"
    );

    // Exact comparison only on the strict lane, whose stack is the
    // pinned toolchain the golden's bytes attest.
    let adapter = device.adapter();
    if adapter.kind != AdapterKind::SoftwareRasterizer {
        assert!(
            !strict(),
            "RENEW_GOLDEN=1 but the selected adapter is {:?} ({}) — the \
             rendering lane must run on the pinned software rasterizer",
            adapter.kind,
            adapter.name
        );
        eprintln!(
            "SKIP exact-golden: adapter {:?} ({}) is not a software rasterizer",
            adapter.kind, adapter.name
        );
        return;
    }
    if !strict() {
        eprintln!(
            "SKIP exact-golden: software rasterizer {} outside the pinned lane \
             (set RENEW_GOLDEN=1 only where the stack matches the golden's provenance)",
            adapter.name
        );
        return;
    }

    let dir = goldens_dir();
    let golden = dir.join("sprites-blend-64x64.rgba");
    let rendered_hash = fnv1a(&pixels);
    let provenance = format!(
        "sprites-blend-64x64.rgba — RGBA8, tightly packed, row-major, {SIZE}x{SIZE}\n\
         fnv1a-64 of the pixel bytes: {rendered_hash:#018x}\n\
         rendered by: {} (kind {:?}, vendor {:#06x}, device {:#06x}, driver {})\n\
         shaders: crates/render2d/shaders (see its compile record)\n\
         ritual: the test never writes the canonical file above — it writes\n\
         *.candidate.rgba and fails; an inspector — a person, or a session\n\
         that records on the pull request what it inspected — renames the\n\
         candidate to the canonical name (a .ppm is written beside it) and\n\
         commits it with this sidecar. To refresh: delete the canonical\n\
         file, rerun on the pinned software rasterizer, repeat the ritual.\n",
        adapter.name, adapter.kind, adapter.vendor_id, adapter.device_id, adapter.driver_version
    );

    if !golden.exists() {
        std::fs::create_dir_all(&dir).expect("create goldens dir");
        let candidate = dir.join("sprites-blend-64x64.candidate.rgba");
        std::fs::write(&candidate, &pixels).expect("write golden candidate");
        write_ppm(
            &dir.join("sprites-blend-64x64.candidate.ppm"),
            &pixels,
            SIZE,
            SIZE,
        )
        .expect("write candidate ppm");
        std::fs::write(dir.join("sprites-blend-64x64.provenance.txt"), provenance)
            .expect("write provenance sidecar");
        panic!(
            "golden is missing; candidate written to {} (fnv1a {rendered_hash:#018x}) — \
             inspect the .ppm, rename the candidate to the canonical name, and commit \
             it with its sidecar. This test never passes until an inspector does \
             that — a person, or a session that records on the pull request what \
             it inspected.",
            candidate.display()
        );
    }

    let expected = std::fs::read(&golden).expect("read committed golden");
    if pixels != expected {
        let actual = dir.join("sprites-blend-64x64.actual.rgba");
        std::fs::write(&actual, &pixels).expect("write actual for diffing");
        write_ppm(
            &dir.join("sprites-blend-64x64.actual.ppm"),
            &pixels,
            SIZE,
            SIZE,
        )
        .expect("write actual ppm");
        let first_diff = pixels
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        // **The renderer belongs in this message**, and its absence cost a
        // day. A divergence here is either the change under test or the
        // machine under it, and those need opposite responses: the first
        // is a bug to fix, the second is a golden that cannot gate
        // anything. This message used to give offsets, lengths and hashes
        // - everything about the bytes, nothing about what produced them
        // - so an investigation that should have begun by comparing this
        // string against the committed provenance sidecar instead began
        // by re-reading a diff that touched no rendering code at all.
        panic!(
            "rendered bytes diverge from the golden: first difference at byte {first_diff}, \
             lengths {} vs {}, fnv1a {rendered_hash:#018x} vs {:#018x}; rendered by {} ({:?}); \
             actual written to {}. If that renderer differs from the one named in the \
             provenance sidecar beside the golden, this is the machine and not the change.",
            pixels.len(),
            expected.len(),
            fnv1a(&expected),
            device.adapter().name,
            device.adapter().kind,
            actual.display()
        );
    }
    // Nothing is written here. A passing comparison writes no file at all —
    // the sidecar is a committed artifact, and a test that rewrites one on
    // success has a side effect a test may not have: run the suite on a
    // machine whose adapter differs and the committed provenance quietly
    // starts claiming that machine, with the change staged by nobody.
    //
    // The sidecar is authored by the bootstrap path above, which runs only
    // when the canonical image is absent and which fails rather than passing.
    // That is the one moment a human is already looking.
}

/// The capacity refusal fires by name — the retained assertion, caught
/// where tests are allowed to catch panics.
#[test]
fn pushing_past_capacity_is_refused_by_name() {
    let Some(device) = device_or_skip().expect("device bring-up") else {
        return;
    };
    let atlas = atlas_bytes();
    let mut renderer = renderer(&device, &atlas, 2).expect("sprite renderer");
    assert_eq!(renderer.max_sprites(), 2, "capacity must report as fixed");
    renderer.begin();
    renderer.push(&Sprite::new(RED, 0.0, 0.0));
    renderer.push(&Sprite::new(GREEN, 8.0, 0.0));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        renderer.push(&Sprite::new(BLUE, 16.0, 0.0));
    }));
    let message = match result {
        Ok(()) => panic!("a third push into capacity 2 was accepted"),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
            .unwrap_or_default(),
    };
    assert!(
        message.contains("sprite capacity 2 exceeded"),
        "the refusal must name the capacity; got: {message}"
    );
}
