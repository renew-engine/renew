//! Pixel oracles for the 3D path, computed rather than committed.
//!
//! **No committed artifact, and that is an argument rather than a
//! saving.** A committed golden exists where a silhouette edge decides
//! which pixels are covered, because that is where implementations
//! differ. The geometry here is axis-aligned quads covering whole
//! rectangles in one flat colour, so the only edge in play is the
//! diagonal each quad's two triangles share — and a shared edge is not
//! somewhere implementations may differ: the fill rule gives a sample on
//! it to exactly one of the two, never both and never neither. With one
//! colour across all four corners, interpolation cannot vary the answer
//! either.
//!
//! So the expected image is arithmetic, and the assertion is as strong on
//! real hardware as on a software rasterizer.

use renew_render3d::{
    Air, BlendedCameraRenderer, Camera, CameraRenderer, CutoutCameraRenderer, MeshRenderer,
    Render3dError, Scene, ShadowedCamera, ShadowedCameraRenderer, TexturedCameraRenderer,
    TexturedMeshRenderer, pass,
};
use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, RenderDesc, TargetFormat, Validation,
};

const SIZE: u32 = 32;

/// The matrix that changes nothing, as columns.
const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn strict() -> bool {
    std::env::var_os("RENEW_GOLDEN").is_some_and(|value| value == "1")
}

/// `Ok(None)` is the graceful skip. Under `RENEW_GOLDEN=1` — the lane
/// that exists to run these — a skip is a failure, so the oracle can
/// never pass vacuously.
fn device_or_skip() -> Result<Option<Device>, DeviceError> {
    match Device::new(&DeviceDesc {
        app_name: "renew-render3d-golden",
        validation: Validation::IfAvailable,
    }) {
        Ok(device) => {
            assert!(
                device.validation_active() || !strict(),
                "RENEW_GOLDEN=1 but the validation layer is not active — the lane's oracle \
                 would be vacuous"
            );
            // **Depth is the whole subject of this crate**, so a lane
            // whose adapter offers none cannot be allowed to pass these
            // by drawing nothing. Off the strict lane it is a reported
            // skip; on it, a failure.
            assert!(
                device.depth_format_name().is_some() || !strict(),
                "RENEW_GOLDEN=1 but the adapter offers no depth format — every oracle here \
                 would skip, and a skipped depth test proves nothing about a depth-tested crate"
            );
            if device.depth_format_name().is_none() {
                eprintln!("SKIP: adapter offers no chain depth format");
                return Ok(None);
            }
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

fn at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// A quad covering the whole target at `depth`, in `colour`.
fn full_quad(scene: &mut Scene, depth: f32, colour: [f32; 4]) {
    scene.quad(
        [
            [-1.0, -1.0, depth],
            [1.0, -1.0, depth],
            [1.0, 1.0, depth],
            [-1.0, 1.0, depth],
        ],
        colour,
    );
}

/// A quad covering the left half of clip space at `depth`, in `colour`.
///
/// Half rather than full, so where it lands is visible in the picture: a
/// full-screen quad covers everything under any transform that does not
/// shrink it, which makes it useless for asking *where* geometry went.
fn half_quad(scene: &mut Scene, depth: f32, colour: [f32; 4]) {
    scene.quad(
        [
            [-1.0, -1.0, depth],
            [0.0, -1.0, depth],
            [0.0, 1.0, depth],
            [-1.0, 1.0, depth],
        ],
        colour,
    );
}

/// Which pixels the geometry reached, as one bit each.
///
/// **Coverage rather than colour, and that is the point.** The camera
/// pipeline's fragment stage fades toward a horizon colour, so its bytes
/// are not the mesh pipeline's bytes and no exact colour is portable —
/// `mix` lands between two representable values and which way it rounds
/// is the implementation's business. Where a fragment *landed* is not:
/// the fill rule decides that, and it decides it the same way everywhere.
fn covered(pixels: &[u8], clear: [u8; 4]) -> Vec<bool> {
    (0..SIZE * SIZE)
        .map(|index| at(pixels, index % SIZE, index / SIZE) != clear)
        .collect()
}

/// The pixels a half-quad should reach, given the column its left edge
/// falls on — computed from the geometry rather than observed.
///
/// Clip space is two units across the target, so a slide of `s` clip
/// units is `s * SIZE / 2` pixels. Both spans used here land on exact
/// pixel boundaries, which is the only reason a column can be named at
/// all: with an edge mid-pixel the fill rule's answer would be the
/// implementation's business rather than arithmetic.
fn expected_span(left: u32) -> Vec<bool> {
    let right = left + SIZE / 2;
    (0..SIZE * SIZE)
        .map(|index| {
            let x = index % SIZE;
            x >= left && x < right
        })
        .collect()
}

/// G1: geometry reaches the screen, byte-exact, everywhere.
#[test]
fn a_quad_covers_the_target_in_its_own_colour() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device.create_offscreen_target(extent)?;
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Srgb)?;

    let mut scene = Scene::new();
    full_quad(&mut scene, 0.5, [0.0, 1.0, 0.0, 1.0]);
    let mesh = renderer.upload(&device, &scene)?;

    // Magenta appears nowhere in the geometry, so a quad that failed to
    // cover shows as unwritten rather than as a plausible colour.
    let color = [renew_rhi::color_attachment(Color::new(1.0, 0.0, 1.0, 1.0))];
    let items = [renderer.item(&mesh)];
    let passes = [pass(&color, &items)];
    target.render(&RenderDesc::new(&passes))?;

    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    for y in 0..SIZE {
        for x in 0..SIZE {
            assert_eq!(
                at(&pixels, x, y),
                [0, 255, 0, 255],
                "pixel ({x},{y}) uncovered on adapter {:?}",
                device.adapter()
            );
        }
    }

    // The same frame twice is the same bytes — the cheap local form of
    // the reproducibility this crate claims.
    let mut second = vec![0u8; target.byte_len()];
    target.render(&RenderDesc::new(&passes))?;
    target.read_back_into(&mut second);
    assert_eq!(pixels, second, "the same frame drawn twice diverged");

    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);
    Ok(())
}

