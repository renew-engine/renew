//! The device half: one atlas, one pipeline, one buffer, one draw.
//!
//! Everything that touches `renew_rhi` lives in this module — the
//! rendering-crate seam stays one file wide and `fill.rs` never moves.

use renew_rhi::{
    AddressMode, Binding, BindingDesc, BindingSource, Blend, Buffer, BufferUsage, Device, Extent,
    Facing, Filter, FrameData, Item, PipelineDesc, PipelineError, RenderPipeline, SamplerDesc,
    Shaders, TargetError, TargetFormat, TextureDesc, VertexAttribute,
};

use crate::fill::{self, Canvas, Sprite};

/// Vertex stage SPIR-V for the sprite quad, compiled offline by the
/// pinned toolchain (the record lives beside the sources).
static SPRITE_VS_SPV: &[u8] = include_bytes!("../shaders/sprite.vert.spv");
/// Fragment stage SPIR-V sampling the atlas at set 0, binding 0.
static SPRITE_FS_SPV: &[u8] = include_bytes!("../shaders/sprite.frag.spv");

/// The sprite quad's per-instance layout. The shader's `location(0..=8)`
/// list, [`fill::pack`], and this slice describe the same 80 bytes;
/// change one and the others in the same commit or the draw reads
/// garbage.
const SPRITE_LAYOUT: &[VertexAttribute] = &[
    VertexAttribute::Vec2, // corner a: local top-left, NDC
    VertexAttribute::Vec2, // corner b: local top-right
    VertexAttribute::Vec2, // corner c: local bottom-left
    VertexAttribute::Vec2, // corner d: local bottom-right
    VertexAttribute::Vec2, // UV at corner a (swapped by a flip)
    VertexAttribute::Vec2, // UV at corner d
    VertexAttribute::Vec4, // premultiplied tint
    VertexAttribute::Vec2, // effect: saturation, flash
    VertexAttribute::Vec2, // smear, in UV units
];

/// Six expanded vertices per instance, as the vertex stage's corner
/// table declares.
const SPRITE_VERTEX_COUNT: u32 = 6;

/// The atlas: dimensions and **authored** pixels, borrowed for the one
/// call that uploads them, and how its texels are read.
///
/// `#[non_exhaustive]` with a constructor and builders, the descriptor
/// pattern this tree uses everywhere: the sampling fields arrived as
/// builders touching no existing caller.
///
/// # How the texels are read is the atlas's choice
///
/// [`Self::new`] samples **nearest and clamped**: a pixel reads exactly
/// one texel, an atlas has no meaning outside its own edges, and a
/// committed picture stays a function of the engine rather than of how
/// an adapter interpolates. Pixel art wants nothing else.
///
/// Painted art drawn at a scale that is not an integer wants
/// [`Filter::Linear`], which reads the four texels around the sample and
/// blends them by distance. Two consequences follow from the blend,
/// and both are the caller's:
///
/// - **Every region owes a gutter.** A linear sample half a texel from a
///   region's edge reaches the neighbouring texel, at every scale but
///   exactly 1:1 texel-aligned, so the one-texel transparent border that
///   only a turned region owed under nearest sampling is owed by all of
///   them. And the border should carry the art's own colour at zero
///   alpha rather than black: the hardware blends the authored colour
///   *before* the fragment stage multiplies by alpha, so a gutter
///   authored black darkens every edge it touches on its way to
///   transparent, where one authored in the art's colour only fades.
/// - **There is one mip level.** The rendering crate creates no others,
///   so art drawn at less than half its authored size covers more texels
///   per pixel than the four a linear sample reads, and aliases.
///   Minification past 2:1 wants art authored at the smaller size, under
///   either filter.
///
/// [`AddressMode::Repeat`] tiles the whole atlas where a sample lands
/// outside it: a region wider than the atlas, drawn under it, shows the
/// atlas repeated — one tileable texture, drawn as a background from the
/// same renderer. Under the default clamp the same region shows the edge
/// texel stretched. Nothing about a sprite inside the atlas changes
/// either way, so an atlas of many sprites gains nothing from it and
/// keeps the clamp.
///
/// # The bytes are authored colour with straight alpha
///
/// Display-encoded — the values somebody chose by looking at them — and
/// **not** premultiplied. The texture is created as sRGB so the hardware
/// decodes on sample, and the fragment stage multiplies by alpha after
/// that.
///
/// **It used to be the other way round, and the other way round cannot
/// work.** The transfer function does not commute with the alpha
/// multiply, so bytes premultiplied before encoding cannot be decoded
/// correctly by anything — which meant authored sprite colour had nothing
/// to decode it, and every opaque mid-tone arrived lifted by exactly one
/// encode. A sample sprite authored `208` drew as `233`.
///
/// Coverage atlases are unaffected by the change: white and full alpha are
/// both fixed points of the transfer curve, so a mask authored as
/// `(255, 255, 255, a)` and premultiplied in the shader lands on precisely
/// the values it used to supply itself.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct AtlasDesc<'a> {
    /// Dimensions in texels. Neither may be zero (the rendering crate
    /// asserts it).
    pub extent: Extent,
    /// Tightly packed RGBA8 rows, top row first, authored, straight alpha.
    /// Length must be exactly `extent.width * extent.height * 4`.
    pub rgba8: &'a [u8],
    /// How a sample between texel centres is resolved: the nearest texel
    /// by default, or the four around it blended by distance.
    pub filter: Filter,
    /// What a sample outside the atlas reads: the edge texel by default,
    /// or the atlas again.
    pub address: AddressMode,
}

