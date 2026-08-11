//! Mechanical enforcement of the fill-and-render allocation contract:
//! after warmup, a steady-state sprite frame — begin, push, compose,
//! render, read back — performs no heap allocation through the Rust
//! global allocator. The pass composition is stack arrays, so it lives
//! inside the measured window with the render it feeds.
//!
//! One test in this file, deliberately: the `#[global_allocator]` is
//! process-wide, and a second test would race the counters.
//!
//! The measured windows are proven live before they count: every frame
//! pushes real sprites and the read-back is checked for a sprite-covered
//! pixel at both ends of every window — a gate over an empty batch
//! would pass vacuously the moment the fill path allocated.

use renew_memory::{CountingAllocator, counters};
use renew_render2d::{AtlasDesc, Canvas, Region, Sprite, SpriteRenderer};
use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, Pass, RenderDesc, TargetFormat, Validation,
};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SIZE: u32 = 64;
/// Per-frame variation without per-frame allocation: positions read
/// from a fixed table, so the packed bytes differ frame to frame and
/// the copy path cannot be skipped by a caching driver.
const WANDER: [f32; 4] = [24.0, 28.0, 32.0, 36.0];
/// The clear's exact bytes; the conversion is unambiguous by choice of
/// channel values, so any adapter must land on them.
const CLEAR_BYTES: [u8; 4] = [51, 102, 153, 255];
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

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
#[expect(
    clippy::too_many_lines,
    reason = "fixtures, warmup, premise checks, and the measured windows are one protocol"
)]
fn steady_state_fill_and_render_allocates_nothing() {
    let device = match Device::new(&DeviceDesc {
        app_name: "renew-render2d-zero-alloc",
        validation: Validation::Off,
    }) {
        Ok(device) => device,
        Err(DeviceError::LoaderUnavailable { message })
            if std::env::var_os("RENEW_GOLDEN").is_none_or(|v| v != "1") =>
        {
            eprintln!("SKIP: no Vulkan runtime: {message}");
            return;
        }
        Err(error) => panic!("device bring-up failed: {error}"),
    };

    // Everything that is allowed to allocate happens here, outside the
    // window: atlas upload, pipeline, per-frame buffer, scratch, and
    // the read-back buffer.
    let mut atlas = Vec::with_capacity(4 * 4 * 4);
    for index in 0..16u32 {
        let texel: [u8; 4] = if index % 4 < 2 {
            [255, 0, 0, 255]
        } else {
            [0, 255, 0, 255]
        };
        atlas.extend_from_slice(&texel);
    }
    let mut renderer = SpriteRenderer::new(
        &device,
        &AtlasDesc::new(
            Extent {
                width: 4,
                height: 4,
            },
            &atlas,
        ),
        Canvas::new(SIZE, SIZE).expect("nonzero canvas"),
        TargetFormat::Rgba8Unorm,
        core::num::NonZeroU32::new(16).expect("nonzero capacity"),
    )
    .expect("sprite renderer");
    let mut target = device
        .create_offscreen_target(Extent {
            width: SIZE,
            height: SIZE,
        })
        .expect("offscreen target");
    // 51/255, 102/255, 153/255: unambiguous UNORM conversions, so the
    // liveness checks below can demand exact bytes on any adapter.
    let clear = Color::new(51.0 / 255.0, 102.0 / 255.0, 153.0 / 255.0, 1.0);
    let mut pixels = vec![0u8; target.byte_len()];

    // The premise assertions the vacuity lesson requires. The fixed red
    // sprite proves a frame drew; the WANDERING green sprite proves the
    // frame that drew is THIS frame — its bytes differ frame to frame,
    // so a stale image from a skipped render shows green in the wrong
    // place. The third check pins a pixel every wander position leaves
    // clear, so green cannot simply be everywhere.
    let pixel_at = |pixels: &[u8], x: u32, y: u32| {
        let base = ((y * SIZE + x) * 4) as usize;
        [
            pixels[base],
            pixels[base + 1],
            pixels[base + 2],
            pixels[base + 3],
        ]
    };
    let assert_frame_is_live = |pixels: &[u8], index: usize, when: &str| {
        assert_eq!(
            pixel_at(pixels, 12, 12),
            [255, 0, 0, 255],
            "{when}: the fixed sprite never drew — the gate would measure a blank frame"
        );
        let wander_x = WANDER[index % WANDER.len()];
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the wander table holds small positive integers"
        )]
        let sample_x = wander_x as u32 + 4;
        assert_eq!(
            pixel_at(pixels, sample_x, 44),
            [0, 255, 0, 255],
            "{when}: the wandering sprite is not where THIS frame put it —              the read-back is not this frame's image"
        );
        assert_eq!(
            pixel_at(pixels, 20, 44),
            CLEAR_BYTES,
            "{when}: a pixel every wander position leaves clear is not clear"
        );
    };
    let frame = |renderer: &mut SpriteRenderer,
                 target: &mut renew_rhi::OffscreenTarget,
                 pixels: &mut Vec<u8>,
                 index: usize| {
        renderer.begin();
        renderer.push(&Sprite::new(RED, 8.0, 8.0).size(8.0, 8.0));
        renderer.push(&Sprite::new(GREEN, WANDER[index % WANDER.len()], 40.0).size(8.0, 8.0));
        assert!(
            renderer.sprites() > 0,
            "the measured frame must be non-empty"
        );
        let color = [renew_rhi::color_attachment(clear)];
        let items = [renderer.item()];
        let passes = [Pass::new(&color, &items)];
        target
            .render(&RenderDesc::new(&passes))
            .expect("sprite frame");
        target.read_back_into(pixels);
    };

    // Warmup: first frames may lazily initialize driver state.
    for index in 0..3 {
        frame(&mut renderer, &mut target, &mut pixels, index);
    }
    assert_frame_is_live(&pixels, 2, "after warmup");

    // The retry-until-quiet policy lives with the counters it reads;
    // both channels now — a fill that frees is as loud as one that
    // allocates. The frame counter threads through the closure so the
    // liveness check still knows where this window's last frame put
    // the wandering sprite.
    let mut frames_run = 0usize;
    let verdict = counters::quiet_window(5, || {
        for _ in 0..16 {
            frame(&mut renderer, &mut target, &mut pixels, frames_run);
            frames_run += 1;
        }
        assert_frame_is_live(&pixels, frames_run - 1, "at the window's end");
    });
    eprintln!(
        "engine allocation counters after steady state: {:?}",
        counters::snapshot()
    );
    if let Err(activity) = verdict {
        panic!("the fill-and-render path was loud in every window (last: {activity})");
    }
}
