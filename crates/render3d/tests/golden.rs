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
    Camera, CameraRenderer, MeshRenderer, Render3dError, Scene, ShadowMatrices,
    ShadowedCameraRenderer, TexturedCameraRenderer, TexturedMeshRenderer, attachment, pass,
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
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)?;

    let mut scene = Scene::new();
    full_quad(&mut scene, 0.5, [0.0, 1.0, 0.0, 1.0]);
    let mesh = renderer.upload(&device, &scene)?;

    // Magenta appears nowhere in the geometry, so a quad that failed to
    // cover shows as unwritten rather than as a plausible colour.
    let color = [attachment(Color::new(1.0, 0.0, 1.0, 1.0))];
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
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
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
        let color = [attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
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
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
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
        let color = [attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
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
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
    let mut scene = Scene::new();
    full_quad(&mut scene, 0.5, [0.0, 1.0, 0.0, 1.0]);
    let mesh = renderer.upload(&device, &scene)?;

    let color = [attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
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
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
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
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
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
    let clear = [attachment(Color::new(1.0, 0.0, 1.0, 1.0))];
    let mut scene = Scene::new();
    half_quad(&mut scene, 0.5, [0.0, 1.0, 0.0, 1.0]);

    let mut plain_target = device.create_offscreen_target(extent)?;
    let plain = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
    let plain_mesh = plain.upload(&device, &scene)?;
    let plain_items = [plain.item(&plain_mesh)];
    plain_target.render(&RenderDesc::new(&[pass(&clear, &plain_items)]))?;
    let mut plain_pixels = vec![0u8; plain_target.byte_len()];
    plain_target.read_back_into(&mut plain_pixels);

    let mut camera_target = device.create_offscreen_target(extent)?;
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
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
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Unorm)?;

    let mut scene = Scene::new();
    half_quad(&mut scene, 0.5, [0.0, 1.0, 0.0, 1.0]);
    let mesh = through.upload(&device, &scene)?;

    let mut columns = IDENTITY;
    columns[3][0] = 0.5;
    let camera = Camera::from_columns(columns);

    let clear = [attachment(Color::new(1.0, 0.0, 1.0, 1.0))];
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
#[test]
fn the_camera_path_fades_with_distance() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
    let clear = [attachment(Color::new(1.0, 0.0, 1.0, 1.0))];

    // Columns of a matrix whose last row is (0, 0, 1, 0): w becomes z.
    let mut columns = IDENTITY;
    columns[2][3] = 1.0;
    columns[3][3] = 0.0;
    let camera = Camera::from_columns(columns);

    let mut seen = Vec::new();
    for depth in [4.0f32, 40.0] {
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
    assert_ne!(
        near,
        [255, 0, 255, 255],
        "the near quad did not draw at all, so the comparison would be vacuous"
    );
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

/// The camera path refuses an empty scene the same way the mesh path
/// does — the shared refusal is shared in fact, not only in intention.
#[test]
fn the_camera_path_refuses_an_empty_scene_too() -> Result<(), Box<dyn std::error::Error>> {
    let Some(device) = device_or_skip()? else {
        return Ok(());
    };
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
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
    let through = CameraRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
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
    let clear = [attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
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
            TargetFormat::Rgba8Unorm,
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
    let clear = [attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
    let camera = Camera::from_columns(IDENTITY);
    let white: Vec<u8> = [255u8, 255, 255, 255].repeat(4);

    let mut seen = Vec::new();
    for tint in [1.0f32, 0.4] {
        let mut target = device.create_offscreen_target(extent)?;
        let renderer =
            TexturedCameraRenderer::new(&device, TargetFormat::Rgba8Unorm, texture_extent, &white)?;
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

    let through = TexturedCameraRenderer::new(&device, TargetFormat::Rgba8Unorm, extent, &white)?;
    let shown = format!("{through:?}");
    assert!(shown.contains("TexturedCameraRenderer"), "got: {shown}");
    assert!(
        shown.contains(".."),
        "the omission should be visible: {shown}"
    );

    let plain = TexturedMeshRenderer::new(&device, TargetFormat::Rgba8Unorm, extent, &white)?;
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

    let through = TexturedCameraRenderer::new(&device, TargetFormat::Rgba8Unorm, extent, &white)?;
    assert!(matches!(
        through.upload(&device, &Scene::new()),
        Err(Render3dError::EmptyScene)
    ));

    let plain = TexturedMeshRenderer::new(&device, TargetFormat::Rgba8Unorm, extent, &white)?;
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
        TexturedMeshRenderer::new(&device, TargetFormat::Rgba8Unorm, texture_extent, &blue)?;
    let mut scene = Scene::new();
    full_quad(&mut scene, 0.5, [1.0, 1.0, 1.0, 1.0]);
    let mesh = renderer.upload(&device, &scene)?;

    let clear = [attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
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
    let clear = [attachment(Color::new(0.0, 0.0, 0.0, 1.0))];

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
    let camera = Camera::from_columns(light_columns);
    let matrices = ShadowMatrices::from_columns(IDENTITY, light_columns);

    let renderer = ShadowedCameraRenderer::new(
        &device,
        TargetFormat::Rgba8Unorm,
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
            let items = [renderer.item(&mesh, &matrices)];
            let passes = [shadow, pass(&clear, &items)];
            target.render(&RenderDesc::new(&passes))?;
            let mut pixels = vec![0u8; target.byte_len()];
            target.read_back_into(&mut pixels);
            Ok(pixels)
        };
        let mut target = device.create_offscreen_target(extent)?;
        let first = run(&mut target)?;
        let second = run(&mut target)?;
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
