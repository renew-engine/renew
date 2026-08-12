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
    r: 51.0 / 255.0,
    g: 102.0 / 255.0,
    b: 153.0 / 255.0,
    a: 1.0,
};
const CLEAR_BYTES: [u8; 4] = [51, 102, 153, 255];

/// The 4×4 test atlas, four 2×2 solid regions: opaque red, opaque
/// green, opaque blue, and half-alpha red — premultiplied, as every
/// byte handed to the renderer must be (128 ≈ 0.5·255 in both the
/// color and alpha channels).
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
                (false, false) => [128, 0, 0, 128],
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
    for pixel in pixels.chunks_exact(4) {
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
        TargetFormat::Rgba8Unorm,
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
    renderer.begin();
    renderer.push(&Sprite::new(RED, 8.0, 8.0).size(16.0, 16.0));
    renderer.push(&Sprite::new(GREEN, 32.0, 8.0).size(16.0, 16.0));
    renderer.push(&Sprite::new(BLUE, 16.0, 16.0).size(16.0, 16.0));
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
    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);

    // The expected image, computed by the same painter's algorithm the
    // fill promises: clear, then each sprite's rectangle in push order.
    let mut expected = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        expected.extend_from_slice(&CLEAR_BYTES);
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
        assert_eq!(pixel_at(x, y), CLEAR_BYTES, "corner ({x},{y}) not clear");
    }
    let over_clear = pixel_at(36, 36); // half-red over clear only
    assert_ne!(over_clear, CLEAR_BYTES, "half-alpha sprite left no trace");
    assert_ne!(
        over_clear,
        [128, 0, 0, 128],
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
        panic!(
            "rendered bytes diverge from the golden: first difference at byte {first_diff}, \
             lengths {} vs {}, fnv1a {rendered_hash:#018x} vs {:#018x}; actual written to {}",
            pixels.len(),
            expected.len(),
            fnv1a(&expected),
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
