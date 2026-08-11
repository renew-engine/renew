//! Drawing the world into a PNG, offscreen.
//!
//! Behind the `render` feature, so the game a player runs carries no
//! graphics crate at all — the same shape the platformer uses for its
//! window, and the reason the build matrix can still prove the sample
//! removable.
//!
//! This module is the only one here that names a rendering crate. What it
//! draws comes from [`crate::mesh`], where it lands comes from
//! [`crate::projection`], and what it writes comes from [`renew_png`];
//! all three are pure and tested without a device.

use renew_render3d::{
    Camera as RenderCamera, MeshRenderer, Scene, ShadowMatrices, ShadowedCameraRenderer,
    TexturedMeshRenderer, attachment, pass,
};
use renew_rhi::{Color, Device, DeviceDesc, Extent, RenderDesc, TargetFormat, Validation};
use renew_sample_cube_world::grid::{Cell, Grid};

use crate::mesh::{aimed_colour, colour, corner_shades, faces};
use crate::projection::Projection;

/// Why a render did not happen.
#[derive(Debug)]
pub enum RenderError {
    /// No Vulkan runtime, or no adapter. Not a failure of the sample:
    /// a machine without a GPU still runs everything else here.
    NoDevice(String),
    /// The rendering stack refused something.
    Refused(String),
    /// The world had no visible geometry to draw.
    Empty,
    /// The image could not be encoded, or the file could not be written.
    Output(String),
}

impl core::fmt::Display for RenderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoDevice(why) => write!(f, "no graphics device: {why}"),
            Self::Refused(why) => write!(f, "the renderer refused the frame: {why}"),
            Self::Empty => write!(f, "the world has no faces turned toward the viewer"),
            Self::Output(why) => write!(f, "writing the image: {why}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// The picture's edge length in pixels.
///
/// Square because the view is isometric and the world is a cube: a wider
/// frame would be margin. Small enough that the committed file stays a
/// couple of kilobytes, large enough that a block is several pixels.
pub const SIZE: u32 = 512;

/// The shadow map's side, in texels.
///
/// The sun's box spans the arena's bounding sphere — about 59 world
/// units across for this arena — so 2048 texels is roughly 35 per
/// world unit, or 35 across one block's face. **Chosen by comparing
/// pictures, not by taste**: at 1024 the shadow the far wall throws
/// across the floor in `digging.png` carried a step visible at the
/// committed size, and doubling the side halved it. It still steps
/// where the light grazes a surface — one unfiltered tap decides
/// every edge (DEBT-0067) — but at block scale the edges read clean.
/// The map costs sixteen megabytes of device memory.
pub const SHADOW_MAP_SIZE: u32 = 2048;

/// The background. Not black and not any face colour, so a gap in the
/// geometry reads as a gap rather than as a shadowed surface.
const BACKDROP: Color = Color::new(
    renew_rhi::builtin::HORIZON[0],
    renew_rhi::builtin::HORIZON[1],
    renew_rhi::builtin::HORIZON[2],
    1.0,
);

/// Draw `grid` and write it to `path` as a PNG.
///
/// # Errors
///
/// [`RenderError`], which distinguishes a machine with no GPU from a
/// refusal by the renderer — the first is not the sample's fault and the
/// caller may reasonably carry on.
pub fn to_png(grid: &Grid, path: &std::path::Path) -> Result<(), RenderError> {
    let pixels = draw(grid)?;
    let png = renew_png::encode(SIZE, SIZE, &pixels)
        .map_err(|error| RenderError::Output(error.to_string()))?;
    std::fs::write(path, png).map_err(|error| RenderError::Output(error.to_string()))
}

/// Draw `grid` and hand back the raw RGBA pixels.
///
/// Separate from [`to_png`] so a test can assert on the picture without
/// touching the filesystem.
///
/// # Errors
///
/// As [`to_png`], less the file.
pub fn draw(grid: &Grid) -> Result<Vec<u8>, RenderError> {
    draw_clip_space(&build(grid), ClipSurface::Textured)
}

/// What a clip-space draw samples.
///
/// **Explicit, because getting it wrong is invisible until you look.**
/// Drawing an overlay through the world's pipeline samples the block
/// atlas at whatever coordinates the overlay happened to carry — which
/// tints a white crosshair grey and looks like a shading bug rather than
/// a wrong pipeline. The caller knows which it wants; this makes it say
/// so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipSurface {
    /// The vertex colour, sampling nothing. Overlays: a crosshair is not
    /// made of stone.
    Flat,
    /// The block atlas, tinted by the vertex colour. The world.
    Textured,
}

