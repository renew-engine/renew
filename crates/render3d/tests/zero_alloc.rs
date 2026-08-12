//! Mechanical enforcement of this crate's allocation contract: after
//! warmup, describing and rendering a steady-state shadowed frame — two
//! passes, three items, a depth map rendered into and then sampled —
//! performs no heap allocation through the Rust global allocator.
//!
//! **Why this gate exists now and did not before.** The claim was
//! recorded rather than taken while the crate's frame path was two
//! struct constructions and no buffer; a gate over that would have
//! asserted nothing. The shadowed renderer changed it: a frame now
//! builds a caster item, a whole second pass targeting an image the
//! renderer owns across frames, and a lit item carrying two matrices and
//! two sampled bindings. That is the "a second draw call per frame" arm
//! of the trigger, and it has fired.
//!
//! One test in this file, deliberately: the `#[global_allocator]` is
//! process-wide, and a second test would race the counters.
//!
//! The measured window is proven live before it counts. Every frame
//! moves the light, so the pushed bytes differ frame to frame and a
//! caching driver cannot skip the work; the read-back is checked at both
//! ends of the window for a lit floor and for a shadow that is actually
//! darker, so a gate over a frame that stopped drawing would fail rather
//! than pass quietly.

use renew_memory::{CountingAllocator, counters};
use renew_render3d::{Camera, Scene, ShadowMatrices, ShadowedCameraRenderer, pass};
use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, ItemList, RenderDesc, TargetFormat, Validation,
};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SIZE: u32 = 32;
const SHADOW_MAP: u32 = 128;

/// Column-major identity, the spelling the rest of this crate's tests
/// use.
const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Per-frame variation without per-frame allocation: the light shears by
/// a value read from a fixed table, so every frame pushes different
/// bytes and re-renders the map.
const SHEAR: [f32; 4] = [0.35, 0.45, 0.55, 0.65];

fn at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let base = ((y * SIZE + x) * 4) as usize;
    [
        pixels[base],
        pixels[base + 1],
        pixels[base + 2],
        pixels[base + 3],
    ]
}

/// The sum of a pixel's colour channels — enough to say "this one is
/// darker than that one" without claiming exact bytes, which the fade in
/// the fragment stage makes unportable.
fn brightness(pixel: [u8; 4]) -> u32 {
    u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2])
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn the_steady_state_shadowed_frame_allocates_nothing() {
    let device = match Device::new(&DeviceDesc {
        app_name: "renew-render3d-zero-alloc",
        validation: Validation::Off,
    }) {
        Ok(device) => device,
        Err(DeviceError::LoaderUnavailable { message })
            if std::env::var_os("RENEW_GOLDEN").is_none_or(|value| value != "1") =>
        {
            eprintln!("SKIP: no Vulkan runtime: {message}");
            return;
        }
        Err(error) => panic!("device bring-up failed: {error}"),
    };

    // Everything allowed to allocate happens out here: the atlas upload,
    // both pipelines, the shadow map, the mesh, the target, and the
    // read-back buffer.
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let white = [255u8; 16];
    let renderer = ShadowedCameraRenderer::new(
        &device,
        TargetFormat::Rgba8Unorm,
        texture_extent,
        &white,
        SHADOW_MAP,
    )
    .expect("shadowed renderer");

    // The shadow golden's fixture, because a gate wants geometry already
    // known to cast rather than geometry invented for it. The floor spans
    // the view at depth 0.3; the blocker is a nearer patch bounded in
    // BOTH axes, so the map varies along each and a lookup that flipped
    // one would sample a different texel rather than an identical one.
    let mut scene = Scene::new();
    scene.quad(
        [
            [-1.0, -1.0, 0.3],
            [1.0, -1.0, 0.3],
            [1.0, 1.0, 0.3],
            [-1.0, 1.0, 0.3],
        ],
        [1.0, 1.0, 1.0, 1.0],
    );
    scene.quad(
        [
            [-0.25, -1.0, 0.8],
            [0.25, -1.0, 0.8],
            [0.25, 0.0, 0.8],
            [-0.25, 0.0, 0.8],
        ],
        [1.0, 1.0, 1.0, 1.0],
    );
    let mesh = renderer.upload(&device, &scene).expect("mesh upload");

    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device
        .create_offscreen_target(extent)
        .expect("offscreen target");
    let mut pixels = vec![0u8; target.byte_len()];
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];

    // The probes: one inside the cast patch, one on open floor at the
    // same height. Both sample the same white texel and take the same
    // fade, so the only thing that can separate them is the map.
    //
    // The light shears x by `s`, so a ray through the blocker at depth
    // 0.8 lands on the floor at 0.3 shifted by `s * 0.5`: the blocker's
    // x span of [-0.25, 0.25] casts onto [-0.25 + 0.5s, 0.25 + 0.5s].
    // Across every shear in the table below that patch always contains
    // clip x = 0.375 and never contains clip x = -0.5, which is what
    // lets the light move frame to frame while both probes keep their
    // meaning.
    let shadowed_x = 11 * SIZE / 16; // clip x = +0.375, inside the cast
    let open_x = SIZE / 4; // clip x = -0.5, open floor
    let probe_y = SIZE / 4; // clip y = -0.5, the blocked half

    // One frame drawn outside the window, both to warm every lazily
    // built thing and to establish the premise the window then re-checks.
    let mut draw = |shear: f32, pixels: &mut Vec<u8>| {
        let mut light = IDENTITY;
        light[2][0] = shear;
        let camera = Camera::from_columns(light);
        let matrices = ShadowMatrices::from_columns(IDENTITY, light);
        let casting = [renderer.caster_item(&mesh, &camera)];
        let shadow = renderer.shadow_pass(&casting);
        let items = ItemList::<1>::new(renderer.item(&mesh, &matrices));
        let passes = [shadow, pass(&clear, items.as_slice())];
        target
            .render(&RenderDesc::new(&passes))
            .expect("the shadowed frame renders");
        target.read_back_into(pixels);
    };

    draw(SHEAR[0], &mut pixels);
    let warm_shadowed = brightness(at(&pixels, shadowed_x, probe_y));
    let warm_open = brightness(at(&pixels, open_x, probe_y));
    assert!(
        warm_open > 0,
        "premise: the floor must actually be drawn, or this gate measures an empty frame"
    );
    assert!(
        warm_shadowed < warm_open,
        "premise: the shadow must actually darken its probe ({warm_shadowed} against \
         {warm_open}), or the second pass is not doing anything for the gate to measure"
    );

    // The measured window: the whole per-frame description and render,
    // repeatedly, with the light moving so no frame repeats another.
    let verdict = counters::quiet_window(5, || {
        for step in 0..8usize {
            draw(SHEAR[step % SHEAR.len()], &mut pixels);
            let shadowed = brightness(at(&pixels, shadowed_x, probe_y));
            let open = brightness(at(&pixels, open_x, probe_y));
            assert!(open > 0, "every windowed frame must really draw the floor");
            assert!(
                shadowed < open,
                "and every windowed frame must really cast: {shadowed} against {open}"
            );
        }
    });
    verdict.expect("the shadowed frame's steady state stays heap-silent");

    drop(target);
    drop(renderer);
}