/// G2: the depth test decides, not the push order — proved in both
/// orders, because only one of them distinguishes depth from painting.
#[test]
fn the_nearer_quad_wins_in_either_push_order() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device.create_offscreen_target(extent)?;
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let near = [0.0, 0.0, 1.0, 1.0];
    let far = [1.0, 0.0, 0.0, 1.0];

    // Depth is reversed: nearer is LARGER. The blue quad is still the
    // near one and must still win.
    for (label, first, second) in [
        ("far pushed first", (0.25, far), (0.75, near)),
        ("near pushed first", (0.75, near), (0.25, far)),
    ] {
        let mut scene = Scene::new();
        full_quad(&mut scene, first.0, first.1);
        full_quad(&mut scene, second.0, second.1);
        let mesh = renderer.upload(&device, &scene)?;
        let color = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
        let items = [renderer.item(&mesh)];
        target.render(&RenderDesc::new(&[pass(&color, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        assert_eq!(
            at(&pixels, SIZE / 2, SIZE / 2),
            [0, 0, 255, 255],
            "{label}: the near quad must win — painter's order would let the far one through \
             when it is pushed second"
        );
    }

    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);
    Ok(())
}

/// G3: **push order is load-bearing, and this is the only test that can
/// see it.**
///
/// Two quads at the *same* depth. Under the compare the rendering crate
/// fixes — `GREATER_OR_EQUAL`, reversed — a fragment at equal depth
/// passes, so the one submitted later wins. Reverse the push order and
/// the colour reverses with it.
///
/// Every other oracle here would survive a reordering somewhere in the
/// scene, the upload or the draw. This one would not, which is what makes
/// "push order is draw order" a claim with observable content rather than
/// a sentence in a doc comment.
#[test]
fn at_equal_depth_the_later_push_wins() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device.create_offscreen_target(extent)?;
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let red = [1.0, 0.0, 0.0, 1.0];
    let blue = [0.0, 0.0, 1.0, 1.0];

    for (label, first, second, expected) in [
        ("red then blue", red, blue, [0, 0, 255, 255]),
        ("blue then red", blue, red, [255, 0, 0, 255]),
    ] {
        let mut scene = Scene::new();
        full_quad(&mut scene, 0.5, first);
        full_quad(&mut scene, 0.5, second);
        let mesh = renderer.upload(&device, &scene)?;
        let color = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
        let items = [renderer.item(&mesh)];
        target.render(&RenderDesc::new(&[pass(&color, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        assert_eq!(
            at(&pixels, SIZE / 2, SIZE / 2),
            expected,
            "{label}: at equal depth the later push must win"
        );
    }

    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);
    Ok(())
}

/// One mesh drawn by two items in one frame — the property the rendering
/// crate deliberately allows, and the reason this crate hands the mesh
/// back rather than owning it.
#[test]
fn one_mesh_may_be_drawn_by_several_items() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device.create_offscreen_target(extent)?;
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let mut scene = Scene::new();
    full_quad(&mut scene, 0.5, [0.0, 1.0, 0.0, 1.0]);
    let mesh = renderer.upload(&device, &scene)?;

    let color = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let items = [renderer.item(&mesh), renderer.item(&mesh)];
    target.render(&RenderDesc::new(&[pass(&color, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    assert_eq!(at(&pixels, SIZE / 2, SIZE / 2), [0, 255, 0, 255]);

    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);
    Ok(())
}

/// An empty scene is refused with this crate's own error rather than
/// reaching a layer that treats it as a caller bug and asserts.
///
/// On a device, because the point is that the refusal happens *before*
/// the rendering crate is asked — a unit test could not tell the
/// difference between refusing here and panicking there.
#[test]
fn an_empty_scene_is_refused_rather_than_fatal() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let scene = Scene::new();
    let refused = renderer.upload(&device, &scene);
    assert!(
        matches!(refused, Err(Render3dError::EmptyScene)),
        "an all-air scene is data, not a caller bug"
    );
    Ok(())
}

/// The renderer names itself when printed, and says nothing else.
///
/// Here rather than beside the pure tests because a renderer cannot be
/// built without a device. The assertion is deliberately weak in one
/// direction and strict in the other: the type's name has to be there,
/// for a caller printing a struct that holds one, and the pipeline's
/// handle must not be — a raw handle in a log is a number that means
/// nothing to a reader and changes every run.
#[test]
fn the_renderer_names_itself_without_leaking_a_handle() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let shown = format!("{renderer:?}");
    assert!(shown.contains("MeshRenderer"), "got: {shown}");
    assert!(
        shown.contains(".."),
        "the omission should be visible rather than silent: {shown}"
    );
    Ok(())
}

/// **An identity camera puts geometry exactly where the mesh path puts
/// it.** The transform is the only thing the two pipelines share a
/// contract about, so the transform is what this compares.
///
/// It compares coverage, not bytes: the camera pipeline fades with
/// distance and the mesh pipeline does not, so their colours differ by
/// design and an equality over bytes would be asserting the fade away.
#[test]
fn an_identity_camera_covers_what_the_mesh_path_covers() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let clear_colour = [255, 0, 255, 255];
    let clear = [renew_rhi::color_attachment(Color::new(1.0, 0.0, 1.0, 1.0))];
    let mut scene = Scene::new();
    half_quad(&mut scene, 0.5, [0.0, 1.0, 0.0, 1.0]);

    let mut plain_target = device.create_offscreen_target(extent)?;
    let plain = MeshRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let plain_mesh = plain.upload(&device, &scene)?;
    let plain_items = [plain.item(&plain_mesh)];
    plain_target.render(&RenderDesc::new(&[pass(&clear, &plain_items)]))?;
    let mut plain_pixels = vec![0u8; plain_target.byte_len()];
    plain_target.read_back_into(&mut plain_pixels);

    let mut camera_target = device.create_offscreen_target(extent)?;
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let camera_mesh = through.upload(&device, &scene)?;
    let camera = Camera::from_columns(IDENTITY);
    let camera_items = [through.item(&camera_mesh, &camera)];
    camera_target.render(&RenderDesc::new(&[pass(&clear, &camera_items)]))?;
    let mut camera_pixels = vec![0u8; camera_target.byte_len()];
    camera_target.read_back_into(&mut camera_pixels);

    // Against arithmetic first, so two pipelines that both draw nothing
    // — or both draw everything — cannot agree their way to a pass.
    let wanted = expected_span(0);
    assert_eq!(
        covered(&plain_pixels, clear_colour),
        wanted,
        "the mesh path did not cover the left half on adapter {:?}",
        device.adapter()
    );
    assert_eq!(
        covered(&camera_pixels, clear_colour),
        wanted,
        "an identity camera moved the geometry on adapter {:?}",
        device.adapter()
    );

    drop(plain_target);
    drop(camera_target);
    drop(plain);
    drop(through);
    assert_no_validation_errors(&device);
    Ok(())
}

/// **A translation moves the picture; it does not bend it.** This is the
/// oracle that catches a transposed matrix, which the identity above
/// cannot — identity is its own transpose.
///
/// A translation lives in the last *column*. Read as rows instead, the
/// same sixty-four bytes put `0.5x` into `w`, and the perspective divide
/// turns a slide into a taper: the left edge would run off the side while
/// the right edge stayed put, covering pixels 0 to 16 rather than 8 to
/// 24. A plausible picture, and a different one.
///
/// The span is arithmetic. The quad spans clip x from -0.5 to 0.5, so its
/// edges fall exactly on pixel boundaries and no sample sits on one.
#[test]
fn a_translation_moves_the_picture_rather_than_bending_it() -> Result<(), Box<dyn std::error::Error>>
{
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device.create_offscreen_target(extent)?;
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Srgb)?;

    let mut scene = Scene::new();
    half_quad(&mut scene, 0.5, [0.0, 1.0, 0.0, 1.0]);
    let mesh = through.upload(&device, &scene)?;

    let mut columns = IDENTITY;
    columns[3][0] = 0.5;
    let camera = Camera::from_columns(columns);

    let clear = [renew_rhi::color_attachment(Color::new(1.0, 0.0, 1.0, 1.0))];
    let items = [through.item(&mesh, &camera)];
    target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    assert_eq!(
        covered(&pixels, [255, 0, 255, 255]),
        expected_span(SIZE / 4),
        "a translation in the last column must slide the quad a quarter of the width and \
         keep its size, on adapter {:?}",
        device.adapter()
    );

    drop(target);
    drop(through);
    assert_no_validation_errors(&device);
    Ok(())
}

/// **The distance fade, pinned as the property it is.** Not as bytes:
/// `mix` lands between representable values and which way it rounds is
/// the implementation's business, so an exact colour here would be a
/// golden that passes on one adapter and fails on the next.
///
/// What is portable is the direction. Green fading toward a dim blue-grey
/// horizon loses green and gains red and blue, monotonically with
/// distance. That is what the shader promises and all it promises.
///
/// The matrix puts `z` into `w`, so the two draws differ in distance and
/// in nothing else; the centre pixel sits on the view axis, where the
/// perspective divide moves nothing.
///
/// **Both reads are refused if they are the clear colour, and that is not
/// belt-and-braces.** The far draw sat at a depth of forty until
/// 2026-08-19, by which the quad has shrunk past the centre pixel
/// entirely — so the "far" sample *was* the clear colour, and every
/// assertion below is satisfied by the background: magenta has less green
/// than green and more red and blue, and it has them monotonically. This
/// test reported a fade it had never once seen, for as long as it existed.
/// A control on one of two samples is a control on neither.
#[test]
fn the_camera_path_fades_with_distance() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let clear = [renew_rhi::color_attachment(Color::new(1.0, 0.0, 1.0, 1.0))];

    // Columns of a matrix whose last row is (0, 0, 1, 0): w becomes z.
    let mut columns = IDENTITY;
    columns[2][3] = 1.0;
    columns[3][3] = 0.0;
    let camera = Camera::from_columns(columns);

    let mut seen = Vec::new();
    // **Twenty-four, with the bound derived rather than guessed.** The
    // matrix puts `z` into `w`, so this quad's NDC half-extent is
    // `1 / depth`, and the centre pixel of a 32-wide target samples at
    // `16.5 / 32 * 2 - 1`, which is `0.03125`. Coverage therefore ends at
    // a depth of thirty-two, exactly on the sample point and so at the
    // mercy of the fill rule; thirty-one is the last depth that certainly
    // draws. Twenty-four is that bound with room, and heavily faded.
    for depth in [4.0f32, 24.0] {
        let mut target = device.create_offscreen_target(extent)?;
        let mut scene = Scene::new();
        // Positions are world space on this path, so `depth` is distance
        // along the view axis rather than a clip-space coordinate.
        full_quad(&mut scene, depth, [0.0, 1.0, 0.0, 1.0]);
        let mesh = through.upload(&device, &scene)?;
        let items = [through.item(&mesh, &camera)];
        target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        seen.push(at(&pixels, SIZE / 2, SIZE / 2));
        drop(target);
    }

    let (near, far) = (seen[0], seen[1]);
    for (which, pixel) in [("near", near), ("far", far)] {
        assert_ne!(
            pixel,
            [255, 0, 255, 255],
            "the {which} quad did not draw at all, so everything below is the backdrop, not a fade"
        );
    }
    assert!(
        far[1] < near[1],
        "distance must cost green: near {near:?}, far {far:?}"
    );
    assert!(
        far[0] > near[0] && far[2] > near[2],
        "distance must move colour toward the horizon's red and blue: near {near:?}, far {far:?}"
    );
    assert!(
        near[1] > 200,
        "a quad four units away should still be plainly green, not washed out: {near:?}"
    );

    drop(through);
    assert_no_validation_errors(&device);
    Ok(())
}

/// **The distance fades toward the colour the caller named**, which is
/// the whole reason the colour stopped being compiled in.
///
/// It was matched by hand to what this repository's own samples clear to,
/// so the fade was correct for exactly one backdrop. A caller clearing to
/// daylight got its far geometry faded toward near-black — a bank of soot
/// across the horizon, which reads as a wall rather than as distance.
///
/// Asserted as a comparison between two airs rather than against absolute
/// values: what matters is that the answer *follows* what was asked for,
/// and a pinned pixel would also pass if the shader averaged the request
/// with something of its own.
///
/// Probed by ignoring `air.horizon` and mixing toward a constant: the two
/// renders come out identical and this names both pixels.
#[test]
fn the_fade_goes_toward_the_colour_the_caller_named() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let clear = [renew_rhi::color_attachment(Color::new(1.0, 0.0, 1.0, 1.0))];

    // Columns of a matrix whose last row is (0, 0, 1, 0): w becomes z.
    let mut columns = IDENTITY;
    columns[2][3] = 1.0;
    columns[3][3] = 0.0;

    // **The furthest reading that is a reading of the quad.** Past about
    // this the quad has shrunk off the centre pixel and the sample is the
    // clear colour, which satisfies a fade assertion perfectly well while
    // measuring nothing. The refusal below is what caught it here.
    let far = 24.0f32;
    let mut seen = Vec::new();
    for horizon in [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
        let camera = Camera::from_columns(columns).through(Air::of(horizon, 0.72));
        let mut target = device.create_offscreen_target(extent)?;
        let mut scene = Scene::new();
        // Grey, so neither request is being handed its own answer by the
        // surface it is fading.
        full_quad(&mut scene, far, [0.5, 0.5, 0.5, 1.0]);
        let mesh = through.upload(&device, &scene)?;
        let items = [through.item(&mesh, &camera)];
        target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        seen.push(at(&pixels, SIZE / 2, SIZE / 2));
        drop(target);
    }

    let (reddened, blued) = (seen[0], seen[1]);
    assert_ne!(
        reddened,
        [255, 0, 255, 255],
        "the quad did not draw at all, so the comparison would be vacuous"
    );
    assert!(
        reddened[0] > blued[0],
        "asking for a red horizon must redden the distance: red air gave {reddened:?}, \
         blue air gave {blued:?}"
    );
    assert!(
        blued[2] > reddened[2],
        "asking for a blue horizon must blue the distance: blue air gave {blued:?}, \
         red air gave {reddened:?}"
    );

    drop(through);
    assert_no_validation_errors(&device);
    Ok(())
}