/// Draw a scene whose positions are **already clip space**, offscreen.
///
/// The guts of [`draw`], named because the world is not the only thing
/// drawn that way: an overlay is clip space by definition, and a test
/// that wants to look at one needs somewhere to draw it that is not a
/// window.
///
/// # Errors
///
/// As [`draw`].
pub fn draw_clip_space(scene: &Scene, surface: ClipSurface) -> Result<Vec<u8>, RenderError> {
    if scene.is_empty() {
        return Err(RenderError::Empty);
    }

    let device = Device::new(&DeviceDesc {
        app_name: "cube",
        validation: Validation::IfAvailable,
    })
    .map_err(|error| RenderError::NoDevice(error.to_string()))?;

    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device
        .create_offscreen_target(extent)
        .map_err(|error| RenderError::Refused(error.to_string()))?;

    // Either renderer, held for as long as the item that borrows it.
    let flat;
    let textured;
    let mesh;
    let items = match surface {
        ClipSurface::Flat => {
            flat = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)
                .map_err(|error| RenderError::Refused(error.to_string()))?;
            mesh = flat
                .upload(&device, scene)
                .map_err(|error| RenderError::Refused(error.to_string()))?;
            [flat.item(&mesh)]
        }
        ClipSurface::Textured => {
            textured = TexturedMeshRenderer::new(
                &device,
                TargetFormat::Rgba8Unorm,
                Extent {
                    width: crate::atlas::WIDTH,
                    height: crate::atlas::HEIGHT,
                },
                &crate::atlas::pixels(),
            )
            .map_err(|error| RenderError::Refused(error.to_string()))?;
            mesh = textured
                .upload(&device, scene)
                .map_err(|error| RenderError::Refused(error.to_string()))?;
            [textured.item(&mesh)]
        }
    };

    let color = [attachment(BACKDROP)];
    let passes = [pass(&color, &items)];
    target
        .render(&RenderDesc::new(&passes))
        .map_err(|error| RenderError::Refused(error.to_string()))?;

    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    Ok(pixels)
}

/// The scene a grid draws to: its visible faces, turned toward the eye,
/// in clip space.
///
/// Pure, so the geometry can be counted without a device.
#[must_use]
pub fn build(grid: &Grid) -> Scene {
    let view = Projection::isometric(low_corner(grid), high_corner(grid));
    let mut scene = Scene::new();
    for quad in faces(grid) {
        let (dx, dy, dz) = quad.face.step();
        let normal = [normalised(dx), normalised(dy), normalised(dz)];
        // The cutaway. Without it the nearest thing to the eye is the
        // underside of the near wall, which fills the frame.
        if !view.faces_viewer(normal) {
            continue;
        }
        let corners = quad.corners();
        let paint = colour(quad.block, quad.face);
        // The same corner darkening the perspective views get. It is a
        // property of the geometry, so the view it is drawn through
        // changes nothing about which corners are enclosed.
        let shaded = corner_shades(grid, quad).map(|shade| {
            [
                paint[0] * shade,
                paint[1] * shade,
                paint[2] * shade,
                paint[3],
            ]
        });
        scene.quad_uv(
            [
                view.project(corners[0]),
                view.project(corners[1]),
                view.project(corners[2]),
                view.project(corners[3]),
            ],
            shaded,
            crate::atlas::tile_uv(crate::atlas::tile_for(quad.face)),
        );
    }
    scene
}

