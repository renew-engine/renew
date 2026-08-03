//! Mechanical enforcement of the fill-and-render allocation contract:
//! after warmup, a steady-state sprite frame — begin, push, describe,
//! render, read back — performs no heap allocation through the Rust
//! global allocator.
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
use renew_rhi::{Color, Device, DeviceDesc, DeviceError, Extent, TargetFormat, Validation};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Allocations recorded so far, process-wide.
fn allocations() -> u64 {
    counters::snapshot().allocations
}

const SIZE: u32 = 64;
/// Per-frame variation without per-frame allocation: positions read
/// from a fixed table, so the packed bytes differ frame to frame and
/// the copy path cannot be skipped by a caching driver.
const WANDER: [f32; 4] = [24.0, 28.0, 32.0, 36.0];
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
    let clear = Color::new(0.1, 0.2, 0.3, 1.0);
    let mut pixels = vec![0u8; target.byte_len()];

    // The premise assertion the vacuity lesson requires: a fixed sprite
    // covers this pixel every frame, and the frame is only credible if
    // the read-back shows it.
    let sprite_pixel = |pixels: &[u8]| {
        let base = ((12 * SIZE + 12) * 4) as usize;
        [
            pixels[base],
            pixels[base + 1],
            pixels[base + 2],
            pixels[base + 3],
        ]
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
        target.render(&renderer.desc(clear)).expect("sprite frame");
        target.read_back_into(pixels);
    };

    // Warmup: first frames may lazily initialize driver state.
    for index in 0..3 {
        frame(&mut renderer, &mut target, &mut pixels, index);
    }
    assert_eq!(
        sprite_pixel(&pixels),
        [255, 0, 0, 255],
        "warmup frames never drew the fixed sprite — the gate would measure a blank frame"
    );

    // Measurement protocol: the counter is process-wide and the harness
    // thread can allocate concurrently, so the window retries — one-shot
    // neighbor noise rides out, while a real fill-or-render allocation
    // reproduces in every window and still fails.
    let mut last_delta = 0u64;
    let mut observed_zero = false;
    for attempt in 0..5 {
        let before = allocations();
        for index in 0..16 {
            frame(
                &mut renderer,
                &mut target,
                &mut pixels,
                attempt * 16 + index,
            );
        }
        let after = allocations();
        assert_eq!(
            sprite_pixel(&pixels),
            [255, 0, 0, 255],
            "the measured window's last frame never drew the fixed sprite"
        );
        last_delta = after - before;
        if last_delta == 0 {
            observed_zero = true;
            break;
        }
    }
    eprintln!(
        "engine allocation counters after steady state: {:?}",
        counters::snapshot()
    );
    assert!(
        observed_zero,
        "the fill-and-render path heap-allocated in every window (last delta: {last_delta})"
    );
}