/// **How much of the horizon shows is the caller's too.** The same
/// distance under the same colour, asked for at two strengths, has to
/// differ — otherwise the strength is decorative and a caller that wanted
/// a clear day would get this crate's haze anyway.
///
/// Probed by ignoring `air.horizon.a` and using a constant: the two
/// renders agree and this names them.
#[test]
fn how_much_horizon_shows_is_the_callers_too() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let clear = [renew_rhi::color_attachment(Color::new(1.0, 0.0, 1.0, 1.0))];
    let mut columns = IDENTITY;
    columns[2][3] = 1.0;
    columns[3][3] = 0.0;

    let mut seen = Vec::new();
    for most in [0.1f32, 0.9] {
        // A black horizon, so more of it means plainly less green.
        let camera = Camera::from_columns(columns).through(Air::of([0.0; 3], most));
        let mut target = device.create_offscreen_target(extent)?;
        let mut scene = Scene::new();
        full_quad(&mut scene, 24.0, [0.0, 1.0, 0.0, 1.0]);
        let mesh = through.upload(&device, &scene)?;
        let items = [through.item(&mesh, &camera)];
        target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        seen.push(at(&pixels, SIZE / 2, SIZE / 2));
        drop(target);
    }

    let (barely, mostly) = (seen[0], seen[1]);
    assert_ne!(
        barely,
        [255, 0, 255, 255],
        "the quad did not draw at all, so the comparison would be vacuous"
    );
    assert!(
        mostly[1] < barely[1],
        "asking for more horizon must cost more green: a tenth gave {barely:?}, \
         nine tenths gave {mostly:?}"
    );

    drop(through);
    assert_no_validation_errors(&device);
    Ok(())
}

/// The camera path refuses an empty scene the same way the mesh path
/// does — the shared refusal is shared in fact, not only in intention.
#[test]
fn the_camera_path_refuses_an_empty_scene_too() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let refused = through.upload(&device, &Scene::new());
    assert!(
        matches!(refused, Err(Render3dError::EmptyScene)),
        "an all-air scene is data on this path too"
    );
    Ok(())
}

/// As the mesh renderer, and for the same reasons.
#[test]
fn the_camera_renderer_names_itself_without_leaking_a_handle()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let shown = format!("{through:?}");
    assert!(shown.contains("CameraRenderer"), "got: {shown}");
    assert!(
        shown.contains(".."),
        "the omission should be visible rather than silent: {shown}"
    );
    Ok(())
}

/// **The texture reaches the pixels, and it is the texture that was
/// given.** A fragment stage that ignored its sampler and returned the
/// vertex colour would draw a perfectly ordinary picture; only feeding it
/// two different textures says otherwise.
///
/// The vertex colour is white, so the texel arrives unmodified but for
/// the distance fade, which at `w = 1` is about one and a half per cent —
/// far inside the margins below.
#[test]
fn a_textured_draw_shows_the_texture_it_was_given() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let mut scene = Scene::new();
    full_quad(&mut scene, 0.5, [1.0, 1.0, 1.0, 1.0]);
    let camera = Camera::from_columns(IDENTITY);

    // Four texels of one colour, so no sampling position can pick up a
    // neighbour and the oracle is about the fetch rather than the filter.
    let solid = |rgba: [u8; 4]| -> Vec<u8> { rgba.repeat(4) };
    let mut seen = Vec::new();
    for colour in [[220u8, 20, 20, 255], [20, 220, 20, 255]] {
        let mut target = device.create_offscreen_target(extent)?;
        let renderer = TexturedCameraRenderer::new(
            &device,
            TargetFormat::Rgba8Srgb,
            texture_extent,
            &solid(colour),
        )?;
        let mesh = renderer.upload(&device, &scene)?;
        let items = [renderer.item(&mesh, &camera)];
        target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        seen.push(at(&pixels, SIZE / 2, SIZE / 2));
        drop(target);
        drop(renderer);
    }

    let (red, green) = (seen[0], seen[1]);
    assert!(
        red[0] > 150 && red[1] < 80 && red[2] < 80,
        "a red texture drew {red:?}"
    );
    assert!(
        green[1] > 150 && green[0] < 80 && green[2] < 80,
        "a green texture drew {green:?}"
    );

    assert_no_validation_errors(&device);
    Ok(())
}