/// The scene for a camera: every visible face, in **world** space.
///
/// The isometric path projects on the way in and hands the renderer clip
/// space; this hands over the world and lets the matrix do it on the GPU.
/// The difference is not a preference — a camera inside a room has
/// geometry behind it, and only a real `w` and the hardware clipper deal
/// with that.
///
/// No facing filter here either. The cutaway exists because an *outside*
/// view of a closed box shows the underside of the near wall; from inside
/// the room, the walls behind the viewer are what the clipper removes and
/// the depth test sorts.
#[must_use]
pub fn build_world_space(grid: &Grid, aimed: Option<Cell>) -> Scene {
    let mut scene = Scene::new();
    for quad in faces(grid) {
        let paint = if Some(quad.cell) == aimed {
            aimed_colour(quad.block, quad.face)
        } else {
            colour(quad.block, quad.face)
        };
        // Corner darkening rides on the colour rather than replacing it:
        // the face still says which way it points, and the corners say
        // where the geometry turns.
        let shades = corner_shades(grid, quad);
        let corners = shades.map(|shade| {
            [
                paint[0] * shade,
                paint[1] * shade,
                paint[2] * shade,
                paint[3],
            ]
        });
        scene.quad_uv(
            quad.corners(),
            corners,
            crate::atlas::tile_uv(crate::atlas::tile_for(quad.face)),
        );
    }
    scene
}

/// Draw `grid` seen through `camera`, and hand back the pixels.
///
/// # Errors
///
/// As [`draw`].
pub(crate) fn draw_through(
    grid: &Grid,
    camera: &crate::camera::Camera,
    aimed: Option<Cell>,
    overlay: Option<&Scene>,
    dust: Option<&renew_particles::ParticleSystem>,
) -> Result<Vec<u8>, RenderError> {
    let light = crate::camera::sun_light(low_corner(grid), high_corner(grid));
    draw_scene(
        &build_world_space(grid, aimed),
        &casting_scene(grid),
        camera,
        &light,
        overlay,
        dust,
    )
}

