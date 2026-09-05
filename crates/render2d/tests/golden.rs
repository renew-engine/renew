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
         *.candidate.rgba and fails; a human inspects the candidate (a .ppm\n\
         is written beside it), renames it to the canonical name, and commits\n\
         it with this sidecar. To refresh: delete the canonical file, rerun\n\
         on the pinned software rasterizer, repeat the ritual.\n",
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
             it with its sidecar. This test never passes until a human does that.",
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