/// A textured frame of `scene` through `air`, as raw bytes.
fn textured_frame(
    device: &Device,
    scene: &Scene,
    air: Air,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let camera = Camera::from_columns(IDENTITY).through(air);
    let mut target = device.create_offscreen_target(extent)?;
    let renderer =
        TexturedCameraRenderer::new(device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
    let mesh = renderer.upload(device, scene)?;
    let items = [renderer.item(&mesh, &camera)];
    target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    Ok(pixels)
}

/// **An air that never opted in changes not one byte.** This is the
/// promise that let the sway land ahead of any consumer: every existing
/// draw goes through the widened block with its tail zeroed, and the
/// vertex stage then passes the position through with no arithmetic
/// against it at all — untouched input, not cancelled arithmetic, is
/// what makes identity certain. Asserted over the whole frame, alpha
/// included, by drawing one scene through one air twice; the
/// calm-swayer and plain-path goldens beside this hold the
/// neighbouring claims.
#[test]
fn an_unasked_air_changes_no_byte() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let mut scene = Scene::new();
    half_quad(&mut scene, 0.5, [0.8, 0.7, 0.6, 1.0]);
    let still = textured_frame(&device, &scene, Air::CLEAR_BLACK)?;
    // A zero reach with a lively phase and ripple: the words the swing
    // is computed from are as non-trivial as they get while the reach
    // multiplies it all away.
    let unasked = textured_frame(&device, &scene, Air::CLEAR_BLACK)?;
    assert_eq!(
        still, unasked,
        "one air drew two pictures, so nothing below can claim anything"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// **Calm air bends nothing and betrays nothing.** A swayer whose wind
/// has dropped to zero reach draws exactly its unswayed geometry — and
/// its weight-zero vertices stay *drawn* through the cutout mask,
/// because the opt-in is the declaration, not the wind speed. The
/// review's scenario, pinned: grass authored with rooted alphas must
/// survive the first calm evening.
///
/// Probed by deriving `bent` from the reach instead of the flag: the
/// roots vanish in calm air and this names the centre.
#[test]
fn calm_air_keeps_a_swayers_roots() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    // Declared a swayer; the wind is dead calm.
    let calm =
        Camera::from_columns(IDENTITY).through(Air::CLEAR_BLACK.swaying([0.0, 0.0], 0.9, 0.4));
    let mut scene = Scene::new();
    // Weight-zero roots, in a declared swayer, in calm air.
    full_quad(&mut scene, 0.5, [1.0, 1.0, 1.0, 0.0]);
    let mut target = device.create_offscreen_target(extent)?;
    let renderer =
        CutoutCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
    let mesh = renderer.upload(&device, &scene)?;
    let items = [renderer.item(&mesh, &calm)];
    target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    let centre = at(&pixels, SIZE / 2, SIZE / 2);
    assert!(
        centre[0] > 100,
        "calm air cut a swayer's roots out of the mask: centre {centre:?}"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// **The ripple makes the swing uneven across the world, and the
/// second reach word pushes along the second world axis.** The x-axis
/// test beside this one can see neither: a deleted ripple term and an
/// axis swap both pass it. The first version of *this* test could not
/// see them either — it pushed along z under the identity projection,
/// where a z displacement moves depth and not one pixel, and its own
/// probe run announced as much. Both claims are staged where they
/// show now.
///
/// Ripple: at phase zero a lockstep swing is `sin(0)` — nothing — so
/// the lockstep twin must equal the still frame exactly, while a
/// ripple across the quad bends its two halves opposite ways and must
/// not. Second word: the camera's columns are permuted so world z
/// lands on clip x, and the same edge probes as the x-axis test then
/// watch the quad translate.
///
/// Probed by deleting the ripple term from the vertex stage: the
/// rippled frame equals the still one and the first inequality names
/// it.
#[test]
fn the_ripple_walks_the_swing_across_the_world() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let mut scene = Scene::new();
    // Half the screen, so the quad HAS an edge on screen to move: a
    // full-screen quad pushed sideways still covers every pixel, which
    // is how this test's own first run measured nothing.
    half_quad(&mut scene, 0.5, [1.0, 1.0, 1.0, 1.0]);
    let still = textured_frame(&device, &scene, Air::CLEAR_BLACK)?;
    // Phase zero: whatever swings, swings by the ripple term alone.
    let rippled = textured_frame(
        &device,
        &scene,
        Air::CLEAR_BLACK.swaying([0.4, 0.0], 0.0, 2.0),
    )?;
    let lockstep = textured_frame(
        &device,
        &scene,
        Air::CLEAR_BLACK.swaying([0.4, 0.0], 0.0, 0.0),
    )?;
    assert_ne!(
        rippled, still,
        "a ripple across the world changed nothing, so fields move in lockstep"
    );
    assert_eq!(
        lockstep, still,
        "a lockstep swing at phase zero moved something, so the ripple test measures noise"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// **The second reach word pushes along the second world axis.** World
/// z is steered onto clip x by a permuted camera, so the half-quad
/// must vacate its old ground and claim new ground exactly as the
/// x-axis test's quad does — and an axis swap that pushed y instead
/// would move it vertically, which the row probes cannot mistake.
///
/// Probed by swapping the displacement to `vec3(sway.x, sway.y, 0.0)`
/// in the vertex stage: nothing moves along z and the claimed-ground
/// probe names it.
#[test]
fn the_second_reach_word_pushes_along_z() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    // World z out to clip x, world x into clip z: a z push becomes a
    // horizontal move on screen.
    let steered = [
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    // The quad spans world z in [-1, 0] at world x = 0.5: on screen,
    // the left half at depth 0.5, exactly the half_quad picture.
    let mut scene = Scene::new();
    scene.quad(
        [
            [0.5, -1.0, -1.0],
            [0.5, -1.0, 0.0],
            [0.5, 1.0, 0.0],
            [0.5, 1.0, -1.0],
        ],
        [1.0, 1.0, 1.0, 1.0],
    );
    let frame = |air: Air| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let camera = Camera::from_columns(steered).through(air);
        let mut target = device.create_offscreen_target(extent)?;
        let renderer =
            TexturedCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
        let mesh = renderer.upload(&device, &scene)?;
        let items = [renderer.item(&mesh, &camera)];
        target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        Ok(pixels)
    };
    let still = frame(Air::CLEAR_BLACK)?;
    // sin(pi/2) = 1: displacement is exactly the half-unit reach.
    let blown = frame(Air::CLEAR_BLACK.swaying([0.0, 0.5], std::f32::consts::FRAC_PI_2, 0.0))?;
    let vacated = SIZE / 8;
    let claimed = 5 * SIZE / 8;
    let row = SIZE / 2;
    assert!(
        at(&still, vacated, row)[0] > 0 && at(&still, claimed, row)[0] == 0,
        "the resting quad is not where this test thinks it is"
    );
    assert!(
        at(&blown, claimed, row)[0] > 0,
        "the second reach word claimed no new ground along z"
    );
    assert!(
        at(&blown, vacated, row)[0] == 0,
        "the quad smeared along z instead of moving"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// **The plain camera path stands still under swaying air.** The sway
/// words live in a block every camera pipeline binds, and only the
/// textured vertex stage reads them — a claim the docs make and this
/// holds behaviorally, so a swayer and the plain-drawn world beside it
/// cannot shear apart by accident of which pipeline read what.
#[test]
fn the_plain_path_ignores_the_sway() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let mut scene = Scene::new();
    half_quad(&mut scene, 0.5, [0.9, 0.8, 0.7, 1.0]);
    let frame = |air: Air| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let camera = Camera::from_columns(IDENTITY).through(air);
        let mut target = device.create_offscreen_target(extent)?;
        let renderer = CameraRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
        let mesh = renderer.upload(&device, &scene)?;
        let items = [renderer.item(&mesh, &camera)];
        target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        Ok(pixels)
    };
    let still = frame(Air::CLEAR_BLACK)?;
    let blown = frame(Air::CLEAR_BLACK.swaying([0.6, 0.6], 1.0, 1.0))?;
    assert_eq!(
        still, blown,
        "the plain path bent under swaying air, so a mixed frame shears apart"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// **The reach carries the weighted and the weightless hold still.** A
/// half-screen quad at full weight, pushed a quarter of clip space at
/// the top of its swing, must vacate its old left edge and cover ground
/// past its old right edge — a translation, not a smear. The same quad
/// at weight zero must sit exactly where the still frame put it.
///
/// Probed by zeroing the weight multiply in the vertex stage: the
/// weightless arm moves too, and the equality names it.
#[test]
fn the_reach_carries_the_weighted_and_pins_the_weightless() -> Result<(), Box<dyn std::error::Error>>
{
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    // sin(pi/2) = 1: the swing is at its top, so the displacement is
    // exactly the reach and the probes below are plain arithmetic.
    let wind = Air::CLEAR_BLACK.swaying([0.5, 0.0], std::f32::consts::FRAC_PI_2, 0.0);

    let mut weighted = Scene::new();
    half_quad(&mut weighted, 0.5, [1.0, 1.0, 1.0, 1.0]);
    let still = textured_frame(&device, &weighted, Air::CLEAR_BLACK)?;
    let blown = textured_frame(&device, &weighted, wind)?;
    // Clip x = -0.75: inside the quad at rest, vacated once it moves.
    let vacated = SIZE / 8;
    // Clip x = +0.25: past the quad's resting edge, covered once blown.
    let claimed = 5 * SIZE / 8;
    let row = SIZE / 2;
    assert!(
        at(&still, vacated, row)[0] > 0 && at(&still, claimed, row)[0] == 0,
        "the resting quad is not where this test thinks it is"
    );
    assert!(
        at(&blown, claimed, row)[0] > 0,
        "full weight at full swing claimed no new ground"
    );
    assert!(
        at(&blown, vacated, row)[0] == 0,
        "the quad smeared instead of moving: its old ground is still covered"
    );

    let mut weightless = Scene::new();
    half_quad(&mut weightless, 0.5, [1.0, 1.0, 1.0, 0.0]);
    let pinned = textured_frame(&device, &weightless, wind)?;
    for (x, name) in [(vacated, "its own ground"), (claimed, "new ground")] {
        let (was, is) = (at(&still, x, row), at(&pinned, x, row));
        assert_eq!(
            was[0..3],
            is[0..3],
            "a weightless vertex moved: {name} at column {x} was {was:?} and is {is:?}"
        );
    }
    assert_no_validation_errors(&device);
    Ok(())
}

/// **A swaying draw spends its weight before the mask reads it.** The
/// cutout pipeline discards where `texel.a * colour.a` falls below
/// half, and the sway borrows exactly that alpha as its bend weight —
/// so a rooted vertex at weight zero would vanish from a cutout the
/// moment its draw started swaying, unless the vertex stage hands the
/// fragment stage a one in its place. The grass this exists for is
/// rooted at weight zero everywhere it meets the ground.
///
/// Probed by forwarding the raw alpha in the vertex stage: the centre
/// goes to the clear colour and this names it.
#[test]
fn a_swaying_cutout_keeps_its_weightless_roots() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let camera =
        Camera::from_columns(IDENTITY).through(Air::CLEAR_BLACK.swaying([0.1, 0.0], 0.0, 0.0));
    let mut scene = Scene::new();
    // Weight zero everywhere: rooted vertices, in a draw that sways.
    full_quad(&mut scene, 0.5, [1.0, 1.0, 1.0, 0.0]);
    let mut target = device.create_offscreen_target(extent)?;
    let renderer =
        CutoutCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
    let mesh = renderer.upload(&device, &scene)?;
    let items = [renderer.item(&mesh, &camera)];
    target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    let centre = at(&pixels, SIZE / 2, SIZE / 2);
    assert!(
        centre[0] > 100,
        "a weightless vertex was discarded by the mask it sways under: centre {centre:?}"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// **The fade completes where the caller says, and silence means the
/// compiled default exactly.** Under this identity projection every
/// vertex sits at w = 1: against the compiled forty-eight that is a
/// fade of one part in forty-eight — a quad drawn essentially unfaded —
/// while a caller who says the fade completes at half a unit gets a
/// frame pulled its full fraction toward the horizon. And a caller
/// passing zero has said nothing, byte for byte, which is what lets a
/// consumer thread the value through unconditionally.
///
/// Probed by ignoring the distance word in the textured stage: the
/// near frame equals the far one and the inequality names it.
#[test]
fn the_fade_completes_where_the_caller_says() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let mut scene = Scene::new();
    half_quad(&mut scene, 0.5, [1.0, 1.0, 1.0, 1.0]);
    let silent = textured_frame(&device, &scene, Air::CLEAR_BLACK)?;
    let explicit_zero = textured_frame(&device, &scene, Air::CLEAR_BLACK.fading_over(0.0))?;
    assert_eq!(
        silent, explicit_zero,
        "zero is not the default it promises to be"
    );
    let near = textured_frame(&device, &scene, Air::CLEAR_BLACK.fading_over(0.5))?;
    let (x, row) = (SIZE / 4, SIZE / 2);
    let far_pixel = at(&silent, x, row);
    let near_pixel = at(&near, x, row);
    assert!(
        u32::from(near_pixel[0]) * 10 < u32::from(far_pixel[0]) * 7,
        "a fade completing at half a unit did not darken the quad: {near_pixel:?} \
         against {far_pixel:?}"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// One blended frame: a solid red backdrop through the opaque textured
/// pipeline, then `layers` drawn through the blended one, in order.
/// **A surface that only leans reads as a rigid sheet sliding.** The
/// sway displaces across the ground plane and nowhere else, so a flat
/// draw under it translates - every vertex the same way at the same
/// moment - and a plane moving sideways looks like a plane moving
/// sideways however small the throw. `bend.w` is the vertical half, a
/// quarter turn behind, so a vertex traces an ellipse instead of a
/// line.
///
/// **The quarter turn is what this test is really about**, and the
/// phase makes it visible: at phase zero the lean is `sin(0)`, which is
/// nothing at all, while the lift is `cos(0)`, which is the whole
/// reach. So the leaning arm here is byte-identical to the still one -
/// if the two halves shared a phase, it could not be - and every pixel
/// the lifted arm moves is the lift's doing.
///
/// The columns are held as well as the rows: a vertical reach must not
/// move anything sideways, or the two words are not independent and a
/// caller cannot tune one without the other.
///
/// Probed by driving the vertical from `sin` like the lean: at phase
/// zero both halves are then nothing, the lifted arm stops moving at
/// all, and "left the quad's edge exactly where it was" names it. That
/// is the failure a shared phase produces here, and it is worth being
/// exact about - the first assertion would catch a lift driven from
/// something that is *not* zero at phase zero, which is the other way
/// to get this wrong.
#[test]
fn the_lift_moves_a_draw_the_lean_leaves_still() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    // Phase zero: sin is nothing, cos is everything.
    let calm = Air::CLEAR_BLACK.swaying([0.5, 0.0], 0.0, 0.0);
    let mut quad = Scene::new();
    // A quad over the lower-left quarter of clip space, so there is
    // room above it to move into and an edge to measure.
    quad.quad(
        [
            [-1.0, -1.0, 0.5],
            [0.0, -1.0, 0.5],
            [0.0, 0.0, 0.5],
            [-1.0, 0.0, 0.5],
        ],
        [1.0, 1.0, 1.0, 1.0],
    );
    let still = textured_frame(&device, &quad, Air::CLEAR_BLACK)?;
    let leaning = textured_frame(&device, &quad, calm)?;
    let lifted = textured_frame(&device, &quad, calm.lifting(0.5))?;

    assert_eq!(
        still, leaning,
        "a swing of sin(0) moved something: the lean and the lift share a phase"
    );

    // Where the quad's edge sits in a column that runs through it, and
    // which columns it covers in a row that runs through it. Read from
    // the picture rather than worked out from the matrix, because which
    // way clip y points is the API's business and not this claim's.
    let covered = |pixels: &[u8], x: u32, y: u32| at(pixels, x, y)[0] > 0;
    let edge = |pixels: &[u8]| -> Option<u32> {
        let column = SIZE / 4;
        (0..SIZE).find(|y| covered(pixels, column, *y))
    };
    let width = |pixels: &[u8]| -> usize {
        let row = SIZE / 4;
        (0..SIZE).filter(|x| covered(pixels, *x, row)).count()
    };

    let resting = edge(&still).expect("the quad is drawn at all");
    let raised = edge(&lifted).expect("the lifted quad is drawn at all");
    assert_ne!(
        resting, raised,
        "a lift of half a unit left the quad's edge exactly where it was"
    );
    assert_eq!(
        width(&still),
        width(&lifted),
        "the lift moved the quad sideways: the vertical word is not independent of the lean"
    );
    Ok(())
}

/// **The even weight moves what alpha pins.** `bend.z` is where a
/// swaying draw's weight rides when its alpha is spoken for: a quad
/// authored at alpha zero — pinned under the alpha contract, and the
/// control arm proves it stays pinned — crosses the frame the moment
/// the air carries an even weight of one. Same reach, same swing, same
/// mesh; the only difference is which word the vertex stage weighed.
///
/// Probed by weighing the vertex alpha regardless of the even word:
/// the blown arm stays pinned and "claimed no new ground" names it.
#[test]
fn the_even_weight_moves_what_alpha_pins() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    // sin(pi/2) = 1: full swing, so the displacement is exactly the
    // reach and the probe columns are the weighted golden's own.
    let wind = Air::CLEAR_BLACK.swaying([0.5, 0.0], std::f32::consts::FRAC_PI_2, 0.0);
    let mut weightless = Scene::new();
    half_quad(&mut weightless, 0.5, [1.0, 1.0, 1.0, 0.0]);
    let still = textured_frame(&device, &weightless, Air::CLEAR_BLACK)?;
    let pinned = textured_frame(&device, &weightless, wind)?;
    let blown = textured_frame(&device, &weightless, wind.bending_evenly(1.0))?;
    // Clip x = -0.75: inside the quad at rest. Clip x = +0.25: past its
    // resting edge, covered only if the quad moved.
    let vacated = SIZE / 8;
    let claimed = 5 * SIZE / 8;
    let row = SIZE / 2;
    assert!(
        at(&still, vacated, row)[0] > 0 && at(&still, claimed, row)[0] == 0,
        "the resting quad is not where this test thinks it is"
    );
    for (x, name) in [(vacated, "its own ground"), (claimed, "new ground")] {
        assert_eq!(
            at(&still, x, row)[0..3],
            at(&pinned, x, row)[0..3],
            "the control arm moved: an alpha-zero quad swayed with no even word, at {name}"
        );
    }
    assert!(
        at(&blown, claimed, row)[0] > 0,
        "an even weight of one at full swing claimed no new ground"
    );
    assert!(
        at(&blown, vacated, row)[0] == 0,
        "the quad smeared instead of moving: its old ground is still covered"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// One veil through the blended pair under a chosen air: the opaque red
/// backdrop at 0.3, a half-alpha green veil at 0.5, and the air on the
/// veil's camera alone — the fixture for holding what an even swayer
/// does to the alpha it was told to leave.
fn veiled_frame(device: &Device, air: Air) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let camera = Camera::from_columns(IDENTITY);
    let opaque =
        TexturedCameraRenderer::new(device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
    let blended =
        BlendedCameraRenderer::new(device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
    let mut backdrop = Scene::new();
    full_quad(&mut backdrop, 0.3, [1.0, 0.0, 0.0, 1.0]);
    let floor = opaque.upload(device, &backdrop)?;
    let mut layer = Scene::new();
    full_quad(&mut layer, 0.5, [0.0, 1.0, 0.0, 0.5]);
    let veil = blended.upload(device, &layer)?;
    let aired = camera.through(air);
    let items = [opaque.item(&floor, &camera), blended.item(&veil, &aired)];
    let mut target = device.create_offscreen_target(extent)?;
    target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    Ok(pixels)
}

/// **An even swayer leaves the alpha unspent.** On the blended pair
/// alpha is translucency, and spending it as bend weight is exactly
/// the conflict `bend.z` resolves: a half-alpha veil swaying at zero
/// swing — phase zero, no ripple, so the displacement arithmetic is
/// exact zeros and the geometry is byte-identical — must blend exactly
/// as the becalmed veil does. The discriminating arm sways without the
/// even word: alpha is spent, the veil turns opaque, and the frame
/// changes — which is what would silently happen to every translucent
/// swayer if this contract broke.
///
/// Probed by spending the alpha in the vertex stage even when the
/// weight rode the air: the even arm turns opaque and the first
/// equality names it.
#[test]
fn a_zero_swing_even_swayer_blends_like_a_still_one() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let wind = Air::CLEAR_BLACK.swaying([0.5, 0.0], 0.0, 0.0);
    let becalmed = veiled_frame(&device, Air::CLEAR_BLACK)?;
    let even = veiled_frame(&device, wind.bending_evenly(1.0))?;
    let spent = veiled_frame(&device, wind)?;
    assert_eq!(
        becalmed, even,
        "an even swayer at zero swing changed the blend: it spent the alpha it was told to leave"
    );
    assert_ne!(
        becalmed, spent,
        "the discriminator went dull: spending the alpha no longer changes the blend, so the equality above holds nothing"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// **An even weight of zero is the alpha contract.** The same
/// zero-means-default the fade distance rides: a caller that says
/// nothing — or says zero — gets the authored behaviour, byte for
/// byte, because the builder writes the same zeros the block was born
/// with.
#[test]
fn an_even_weight_of_zero_is_the_alpha_contract() {
    let wind = Air::CLEAR_BLACK.swaying([0.5, 0.0], 1.0, 0.7);
    assert_eq!(
        wind.bending_evenly(0.0).bytes(),
        wind.bytes(),
        "a zero even weight moved a byte; silent callers are no longer identical"
    );
}

/// **A lift of zero is the air that never asked for one.** The same
/// zero-means-default the fade distance and the even weight ride: a
/// caller that says nothing, or says zero, gets the picture it always
/// got, byte for byte, because the builder writes the same zeros the
/// block was born with. The vertical term is taken through the same
/// multiply whatever the reach, and zero times a cosine is zero.
#[test]
fn a_lift_of_zero_is_the_air_that_never_asked() {
    let wind = Air::CLEAR_BLACK.swaying([0.5, 0.0], 1.0, 0.7);
    assert_eq!(
        wind.lifting(0.0).bytes(),
        wind.bytes(),
        "a zero lift moved a byte; silent callers are no longer identical"
    );
    assert_ne!(
        wind.lifting(0.02).bytes(),
        wind.bytes(),
        "a lift that is not zero left the block unchanged, so nothing reads it"
    );
}

fn blended_frame(device: &Device, layers: &[Scene]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let camera = Camera::from_columns(IDENTITY);
    let opaque =
        TexturedCameraRenderer::new(device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
    let blended =
        BlendedCameraRenderer::new(device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
    // Reversed-Z: nearer is larger, so the backdrop sits at 0.3 and
    // every veil above it — the same convention the shadow golden's
    // floor-and-blocker fixture documents.
    let mut backdrop = Scene::new();
    full_quad(&mut backdrop, 0.3, [1.0, 0.0, 0.0, 1.0]);
    let floor = opaque.upload(device, &backdrop)?;
    let meshes: Vec<renew_rhi::Mesh> = layers
        .iter()
        .map(|layer| blended.upload(device, layer))
        .collect::<Result<_, _>>()?;
    let mut items = vec![opaque.item(&floor, &camera)];
    for mesh in &meshes {
        items.push(blended.item(mesh, &camera));
    }
    let mut target = device.create_offscreen_target(extent)?;
    target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    Ok(pixels)
}

/// **Half there means half its colour over what stood behind.** A
/// half-alpha green quad over a red backdrop must read as a red-green
/// mix — not as opaque green, which is what this pipeline drawn with
/// blending disabled produces, and not as pure red, which is a draw
/// that landed nowhere.
///
/// Probed by building the pipeline with `Blend::Opaque`: the mix
/// vanishes and the red channel names it.
#[test]
fn a_half_alpha_quad_mixes_with_what_stood_behind() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let mut veil = Scene::new();
    full_quad(&mut veil, 0.5, [0.0, 1.0, 0.0, 0.5]);
    let pixels = blended_frame(&device, &[veil])?;
    let centre = at(&pixels, SIZE / 2, SIZE / 2);
    assert!(
        centre[0] > 60 && centre[1] > 60,
        "a half-green veil over red lost one of its parents: {centre:?}"
    );
    assert!(
        centre[0] < 220 && centre[1] < 220,
        "a half-green veil over red kept a parent whole: {centre:?}"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// **The caller owes the order, and the pipeline makes that owing
/// visible.** Two overlapping translucent veils drawn in opposite
/// orders produce different frames — blending's own equation says so —
/// which is the sorting contract stated as arithmetic rather than as a
/// sentence in a doc. A pipeline where the swap changed nothing would
/// be one where blending was silently off.
///
/// Probed by giving both veils full alpha: order stops mattering for
/// the winner-takes-all case only because depth is tested, and the
/// assertion names the equality.
#[test]
fn swapping_two_veils_changes_the_picture() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let mut green = Scene::new();
    full_quad(&mut green, 0.5, [0.0, 1.0, 0.0, 0.5]);
    let mut blue = Scene::new();
    full_quad(&mut blue, 0.6, [0.0, 0.0, 1.0, 0.5]);
    let green_first = blended_frame(&device, &[green.clone(), blue.clone()])?;
    let blue_first = blended_frame(&device, &[blue, green])?;
    assert_ne!(
        green_first, blue_first,
        "swapping two translucent veils changed nothing, so blending is not blending"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// **Translucency leaves no footprint in depth.** A veil drawn first
/// must not occlude opaque geometry drawn after it at a farther depth
/// — the pipeline tests depth and never writes it, so a pond cannot
/// eat the ground behind it just because the pond drew first.
///
/// Probed with `DepthState::read_write` on the blended pipeline: the
/// late floor vanishes behind the veil and the red channel names it.
#[test]
fn a_veil_never_occludes_what_draws_after_it() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let camera = Camera::from_columns(IDENTITY);
    let opaque =
        TexturedCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
    let blended =
        BlendedCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
    let mut veil = Scene::new();
    full_quad(&mut veil, 0.5, [0.0, 1.0, 0.0, 0.5]);
    // Farther than the veil under reversed-Z: smaller.
    let mut floor = Scene::new();
    full_quad(&mut floor, 0.3, [1.0, 0.0, 0.0, 1.0]);
    let veil_mesh = blended.upload(&device, &veil)?;
    let floor_mesh = opaque.upload(&device, &floor)?;
    // The veil draws FIRST, the opaque floor after and farther away.
    let items = [
        blended.item(&veil_mesh, &camera),
        opaque.item(&floor_mesh, &camera),
    ];
    let mut target = device.create_offscreen_target(extent)?;
    target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    let centre = at(&pixels, SIZE / 2, SIZE / 2);
    assert!(
        centre[0] > 100,
        "the floor vanished behind a veil that drew first: {centre:?} — translucency \
         wrote depth"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// **The vertex colour tints the texel rather than being replaced by
/// it.** The colour is where face shading and corner darkening live; a
/// fragment stage that returned the texel alone would draw an evenly lit
/// world with a pattern on it, which is flat again.
#[test]
fn the_vertex_colour_tints_the_texture() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let camera = Camera::from_columns(IDENTITY);
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);

    let mut seen = Vec::new();
    for tint in [1.0f32, 0.4] {
        let mut target = device.create_offscreen_target(extent)?;
        let renderer =
            TexturedCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &white)?;
        let mut scene = Scene::new();
        full_quad(&mut scene, 0.5, [tint, tint, tint, 1.0]);
        let mesh = renderer.upload(&device, &scene)?;
        let items = [renderer.item(&mesh, &camera)];
        target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        seen.push(at(&pixels, SIZE / 2, SIZE / 2));
        drop(target);
        drop(renderer);
    }

    let (bright, dim) = (seen[0], seen[1]);
    assert!(
        dim[0] < bright[0],
        "a darker vertex colour must darken a white texture: {bright:?} then {dim:?}"
    );
    assert!(
        bright[0] > 200,
        "a white texture under a white colour should stay bright: {bright:?}"
    );

    assert_no_validation_errors(&device);
    Ok(())
}

/// As the plain renderers, and for the same reasons: the type's name has
/// to be there for a caller printing a struct that holds one, and the
/// pipeline's handle must not be.
#[test]
fn the_textured_renderers_name_themselves_without_leaking_a_handle()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: 2,
        height: 2,
    };
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);

    let through = TexturedCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, extent, &white)?;
    let shown = format!("{through:?}");
    assert!(shown.contains("TexturedCameraRenderer"), "got: {shown}");
    assert!(
        shown.contains(".."),
        "the omission should be visible: {shown}"
    );

    let plain = TexturedMeshRenderer::new(&device, TargetFormat::Rgba8Srgb, extent, &white)?;
    let shown = format!("{plain:?}");
    assert!(shown.contains("TexturedMeshRenderer"), "got: {shown}");
    assert!(
        shown.contains(".."),
        "the omission should be visible: {shown}"
    );
    Ok(())
}

/// The textured paths refuse an empty scene the same way every other
/// path does — the shared refusal is shared in fact, not intention.
#[test]
fn the_textured_paths_refuse_an_empty_scene() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: 2,
        height: 2,
    };
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);

    let through = TexturedCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, extent, &white)?;
    assert!(matches!(
        through.upload(&device, &Scene::new()),
        Err(Render3dError::EmptyScene)
    ));

    let plain = TexturedMeshRenderer::new(&device, TargetFormat::Rgba8Srgb, extent, &white)?;
    assert!(matches!(
        plain.upload(&device, &Scene::new()),
        Err(Render3dError::EmptyScene)
    ));
    Ok(())
}

/// **The plain textured path draws its texture**, with no camera in
/// sight: clip-space positions, one sampler, and the vertex colour as a
/// tint.
#[test]
fn the_plain_textured_path_draws_its_texture() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let blue: Vec<u8> = [30u8, 30, 220, 255].repeat(4);
    let mut target = device.create_offscreen_target(extent)?;
    let renderer =
        TexturedMeshRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &blue)?;
    let mut scene = Scene::new();
    full_quad(&mut scene, 0.5, [1.0, 1.0, 1.0, 1.0]);
    let mesh = renderer.upload(&device, &scene)?;

    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let items = [renderer.item(&mesh)];
    target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    let centre = at(&pixels, SIZE / 2, SIZE / 2);
    assert!(
        centre[2] > 150 && centre[0] < 90 && centre[1] < 90,
        "a blue texture drew {centre:?}"
    );

    drop(target);
    drop(renderer);
    assert_no_validation_errors(&device);
    Ok(())
}

/// **The shadow dims exactly what the light cannot see.** One frame:
/// a depth-only caster pass draws a white floor and a nearer blocker
/// into the map from a light shifted half a screen sideways, then the
/// camera draws both. In the light's frame the blocker covers a strip
/// of floor the CAMERA still sees plainly — so that strip must read
/// darker than the open floor beside it, by the pipeline's own
/// dimming, while a frame whose caster pass is empty lights both
/// strips alike. Same-frame determinism rides along: both frames are
/// drawn twice and compared byte for byte.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one fixture, four claims: the dim, the v-flip guard, the empty map, and the kept-map cadence"
)]
fn a_caster_between_light_and_floor_dims_the_floor() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];

    // The floor spans the whole target at depth 0.3; the blocker is a
    // nearer patch over clip x in [-0.25, 0.25] AND clip y in [-1, 0].
    // The light SHEARS x by half the depth (a translation would keep
    // its rays parallel to the camera's, hiding every cast directly
    // behind its caster): a ray through the blocker at depth 0.8 lands
    // on the floor at depth 0.3 shifted +0.25 in x, so the cast patch
    // is x in [0, 0.5] — half of it in the camera's plain view beside
    // the blocker.
    //
    // **Bounded in y deliberately.** A blocker spanning the full
    // height would make every column of the map constant, and a
    // shadow-uv lookup that flipped v would sample an identical texel
    // and pass — the classic silent bug this suite exists to catch.
    // Bounded, the cast covers clip y in [-1, 0] only: the top half of
    // the screen, since clip y points down.
    let mut scene = Scene::new();
    full_quad(&mut scene, 0.3, [1.0, 1.0, 1.0, 1.0]);
    scene.quad(
        [
            [-0.25, -1.0, 0.8],
            [0.25, -1.0, 0.8],
            [0.25, 0.0, 0.8],
            [-0.25, 0.0, 0.8],
        ],
        [1.0, 1.0, 1.0, 1.0],
    );
    let mut light_columns = IDENTITY;
    light_columns[2][0] = 0.5;
    // **One value, both halves.** The caster reads its light rows and
    // the lit pass reads all of it, so the map cannot be written with a
    // light the lit pass does not sample. Packing the light twice — a
    // `Camera` for the caster and a second copy here — was how a
    // row/column mistake used to move the cast a little instead of
    // making the shadow vanish.
    // **Through the default air, which is what makes this golden evidence.**
    // The shadowed path reads the fade's colour from the same block the
    // other camera paths do; `Air::CLEAR_BLACK` carries exactly the values
    // the shaders used to compile in, so this picture is the picture it
    // was before there was anything to say — and that it still matches is
    // the claim, not an accident of the arm never being taken.
    let camera = ShadowedCamera::from_columns(IDENTITY, light_columns).through(Air::CLEAR_BLACK);

    let renderer = ShadowedCameraRenderer::new(
        &device,
        TargetFormat::Rgba8Srgb,
        texture_extent,
        &white,
        256,
    )?;
    let mesh = renderer.upload(&device, &scene)?;

    // Three probes on the floor, all the same texel and the same
    // fade, differing only in what the map says about them:
    //   - `shadowed`: inside the cast patch (x in the strip, y in the
    //     blocked half),
    //   - `lit_beside`: the same row, open floor away from the strip,
    //   - `lit_below`: the same COLUMN, the half of the screen the
    //     bounded blocker does not cover — the v-flip discriminator.
    let shadowed_x = 11 * SIZE / 16; // clip x = 0.375: cast strip, beside the blocker
    let open_x = SIZE / 4; // clip x = -0.5: open floor
    let shadow_y = SIZE / 4; // clip y = -0.5: the blocked half (clip y points down)
    let open_y = 3 * SIZE / 4; // clip y = +0.5: below the blocker's reach
    let draw = |cast: bool| -> Result<(Vec<u8>, Vec<u8>), Box<dyn std::error::Error>> {
        let run = |target: &mut renew_rhi::OffscreenTarget|
        -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            let casting_items = [renderer.caster_item(&mesh, &camera)];
            let empty: [renew_rhi::Item; 0] = [];
            let shadow = if cast {
                renderer.shadow_pass(&casting_items)
            } else {
                renderer.shadow_pass(&empty)
            };
            let items = [renderer.item(&mesh, &camera)];
            let passes = [shadow, pass(&clear, &items)];
            target.render(&RenderDesc::new(&passes))?;
            let mut pixels = vec![0u8; target.byte_len()];
            target.read_back_into(&mut pixels);
            Ok(pixels)
        };
        let mut target = device.create_offscreen_target(extent)?;
        let first = run(&mut target)?;
        let second = run(&mut target)?;
        // The cadence contract, end to end, on the casting side: the
        // map is kept, so a frame may omit the caster pass outright
        // and its lit pass samples what the last casting frame stored
        // — the picture must be the shadowed picture, to the byte.
        // Same target as the frames above: the per-frame block buffer
        // belongs to one target by the buffer's own rule.
        if cast {
            let items = [renderer.item(&mesh, &camera)];
            let sampling_only = [pass(&clear, &items)];
            target.render(&RenderDesc::new(&sampling_only))?;
            let mut omitted = vec![0u8; target.byte_len()];
            target.read_back_into(&mut omitted);
            assert_eq!(
                second, omitted,
                "a frame that omitted the caster pass must sample what the last casting                  frame kept"
            );
        }
        Ok((first, second))
    };

    let (cast_first, cast_second) = draw(true)?;
    assert_eq!(
        cast_first, cast_second,
        "the same shadowed frame twice diverged"
    );
    let (open_first, open_second) = draw(false)?;
    assert_eq!(
        open_first, open_second,
        "the same open frame twice diverged"
    );

    let shadowed = at(&cast_first, shadowed_x, shadow_y);
    let lit_beside = at(&cast_first, open_x, shadow_y);
    let lit_below = at(&cast_first, shadowed_x, open_y);
    let unshadowed = at(&open_first, shadowed_x, shadow_y);
    // The patch is dimmed, not black: well below its lit neighbour,
    // and well above zero — the dim factor is a little over half, so a
    // threshold of 100 admits the shipped value (~138) while refusing
    // anything that reads as a hole.
    assert!(
        u32::from(shadowed[0]) * 10 < u32::from(lit_beside[0]) * 8,
        "the shadowed patch {shadowed:?} should be darker than the open floor {lit_beside:?}"
    );
    assert!(shadowed[0] > 100, "a shadow is a dimming, got {shadowed:?}");
    // The same column, the other half of the screen: the bounded
    // blocker casts nothing there, so it must read exactly like open
    // floor. A shadow-uv lookup with v flipped would put the cast
    // here instead, and this is what would catch it.
    assert_eq!(
        lit_below, lit_beside,
        "the half the blocker does not cover must be lit; a flipped shadow uv would \
         darken it instead"
    );
    // With an empty caster the same pixel reads like the open floor:
    // what dimmed it was the map's contents, nothing else.
    assert_eq!(
        unshadowed, lit_beside,
        "an empty map must light the patch exactly like the open floor"
    );
    // The renderer names itself without leaking a handle, as its
    // siblings do — asserted here because this suite is where a
    // device exists to build one.
    let shown = format!("{renderer:?}");
    assert!(shown.starts_with("ShadowedCameraRenderer"), "{shown}");
    assert!(!shown.contains("0x"), "a handle leaked into {shown}");

    drop(mesh);
    drop(renderer);
    assert_no_validation_errors(&device);
    Ok(())
}