/// Draw an already-built scene through `camera`.
///
/// # Errors
///
/// As [`draw_through`].
pub(crate) fn draw_scene(
    scene: &Scene,
    caster: &Scene,
    camera: &crate::camera::Camera,
    light: &renew_camera::LightCamera,
    overlay: Option<&Scene>,
    dust: Option<&renew_particles::ParticleSystem>,
) -> Result<Vec<u8>, RenderError> {
    if scene.is_empty() {
        return Err(RenderError::Empty);
    }

    let device = Device::new(&DeviceDesc {
        app_name: "cube",
        validation: Validation::IfAvailable,
    })
    .map_err(|error| RenderError::NoDevice(error.to_string()))?;

    let extent = Extent {
        width: SIZE,
        height: SIZE,
    };
    let mut target = device
        .create_offscreen_target(extent)
        .map_err(|error| RenderError::Refused(error.to_string()))?;
    // The shadowed camera path — the same pipeline the window draws
    // with, because a still that lit the world differently would be a
    // picture of a world this sample does not render. The atlas is
    // generated, so building the renderer is where it is uploaded.
    let renderer = ShadowedCameraRenderer::new(
        &device,
        TargetFormat::Rgba8Unorm,
        Extent {
            width: crate::atlas::WIDTH,
            height: crate::atlas::HEIGHT,
        },
        &crate::atlas::pixels(),
        SHADOW_MAP_SIZE,
    )
    .map_err(|error| RenderError::Refused(error.to_string()))?;
    let mesh = renderer
        .upload(&device, scene)
        .map_err(|error| RenderError::Refused(error.to_string()))?;
    // The caster's own mesh: the same world minus the roof — see
    // `casting_scene` for why the sun's map leaves the roof out.
    let caster_mesh = renderer
        .upload(&device, caster)
        .map_err(|error| RenderError::Refused(error.to_string()))?;
    let light_packed = RenderCamera::from_columns(light.columns());
    let packed = ShadowMatrices::from_columns(camera.columns(), light.columns());

    // The overlay, if there is one: geometry that is already clip space
    // and so needs the pipeline that does not transform. Built here
    // rather than taken as a mesh because a mesh belongs to a device and
    // the caller has none.
    //
    // One `Option` of a pair, not a pair of `Option`s: the pipeline and
    // the mesh are made together or not at all, and two of them would
    // need every reader to work that out again.
    let over = match overlay {
        Some(overlay) if !overlay.is_empty() => {
            let plain = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)
                .map_err(|error| RenderError::Refused(error.to_string()))?;
            let uploaded = plain
                .upload(&device, overlay)
                .map_err(|error| RenderError::Refused(error.to_string()))?;
            Some((plain, uploaded))
        }
        _ => None,
    };

    // The dust, when a pool rides along and holds anything: its own
    // renderer per call like the textured one above, blended as media
    // because dust occludes rather than glows, its instances packed
    // into a scratch sized by the pool. Built before the item list
    // because the items borrow it.
    let dust_parts = match dust {
        Some(pool) if pool.live() > 0 => {
            let (side, tile) = crate::atlas::particle_pixels();
            let sprinkler = renew_particles::ParticleRenderer::new(
                &device,
                TargetFormat::Rgba8Unorm,
                Extent {
                    width: side,
                    height: side,
                },
                &tile,
                renew_particles::ParticleBlend::Alpha,
                pool.capacity(),
            )
            .map_err(|error| RenderError::Refused(error.to_string()))?;
            let mut instances =
                vec![0u8; pool.capacity() as usize * renew_particles::INSTANCE_STRIDE];
            let live = pool.write_instances(&mut instances);
            let (right, up, _) = camera.view.axes();
            let push = renew_particles::CameraPush::from_parts(
                camera.columns(),
                [right.x, right.y, right.z],
                [up.x, up.y, up.z],
            );
            Some((sprinkler, instances, live, push))
        }
        _ => None,
    };

    let color = [attachment(BACKDROP)];
    // The world first, whatever is over it second. Both are in one pass:
    // the overlay sits at the near plane, so the depth test cannot put
    // the world in front of it, and the order settles it regardless.
    // Dust after the world and before the overlay: it tests the world's
    // depth without writing its own, and the crosshair stays on top of
    // everything because a sight that hides behind smoke is not a sight.
    let world_item = renderer.item(&mesh, &packed);
    let mut items = Vec::with_capacity(3);
    items.push(world_item);
    if let Some((sprinkler, instances, live, push)) = dust_parts.as_ref() {
        items.push(sprinkler.item(instances, *live, push));
    }
    if let Some((plain, mesh)) = over.as_ref() {
        items.push(plain.item(mesh));
    }
    // The caster pass leads: the world's depth as the sun sees it,
    // into the map the world item samples.
    let casting = [renderer.caster_item(&caster_mesh, &light_packed)];
    let passes = [renderer.shadow_pass(&casting), pass(&color, &items)];
    target
        .render(&RenderDesc::new(&passes))
        .map_err(|error| RenderError::Refused(error.to_string()))?;

    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
    Ok(pixels)
}

/// Draw `grid` through `camera` and write it to `path`.
///
/// # Errors
///
/// As [`draw_through`], plus the file.
pub fn to_png_through(
    grid: &Grid,
    camera: &crate::camera::Camera,
    path: &std::path::Path,
    aimed: Option<Cell>,
    overlay: Option<&Scene>,
    dust: Option<&renew_particles::ParticleSystem>,
) -> Result<(), RenderError> {
    let pixels = draw_through(grid, camera, aimed, overlay, dust)?;
    let png = renew_png::encode(SIZE, SIZE, &pixels)
        .map_err(|error| RenderError::Output(error.to_string()))?;
    std::fs::write(path, png).map_err(|error| RenderError::Output(error.to_string()))
}

