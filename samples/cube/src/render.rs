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
    Camera as RenderCamera, CameraRenderer, MeshRenderer, Scene, attachment, pass,
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
    draw_clip_space(&build(grid))
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
pub fn draw_clip_space(scene: &Scene) -> Result<Vec<u8>, RenderError> {
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
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)
        .map_err(|error| RenderError::Refused(error.to_string()))?;
    let mesh = renderer
        .upload(&device, scene)
        .map_err(|error| RenderError::Refused(error.to_string()))?;

    let color = [attachment(BACKDROP)];
    let items = [renderer.item(&mesh)];
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
        scene.quad_shaded(
            [
                view.project(corners[0]),
                view.project(corners[1]),
                view.project(corners[2]),
                view.project(corners[3]),
            ],
            shaded,
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
        scene.quad_shaded(quad.corners(), corners);
    }
    scene
}

/// Draw `grid` seen through `camera`, and hand back the pixels.
///
/// # Errors
///
/// As [`draw`].
pub fn draw_through(
    grid: &Grid,
    camera: &crate::camera::Camera,
    aimed: Option<Cell>,
    overlay: Option<&Scene>,
) -> Result<Vec<u8>, RenderError> {
    draw_scene(&build_world_space(grid, aimed), camera, overlay)
}

/// Draw an already-built scene through `camera`.
///
/// # Errors
///
/// As [`draw_through`].
pub fn draw_scene(
    scene: &Scene,
    camera: &crate::camera::Camera,
    overlay: Option<&Scene>,
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
    let renderer = CameraRenderer::new(&device, TargetFormat::Rgba8Unorm)
        .map_err(|error| RenderError::Refused(error.to_string()))?;
    let mesh = renderer
        .upload(&device, scene)
        .map_err(|error| RenderError::Refused(error.to_string()))?;
    let packed = RenderCamera::from_columns(camera.view_projection());

    // The overlay, if there is one: geometry that is already clip space
    // and so needs the pipeline that does not transform. Built here
    // rather than taken as a mesh because a mesh belongs to a device and
    // the caller has none.
    let (plain, plain_mesh) = match overlay {
        Some(overlay) if !overlay.is_empty() => {
            let plain = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)
                .map_err(|error| RenderError::Refused(error.to_string()))?;
            let uploaded = plain
                .upload(&device, overlay)
                .map_err(|error| RenderError::Refused(error.to_string()))?;
            (Some(plain), Some(uploaded))
        }
        _ => (None, None),
    };

    let color = [attachment(BACKDROP)];
    // The world first, whatever is over it second. Both are in one pass:
    // the overlay sits at the near plane, so the depth test cannot put
    // the world in front of it, and the order settles it regardless.
    let world_item = renderer.item(&mesh, &packed);
    let over = plain
        .as_ref()
        .zip(plain_mesh.as_ref())
        .map(|(plain, mesh)| plain.item(mesh));
    let mut items = Vec::with_capacity(2);
    items.push(world_item);
    if let Some(over) = over {
        items.push(over);
    }
    let passes = [pass(&color, &items)];
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
) -> Result<(), RenderError> {
    let pixels = draw_through(grid, camera, aimed, overlay)?;
    let png = renew_png::encode(SIZE, SIZE, &pixels)
        .map_err(|error| RenderError::Output(error.to_string()))?;
    std::fs::write(path, png).map_err(|error| RenderError::Output(error.to_string()))
}

/// The world-space corner below every cell in `grid`.
///
/// **Derived rather than written down.** These were the arena's own
/// numbers, typed in — correct for the one world this sample builds and
/// silently wrong for any other, framing a box that no longer matches
/// what is drawn. A cell spans one unit centred on its integer, so the
/// world starts half a unit below the lowest cell.
fn low_corner(grid: &Grid) -> [f32; 3] {
    let min = grid.min();
    [
        world_edge(min.x, -1),
        world_edge(min.y, -1),
        world_edge(min.z, -1),
    ]
}

/// The world-space corner above every cell in `grid`.
fn high_corner(grid: &Grid) -> [f32; 3] {
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
        let through = draw_through(&empty, &camera, None, None);
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
        if let Some(plain) =
            pixels_or_skip(draw_through(&grid, &camera, None, None), golden_strict())
        {
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
        if let Some(plain) =
            pixels_or_skip(draw_through(&grid, &camera, None, None), golden_strict())
        {
            let overlay = crate::crosshair::scene(1.0);
            let marked = draw_through(&grid, &camera, None, Some(&overlay))
                .expect("the overlay draw should succeed");
            assert!(
                differing_bytes(&plain, &marked) > 0,
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

            // An empty overlay is not an error and not a mark: it is the
            // same picture. The guard that decides this is otherwise a
            // branch nothing takes, since the one real overlay is never
            // empty.
            let nothing = Scene::new();
            let unmarked = draw_through(&grid, &camera, None, Some(&nothing))
                .expect("an empty overlay should draw the world and nothing else");
            assert_eq!(
                differing_bytes(&plain, &unmarked),
                0,
                "an empty overlay must leave the picture alone"
            );
        }
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
            draw_through(grid, camera, Some(visible), None).expect("the draw should succeed");
        assert!(
            differing_bytes(plain, &aimed) > 0,
            "lighting a visible block changed no pixel, so the aim never reached the scene"
        );

        // A block enclosed on all six sides is drawn by nobody, so
        // lighting it must change nothing. Without this the assertion
        // above would pass on a scene that lit everything.
        let enclosed = Cell::new(4, 1, 0);
        let hidden =
            draw_through(grid, camera, Some(enclosed), None).expect("the draw should succeed");
        assert_eq!(
            differing_bytes(plain, &hidden),
            0,
            "an enclosed block has no face in the mesh, so lighting it must change no pixel"
        );
    }
}