/// **A scene light dims a shadowed world without moving its shadow** —
/// the thing this path could not do until the light and the light's
/// matrix fitted in one push block together.
///
/// No golden here could be this before: the shadowed path carried no
/// light, and the lit path carried no shadow, so a consumer wanting a
/// time of day *and* a sun had to pick one. The failure this refuses is
/// the plausible one — a picture that is correctly lit and wrongly
/// shadowed, or the reverse.
///
/// It asserts three things about the same two frames, drawn identically
/// but for the brightness:
///
/// 1. **The light dims.** Every probe is darker under a half light.
/// 2. **The light dims by the light's amount, and does so everywhere.**
///    Shadowed and open floor scale by the same ratio, so the light is a
///    multiplier over the whole scene rather than something folded into
///    the shadow term.
/// 3. **The shadow does not move.** The shadowed pixel is still darker
///    than its lit neighbour under the dimmer light, in the same places.
///
/// Ratios rather than absolute values, with the tolerance set from what
/// the adapter actually produced: the target is sRGB, so a half light is
/// not half a byte, and predicting the encoded value would be asserting
/// arithmetic nobody performed.
///
/// Probed by dropping the light multiply from the vertex stage (nothing
/// dims), by applying it to the shadow term instead of the surface (the
/// ratios diverge between shadowed and open floor), and by packing the
/// light where the rows belong (the shadow vanishes and assertion 3
/// fails).
#[test]
fn a_scene_light_dims_a_shadowed_world_without_moving_its_shadow()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let white = [0xffu8; 16];
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];

    // The same scene the shadow golden uses: a floor, and a bounded
    // blocker between it and the light.
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
    let mut light_columns = IDENTITY;
    light_columns[2][0] = 0.5;

    let renderer = ShadowedCameraRenderer::new(
        &device,
        TargetFormat::Rgba8Srgb,
        texture_extent,
        &white,
        256,
    )?;
    let mesh = renderer.upload(&device, &scene)?;
    let mut target = device.create_offscreen_target(extent)?;

    let mut draw = |brightness: [f32; 3]| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        // One record, both halves — the caster reads its light rows.
        let camera = ShadowedCamera::lit(IDENTITY, light_columns, brightness);
        let casting = [renderer.caster_item(&mesh, &camera)];
        let items = [renderer.item(&mesh, &camera)];
        let passes = [renderer.shadow_pass(&casting), pass(&clear, &items)];
        target.render(&RenderDesc::new(&passes))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        Ok(pixels)
    };

    let bright = draw([1.0, 1.0, 1.0])?;
    let dim = draw([0.5, 0.5, 0.5])?;

    // The shadow golden's own probe positions, for the same reasons.
    let shadowed_x = 11 * SIZE / 16;
    let open_x = SIZE / 4;
    let shadow_y = SIZE / 4;

    let bright_shadowed = at(&bright, shadowed_x, shadow_y);
    let bright_open = at(&bright, open_x, shadow_y);
    let dim_shadowed = at(&dim, shadowed_x, shadow_y);
    let dim_open = at(&dim, open_x, shadow_y);

    // 1 · The light dims, and the frames are not the same picture.
    assert_ne!(
        bright, dim,
        "a half light drew the same frame as a full one"
    );
    assert!(
        dim_open[0] < bright_open[0],
        "open floor did not dim: {dim_open:?} against {bright_open:?}"
    );
    assert!(
        dim_shadowed[0] < bright_shadowed[0],
        "shadowed floor did not dim: {dim_shadowed:?} against {bright_shadowed:?}"
    );

    // 2 · **By the amount asked for, measured in linear light.**
    //
    // An earlier version compared the shadowed ratio against the open
    // ratio and could not fail: both reduce to the same function of
    // brightness for any multiplicative light, so folding the light into
    // the shadow term instead of the surface renders bit-identically —
    // `shade * b` and `surface * b` are one product. That assertion was
    // unfalsifiable and its documented probe was wrong.
    //
    // Asking whether HALF a light halves the surface is falsifiable: a
    // light applied at the wrong strength, applied twice, or applied to
    // an already-encoded value all fail it. The target is sRGB, so the
    // bytes are decoded first — halving a light does not halve a byte,
    // and comparing the encoded values would be asserting a curve nobody
    // applied.
    let linear = |byte: u8| f64::from(renew_rhi::srgb::decode(byte));
    for (what, bright, dim) in [
        ("open floor", bright_open[0], dim_open[0]),
        ("shadowed floor", bright_shadowed[0], dim_shadowed[0]),
    ] {
        let ratio = linear(dim) / linear(bright);
        assert!(
            (ratio - 0.5).abs() < 0.03,
            "{what}: a half light scaled linear surface by {ratio:.3}, not by a half \
             ({dim} against {bright} encoded)"
        );
    }

    // 3 · And the shadow is still a shadow under the dimmer light: same
    // relationship, same places. A light that moved the cast would break
    // this while leaving 1 and 2 intact.
    assert!(
        u32::from(dim_shadowed[0]) * 10 < u32::from(dim_open[0]) * 8,
        "under a half light the shadowed patch {dim_shadowed:?} is not darker than the \
         open floor {dim_open:?}"
    );
    assert!(
        dim_shadowed[0] > 40,
        "a shadow is a dimming, not a hole, even under a dim light: {dim_shadowed:?}"
    );
    Ok(())
}