/// The world as the sun's shadow map sees it: every solid except the
/// ceiling layer.
///
/// **The roof does not cast, and that is what lets a sun light a
/// closed box.** The arena's shell has a ceiling; a light outside it
/// would find every ray stopped there and dim the whole interior
/// uniformly, which is not a picture of anything. Leaving the top
/// layer out of the map is the honest form of "the roof is glass":
/// the walls, the mound and everything a player builds still cast,
/// and only the surface nobody looks at stops blocking the light.
///
/// The aim highlight is deliberately absent (`None`): what a player
/// is pointing at changes how a cell is *drawn*, never what it
/// blocks, and a caster that tracked the aim would remesh the arena
/// every time the mouse crossed a block boundary.
pub(crate) fn casting_scene(grid: &Grid) -> Scene {
    let (width, height, depth) = grid.size();
    let min = grid.min();
    let mut roofless = Grid::new(min, (width, height, depth));
    let top = min.y + height - 1;
    for (cell, block) in grid.solids() {
        if cell.y < top {
            roofless.set(cell, block);
        }
    }
    build_world_space(&roofless, None)
}

/// The world-space corner below every cell in `grid`.
///
/// **Derived rather than written down.** These were the arena's own
/// numbers, typed in — correct for the one world this sample builds and
/// silently wrong for any other, framing a box that no longer matches
/// what is drawn. A cell spans one unit centred on its integer, so the
/// world starts half a unit below the lowest cell.
pub(crate) fn low_corner(grid: &Grid) -> [f32; 3] {
    let min = grid.min();
    [
        world_edge(min.x, -1),
        world_edge(min.y, -1),
        world_edge(min.z, -1),
    ]
}

/// The world-space corner above every cell in `grid`.
pub(crate) fn high_corner(grid: &Grid) -> [f32; 3] {
    let min = grid.min();
    let (x, y, z) = grid.size();
    [
        world_edge(min.x.saturating_add(x).saturating_sub(1), 1),
        world_edge(min.y.saturating_add(y).saturating_sub(1), 1),
        world_edge(min.z.saturating_add(z).saturating_sub(1), 1),
    ]
}

/// One edge of a cell, half a unit out from its centre.
///
/// The same bound `Quad::corners` relies on: a coordinate large enough to
/// lose precision as an `f32` needs a grid too large to allocate.
#[expect(
    clippy::cast_precision_loss,
    reason = "a coordinate past 2^24 needs a grid too large to allocate"
)]
fn world_edge(cell: i32, side: i32) -> f32 {
    cell as f32 + (side as f32) * 0.5
}