impl<'a> AtlasDesc<'a> {
    /// An atlas of `extent` texels backed by authored `rgba8`, sampled
    /// nearest and clamped.
    #[must_use]
    pub fn new(extent: Extent, rgba8: &'a [u8]) -> Self {
        Self {
            extent,
            rgba8,
            filter: Filter::Nearest,
            address: AddressMode::ClampToEdge,
        }
    }

    /// Resolve samples between texel centres by `filter` instead of
    /// taking the nearest texel. See the type's documentation for what
    /// [`Filter::Linear`] asks of the art.
    #[must_use]
    pub fn filter(mut self, filter: Filter) -> Self {
        self.filter = filter;
        self
    }

    /// Read samples outside the atlas by `address` instead of clamping
    /// to its edge texel.
    #[must_use]
    pub fn address(mut self, address: AddressMode) -> Self {
        self.address = address;
        self
    }

    /// The rendering crate's sampler for these choices: its atlas
    /// default with the two fields this type exposes written over it.
    fn sampler(&self) -> SamplerDesc {
        let mut sampler = SamplerDesc::atlas();
        sampler.filter = self.filter;
        sampler.address = self.address;
        sampler
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
/// Every allocation happens in [`SpriteRenderer::with_blend`] — which
/// [`SpriteRenderer::new`] is, over the default mode; `begin`, `push`
/// and `item` allocate nothing, which the crate's gate measures rather
/// than asserts. Holds `Rc`s into the device spine, so it is `!Send +
/// !Sync` like everything else on it.
pub struct SpriteRenderer {
    pipeline: RenderPipeline,
    binding: Binding,
    buffer: Buffer,
    scratch: Vec<u8>,
    count: u32,
    max_sprites: u32,
    canvas: Canvas,
    atlas_extent: Extent,
    blend: Blend,
    offset: (f32, f32),
    alpha: f32,
}

impl SpriteRenderer {
    /// A renderer drawing `atlas` sprites onto a `canvas`-sized space,
    /// at most `max_sprites` per frame, for targets of `format`,
    /// compositing premultiplied over what the target holds.
    ///
    /// [`Self::with_blend`] with [`Blend::PremultipliedAlpha`], exactly;
    /// the sampling is the atlas's ([`AtlasDesc`]).
    ///
    /// # Errors
    ///
    /// As [`Self::with_blend`].
    pub fn new(
        device: &Device,
        atlas: &AtlasDesc<'_>,
        canvas: Canvas,
        format: TargetFormat,
        max_sprites: core::num::NonZeroU32,
    ) -> Result<Self, Render2dError> {
        Self::with_blend(
            device,
            atlas,
            canvas,
            format,
            max_sprites,
            Blend::PremultipliedAlpha,
        )
    }

    /// [`Self::new`], compositing by `blend` instead of over.
    ///
    /// The blend is the renderer's rather than the atlas's — two
    /// renderers over the same bytes may composite differently, which
    /// is exactly how a scene keeps its glows beside its sprites — so
    /// it is a constructor variant and not a field of [`AtlasDesc`].
    /// Every sprite the renderer draws composites the same way; a scene
    /// wanting both modes builds two renderers and orders their items.
    ///
    /// **The source is premultiplied under every mode**: a texel's
    /// colour arrives multiplied by its alpha and by the tint, so what
    /// differs is only how that source meets the target.
    ///
    /// - [`Blend::PremultipliedAlpha`] composites
    ///   `src + dst · (1 − α_src)`: a sprite covers what is under it in
    ///   proportion to its alpha. The default.
    /// - [`Blend::Additive`] composites `src + dst`, colour and alpha
    ///   alike: a sprite adds its light and covers nothing, and two
    ///   that overlap are brighter where they meet. A tint's colour
    ///   channels scale the light added; its alpha channel scales only
    ///   what the sprite adds to the target's *alpha*, and never
    ///   occludes. `[1.0; 4]`, the identity, adds the texel's
    ///   premultiplied colour and its alpha; the batch fade
    ///   ([`Self::set_alpha`]) dims the light in proportion, as it
    ///   should. Order-independent by arithmetic, up to the target's
    ///   own rounding between fragments.
    /// - [`Blend::Opaque`] replaces: the premultiplied colour and the
    ///   alpha land in the target as they are. For sprites every one of
    ///   which is opaque, and nothing else — a translucent texel writes
    ///   its own alpha over the target's rather than covering.
    ///
    /// Uploads the atlas, builds the one pipeline, and sizes the
    /// per-frame buffer to `max_sprites` packed instances — the only
    /// allocations this type ever makes.
    ///
    /// # Errors
    ///
    /// Whatever the rendering crate reports for the environment —
    /// shader rejection, exhausted device memory — wrapped by which
    /// resource was being built. Wrong-length atlas bytes are a
    /// contract violation and assert there, not here.
    pub fn with_blend(
        device: &Device,
        atlas: &AtlasDesc<'_>,
        canvas: Canvas,
        format: TargetFormat,
        max_sprites: core::num::NonZeroU32,
        blend: Blend,
    ) -> Result<Self, Render2dError> {
        // `colour`, not `new`: these are authored bytes, so the hardware
        // decodes them on sample and shading sees reflectance. The
        // fragment stage premultiplies afterwards.
        let texture = device.create_texture(&TextureDesc::colour(atlas.extent, atlas.rgba8))?;
        let sampler = device.create_sampler(&atlas.sampler())?;
        let binding = device.create_binding(&BindingDesc::new(
            BindingSource::Texture(&texture),
            &sampler,
        ))?;
        let pipeline = device.create_pipeline(
            &PipelineDesc::new(
                Shaders::new(SPRITE_VS_SPV, SPRITE_FS_SPV, SPRITE_VERTEX_COUNT),
                format,
            )
            .instance_input(SPRITE_LAYOUT)
            .sampled_bindings(1)
            .blend(blend)
            // Both faces, deliberately: a negative `Sprite::scale`
            // reverses the quad's winding and must still draw — the
            // crate promises it on the field, and the mirror oracle in
            // `tests/golden.rs` pins it. A one-sided sprite pipeline
            // would erase mirrored sprites with no error anywhere.
            .facing(Facing::Both),
        )?;
        let capacity = max_sprites.get() as usize * fill::INSTANCE_STRIDE;
        let buffer = device.create_buffer(capacity, BufferUsage::PerFrame)?;
        Ok(Self {
            pipeline,
            binding,
            buffer,
            scratch: vec![0u8; capacity],
            count: 0,
            max_sprites: max_sprites.get(),
            canvas,
            atlas_extent: atlas.extent,
            blend,
            offset: (0.0, 0.0),
            alpha: 1.0,
        })
    }

    /// How this renderer's sprites meet the target, fixed at creation.
    #[must_use]
    pub fn blend(&self) -> Blend {
        self.blend
    }

    /// Start a new fill: forget every pushed sprite.
    ///
    /// Explicit rather than folded into [`Self::item`]: a caller that
    /// never begins accumulates, which is a legal static scene filled
    /// once; a caller that begins and pushes nothing draws a legal
    /// empty frame.
    pub fn begin(&mut self) {
        self.count = 0;
        // The offset and the fade reset with the fill. They are state
        // that changes what a later call draws, and state that
        // outlives the frame that set it is the kind a caller forgets
        // to clear exactly once and then cannot find.
        self.offset = (0.0, 0.0);
        self.alpha = 1.0;
    }

    /// Move every sprite pushed after this by (`x`, `y`) logical
    /// pixels.
    ///
    /// Lets a caller slide a whole group — a panel, a page, a row of
    /// cards — without threading an offset through the code that
    /// builds each sprite. Set it back to `(0.0, 0.0)` when the group
    /// ends; [`Self::begin`] does that for the next fill.
    pub fn set_offset(&mut self, x: f32, y: f32) {
        self.offset = (x, y);
    }

    /// The offset every later push is moved by.
    #[must_use]
    pub fn offset(&self) -> (f32, f32) {
        self.offset
    }

    /// Fade every sprite pushed after this to `alpha` of its opacity,
    /// clamped to `0.0..=1.0`.
    ///
    /// # Panics
    ///
    /// On a NaN `alpha`. `f32::clamp` passes NaN straight through, so
    /// an unguarded clamp would multiply it into all four channels of
    /// every later sprite and draw nothing, frame after frame, with no
    /// error anywhere — a silent wrong picture rather than a named
    /// refusal. Infinities need no such guard: they clamp.
    ///
    /// **The tint is premultiplied, so this scales all four channels
    /// and not just the fourth.** In premultiplied RGBA the colour
    /// already carries its own alpha, so halving only the alpha leaves
    /// the colour arriving at full strength while occluding less — the
    /// group brightens as it fades, which is the opposite of the
    /// intent. Scaling the whole tuple is what "half as opaque" means
    /// under this convention, and the convention is the crate's, end
    /// to end.
    pub fn set_alpha(&mut self, alpha: f32) {
        self.alpha = fill::fade(alpha);
    }

    /// The fade every later push is multiplied by.
    #[must_use]
    pub fn alpha(&self) -> f32 {
        self.alpha
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
        // Applied here rather than by the caller, which is the whole
        // point: the code that builds a sprite should not have to know
        // whether the group it belongs to is mid-slide. The arithmetic
        // lives in `fill::placed` so it can be checked without a
        // device.
        let moved = fill::placed(sprite, self.offset, self.alpha);
        let packed = fill::pack(&moved, self.canvas, self.atlas_extent);
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

    /// This frame's draw: every pushed sprite, in push order, as one
    /// item for a pass the caller composes.
    ///
    /// The caller builds the frame on its own stack — the matching
    /// colour attachment is the rendering crate's `color_attachment`,
    /// which every consumer shares — and the borrows end at the
    /// `render` call:
    ///
    /// ```no_run
    /// use renew_render2d::SpriteRenderer;
    /// use renew_rhi::{Color, OffscreenTarget, Pass, RenderDesc, TargetError, color_attachment};
    /// fn frame(
    ///     renderer: &SpriteRenderer,
    ///     target: &mut OffscreenTarget,
    ///     sky: Color,
    /// ) -> Result<(), TargetError> {
    ///     let color = [color_attachment(sky)];
    ///     let items = [renderer.item()];
    ///     let passes = [Pass::new(&color, &items)];
    ///     target.render(&RenderDesc::new(&passes))
    /// }
    /// ```
    ///
    /// Zero pushed sprites is a zero-instance draw — a legal empty
    /// frame, not an error.
    #[must_use]
    pub fn item(&self) -> Item<'_> {
        let filled = self.count as usize * fill::INSTANCE_STRIDE;
        Item::new(&self.pipeline)
            .frame_data(FrameData::new(
                &self.buffer,
                &self.scratch[..filled],
                self.count,
            ))
            .bindings(&[&self.binding])
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
            .field("blend", &self.blend)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stride and the layout describe the same bytes, checked
    /// mechanically so only the shader remains coupled by comment. A
    /// test rather than a const block: const evaluation never executes
    /// at runtime, so its lines read as uncovered to the coverage gate,
    /// and a guard that needs an exemption to exist defeats both.
    /// The packed width of one attribute.
    ///
    /// **Named, so the exhaustive match is reachable.** Inlined into the
    /// sum below it would only ever run for the formats `SPRITE_LAYOUT`
    /// happens to use, leaving the others as lines no passing run
    /// executes — while the match still has to list them, because the
    /// rendering crate's enum carries no `#[non_exhaustive]` precisely so
    /// that a new format is a compile error here. Split out, the
    /// exhaustiveness tripwire is kept and every arm is exercised.
    fn packed_width(attribute: VertexAttribute) -> usize {
        match attribute {
            VertexAttribute::Vec2 | VertexAttribute::Uint32x2 => 8,
            VertexAttribute::Vec3 => 12,
            VertexAttribute::Vec4 => 16,
            VertexAttribute::Uint32 | VertexAttribute::Unorm8x4 => 4,
        }
    }

    #[test]
    fn every_attribute_reports_its_packed_width() {
        assert_eq!(packed_width(VertexAttribute::Vec2), 8);
        assert_eq!(packed_width(VertexAttribute::Vec3), 12);
        assert_eq!(packed_width(VertexAttribute::Vec4), 16);
        assert_eq!(packed_width(VertexAttribute::Uint32), 4);
        assert_eq!(packed_width(VertexAttribute::Uint32x2), 8);
        assert_eq!(packed_width(VertexAttribute::Unorm8x4), 4);
    }

    #[test]
    fn the_stride_and_the_layout_describe_the_same_bytes() {
        let total: usize = SPRITE_LAYOUT.iter().copied().map(packed_width).sum();
        assert_eq!(
            total,
            fill::INSTANCE_STRIDE,
            "the instance layout and the packed stride disagree"
        );
    }

    /// The atlas samples nearest and clamped unless told otherwise, and
    /// the builders reach the sampler the rendering crate is handed —
    /// pinned without a device so the default is checked on every
    /// platform, whether or not it can draw.
    #[test]
    fn an_atlas_samples_nearest_and_clamped_unless_told_otherwise() {
        let extent = Extent {
            width: 1,
            height: 1,
        };
        let bytes = [0u8; 4];
        let atlas = AtlasDesc::new(extent, &bytes);
        let default = SamplerDesc::atlas();
        assert_eq!(
            atlas.filter, default.filter,
            "the default filter is the atlas's"
        );
        assert_eq!(
            atlas.address, default.address,
            "the default address mode is the atlas's"
        );
        let sampler = atlas.sampler();
        assert_eq!(
            (sampler.filter, sampler.address),
            (default.filter, default.address)
        );

        let linear = atlas.filter(Filter::Linear).address(AddressMode::Repeat);
        let sampler = linear.sampler();
        assert_eq!(
            sampler.filter,
            Filter::Linear,
            "the filter builder reaches the sampler"
        );
        assert_eq!(
            sampler.address,
            AddressMode::Repeat,
            "the address builder reaches the sampler"
        );
        assert_eq!(linear.extent, extent, "the builders leave the bytes alone");
        assert_eq!(linear.rgba8, &bytes);
    }

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