/// **A clear texel is not drawn, and does not hide what is behind it.**
///
/// The whole reason this pipeline exists. On the textured path a texture
/// with holes in it draws as a solid rectangle that also writes depth, so
/// the hole is opaque and it occludes; here the hole is a hole.
///
/// The oracle is two draws deep: a red quad at the back, a cutout quad in
/// front of it whose left half is clear. Where the texture is clear the
/// red must show through — which can only happen if the near fragment was
/// discarded before it wrote either colour or depth.
#[test]
fn a_clear_texel_shows_what_is_behind_it() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let camera = Camera::from_columns(IDENTITY);

    // Two texels wide: the left one clear, the right one opaque white.
    // One row, so the sampler cannot pick up a neighbour vertically.
    let texture_extent = Extent {
        width: 2,
        height: 1,
    };
    let masked: Vec<u8> = vec![
        0, 0, 0, 0, // left: nothing there
        255, 255, 255, 255, // right: solid
    ];

    let mut behind = Scene::new();
    full_quad(&mut behind, 0.2, [1.0, 0.0, 0.0, 1.0]);
    let mut cut = Scene::new();
    full_quad(&mut cut, 0.8, [0.0, 0.0, 1.0, 1.0]);

    let mut target = device.create_offscreen_target(extent)?;
    let plain = CameraRenderer::new(&device, TargetFormat::Rgba8Srgb)?;
    let cutout =
        CutoutCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &masked)?;
    let far = plain.upload(&device, &behind)?;
    let near = cutout.upload(&device, &cut)?;
    // The near one second, so if it does not discard it wins the depth
    // test and covers the red — which is exactly the failure this is
    // written to catch.
    let items = [plain.item(&far, &camera), cutout.item(&near, &camera)];
    target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);

    let through = at(&pixels, SIZE / 4, SIZE / 2);
    let solid = at(&pixels, SIZE * 3 / 4, SIZE / 2);
    assert!(
        through[0] > 150 && through[2] < 80,
        "the clear half did not show the red behind it: {through:?}"
    );
    assert!(
        solid[2] > 150 && solid[0] < 80,
        "the opaque half did not draw the blue in front: {solid:?}"
    );

    assert_no_validation_errors(&device);
    Ok(())
}