/// A step component as a float. `Face::step` answers with -1, 0 or 1 and
/// nothing else, so there is no cast and nothing to lose.
fn normalised(component: i32) -> f32 {
    match component {
        1 => 1.0,
        -1 => -1.0,
        _ => 0.0,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    /// **The roof is left out of the shadow map, and only the roof.**
    /// This is what lets a sun light a closed box, so it is worth more
    /// than a picture a human must eyeball: a caster that kept the
    /// ceiling would dim the whole interior uniformly, and one that
    /// dropped a layer too many would stop a wall casting. The grid is
    /// deliberately lopsided with a non-zero minimum, because "the top
    /// layer" is arithmetic that a cubic grid at the origin cannot
    /// distinguish from several wrong answers.
    #[test]
    fn the_caster_drops_the_ceiling_and_nothing_else() {
        use renew_sample_cube_world::grid::{Cell, Grid};
        let min = Cell::new(-3, 5, 2);
        let mut grid = Grid::new(min, (4, 3, 5));
        let top = min.y + 3 - 1;
        // One solid on the ceiling, one directly below it.
        let roof_cell = Cell::new(-2, top, 4);
        let under_cell = Cell::new(-2, top - 1, 4);
        grid.fill(roof_cell, roof_cell, renew_sample_cube_world::grid::STONE);
        let roof_only = super::casting_scene(&grid);
        assert!(
            roof_only.is_empty(),
            "a world whose only solid is in the ceiling must cast nothing"
        );
        grid.fill(under_cell, under_cell, renew_sample_cube_world::grid::STONE);
        let with_block = super::casting_scene(&grid);
        assert!(
            !with_block.is_empty(),
            "a block below the ceiling must cast"
        );
        // And it casts as much as it would with no roof above it at
        // all: the ceiling contributes nothing either way.
        let mut roofless = Grid::new(min, (4, 3, 5));
        roofless.fill(under_cell, under_cell, renew_sample_cube_world::grid::STONE);
        assert_eq!(
            with_block.vertex_count(),
            super::casting_scene(&roofless).vertex_count(),
            "the ceiling changed what the block below it casts"
        );
    }

    use super::*;

    /// An all-air world has no faces, and that is data rather than a bug:
    /// a script that dug everything out would produce one.
    #[test]
    fn a_world_with_nothing_in_it_is_refused_by_name() {
        let empty = Grid::new(Cell::new(0, 0, 0), (2, 2, 2));
        let camera = crate::camera::free_view([4.0, 4.0, 4.0], [0.0, 0.0, 0.0], 1.0);

        // Bound first so `matches!` fits on one line: spread across
        // several, its non-matching arm is a region no passing run
        // executes, and the gate is right to say so.
        let through = draw_through(&empty, &camera, None, None, None);
        assert!(
            matches!(through, Err(RenderError::Empty)),
            "an empty world through a camera must be refused, not drawn"
        );
        assert!(
            matches!(draw(&empty), Err(RenderError::Empty)),
            "and the same holds for the view with no camera"
        );
    }

    /// Every refusal says what happened in words a reader can act on.
    ///
    /// Built by hand: three of the four need a graphics device to occur
    /// naturally, and what is under test is the wording rather than the
    /// occasion.
    #[test]
    fn every_refusal_says_what_it_was() {
        let cases = [
            (
                RenderError::NoDevice("no adapter".to_string()),
                "no graphics device",
            ),
            (
                RenderError::Refused("out of memory".to_string()),
                "refused the frame",
            ),
            (RenderError::Empty, "no faces"),
            (
                RenderError::Output("permission denied".to_string()),
                "writing the image",
            ),
        ];
        for (error, needle) in cases {
            let shown = error.to_string();
            assert!(shown.contains(needle), "`{shown}` missing `{needle}`");
        }
    }

    /// How many bytes two draws disagree on.
    ///
    /// Counted rather than compared whole: these buffers are a megabyte
    /// each, and an `assert_eq!` over them prints both on failure.
    fn differing_bytes(left: &[u8], right: &[u8]) -> usize {
        left.iter().zip(right).filter(|(a, b)| a != b).count()
    }

    /// Whether this lane refuses to skip.
    pub(crate) fn golden_strict() -> bool {
        std::env::var_os("RENEW_GOLDEN").is_some_and(|value| value == "1")
    }

    /// Pixels, or `None` meaning "this machine has no device; skip".
    ///
    /// **A function rather than a match arm, and the reason is coverage
    /// rather than tidiness.** Written inline, the skip and the panic are
    /// arms that run only when the machine has no driver or when the draw
    /// fails — so on every lane that *does* draw they are lines nothing
    /// executes, and on the lane that does not they are the only lines
    /// executed. Neither lane covers both. Passed the outcome and the
    /// strictness, all four cases can be driven from a test with values
    /// built by hand.
    ///
    /// # Panics
    ///
    /// On a refusal that is not an absent device: those are defects
    /// rather than absences. And on an absent device when `strict`,
    /// which is the lane that exists to run these — a skip there would
    /// let the oracle pass by not running.
    pub(crate) fn pixels_or_skip(
        outcome: Result<Vec<u8>, RenderError>,
        strict: bool,
    ) -> Option<Vec<u8>> {
        match outcome {
            Ok(pixels) => Some(pixels),
            Err(RenderError::NoDevice(why)) => {
                assert!(!strict, "RENEW_GOLDEN=1 but there is no device: {why}");
                eprintln!("SKIP: {why}");
                None
            }
            Err(other) => panic!("the draw failed for a reason that is not the device: {other}"),
        }
    }

    /// A present device hands the pixels back; an absent one is a skip.
    #[test]
    fn an_absent_device_is_a_skip_and_pixels_are_not() {
        assert_eq!(
            pixels_or_skip(Ok(vec![1, 2, 3]), false),
            Some(vec![1, 2, 3])
        );
        assert_eq!(
            pixels_or_skip(Err(RenderError::NoDevice("no adapter".to_string())), false),
            None
        );
    }

    /// On the lane that exists to run these, an absent device is a
    /// failure rather than a skip.
    #[test]
    #[should_panic(expected = "there is no device")]
    fn a_strict_lane_refuses_to_skip() {
        drop(pixels_or_skip(
            Err(RenderError::NoDevice("no adapter".to_string())),
            true,
        ));
    }

    /// Anything else is a defect, and says so rather than skipping past.
    #[test]
    #[should_panic(expected = "not the device")]
    fn any_other_refusal_is_a_failure() {
        drop(pixels_or_skip(
            Err(RenderError::Refused("out of memory".to_string())),
            false,
        ));
    }

    /// **The aim reaches the picture.** The window lights the block being
    /// aimed at; a still from the same viewpoint that left it out would
    /// be a picture of a different program, and the argument for drawing
    /// from the player's eyes at all is that the picture is evidence
    /// about the game.
    #[test]
    fn a_still_shows_the_block_being_aimed_at() {
        let grid = crate::arena();
        let camera = crate::camera::free_view([-8.0, 6.0, -10.0], [4.0, 1.5, 0.0], 1.0);

        // **The graceful skip, and the lane that refuses it.** Several
        // jobs build this crate on runners with no Vulkan driver at all —
        // the removability matrix, for one, whose subject is which crates
        // are in the graph rather than what they draw. Under
        // `RENEW_GOLDEN=1`, the lane that exists to run these, a skip is
        // a failure instead, so the oracle can never pass by not running.
        //
        // `if let` rather than an early return, so the skip costs no line
        // that a lane which draws can never execute.
        if let Some(plain) = pixels_or_skip(
            draw_through(&grid, &camera, None, None, None),
            golden_strict(),
        ) {
            assert_the_highlight_reaches_the_pixels(&grid, &camera, &plain);
        }
    }

    /// **The overlay reaches the still.** The window draws the crosshair
    /// every frame; the player's-eye still draws it for the same reason
    /// that view lights the aimed block. Nothing else asserts that the
    /// argument is used — the command-line run executes the branch
    /// without checking what came out of it.
    #[test]
    fn an_overlay_is_drawn_over_the_world() {
        let grid = crate::arena();
        let camera = crate::camera::free_view([-8.0, 6.0, -10.0], [4.0, 1.5, 0.0], 1.0);
        if let Some(plain) = pixels_or_skip(
            draw_through(&grid, &camera, None, None, None),
            golden_strict(),
        ) {
            assert_the_overlay_reaches_the_middle(&grid, &camera, &plain);
        }
    }

    /// **The dust reaches the still, deterministically, and only while
    /// it lives.** A young burst changes pixels; the same pool drawn
    /// twice is byte-identical, which is what lets a committed render
    /// stand as evidence; a pool whose particles have all died leaves
    /// the picture exactly alone, so the quiet path and the dead path
    /// are the same picture.
    #[test]
    fn dust_reaches_the_picture_only_while_it_lives() {
        let grid = crate::arena();
        let camera = crate::camera::free_view([-8.0, 6.0, -10.0], [4.0, 1.5, 0.0], 1.0);
        if let Some(plain) = pixels_or_skip(
            draw_through(&grid, &camera, None, None, None),
            golden_strict(),
        ) {
            assert_dust_reaches_only_while_alive(&grid, &camera, &plain);
        }
    }

    /// A living burst changes pixels, the same pool twice is
    /// byte-identical, and a dead pool changes nothing.
    fn assert_dust_reaches_only_while_alive(
        grid: &Grid,
        camera: &crate::camera::Camera,
        plain: &[u8],
    ) {
        // A burst in the open air between the eye and the mound, three
        // steps old: young enough that all of it lives, and in front
        // of everything solid so the depth test cannot silently
        // discard the evidence.
        let mut young = crate::burst::pool();
        young.burst([-2.0, 3.75, -5.0], crate::burst::BURST);
        for _ in 0..3 {
            young.step(crate::burst::DT);
        }
        let dusty = draw_through(grid, camera, None, None, Some(&young))
            .expect("the dusty draw should succeed");
        assert!(
            differing_bytes(plain, &dusty) > 0,
            "a living burst changed no pixel, so it never reached the pass"
        );
        let again = draw_through(grid, camera, None, None, Some(&young))
            .expect("the second dusty draw should succeed");
        assert_eq!(
            differing_bytes(&dusty, &again),
            0,
            "the same dust must draw the same picture"
        );

        // The same burst stepped past every possible lifetime.
        let mut dead = crate::burst::pool();
        dead.burst([-2.0, 3.75, -5.0], crate::burst::BURST);
        for _ in 0..60 {
            dead.step(crate::burst::DT);
        }
        assert_eq!(dead.live(), 0, "a second is longer than any dust lifetime");
        let after = draw_through(grid, camera, None, None, Some(&dead))
            .expect("the dead-pool draw should succeed");
        assert_eq!(
            differing_bytes(plain, &after),
            0,
            "dead dust must leave the picture exactly alone"
        );
    }

    /// An overlay marks the middle of the picture; an empty one marks
    /// nothing at all.
    fn assert_the_overlay_reaches_the_middle(
        grid: &Grid,
        camera: &crate::camera::Camera,
        plain: &[u8],
    ) {
        let overlay = crate::crosshair::scene(1.0);
        let marked = draw_through(grid, camera, None, Some(&overlay), None)
            .expect("the overlay draw should succeed");
        assert!(
            differing_bytes(plain, &marked) > 0,
            "an overlay changed no pixel, so it never reached the pass"
        );

        // And it is at the centre, where the aim goes — not merely
        // somewhere.
        let middle = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
        assert_ne!(
            marked[middle..middle + 3],
            plain[middle..middle + 3],
            "the overlay drew somewhere other than the middle of the picture"
        );

        // An empty overlay is not an error and not a mark: it is the same
        // picture. The guard that decides this is otherwise a branch
        // nothing takes, since the one real overlay is never empty.
        let nothing = Scene::new();
        let unmarked = draw_through(grid, camera, None, Some(&nothing), None)
            .expect("an empty overlay should draw the world and nothing else");
        assert_eq!(
            differing_bytes(plain, &unmarked),
            0,
            "an empty overlay must leave the picture alone"
        );
    }

    /// Lighting a visible block changes the picture; lighting one nobody
    /// can see changes nothing.
    fn assert_the_highlight_reaches_the_pixels(
        grid: &Grid,
        camera: &crate::camera::Camera,
        plain: &[u8],
    ) {
        // The mound spans x 2..=6, y 1..=2, z -2..=2, so this is a top
        // face with air above it and nothing between it and the eye.
        let visible = Cell::new(4, 2, 0);
        let aimed =
            draw_through(grid, camera, Some(visible), None, None).expect("the draw should succeed");
        assert!(
            differing_bytes(plain, &aimed) > 0,
            "lighting a visible block changed no pixel, so the aim never reached the scene"
        );

        // A block enclosed on all six sides is drawn by nobody, so
        // lighting it must change nothing. Without this the assertion
        // above would pass on a scene that lit everything.
        let enclosed = Cell::new(4, 1, 0);
        let hidden = draw_through(grid, camera, Some(enclosed), None, None)
            .expect("the draw should succeed");
        assert_eq!(
            differing_bytes(plain, &hidden),
            0,
            "an enclosed block has no face in the mesh, so lighting it must change no pixel"
        );
    }
}
