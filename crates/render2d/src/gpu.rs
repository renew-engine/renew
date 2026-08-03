//! The device half: one atlas, one pipeline, one buffer, one draw.
//!
//! Everything that touches `renew_rhi` lives in this module — when the
//! pass model arrives, this file is the migration seam and `fill.rs`
//! does not move.

use std::rc::Rc;

use renew_rhi::{
    Blend, Buffer, BufferUsage, Color, Device, Extent, FrameData, InstanceAttribute, PipelineDesc,
    PipelineError, RenderDesc, RenderPipeline, SamplerDesc, Shaders, TargetError, TargetFormat,
    TextureDesc,
};

use crate::fill::{self, Canvas, Sprite};

/// Vertex stage SPIR-V for the sprite quad, compiled offline by the
/// pinned toolchain (the record lives beside the sources).
static SPRITE_VS_SPV: &[u8] = include_bytes!("../shaders/sprite.vert.spv");
/// Fragment stage SPIR-V sampling the atlas at set 0, binding 0.
static SPRITE_FS_SPV: &[u8] = include_bytes!("../shaders/sprite.frag.spv");

/// The sprite quad's per-instance layout. The shader's `location(0..=4)`
/// list, [`fill::pack`], and this slice describe the same 48 bytes;
/// change one and the others in the same commit or the draw reads
/// garbage.
const SPRITE_LAYOUT: &[InstanceAttribute] = &[
    InstanceAttribute::Vec2, // NDC min
    InstanceAttribute::Vec2, // NDC max
    InstanceAttribute::Vec2, // UV min
    InstanceAttribute::Vec2, // UV max
    InstanceAttribute::Vec4, // premultiplied tint
];

/// Six expanded vertices per instance, as the vertex stage's corner
/// table declares.
const SPRITE_VERTEX_COUNT: u32 = 6;

/// The stride and the layout describe the same bytes, checked at
/// compile time so only the shader remains coupled by comment.
const _: () = {
    const fn width(attribute: InstanceAttribute) -> usize {
        match attribute {
            InstanceAttribute::Vec2 => 8,
            InstanceAttribute::Vec4 => 16,
        }
    }
    let mut total = 0usize;
    let mut index = 0;
    while index < SPRITE_LAYOUT.len() {
        total += width(SPRITE_LAYOUT[index]);
        index += 1;
    }
    assert!(
        total == fill::INSTANCE_STRIDE,
        "the instance layout and the packed stride disagree"
    );
};

/// The atlas: dimensions and premultiplied pixels, borrowed for the one
/// call that uploads them.
///
/// `#[non_exhaustive]` with a constructor, the descriptor pattern this
/// tree uses everywhere. The field name carries the obligation:
/// every texel's color channels are already multiplied by its alpha —
/// the renderer cannot verify that from bytes, so the type says it
/// wherever the bytes are handed over.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct AtlasDesc<'a> {
    /// Dimensions in texels. Neither may be zero (the rendering crate
    /// asserts it).
    pub extent: Extent,
    /// Tightly packed RGBA8 rows, top row first, premultiplied.
    /// Length must be exactly `extent.width * extent.height * 4`.
    pub rgba8_premultiplied: &'a [u8],
}

impl<'a> AtlasDesc<'a> {
    /// An atlas of `extent` texels backed by `rgba8_premultiplied`.
    #[must_use]
    pub fn new(extent: Extent, rgba8_premultiplied: &'a [u8]) -> Self {
        Self {
            extent,
            rgba8_premultiplied,
        }
    }
}

/// What can go wrong building a [`SpriteRenderer`] — creation only; the
/// fill path cannot fail and the render belongs to the target.
#[derive(Debug)]
#[non_exhaustive]
pub enum Render2dError {
    /// Shader, sampler, or pipeline creation failed.
    Pipeline(PipelineError),
    /// Texture or buffer creation failed.
    Target(TargetError),
}

impl core::fmt::Display for Render2dError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Pipeline(error) => write!(f, "building the sprite pipeline: {error}"),
            Self::Target(error) => write!(f, "building the sprite renderer's resources: {error}"),
        }
    }
}

impl std::error::Error for Render2dError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pipeline(error) => Some(error),
            Self::Target(error) => Some(error),
        }
    }
}

impl From<PipelineError> for Render2dError {
    fn from(error: PipelineError) -> Self {
        Self::Pipeline(error)
    }
}

impl From<TargetError> for Render2dError {
    fn from(error: TargetError) -> Self {
        Self::Target(error)
    }
}

/// Batched 2D sprites: fill in canvas space, draw in one instanced
/// call, in exactly the order pushed.
///
/// Every allocation happens in [`SpriteRenderer::new`]; `begin`, `push`
/// and `desc` allocate nothing, which the crate's gate measures rather
/// than asserts. Holds `Rc`s into the device spine, so it is `!Send +
/// !Sync` like everything else on it.
pub struct SpriteRenderer {
    pipeline: RenderPipeline,
    buffer: Buffer,
    scratch: Vec<u8>,
    count: u32,
    max_sprites: u32,
    canvas: Canvas,
    atlas_extent: Extent,
}