/// **What survives the cut is drawn exactly as the textured path draws
/// it**, tint and all — the pipelines differ in what they throw away, not
/// in how they shade what they keep.
///
/// Two pipelines over one fully-opaque texture must agree pixel for
/// pixel. If they ever stop, a world drawn half on each shows the seam.
#[test]
fn what_survives_is_shaded_like_the_textured_path() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let camera = Camera::from_columns(IDENTITY);
    // Opaque throughout, so nothing is cut and the only question is how
    // the survivors are shaded. Tinted by the vertex colour, so the
    // comparison covers the multiply as well as the fetch.
    let opaque: Vec<u8> = [200u8, 180, 60, 255].repeat(4);
    let mut scene = Scene::new();
    full_quad(&mut scene, 0.5, [0.5, 0.8, 1.0, 1.0]);

    let mut seen: Vec<Vec<u8>> = Vec::new();
    {
        let mut target = device.create_offscreen_target(extent)?;
        let renderer =
            TexturedCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &opaque)?;
        let mesh = renderer.upload(&device, &scene)?;
        let items = [renderer.item(&mesh, &camera)];
        target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        seen.push(pixels);
    }
    {
        let mut target = device.create_offscreen_target(extent)?;
        let renderer =
            CutoutCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &opaque)?;
        let mesh = renderer.upload(&device, &scene)?;
        let items = [renderer.item(&mesh, &camera)];
        target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        seen.push(pixels);
    }

    assert_eq!(
        seen[0], seen[1],
        "the two textured paths shade an opaque texture differently"
    );
    assert_no_validation_errors(&device);
    Ok(())
}

