//! Drawing the world into a PNG, offscreen.
//!
//! Behind the `render` feature, so the game a player runs carries no
//! graphics crate at all — the same shape the platformer uses for its
//! window, and the reason the build matrix can still prove the sample
//! removable.
//!
//! This module is the only one here that names a rendering crate. What it
//! draws comes from [`crate::mesh`], where it lands comes from
//! [`crate::projection`], and what it writes comes from [`crate::png`];
//! all three are pure and tested without a device.

use renew_render3d::{MeshRenderer, Scene, attachment, pass};
use renew_rhi::{Color, Device, DeviceDesc, Extent, RenderDesc, TargetFormat, Validation};
use renew_sample_cube_world::grid::Grid;

use crate::mesh::{colour, faces};
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
const BACKDROP: Color = Color::new(0.09, 0.10, 0.13, 1.0);

/// Draw `grid` and write it to `path` as a PNG.
///
/// # Errors
///
/// [`RenderError`], which distinguishes a machine with no GPU from a
/// refusal by the renderer — the first is not the sample's fault and the
/// caller may reasonably carry on.
pub fn to_png(grid: &Grid, path: &std::path::Path) -> Result<(), RenderError> {
    let pixels = draw(grid)?;
    let png = crate::png::encode(SIZE, SIZE, &pixels)
        .ok_or_else(|| RenderError::Output("the encoder refused the image".to_string()))?;
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
    let scene = build(grid);
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
        .upload(&device, &scene)
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
    let view = Projection::isometric([-20.5, -0.5, -20.5], [20.5, 11.5, 20.5]);
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
        scene.quad(
            [
                view.project(corners[0]),
                view.project(corners[1]),
                view.project(corners[2]),
                view.project(corners[3]),
            ],
            colour(quad.block, quad.face),
        );
    }
    scene
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