impl SpriteRenderer {
    /// A renderer drawing `atlas` sprites onto a `canvas`-sized space,
    /// at most `max_sprites` per frame, for targets of `format`.
    ///
    /// Uploads the atlas, builds the one pipeline (premultiplied
    /// blending, nearest/clamped sampling), and sizes the per-frame
    /// buffer to `max_sprites` packed instances — the only allocations
    /// this type ever makes.
    ///
    /// # Errors
    ///
    /// Whatever the rendering crate reports for the environment —
    /// shader rejection, exhausted device memory — wrapped by which
    /// resource was being built. Wrong-length atlas bytes are a
    /// contract violation and assert there, not here.
    pub fn new(
        device: &Device,
        atlas: &AtlasDesc<'_>,
        canvas: Canvas,
        format: TargetFormat,
        max_sprites: core::num::NonZeroU32,
    ) -> Result<Self, Render2dError> {
        let texture =
            device.create_texture(&TextureDesc::new(atlas.extent, atlas.rgba8_premultiplied))?;
        let sampler = device.create_sampler(&SamplerDesc::atlas())?;
        let pipeline = device.create_pipeline(
            &PipelineDesc::new(
                Shaders::new(SPRITE_VS_SPV, SPRITE_FS_SPV, SPRITE_VERTEX_COUNT),
                format,
            )
            .instance_input(SPRITE_LAYOUT)
            .texture(Rc::new(texture), Rc::new(sampler))
            .blend(Blend::PremultipliedAlpha),
        )?;
        let capacity = max_sprites.get() as usize * fill::INSTANCE_STRIDE;
        let buffer = device.create_buffer(capacity, BufferUsage::PerFrame)?;
        Ok(Self {
            pipeline,
            buffer,
            scratch: vec![0u8; capacity],
            count: 0,
            max_sprites: max_sprites.get(),
            canvas,
            atlas_extent: atlas.extent,
        })
    }

    /// Start a new fill: forget every pushed sprite.
    ///
    /// Explicit rather than folded into [`Self::desc`]: a caller that
    /// never begins accumulates, which is a legal static scene filled
    /// once; a caller that begins and pushes nothing draws a legal
    /// empty frame.
    pub fn begin(&mut self) {
        self.count = 0;
    }

    /// Append `sprite`; it draws over everything pushed before it.
    ///
    /// Refuses the push past `max_sprites` with a retained assertion —
    /// a deliberate choice the README's contract carries: the scratch
    /// is a fixed preallocation whose
    /// slice write would panic on its own bounds check anyway, so the
    /// assertion buys a named refusal at the API boundary for a branch
    /// release mode pays either way. Not a memory-safety guard.
    ///
    /// # Panics
    ///
    /// When pushed past `max_sprites` — a sizing bug in the caller,
    /// refused by name rather than truncated into a quiet wrong draw.
    pub fn push(&mut self, sprite: &Sprite) {
        assert!(
            self.count < self.max_sprites,
            "sprite capacity {} exceeded; size the renderer for its scene",
            self.max_sprites
        );
        let packed = fill::pack(sprite, self.canvas, self.atlas_extent);
        let offset = self.count as usize * fill::INSTANCE_STRIDE;
        self.scratch[offset..offset + fill::INSTANCE_STRIDE].copy_from_slice(&packed);
        self.count += 1;
    }

    /// Sprites pushed since the last [`Self::begin`].
    #[must_use]
    pub fn sprites(&self) -> u32 {
        self.count
    }

    /// The per-frame capacity fixed at creation.
    #[must_use]
    pub fn max_sprites(&self) -> u32 {
        self.max_sprites
    }

    /// Everything one frame needs, for either target: clear to `clear`,
    /// then draw every pushed sprite in push order.
    ///
    /// Zero pushed sprites is a clear with a zero-instance draw — a
    /// legal empty frame, not an error.
    #[must_use]
    pub fn desc(&self, clear: Color) -> RenderDesc<'_> {
        let filled = self.count as usize * fill::INSTANCE_STRIDE;
        RenderDesc::new(clear)
            .pipeline(&self.pipeline)
            .frame_data(FrameData::new(
                &self.buffer,
                &self.scratch[..filled],
                self.count,
            ))
    }
}

impl core::fmt::Debug for SpriteRenderer {
    /// Counts and dimensions, not handles — the same posture as the
    /// rendering crate's own types.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SpriteRenderer")
            .field("sprites", &self.count)
            .field("max_sprites", &self.max_sprites)
            .field("canvas", &self.canvas)
            .field("atlas_extent", &self.atlas_extent)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_display_their_context_and_expose_their_source() {
        // Both variants, both conversions, the Display prefix, and the
        // source chain -- the parts of the error surface no
        // device-dependent test reaches on its success path.
        let pipeline: Render2dError = PipelineError::InvalidSpirv {
            stage: "vertex",
            reason: "test fixture",
        }
        .into();
        assert!(
            pipeline
                .to_string()
                .starts_with("building the sprite pipeline:"),
            "unexpected Display: {pipeline}"
        );
        assert!(std::error::Error::source(&pipeline).is_some());

        let target: Render2dError = TargetError::SurfaceCreation { code: -1 }.into();
        assert!(
            target
                .to_string()
                .starts_with("building the sprite renderer's resources:"),
            "unexpected Display: {target}"
        );
        assert!(std::error::Error::source(&target).is_some());
        assert!(format!("{pipeline:?}").contains("Pipeline"));
        assert!(format!("{target:?}").contains("Target"));
    }
}