/// The threshold reads the vertex colour's alpha too, so a caller can
/// fade a whole draw out rather than having it stay solid and then
/// vanish.
#[test]
fn a_faded_draw_is_cut_away() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let texture_extent = Extent {
        width: 2,
        height: 2,
    };
    let clear = [renew_rhi::color_attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let camera = Camera::from_columns(IDENTITY);
    let opaque: Vec<u8> = [255u8, 255, 255, 255].repeat(4);

    // The same opaque texture, drawn once at full alpha and once faded
    // below the threshold.
    let mut seen = Vec::new();
    for alpha in [1.0_f32, 0.25] {
        let mut scene = Scene::new();
        full_quad(&mut scene, 0.5, [1.0, 1.0, 1.0, alpha]);
        let mut target = device.create_offscreen_target(extent)?;
        let renderer =
            CutoutCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, texture_extent, &opaque)?;
        let mesh = renderer.upload(&device, &scene)?;
        let items = [renderer.item(&mesh, &camera)];
        target.render(&RenderDesc::new(&[pass(&clear, &items)]))?;
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        seen.push(at(&pixels, SIZE / 2, SIZE / 2));
    }

    assert!(
        seen[0][0] > 150,
        "a full-alpha draw was cut away: {:?}",
        seen[0]
    );
    assert!(
        seen[1][0] < 40 && seen[1][1] < 40,
        "a faded draw was kept: {:?}",
        seen[1]
    );
    assert_no_validation_errors(&device);
    Ok(())
}
