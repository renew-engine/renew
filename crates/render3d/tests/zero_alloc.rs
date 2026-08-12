//! The shadowed frame describes and renders without reaching the heap:
//! after warmup, two passes and two items — a depth map rendered into and
//! then sampled — allocate nothing through the Rust global allocator.
//!
//! Stated as what this measures rather than as "the crate's allocation
//! contract": the crate's Contract heading carries no allocation clause,
//! and a test that cites a contract which does not exist is worse than
//! one that simply says what it checked.
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
//! The measured window is proven live before it counts. Within one pass
//! of the window every frame carries a different light, so the pushed
//! bytes differ and a caching driver cannot skip the work. Across passes
//! they repeat: `quiet_window` retries its whole body until a quiet run
//! is observed, so any given frame may be drawn several times, and that
//! is fine — the claim being made is about the heap, and the variation
//! exists only so that one pass cannot be serviced from a cache.
//!
//! Every frame — not only the ends — is checked for a lit floor and for a
//! shadow that is actually darker, so a frame that stopped drawing or
//! stopped casting fails rather than passing quietly.

use renew_memory::{CountingAllocator, counters};
use renew_render3d::{Camera, Render3dError, Scene, ShadowMatrices, ShadowedCameraRenderer, pass};
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
/// a value read from a fixed table, one entry per windowed step, so no
/// frame within a pass repeats another.
///
/// The values are chosen for margin, not for spread — see the probe
/// arithmetic below. The smallest of them puts the shadowed probe about a
/// pixel inside the cast; the largest, about four.
const SHEAR: [f32; 8] = [0.45, 0.50, 0.55, 0.60, 0.65, 0.70, 0.75, 0.80];

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
#[expect(
    clippy::too_many_lines,
    reason = "the fixture, the warmup, the premise checks and the measured window are one               protocol, and splitting them would let a caller run the window without them"
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
    // A depth format is not universal, and this is the only depth-
    // dependent device test in the tree that would otherwise take an
    // adapter without one as a failure. Skip it, unless this is the lane
    // that exists to run these — there a skip is the failure.
    let renderer = match ShadowedCameraRenderer::new(
        &device,
        TargetFormat::Rgba8Srgb,
        texture_extent,
        &white,
        SHADOW_MAP,
    ) {
        Ok(renderer) => renderer,
        Err(Render3dError::DepthUnsupported { chain })
            if std::env::var_os("RENEW_GOLDEN").is_none_or(|value| value != "1") =>
        {
            eprintln!("SKIP: adapter offers no chain depth format: {chain:?}");
            return;
        }
        Err(error) => panic!("shadowed renderer: {error}"),
    };

    // The two quads are the shadow golden's, verbatim, because a gate
    // wants geometry already known to cast. The map size, the shears and
    // the probes are this test's own and are justified where they appear.
    // The floor spans the view at depth 0.3; the blocker is a nearer
    // patch bounded in BOTH axes, so the map varies along each and a
    // lookup that flipped one would sample a different texel rather than
    // an identical one.
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
    // **A pixel is sampled at its centre**, which the arithmetic has to
    // account for: pixel `p` of `SIZE` sits at clip x = 2(p + 0.5)/SIZE
    // - 1, so pixel 22 of 32 is 0.40625 rather than the 0.375 its index
    // suggests. Getting this wrong is how a probe ends up one pixel from
    // an edge while its comment claims comfort.
    //
    // The light shears x by `s`, so a ray through the blocker at depth
    // 0.8 lands on the floor at 0.3 shifted by 0.5s: the blocker's span
    // of [-0.25, 0.25] casts onto [-0.25 + 0.5s, 0.25 + 0.5s]. Only the
    // part right of +0.25 is observable, because the blocker is nearer
    // and lit, so it hides the left half of its own cast. The observable
    // dark run is therefore (0.25, 0.25 + 0.5s), and the probe needs
    // 0.25 + 0.5s > 0.40625, i.e. s > 0.3125. The table starts at 0.45,
    // which clears that by about a pixel and widens from there.
    let shadowed_x = 11 * SIZE / 16; // clip x = +0.40625, inside the cast
    let open_x = SIZE / 4; // clip x = -0.46875, open floor
    let probe_y = SIZE / 4; // clip y = -0.5, the blocked half
    //
    // The open probe is not the mirror of the shadowed one and is not
    // meant to be: no shear in this fixture can darken a pixel left of
    // +0.25, so it is an unconditionally lit reference rather than a
    // second case. That is what makes it useful — it moves only if the
    // floor itself stops drawing.

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

    // Deliberately not a value from the table: a warmup sharing the
    // window's first frame is the one frame a cache could serve.
    draw(0.35, &mut pixels);
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
        for &shear in &SHEAR {
            draw(shear, &mut pixels);
            let shadowed = brightness(at(&pixels, shadowed_x, probe_y));
            let open = brightness(at(&pixels, open_x, probe_y));
            assert!(open > 0, "every windowed frame must really draw the floor");
            assert!(
                shadowed < open,
                "and every windowed frame must really cast: {shadowed} against {open} \
                 at shear {shear}"
            );
        }
    });
    verdict.expect("the shadowed frame's steady state stays heap-silent");
}
