//! The device half: one mesh pipeline, one upload, one draw item.
//!
//! Everything that names the rendering crate lives here, so the seam is
//! one module wide and `scene.rs` never moves.
//!
//! # This crate does not render
//!
//! It hands back an [`Item`] and the attachments that go with it; the
//! caller composes the frame on its own stack and gives it to whichever
//! target it holds. The 2D sibling states the same posture, and the
//! reason is the same: a crate that owned the render loop would have to
//! own the target, and then it could not be used offscreen and windowed
//! by the same caller. Nothing here presents, and nothing here touches a
//! window.

use renew_rhi::{
    Attachment, ClearValue, Device, Item, LoadOp, Mesh, MeshDesc, Pass, PipelineDesc,
    PipelineError, RenderPipeline, StoreOp, TargetError, TargetFormat, VertexAttribute, builtin,
};

use crate::scene::{Scene, VERTEX_STRIDE};

/// How many bytes the fade block carries.
///
/// Sixteen: a `vec4` whose `w` is unused. std140 rounds a `vec3` up to
/// sixteen anyway, so the padding exists either way and a named spare is
/// honester than a silent one.
const FADE_BYTES: usize = 16;

/// The same, as the width the pipeline wants.
const FADE_BYTES_U32: u32 = 16;

/// The scene-wide values every camera pipeline reads in its fragment
/// stage, and the buffer they cross in.
///
/// **A block rather than push data, and not for want of space.** This
/// engine declares its push range for the vertex stage alone, so a
/// fragment shader cannot read one at all — which is why the horizon was
/// a compiled-in constant for as long as it was. The block is visible to
/// both stages and is the only channel that reaches here.
///
/// One per renderer, because the horizon is a property of the scene and
/// not of a draw: every item a renderer makes in a frame reads the same
/// one, and threading a colour through four `item` signatures would say
/// otherwise.
struct Fade {
    binding: renew_rhi::Binding,
    bytes: [u8; FADE_BYTES],
}

impl Fade {
    /// A fade holding the horizon the shaders used to compile in, so a
    /// caller that says nothing gets the picture it already had.
    fn new(device: &Device) -> Result<Self, Render3dError> {
        let buffer = device
            .create_buffer(FADE_BYTES, renew_rhi::BufferUsage::PerFrame)
            .map_err(Render3dError::Texture)?;
        let binding = device.create_binding(&renew_rhi::BindingDesc::uniform(&buffer))?;
        let mut fade = Self {
            binding,
            bytes: [0; FADE_BYTES],
        };
        fade.set(builtin::HORIZON);
        Ok(fade)
    }

    /// Write a colour into the bytes the next item will carry.
    fn set(&mut self, horizon: [f32; 3]) {
        for (slot, value) in self
            .bytes
            .chunks_exact_mut(4)
            .zip(horizon.into_iter().chain([1.0]))
        {
            slot.copy_from_slice(&value.to_ne_bytes());
        }
    }
}

/// The per-vertex layout this crate's pipeline declares.
///
/// **The rendering crate's own constant, not a copy of it.** `MESH_LAYOUT`
/// ships beside `MESH` in `builtin`, where the shader and the layout that
/// describes its inputs sit together and are changed together. Declaring
/// the same two attributes here instead would have looked equivalent and
/// would not have been: the record-time assertion that a mesh's stride
/// matches its pipeline's compares two numbers *both* derived from
/// whichever layout this crate passed in, so they agree by construction
/// and cannot notice a shader that has moved on. There is no reflection
/// anywhere in the rendering crate to catch it either. Naming the
/// constant is what actually couples this pipeline to those shaders.
const LAYOUT: &[VertexAttribute] = builtin::MESH_LAYOUT;

/// What can go wrong building or uploading. Creation only: the draw
/// itself cannot fail, and the render belongs to the target.
#[derive(Debug)]
#[non_exhaustive]
pub enum Render3dError {
    /// The adapter offers no depth format in the chain the rendering
    /// crate tries, so a depth-tested pipeline cannot be built.
    ///
    /// **Translated from the rendering crate's own refusal rather than
    /// detected a second time.** Pipeline creation checks depth before it
    /// creates anything and names the chain it refused; asking the device
    /// separately would put two authorities on one fact, and would put
    /// the refusal on a path no lane can execute — every adapter the
    /// tests run on offers depth.
    DepthUnsupported {
        /// The format chain that was refused, for the diagnostic.
        chain: &'static str,
    },
    /// Building the pipeline failed for a reason that is not depth.
    Pipeline(PipelineError),
    /// Uploading the geometry failed.
    Upload(TargetError),
    /// The texture or its sampler could not be created.
    ///
    /// Its own variant rather than [`Self::Upload`], which is where the
    /// blanket conversion from a target failure would have put it: this
    /// happens while building a renderer, before any geometry exists,
    /// and reporting it as an upload of geometry would send a reader to
    /// look at a scene nobody has offered yet. (A `CameraBuffer` variant
    /// once stood beside it for the same reason; it left when the camera
    /// moved to push constants and the renderers stopped owning a
    /// buffer at all.)
    Texture(TargetError),
    /// The scene has no geometry.
    ///
    /// **Refused here rather than downstream, and that is deliberate.**
    /// The rendering crate treats empty geometry as a caller bug and
    /// asserts on it before returning its error, so an empty scene handed
    /// through would panic in every build this repository tests in. An
    /// all-air world and a fully culled mesh are ordinary data, not
    /// mistakes, so they get an ordinary refusal.
    EmptyScene,
}

impl core::fmt::Display for Render3dError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DepthUnsupported { chain } => write!(
                f,
                "this adapter offers no depth format ({chain}), and 3D geometry is drawn depth-tested"
            ),
            Self::Pipeline(error) => write!(f, "building the mesh pipeline: {error}"),
            Self::Upload(error) => write!(f, "uploading the geometry: {error}"),
            Self::Texture(error) => write!(f, "creating the texture: {error}"),
            Self::EmptyScene => write!(f, "the scene has no geometry to upload"),
        }
    }
}

impl std::error::Error for Render3dError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pipeline(error) => Some(error),
            // One arm: both wrap a target failure, and clippy is right
            // that two arms with one body is a distinction without a
            // difference. They differ in what they *say*, which is
            // Display's business, not this one's.
            Self::Upload(error) | Self::Texture(error) => Some(error),
            Self::DepthUnsupported { .. } | Self::EmptyScene => None,
        }
    }
}

impl From<PipelineError> for Render3dError {
    /// The depth refusal is recognised and given this crate's name for
    /// it; everything else passes through carrying its own words.
    fn from(error: PipelineError) -> Self {
        match error {
            PipelineError::DepthUnsupported { chain } => Self::DepthUnsupported { chain },
            other => Self::Pipeline(other),
        }
    }
}

impl From<TargetError> for Render3dError {
    fn from(error: TargetError) -> Self {
        Self::Upload(error)
    }
}

/// Draws indexed geometry, depth-tested, into a target of one format.
///
/// Holds the pipeline and nothing else — in particular it does **not**
/// hold a mesh. The rendering crate deliberately allows one mesh to be
/// drawn by several items in a frame, and a renderer that owned exactly
/// one would discard that for no gain. Geometry is uploaded separately
/// and handed back to the caller to keep.
pub struct MeshRenderer {
    pipeline: RenderPipeline,
}

impl MeshRenderer {
    /// Build the pipeline for targets of `format`.
    ///
    /// Depth testing and depth writing are both on, with the compare the
    /// rendering crate fixes in v0. There is no knob: a 3D frame drawn
    /// without depth is a wrong picture that looks plausible, which is
    /// the failure this step exists to make impossible rather than
    /// optional.
    ///
    /// # Errors
    ///
    /// [`Render3dError::DepthUnsupported`] when the adapter offers no
    /// depth format — refused before anything is created, so nothing is
    /// left behind. [`Render3dError::Pipeline`] for any other refusal,
    /// carrying the rendering crate's own words.
    pub fn new(device: &Device, format: TargetFormat) -> Result<Self, Render3dError> {
        let pipeline = device.create_pipeline(
            &PipelineDesc::mesh(builtin::MESH, format, LAYOUT)
                .depth_state(renew_rhi::DepthState::read_write()),
        )?;
        Ok(Self { pipeline })
    }

    /// Upload `scene` into geometry the GPU can draw.
    ///
    /// The bytes are copied during the call, so the scene may be cleared
    /// or dropped the moment it returns. The mesh belongs to the caller:
    /// keep it while it is drawn, drop it when the geometry changes, and
    /// upload again.
    ///
    /// # Errors
    ///
    /// [`Render3dError::EmptyScene`] when there is nothing to draw — see
    /// the variant, which explains why this is caught here rather than
    /// below. [`Render3dError::Upload`] for a driver refusal.
    pub fn upload(&self, device: &Device, scene: &Scene) -> Result<Mesh, Render3dError> {
        upload_scene(device, scene)
    }

    /// The draw for `mesh`, ready to sit in a pass.
    ///
    /// Every index the mesh holds, in the order the scene pushed them.
    #[must_use]
    pub fn item<'a>(&'a self, mesh: &'a Mesh) -> Item<'a> {
        Item::new(&self.pipeline).mesh(mesh)
    }
}

/// The upload both renderers perform, so an empty scene is refused the
/// same way whichever one asked.
fn upload_scene(device: &Device, scene: &Scene) -> Result<Mesh, Render3dError> {
    if scene.is_empty() {
        return Err(Render3dError::EmptyScene);
    }
    let mesh = device.create_mesh(&MeshDesc::new(
        scene.vertices(),
        VERTEX_STRIDE,
        scene.indices(),
    ))?;
    Ok(mesh)
}

/// A view-projection matrix, packed for the push-constant block.
///
/// Four columns of four floats, column-major — the order
/// `renew_math::Mat4` stores and the order a GLSL `mat4` inside a push
/// block reads, so the *order* crosses unchanged.
///
/// Each float is written in native byte order, which is what a Vulkan
/// device on every target this repository builds for expects. That is a
/// statement about those targets, not a law: on a big-endian host the
/// bytes would need swapping, and the place to do it is here.
///
/// **Plain columns rather than a matrix type**, so this crate keeps its
/// single dependency. Whoever owns a camera owns the maths that built it;
/// what crosses the boundary is sixty-four bytes with a stated order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Camera {
    bytes: [u8; 80],
}

impl Camera {
    /// Pack four column vectors, fully lit.
    ///
    /// Light defaults to white, so a caller with nothing to say about it
    /// gets exactly the picture it got before there was anything to say.
    #[must_use]
    pub fn from_columns(columns: [[f32; 4]; 4]) -> Self {
        Self::lit(columns, [1.0; 3])
    }

    /// The same, under a light.
    ///
    /// **One multiplier for the whole draw, and deliberately not a light
    /// source.** A position, a falloff and a direction are a lighting
    /// model, and a lighting model is a decision about how a world looks
    /// that belongs to whoever is drawing it. This is the smaller thing
    /// underneath every version of that decision: how bright the scene
    /// is. A day and night cycle needs exactly this and nothing more, and
    /// so does going underground.
    ///
    /// Multiplied into the vertex colour, which the texture then
    /// multiplies in turn — multiplications that compose in any order, so
    /// no pipeline has to care which came first.
    #[must_use]
    pub fn lit(columns: [[f32; 4]; 4], light: [f32; 3]) -> Self {
        let mut bytes = [0u8; 80];
        let mut at = 0;
        for column in columns {
            for value in column {
                bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
                at += 4;
            }
        }
        for value in [light[0], light[1], light[2], 1.0] {
            bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
            at += 4;
        }
        Self { bytes }
    }

    /// The packed bytes, exactly the length the pipelines' declared
    /// push-constant range wants.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Draws indexed clip-space geometry, depth-tested, sampling a texture.
///
/// [`MeshRenderer`] with a sampler: the texel at each vertex's coordinate
/// multiplies the interpolated colour. No camera and no distance fade —
/// positions are clip space already, so there is no view distance to fade
/// by, and a caller that wants one has projected the world itself.
pub struct TexturedMeshRenderer {
    pipeline: RenderPipeline,
    binding: renew_rhi::Binding,
}

impl TexturedMeshRenderer {
    /// Build the pipeline and upload `pixels` as the texture it samples.
    ///
    /// `pixels` is RGBA8, row-major, `extent.width * extent.height * 4`
    /// bytes long.
    ///
    /// # Errors
    ///
    /// As [`MeshRenderer::new`], plus a refusal to create the texture or
    /// the sampler.
    pub fn new(
        device: &Device,
        format: TargetFormat,
        extent: renew_rhi::Extent,
        pixels: &[u8],
    ) -> Result<Self, Render3dError> {
        let texture = device
            .create_texture(&renew_rhi::TextureDesc::colour(extent, pixels))
            .map_err(Render3dError::Texture)?;
        let sampler = device.create_sampler(&renew_rhi::SamplerDesc::atlas())?;
        let binding = device.create_binding(&renew_rhi::BindingDesc::new(
            renew_rhi::BindingSource::Texture(&texture),
            &sampler,
        ))?;
        let pipeline = device.create_pipeline(
            &PipelineDesc::mesh(builtin::MESH_TEXTURED, format, LAYOUT)
                .sampled_bindings(1)
                .depth_state(renew_rhi::DepthState::read_write()),
        )?;
        Ok(Self { pipeline, binding })
    }

    /// Upload `scene` into geometry the GPU can draw.
    ///
    /// Positions are **clip space**, as [`MeshRenderer::upload`].
    ///
    /// # Errors
    ///
    /// As [`MeshRenderer::upload`].
    pub fn upload(&self, device: &Device, scene: &Scene) -> Result<Mesh, Render3dError> {
        upload_scene(device, scene)
    }

    /// The draw for `mesh`, ready to sit in a pass.
    #[must_use]
    pub fn item<'a>(&'a self, mesh: &'a Mesh) -> Item<'a> {
        Item::new(&self.pipeline)
            .mesh(mesh)
            .bindings(&[&self.binding])
    }
}

impl core::fmt::Debug for TexturedMeshRenderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TexturedMeshRenderer")
            .finish_non_exhaustive()
    }
}

/// Draws indexed geometry through a camera, depth-tested, sampling a
/// texture.
///
/// The difference from [`CameraRenderer`] is one multiplication in the
/// fragment stage: the texel at each vertex's coordinate multiplies the
/// interpolated colour rather than replacing it. **Replacing it would
/// throw away the shading** — which way a face points, how enclosed each
/// of its corners is — and leave an evenly lit world that is flat again,
/// with a pattern on it.
///
/// The texture rides the one binding this renderer holds, so one
/// renderer draws one atlas — which suits a voxel world, where every
/// block samples the same sheet.
///
/// # Colour is not carried through unchanged
///
/// As [`CameraRenderer`]: this path fades toward a horizon with distance,
/// with the same two constants, because two pipelines drawing one world
/// must fade alike or the seam between them shows.
pub struct TexturedCameraRenderer {
    /// The scene values its fragment stage reads.
    fade: Fade,
    pipeline: RenderPipeline,
    binding: renew_rhi::Binding,
}

impl TexturedCameraRenderer {
    /// Build the pipeline and upload `pixels` as the texture it samples.
    ///
    /// `pixels` is RGBA8, row-major, `extent.width * extent.height * 4`
    /// bytes long.
    ///
    /// # Errors
    ///
    /// As [`CameraRenderer::new`], plus a refusal to create the texture
    /// or the sampler.
    pub fn new(
        device: &Device,
        format: TargetFormat,
        extent: renew_rhi::Extent,
        pixels: &[u8],
    ) -> Result<Self, Render3dError> {
        let texture = device
            .create_texture(&renew_rhi::TextureDesc::colour(extent, pixels))
            .map_err(Render3dError::Texture)?;
        // A sampler is part of building the pipeline, and the existing
        // arm already says so; only the image itself is a texture
        // failure.
        let sampler = device.create_sampler(&renew_rhi::SamplerDesc::atlas())?;
        let binding = device.create_binding(&renew_rhi::BindingDesc::new(
            renew_rhi::BindingSource::Texture(&texture),
            &sampler,
        ))?;
        let pipeline = device.create_pipeline(
            &PipelineDesc::mesh(builtin::MESH_CAMERA_TEXTURED, format, LAYOUT)
                .push_constant_size(CAMERA_PUSH_BYTES)
                .uniform_block(FADE_BYTES_U32)
                .sampled_bindings(1)
                .depth_state(renew_rhi::DepthState::read_write()),
        )?;
        Ok(Self {
            pipeline,
            binding,
            fade: Fade::new(device)?,
        })
    }

    /// Upload `scene` into geometry the GPU can draw.
    ///
    /// Positions are **world space**, and the coordinates each vertex
    /// carries index the texture this renderer was built with.
    ///
    /// # Errors
    ///
    /// As [`CameraRenderer::upload`].
    pub fn upload(&self, device: &Device, scene: &Scene) -> Result<Mesh, Render3dError> {
        upload_scene(device, scene)
    }

    /// The draw for `mesh` seen through `camera`.
    #[must_use]
    pub fn item<'a>(&'a self, mesh: &'a Mesh, camera: &'a Camera) -> Item<'a> {
        Item::new(&self.pipeline)
            .mesh(mesh)
            .push_data(camera.bytes())
            // **The block last, as the pipeline declares it.** Sampled
            // slots come first and the block takes the set after them.
            .bindings(&[&self.binding, &self.fade.binding])
            .uniform_data(&self.fade.bytes)
    }

    /// What distance fades toward, from the next item onward.
    ///
    /// **A scene value, so it is set here rather than passed per draw.**
    /// One horizon serves every item this renderer makes in a frame;
    /// threading a colour through `item` would say the opposite and put
    /// the same three floats at every call site.
    ///
    /// Defaults to [`renew_rhi::builtin::HORIZON`] — the colour these
    /// shaders compiled in before there was a way to say otherwise — so a
    /// caller that never calls this draws exactly what it drew before.
    ///
    /// **Match it to whatever the pass clears to.** The fade reads as
    /// depth when distance dissolves into the backdrop and as coloured
    /// fog when it does not, and nothing here can check that: the clear
    /// colour belongs to the pass, not to the pipeline.
    pub fn set_horizon(&mut self, horizon: [f32; 3]) {
        self.fade.set(horizon);
    }
}

/// Draws indexed geometry through a camera, depth-tested, sampling a
/// texture whose clear texels are cut away rather than drawn.
///
/// **The difference from [`TexturedCameraRenderer`] is one comparison in
/// the fragment stage**, and it is the difference between a leaf and a
/// green rectangle. That renderer replaces the target wherever a fragment
/// lands and writes depth as it goes, so a texel the artist left empty is
/// drawn opaque *and* hides whatever stands behind it. Anything authored
/// with holes in it — foliage, a grate, a fence, a decal, a sprite
/// standing in a world — is that shape, and none of it was drawable.
///
/// # Why this is not the blended path
///
/// Blending would fix the colour and leave the depth wrong: a see-through
/// fragment that still writes depth occludes what is drawn after it, and
/// putting that right means sorting every draw back to front — a cost
/// every consumer pays, forever, for textures that are usually binary
/// anyway. A discard needs no sorting and imposes no ordering contract,
/// which makes this the one to reach for first. A surface that is
/// genuinely half-there — glass, smoke — wants blending and a caller
/// willing to sort, and that is a different pipeline.
///
/// Everything else is [`TexturedCameraRenderer`]: the same vertex stage,
/// the same layout, the same push block, the same single atlas, and the
/// same fade toward the same horizon, because pipelines drawing one world
/// must fade alike or the seam between them shows.
pub struct CutoutCameraRenderer {
    /// The scene values its fragment stage reads.
    fade: Fade,
    pipeline: RenderPipeline,
    binding: renew_rhi::Binding,
}

impl CutoutCameraRenderer {
    /// Build the pipeline and upload `pixels` as the texture it samples.
    ///
    /// `pixels` is RGBA8, row-major, `extent.width * extent.height * 4`
    /// bytes long. **The alpha channel is read as a mask**: a texel at or
    /// above half survives whole, one below it is not drawn.
    ///
    /// # Errors
    ///
    /// As [`TexturedCameraRenderer::new`].
    pub fn new(
        device: &Device,
        format: TargetFormat,
        extent: renew_rhi::Extent,
        pixels: &[u8],
    ) -> Result<Self, Render3dError> {
        let texture = device
            .create_texture(&renew_rhi::TextureDesc::colour(extent, pixels))
            .map_err(Render3dError::Texture)?;
        let sampler = device.create_sampler(&renew_rhi::SamplerDesc::atlas())?;
        let binding = device.create_binding(&renew_rhi::BindingDesc::new(
            renew_rhi::BindingSource::Texture(&texture),
            &sampler,
        ))?;
        let pipeline = device.create_pipeline(
            &PipelineDesc::mesh(builtin::MESH_CAMERA_CUTOUT, format, LAYOUT)
                .push_constant_size(CAMERA_PUSH_BYTES)
                .uniform_block(FADE_BYTES_U32)
                .sampled_bindings(1)
                // Depth read *and* written, exactly as the opaque paths
                // do. That is the whole point: what survives the cut is
                // solid geometry and occludes properly, which is what
                // blending cannot offer without sorting.
                .depth_state(renew_rhi::DepthState::read_write()),
        )?;
        Ok(Self {
            pipeline,
            binding,
            fade: Fade::new(device)?,
        })
    }

    /// Upload `scene` into geometry the GPU can draw.
    ///
    /// # Errors
    ///
    /// As [`TexturedCameraRenderer::upload`].
    pub fn upload(&self, device: &Device, scene: &Scene) -> Result<Mesh, Render3dError> {
        upload_scene(device, scene)
    }

    /// The draw for `mesh` seen through `camera`.
    #[must_use]
    pub fn item<'a>(&'a self, mesh: &'a Mesh, camera: &'a Camera) -> Item<'a> {
        Item::new(&self.pipeline)
            .mesh(mesh)
            .push_data(camera.bytes())
            // **The block last, as the pipeline declares it.** Sampled
            // slots come first and the block takes the set after them.
            .bindings(&[&self.binding, &self.fade.binding])
            .uniform_data(&self.fade.bytes)
    }

    /// What distance fades toward, from the next item onward.
    ///
    /// **A scene value, so it is set here rather than passed per draw.**
    /// One horizon serves every item this renderer makes in a frame;
    /// threading a colour through `item` would say the opposite and put
    /// the same three floats at every call site.
    ///
    /// Defaults to [`renew_rhi::builtin::HORIZON`] — the colour these
    /// shaders compiled in before there was a way to say otherwise — so a
    /// caller that never calls this draws exactly what it drew before.
    ///
    /// **Match it to whatever the pass clears to.** The fade reads as
    /// depth when distance dissolves into the backdrop and as coloured
    /// fog when it does not, and nothing here can check that: the clear
    /// colour belongs to the pass, not to the pipeline.
    pub fn set_horizon(&mut self, horizon: [f32; 3]) {
        self.fade.set(horizon);
    }
}

impl core::fmt::Debug for CutoutCameraRenderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("CutoutCameraRenderer")
    }
}

impl core::fmt::Debug for TexturedCameraRenderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TexturedCameraRenderer")
            .finish_non_exhaustive()
    }
}

/// Draws indexed geometry through a camera, depth-tested.
///
/// The difference from [`MeshRenderer`] is where the transform happens
/// and therefore what a scene's positions mean: that one takes clip
/// space and draws it straight, this one takes **world space** and
/// multiplies by a matrix on the GPU.
///
/// **Two renderers rather than a flag**, because the two make different
/// promises about the same [`Scene`]. A single type with an optional
/// camera would leave the meaning of a position undecidable from the
/// call site, and the failure mode is a picture rather than an error.
///
/// # Colour is not carried through unchanged
///
/// This path fades a fragment's colour toward a dim horizon as its
/// distance from the eye grows, reaching a fixed fraction of the way by
/// a fixed distance; both constants live in the fragment shader. Callers
/// that need the colour they supplied to arrive intact — a picker buffer,
/// an id pass, anything read back and compared — want [`MeshRenderer`]
/// and their own transform, not this.
///
/// **Why a renderer fades at all**, when a fade is a look and this crate
/// is not in the business of looks: geometry with no lighting gives a
/// viewer very little to judge distance by. A caller can put cues in the
/// vertex colours it supplies — this crate's [`Scene::quad_shaded`]
/// exists so it can — but no per-vertex colour distinguishes a near wall
/// from a far one, because the two are the same geometry seen from
/// different distances. Perspective without a distance cue is not
/// perspective a viewer can see. It is a readability floor rather than a
/// feature, and it is stated here because behaviour a caller cannot
/// predict from the type's name is behaviour the type must name itself.
///
/// The constants stay in the shader deliberately, even though the push
/// block has room beside the matrix: where arithmetic folds decides its
/// floating-point result, and the committed pictures pin the result as
/// it is. They move only when something needs them to vary per draw.
pub struct CameraRenderer {
    /// The scene values its fragment stage reads.
    fade: Fade,
    pipeline: RenderPipeline,
}

/// The camera's push-constant range: one column-major matrix, and the
/// length [`Camera::bytes`] always is. Declared once so the two camera
/// pipelines cannot drift apart.
const CAMERA_PUSH_BYTES: u32 = 80;

// Drift between the declared range and the pack type is a compile
// error, not a record-time panic in a device-requiring test.
const _: () = assert!(CAMERA_PUSH_BYTES as usize == core::mem::size_of::<Camera>());

impl CameraRenderer {
    /// Build the pipeline.
    ///
    /// No buffer: the matrix crosses as push data recorded into the
    /// command stream per draw, so a camera costs no allocation, no
    /// retention slot, and cannot fail for a reason of its own.
    ///
    /// # Errors
    ///
    /// As [`MeshRenderer::new`].
    pub fn new(device: &Device, format: TargetFormat) -> Result<Self, Render3dError> {
        let pipeline = device.create_pipeline(
            &PipelineDesc::mesh(builtin::MESH_CAMERA, format, LAYOUT)
                .push_constant_size(CAMERA_PUSH_BYTES)
                .uniform_block(FADE_BYTES_U32)
                .depth_state(renew_rhi::DepthState::read_write()),
        )?;
        Ok(Self {
            pipeline,
            fade: Fade::new(device)?,
        })
    }

    /// Upload `scene` into geometry the GPU can draw.
    ///
    /// Positions are **world space** here, unlike [`MeshRenderer`].
    ///
    /// # Errors
    ///
    /// As [`MeshRenderer::upload`].
    pub fn upload(&self, device: &Device, scene: &Scene) -> Result<Mesh, Render3dError> {
        upload_scene(device, scene)
    }

    /// The draw for `mesh` seen through `camera`.
    ///
    /// The matrix is recorded as the item's push data — copied into the
    /// command stream at record time, so nothing outlives the call, and
    /// several camera items in one frame spend no allocation, no
    /// retention slot, and no buffer (each records its own sixty-four
    /// bytes; that is the whole per-item cost).
    #[must_use]
    pub fn item<'a>(&'a self, mesh: &'a Mesh, camera: &'a Camera) -> Item<'a> {
        Item::new(&self.pipeline)
            .mesh(mesh)
            .push_data(camera.bytes())
            // This pipeline samples nothing, so the block is its only
            // binding and sits at set zero.
            .bindings(&[&self.fade.binding])
            .uniform_data(&self.fade.bytes)
    }

    /// What distance fades toward, from the next item onward.
    ///
    /// **A scene value, so it is set here rather than passed per draw.**
    /// One horizon serves every item this renderer makes in a frame;
    /// threading a colour through `item` would say the opposite and put
    /// the same three floats at every call site.
    ///
    /// Defaults to [`renew_rhi::builtin::HORIZON`] — the colour these
    /// shaders compiled in before there was a way to say otherwise — so a
    /// caller that never calls this draws exactly what it drew before.
    ///
    /// **Match it to whatever the pass clears to.** The fade reads as
    /// depth when distance dissolves into the backdrop and as coloured
    /// fog when it does not, and nothing here can check that: the clear
    /// colour belongs to the pass, not to the pipeline.
    pub fn set_horizon(&mut self, horizon: [f32; 3]) {
        self.fade.set(horizon);
    }
}

impl core::fmt::Debug for CameraRenderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CameraRenderer").finish_non_exhaustive()
    }
}

impl core::fmt::Debug for MeshRenderer {
    /// Nothing to report but the fact of it: the pipeline's handle is not
    /// information, and this type holds no counts of its own.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MeshRenderer").finish_non_exhaustive()
    }
}

/// Everything a shadowed draw pushes: the camera's view-projection, the
/// light's first three rows, and how brightly the scene is lit — 128
/// bytes, exactly the guaranteed push ceiling, and read by BOTH halves
/// of the path.
///
/// # Three things where two used to be, in the same budget
///
/// The naive union is 144 bytes: two full matrices and a colour. That
/// does not fit, and it is the whole reason no path here carried a scene
/// light and a shadow at once — a consumer wanting a time of day and a
/// sun had to choose.
///
/// The sixteen bytes come from the light's matrix. Its projection is
/// orthographic and its view is rigid, so the product is affine and its
/// bottom row is exactly `(0, 0, 0, 1)`; carrying that row is carrying a
/// constant. Three rows are three tight `vec4`s — 48 bytes — and both
/// shaders write the fourth as a literal one.
///
/// **Rows, and not a `mat4x3`.** That type looks like the same saving
/// and is not: std430 pads each of its four three-component columns back
/// to sixteen bytes, so it is still 64 and the block is still 144, while
/// silently reading padding if the host packed 48.
///
/// # One record for both halves
///
/// The caster reads this same value and takes its light rows from it, so
/// the map cannot be written with one light and sampled with another.
/// That was reachable while the light was packed twice, and it also made
/// a host-side row/column mistake hard to see: with two encodings such a
/// mistake moves the cast slightly, which a golden can miss; with one it
/// makes every surface self-compare and the shadow vanish, which a
/// golden refuses loudly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowedCamera {
    bytes: [u8; 128],
}

impl ShadowedCamera {
    /// Pack the camera's columns and the light's rows, fully lit.
    ///
    /// White leaves a scene exactly as it was, so a caller with nothing
    /// to say about light gets the picture it got before there was
    /// anything to say — [`Camera::from_columns`]'s rule, one path over.
    ///
    /// # Panics
    ///
    /// If `light`'s bottom row is not `(0, 0, 0, 1)`. See [`Self::lit`].
    #[must_use]
    pub fn from_columns(camera: [[f32; 4]; 4], light: [[f32; 4]; 4]) -> Self {
        Self::lit(camera, light, [1.0; 3])
    }

    /// The same, under a scene light.
    ///
    /// `brightness` means what [`Camera::lit`]'s light means and is
    /// applied in the same stage, so a world drawn half by this pipeline
    /// and half by [`CameraRenderer`]'s dims alike rather than showing a
    /// seam between them.
    ///
    /// # Panics
    ///
    /// If `light`'s bottom row is not `(0, 0, 0, 1)`, because this pack
    /// does not transmit that row and both shaders write a literal one
    /// in its place — a projective light would draw a plausible wrong
    /// picture rather than fail. The same refusal covers a second
    /// mistake: the fragment stage's depth bias is a single constant
    /// only because an orthographic light's depth is linear in distance.
    /// Both signed zeroes satisfy the zero test.
    #[must_use]
    pub fn lit(camera: [[f32; 4]; 4], light: [[f32; 4]; 4], brightness: [f32; 3]) -> Self {
        // Retained, not debug-only: the failure it prevents is a picture
        // that looks composed rather than an error anything reports.
        // **Exact, and an epsilon here would be wrong.** The question is
        // not whether the light is nearly affine but whether the row
        // this pack DROPS is the row both shaders assume — a tolerance
        // would admit a matrix whose bottom row does something, and the
        // shaders would go on writing a literal one over it. Every light
        // this engine composes lands on exact zeroes and an exact one,
        // which the camera crate's own contract test holds it to; both
        // signed zeroes compare equal, which is the one slack wanted.
        #[expect(
            clippy::float_cmp,
            reason = "the dropped row must be exactly the row the shaders assume, not near it"
        )]
        let affine =
            light[0][3] == 0.0 && light[1][3] == 0.0 && light[2][3] == 0.0 && light[3][3] == 1.0;
        assert!(
            affine,
            "a shadowed camera's light must be affine — its bottom row must be (0, 0, 0, 1), \
             because the pack drops that row and both shaders write a literal one for it"
        );
        let mut bytes = [0u8; 128];
        let mut at = 0;
        let mut put = |value: f32, at: &mut usize| {
            bytes[*at..*at + 4].copy_from_slice(&value.to_ne_bytes());
            *at += 4;
        };
        // The camera, column-major, byte-identical to `Camera`'s first
        // sixty-four.
        for column in camera {
            for value in column {
                put(value, &mut at);
            }
        }
        // The light, ROW-major and three rows only: `light` is columns,
        // so row `r` is the `r`th element of each column in turn.
        for row in 0..3 {
            for column in light {
                put(column[row], &mut at);
            }
        }
        // Brightness, with alpha pinned to one and unread — a light that
        // touched alpha would dissolve cutout geometry as a scene
        // darkened, which is `Camera::lit`'s reasoning too.
        for value in brightness {
            put(value, &mut at);
        }
        put(1.0, &mut at);
        debug_assert_eq!(at, 128, "the pack must fill the declared range exactly");
        Self { bytes }
    }

    /// The packed bytes, exactly both shadowed pipelines' declared
    /// push-constant range.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// The shadowed pipelines' push-constant range: a camera matrix, the
/// light's three rows, and a scene light — the length
/// [`ShadowedCamera::bytes`] always is, and exactly the guaranteed
/// ceiling.
const SHADOW_PUSH_BYTES: u32 = renew_rhi::builtin::MESH_CAMERA_SHADOW_PUSH_BYTES;

// Drift between the declared range and the pack type is a compile
// error, not a record-time panic in a device-requiring test.
const _: () = assert!(SHADOW_PUSH_BYTES as usize == core::mem::size_of::<ShadowedCamera>());

/// Draws a world with a shadow: a depth-only caster pass renders the
/// scene from the light into a depth image, and the lit pipeline
/// samples that map (and the atlas) to dim what the light cannot see.
///
/// **One type owns the whole story** — the map, the caster, the lit
/// pipeline, and both bindings — because they only mean anything
/// together: a caster without the lit pass renders depth nobody reads,
/// and the lit pipeline without the caster samples undefined pixels
/// (which the frame contract refuses by name). The caster has a
/// depth-only stage of its own, reading the SAME push block the lit
/// pipeline reads and using only its light rows — one record for both
/// halves, so the map cannot be written with a light it is not sampled
/// with, and a host-side row/column mistake makes the shadow vanish
/// where two encodings would have let it merely shift.
///
/// The frame shape this type serves:
///
/// 1. [`Self::shadow_pass`] with [`Self::caster_item`]s — the world
///    from the light, depth only;
/// 2. a surface pass whose [`Self::item`]s draw the same world through
///    the camera, dimmed where the map recorded something nearer.
///
/// # Colour is not carried through unchanged
///
/// As [`CameraRenderer`]: this path fades toward a horizon with
/// distance, with the same two constants, because pipelines drawing
/// one world must fade alike or the seam between them shows. The
/// shadow term multiplies the surface before that fade, so a shadowed
/// face at the far plane is faded, not doubly darkened. The constants
/// stay in the shader for the reason recorded on [`CameraRenderer`].
pub struct ShadowedCameraRenderer {
    /// The scene values its fragment stage reads.
    fade: Fade,
    caster: RenderPipeline,
    lit: RenderPipeline,
    shadow_map: renew_rhi::RenderImage,
    atlas_binding: renew_rhi::Binding,
    shadow_binding: renew_rhi::Binding,
}

impl ShadowedCameraRenderer {
    /// Build the map at `shadow_size` texels square, both pipelines,
    /// and the bindings, uploading `pixels` as the atlas.
    ///
    /// # Errors
    ///
    /// As [`TexturedCameraRenderer::new`], plus the shadow map's own
    /// refusals: [`Render3dError::DepthUnsupported`] on an adapter
    /// with no depth format in the chain, or one whose depth format
    /// cannot be sampled (the rendering crate pre-checks the feature
    /// rather than discovering it in a later frame), and
    /// [`Render3dError::Texture`] for any other creation failure.
    pub fn new(
        device: &Device,
        format: TargetFormat,
        extent: renew_rhi::Extent,
        pixels: &[u8],
        shadow_size: u32,
    ) -> Result<Self, Render3dError> {
        let texture = device
            .create_texture(&renew_rhi::TextureDesc::colour(extent, pixels))
            .map_err(Render3dError::Texture)?;
        let sampler = device.create_sampler(&renew_rhi::SamplerDesc::atlas())?;
        let atlas_binding = device.create_binding(&renew_rhi::BindingDesc::new(
            renew_rhi::BindingSource::Texture(&texture),
            &sampler,
        ))?;
        let shadow_map = device
            .create_render_image(&renew_rhi::RenderImageDesc::new(
                renew_rhi::RenderImageKind::Depth,
                renew_rhi::Extent {
                    width: shadow_size,
                    height: shadow_size,
                },
            ))
            // A depthless adapter refuses the map by name, and that
            // refusal is this crate's own variant rather than a
            // texture failure — the same translation the depth-tested
            // pipelines make, for the same reason: the environment
            // declined, and a reader sent to "creating the texture"
            // would be sent to the wrong thing entirely.
            .map_err(|error| match error {
                TargetError::DepthUnsupported { chain } => {
                    Render3dError::DepthUnsupported { chain }
                }
                other => Render3dError::Texture(other),
            })?;
        let shadow_binding = device.create_binding(&renew_rhi::BindingDesc::new(
            renew_rhi::BindingSource::Image(&shadow_map),
            &sampler,
        ))?;
        let caster = device.create_pipeline(
            &PipelineDesc::depth_mesh(builtin::MESH_CAMERA_SHADOW_CASTER_VS_SPV, LAYOUT)
                .push_constant_size(SHADOW_PUSH_BYTES)
                .depth_state(renew_rhi::DepthState::read_write()),
        )?;
        let lit = device.create_pipeline(
            &PipelineDesc::mesh(builtin::MESH_CAMERA_SHADOW, format, LAYOUT)
                .push_constant_size(SHADOW_PUSH_BYTES)
                .sampled_bindings(2)
                .uniform_block(FADE_BYTES_U32)
                .depth_state(renew_rhi::DepthState::read_write()),
        )?;
        Ok(Self {
            caster,
            lit,
            shadow_map,
            atlas_binding,
            shadow_binding,
            fade: Fade::new(device)?,
        })
    }

    /// Upload `scene` into geometry the GPU can draw — world-space
    /// positions, as [`CameraRenderer::upload`].
    ///
    /// # Errors
    ///
    /// As [`MeshRenderer::upload`].
    pub fn upload(&self, device: &Device, scene: &Scene) -> Result<Mesh, Render3dError> {
        upload_scene(device, scene)
    }

    /// The caster's draw: `mesh` as the LIGHT sees it, depth only.
    ///
    /// **Takes the same value [`Self::item`] takes**, and reads only its
    /// light rows. One record for both halves is what stops the map
    /// being written with one light and sampled with another.
    #[must_use]
    pub fn caster_item<'a>(&'a self, mesh: &'a Mesh, camera: &'a ShadowedCamera) -> Item<'a> {
        Item::new(&self.caster).mesh(mesh).push_data(camera.bytes())
    }

    /// The depth pass that fills the shadow map with `items`, cleared
    /// to the reversed-Z far plane and stored for the lit pass to
    /// sample. Place it before the surface pass that draws the world.
    #[must_use]
    pub fn shadow_pass<'a>(&'a self, items: &'a [Item<'a>]) -> Pass<'a> {
        Pass::render_to(
            &self.shadow_map,
            Attachment::new(LoadOp::Clear(ClearValue::Depth(0.0)), StoreOp::Store),
            items,
        )
    }

    /// The lit draw: `mesh` through the camera, dimmed by the map, under
    /// the scene light.
    #[must_use]
    pub fn item<'a>(&'a self, mesh: &'a Mesh, camera: &'a ShadowedCamera) -> Item<'a> {
        Item::new(&self.lit)
            .mesh(mesh)
            .push_data(camera.bytes())
            .bindings(&[
                &self.atlas_binding,
                &self.shadow_binding,
                &self.fade.binding,
            ])
            .uniform_data(&self.fade.bytes)
    }

    /// What distance fades toward, from the next item onward.
    ///
    /// **A scene value, so it is set here rather than passed per draw.**
    /// One horizon serves every item this renderer makes in a frame;
    /// threading a colour through `item` would say the opposite and put
    /// the same three floats at every call site.
    ///
    /// Defaults to [`renew_rhi::builtin::HORIZON`] — the colour these
    /// shaders compiled in before there was a way to say otherwise — so a
    /// caller that never calls this draws exactly what it drew before.
    ///
    /// **Match it to whatever the pass clears to.** The fade reads as
    /// depth when distance dissolves into the backdrop and as coloured
    /// fog when it does not, and nothing here can check that: the clear
    /// colour belongs to the pass, not to the pipeline.
    pub fn set_horizon(&mut self, horizon: [f32; 3]) {
        self.fade.set(horizon);
    }
}

impl core::fmt::Debug for ShadowedCameraRenderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ShadowedCameraRenderer")
            .finish_non_exhaustive()
    }
}

/// The depth attachment a 3D frame needs: cleared to the far plane,
/// discarded at the end.
///
/// **No load spelling and no clear value to choose.** Loading depth on a
/// frame's first depth use is a contract violation the rendering crate
/// refuses with a retained assertion, and zero is the only clear value
/// that means "nothing is in front yet" under the reversed compare —
/// depth is reversed engine-wide, nearer is larger, and the far plane
/// is zero. A parameter here would be a way to spell a mistake.
///
/// Discarded rather than stored because nothing reads depth after the
/// frame; a caller that grows a use for it composes its own attachment.
#[must_use]
pub fn depth_attachment() -> Attachment {
    Attachment::new(LoadOp::Clear(ClearValue::Depth(0.0)), StoreOp::Discard)
}

/// A pass over `color` drawing `items`, with depth attached.
///
/// The convenience that makes the ordinary path correct: depth is not
/// optional for 3D geometry, and this is the shape that cannot omit it.
/// The parts stay public for the frames this does not fit — a caller
/// composing 3D geometry beside a 2D overlay builds its own pass from
/// the rendering crate's `color_attachment`, [`depth_attachment`] and
/// [`MeshRenderer::item`].
///
/// # One pass per frame
///
/// **Two of these in one frame is a wrong picture, and nothing refuses
/// it.** The depth attachment always clears, so a second pass starts from
/// an empty depth buffer and draws over geometry the first pass put in
/// front — exactly the plausible-looking wrong picture depth exists to
/// prevent. The rendering crate's contract check does not catch it: it
/// refuses a *load* on the frame's first depth use, which is a different
/// mistake. Geometry that belongs in one image belongs in one `pass`,
/// with as many items as it takes. A caller who genuinely wants two
/// depth-sharing passes needs an attachment that loads, which v0 does not
/// offer and which that check would refuse for the first pass anyway —
/// it composes its own from the rendering crate directly, knowing why.
///
/// # Panics
///
/// Not here — this only builds a description. But `color` is handed on
/// unexamined, and the rendering crate asserts at render time, in every
/// profile, that a pass carries exactly one colour attachment. An empty
/// or two-element slice therefore aborts at `render`, not at this call.
/// Unlike an empty scene, which is ordinary data and gets an ordinary
/// refusal, a caller passing the wrong number of attachments has made a
/// structural mistake rather than presented unusual data.
#[must_use]
pub fn pass<'a>(color: &'a [Attachment], items: &'a [Item<'a>]) -> Pass<'a> {
    Pass::new(color, items).depth(depth_attachment())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity, column-major and affine — the shape every light in
    /// these tests starts from.
    const IDENTITY: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    /// **The camera comes first, then the light's ROWS, then the
    /// brightness — and each of those three is a way to get it wrong.**
    /// The camera and the light are the same type and the same size, so
    /// the compiler cannot tell a swapped argument order from a correct
    /// one, and a swap draws a whole world lit from the wrong place: a
    /// plausible picture rather than a failure. Packing the light's
    /// columns where its rows belong is the same kind of mistake one
    /// level down, and it is the tidy this layout invites.
    ///
    /// Distinct values throughout, all exact in `f32`, so a swap, a
    /// reversal or an offset shows — except in the light's bottom row,
    /// which is forced to `(0, 0, 0, 1)` because the pack refuses
    /// anything else. Every one of the block's thirty-two words is
    /// pinned, which is also what proves the dropped row is absent:
    /// nothing else fits in thirty-two words.
    ///
    /// Probed by packing columns instead of rows, by dropping the alpha
    /// pin, and by swapping the camera and light halves.
    #[test]
    fn the_shadow_pack_carries_the_camera_then_the_lights_rows_then_a_brightness() {
        let mut value = 1.0f32;
        let mut next = || {
            let taken = value;
            value += 1.0;
            [taken, taken + 100.0, taken + 200.0, taken + 300.0]
        };
        let camera = [next(), next(), next(), next()];
        // Twelve distinct values in rows 0..2, and the bottom row forced
        // affine so the pack accepts it. `light[c][3]` IS row three.
        let mut light = [next(), next(), next(), next()];
        for (index, column) in light.iter_mut().enumerate() {
            column[3] = if index == 3 { 1.0 } else { 0.0 };
        }
        let packed = ShadowedCamera::lit(camera, light, [0.25, 0.5, 0.75]);
        let bytes = packed.bytes();
        assert_eq!(bytes.len(), SHADOW_PUSH_BYTES as usize);

        let read = |at: usize| {
            f32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };

        // The camera half is byte-identical to what the unshadowed path
        // packs, so a world drawn half by each cannot disagree about
        // where it is being seen from. Survives from the previous shape
        // of this test verbatim.
        assert_eq!(
            &bytes[..64],
            &Camera::from_columns(camera).bytes()[..64],
            "the camera half is not the camera path's own packing"
        );

        // **Rows, not columns.** Row `r` is the `r`th element of each
        // column in turn; packing the columns instead is the tidy this
        // layout invites and the reason this assertion is spelled out
        // rather than looped over the input shape.
        for row in 0..3usize {
            for (index, column) in light.iter().enumerate() {
                let at = 64 + row * 16 + index * 4;
                assert_eq!(
                    read(at).to_bits(),
                    column[row].to_bits(),
                    "light row {row}, column {index}, at byte {at}"
                );
            }
        }

        // Brightness, with alpha pinned.
        for (index, expected) in [0.25f32, 0.5, 0.75, 1.0].into_iter().enumerate() {
            let at = 112 + index * 4;
            assert_eq!(read(at).to_bits(), expected.to_bits(), "brightness at {at}");
        }

        // **Row three is nowhere in the block, and the assertions above
        // are what prove it.** They pin all thirty-two words — sixteen
        // camera, twelve light rows, four brightness — so nothing else
        // can be in a block that is exactly thirty-two words long. A
        // separate "row three is absent" check was tried and deleted: it
        // could never run, because affineness requires that row to be
        // zeroes and a one, leaving no distinctive value to search for.
        // The coverage gate is what noticed it never executed.
    }

    /// A shadowed camera with nothing said about light is a white one,
    /// so every picture committed before there was a light to say
    /// anything about stays exactly where it was.
    ///
    /// Probed by defaulting to anything else: every shadowed golden
    /// moves at once.
    #[test]
    fn an_unlit_shadowed_draw_is_a_white_one() {
        let camera = IDENTITY;
        let light = IDENTITY;
        // **Not compared against `lit(.., [1.0; 3])`**, which is what
        // `from_columns` delegates to — that would be a function against
        // its own body, unable to fail while the delegation exists. The
        // brightness slots are read directly instead, and a non-white
        // light is required to differ, which is what makes "white" mean
        // something rather than "whatever the default happens to be".
        let packed = ShadowedCamera::from_columns(camera, light);
        assert_ne!(
            packed.bytes(),
            ShadowedCamera::lit(camera, light, [0.5; 3]).bytes(),
            "the default is indistinguishable from a half light"
        );
        for index in 0..4usize {
            let at = 112 + index * 4;
            let found = f32::from_ne_bytes([
                packed.bytes()[at],
                packed.bytes()[at + 1],
                packed.bytes()[at + 2],
                packed.bytes()[at + 3],
            ]);
            assert_eq!(found.to_bits(), 1.0f32.to_bits(), "brightness slot {index}");
        }
    }

    /// **A projective light is refused, because the pack drops the row
    /// that would make it projective.** Both shaders write a literal one
    /// where row three belongs, so a light whose bottom row says
    /// otherwise draws a plausible wrong picture rather than failing —
    /// and the fragment stage's single constant depth bias is constant
    /// only because an orthographic light's depth is linear in distance.
    /// One refusal covers both mistakes.
    ///
    /// Written by hand rather than built from `renew-camera`: this crate
    /// depends on the rendering crate alone, deliberately.
    ///
    /// Probed by deleting the assertion: this stops panicking.
    #[test]
    #[should_panic(expected = "affine")]
    fn a_non_affine_light_is_refused_by_the_shadow_pack() {
        // Bottom row (0, 0, 1, 0): the shape a perspective divide takes.
        let mut light = IDENTITY;
        light[2][3] = 1.0;
        light[3][3] = 0.0;
        let _ = ShadowedCamera::from_columns(IDENTITY, light);
    }

    /// And a negative zero is accepted, because composing two legitimate
    /// matrices can produce one and refusing it would reject a light
    /// nothing is wrong with.
    ///
    /// Probed by comparing the bottom row's bits instead of its value:
    /// this fails.
    #[test]
    fn a_negatively_signed_zero_is_still_affine() {
        let mut light = IDENTITY;
        light[0][3] = -0.0;
        light[1][3] = -0.0;
        light[2][3] = -0.0;
        let _ = ShadowedCamera::from_columns(IDENTITY, light);
    }

    /// **An unlit camera is a white one**, so every caller that has
    /// nothing to say about light draws exactly what it drew before there
    /// was anything to say — which is the whole reason the default is not
    /// zero.
    #[test]
    fn a_camera_is_white_unless_told_otherwise() {
        let columns = [[0.0; 4]; 4];
        let plain = Camera::from_columns(columns);
        assert_eq!(plain.bytes(), Camera::lit(columns, [1.0; 3]).bytes());

        let light = |camera: &Camera| {
            let bytes = camera.bytes();
            [0usize, 1, 2, 3].map(|index| {
                let at = 64 + index * 4;
                f32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
            })
        };
        // Compared componentwise: these are exact powers of two written
        // straight into the bytes, so they round-trip exactly — but a
        // float array compared with `==` is a habit worth not having.
        let close = |found: [f32; 4], wanted: [f32; 4]| {
            found
                .iter()
                .zip(wanted)
                .all(|(found, wanted)| (found - wanted).abs() < f32::EPSILON)
        };
        assert!(
            close(light(&plain), [1.0; 4]),
            "an unlit camera is not white"
        );
        // Alpha stays one: this scales a colour, it does not fade one.
        assert!(close(
            light(&Camera::lit(columns, [0.25, 0.5, 0.75])),
            [0.25, 0.5, 0.75, 1.0]
        ));
    }

    /// **The packing, driven with no GPU at all.** The crate claims the
    /// bytes cross unchanged in column order; that claim is arithmetic,
    /// and a claim about bytes that can only be checked by looking at a
    /// picture is a claim checked nowhere most days.
    ///
    /// Sixteen distinct values, so a transposition, a reversal or a
    /// swapped pair all show as a different byte rather than cancelling.
    #[test]
    fn a_camera_packs_its_columns_in_order() {
        let columns = [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0, 16.0],
        ];
        let camera = Camera::from_columns(columns);
        let bytes = camera.bytes();
        assert_eq!(bytes.len(), 80, "four columns of four floats, and a light");
        for (index, expected) in (1u8..=16).enumerate() {
            let at = index * 4;
            let found =
                f32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
            assert!(
                (found - f32::from(expected)).abs() < f32::EPSILON,
                "float {index} of the packed matrix is {found}, wanted {expected} — \
                 the columns must arrive in the order GLSL's mat4(c0, c1, c2, c3) takes"
            );
        }
    }

    /// Two cameras built from the same columns are the same camera.
    ///
    /// The derived equality is what lets a caller notice the view has not
    /// moved and skip work; a hand-written one over a byte array is the
    /// kind of thing that silently compares padding instead.
    #[test]
    fn cameras_compare_by_their_matrix() {
        let columns = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let mut moved = columns;
        moved[3][0] = 0.5;
        assert_eq!(Camera::from_columns(columns), Camera::from_columns(columns));
        assert_ne!(
            Camera::from_columns(columns),
            Camera::from_columns(moved),
            "a camera that has moved must not compare equal to one that has not"
        );
    }

    /// **The depth refusal, driven with no GPU at all.** This is the
    /// whole reason the refusal is a translation rather than a device
    /// query: the mapping is exercised on every machine, where a
    /// pre-flight would need a depthless adapter no lane has.
    #[test]
    fn the_depth_refusal_is_translated_not_swallowed() {
        let refused: Render3dError = PipelineError::DepthUnsupported {
            chain: "D32_SFLOAT, D24_UNORM_S8_UINT",
        }
        .into();
        // `matches!` with the outcome in the message, the shape the
        // rendering crate's own refusal tables use: an arm that runs only
        // when the test fails is a line no passing run ever covers.
        assert!(
            matches!(
                &refused,
                Render3dError::DepthUnsupported { chain }
                    if *chain == "D32_SFLOAT, D24_UNORM_S8_UINT"
            ),
            "the depth refusal must keep its own name and chain, got {refused:?}"
        );
    }

    /// The chain of causes is intact: the two wrapping variants hand back
    /// what they wrap, and the two that stand alone hand back nothing.
    ///
    /// Worth asserting because `source` is what a caller printing a chain
    /// walks — a variant that dropped its cause would print one line
    /// where the useful sentence is the next one.
    #[test]
    fn the_wrapping_variants_expose_their_cause() {
        use std::error::Error as _;

        let pipeline = Render3dError::Pipeline(PipelineError::InvalidSpirv {
            stage: "vertex",
            reason: "bad magic",
        });
        let upload = Render3dError::Upload(TargetError::OutOfDeviceMemory {
            call: "vkAllocateMemory(mesh)",
        });
        assert!(
            pipeline
                .source()
                .is_some_and(|cause| cause.to_string().contains("bad magic")),
            "the pipeline refusal must hand back what it wraps"
        );
        assert!(
            upload
                .source()
                .is_some_and(|cause| cause.to_string().contains("vkAllocateMemory(mesh)")),
            "the upload refusal must hand back what it wraps"
        );
        assert!(
            Render3dError::Texture(TargetError::OutOfDeviceMemory {
                call: "vkAllocateMemory(atlas)",
            })
            .source()
            .is_some_and(|cause| cause.to_string().contains("vkAllocateMemory(atlas)")),
            "the texture refusal must hand back what it wraps"
        );
        assert!(
            Render3dError::EmptyScene.source().is_none(),
            "an empty scene wraps nothing; a cause here would be invented"
        );
        assert!(
            Render3dError::DepthUnsupported {
                chain: "D32_SFLOAT"
            }
            .source()
            .is_none(),
            "the depth refusal is this crate's own words, wrapping nothing"
        );
    }

    /// Every other pipeline refusal keeps the rendering crate's words
    /// rather than being flattened into one message.
    #[test]
    fn other_pipeline_failures_pass_through_intact() {
        let refused: Render3dError = PipelineError::InvalidSpirv {
            stage: "vertex",
            reason: "bad magic",
        }
        .into();
        let shown = refused.to_string();
        assert!(shown.contains("bad magic"), "{shown}");
        assert!(shown.contains("vertex"), "{shown}");
    }

    /// An upload failure names the operation and carries the cause.
    #[test]
    fn an_upload_failure_says_what_it_was_doing() {
        let refused: Render3dError = TargetError::OutOfDeviceMemory {
            call: "vkAllocateMemory(mesh)",
        }
        .into();
        let shown = refused.to_string();
        assert!(shown.contains("uploading the geometry"), "{shown}");
        assert!(shown.contains("vkAllocateMemory(mesh)"), "{shown}");
    }

    /// **A texture failure names the texture.** It happens while a
    /// renderer is being built, before any scene exists, so reporting it
    /// as an upload of geometry would send a reader to look at something
    /// nobody has offered yet — the same trap the matrix buffer fell
    /// into.
    #[test]
    fn a_texture_failure_is_not_reported_as_a_geometry_upload() {
        let out_of_memory = || TargetError::OutOfDeviceMemory {
            call: "vkAllocateMemory",
        };
        let texture = Render3dError::Texture(out_of_memory()).to_string();
        assert!(
            texture.contains("texture"),
            "the texture failure must name the texture: {texture}"
        );
        assert!(
            !texture.contains("geometry"),
            "the texture failure must not send a reader to look at a scene: {texture}"
        );
    }

    /// Every variant says something a reader can act on.
    #[test]
    fn every_variant_displays_its_context() {
        let cases = [
            (
                Render3dError::DepthUnsupported {
                    chain: "D32_SFLOAT",
                },
                "D32_SFLOAT",
            ),
            (Render3dError::EmptyScene, "no geometry"),
            (
                Render3dError::Texture(TargetError::OutOfDeviceMemory {
                    call: "vkAllocateMemory(atlas)",
                }),
                "creating the texture",
            ),
        ];
        for (error, needle) in cases {
            let shown = error.to_string();
            assert!(shown.contains(needle), "`{shown}` missing `{needle}`");
        }
    }

    /// The depth attachment clears to the far plane and keeps nothing —
    /// asserted because both are decisions rather than defaults. Under
    /// reversed depth the far plane is zero.
    #[test]
    fn the_depth_attachment_clears_far_and_discards() {
        let depth = depth_attachment();
        assert!(
            // Bit equality, the rendering crate's own precedent for this:
            // the value is a literal this code wrote and nothing computes
            // on it, so a tolerance would be looser than the truth.
            matches!(depth.load, LoadOp::Clear(ClearValue::Depth(value)) if value.to_bits() == 0.0f32.to_bits()),
            "depth clears to the far plane (zero, reversed), or nothing is in front of anything"
        );
        assert!(matches!(depth.store, StoreOp::Discard));
    }

    /// The layout and the packed stride describe the same bytes, checked
    /// mechanically so only the shader stays coupled by comment. The
    /// rendering crate asserts this equality at record time; failing it
    /// here is a great deal easier to read.
    #[test]
    fn the_layout_and_the_stride_describe_the_same_bytes() {
        let packed: u32 = LAYOUT
            .iter()
            .map(|attribute| attribute_width(*attribute))
            .sum();
        assert_eq!(packed, VERTEX_STRIDE, "the layout and the scene disagree");
    }

    /// The packed width of one attribute.
    ///
    /// Named rather than inlined so the exhaustive match is reachable:
    /// the rendering crate's enum carries no `#[non_exhaustive]`
    /// precisely so a new format is a compile error here, and a match
    /// folded into the sum above would leave the arms this layout does
    /// not use unexecuted.
    fn attribute_width(attribute: VertexAttribute) -> u32 {
        match attribute {
            VertexAttribute::Vec2 => 8,
            VertexAttribute::Vec3 => 12,
            VertexAttribute::Vec4 => 16,
        }
    }

    #[test]
    fn every_attribute_reports_its_packed_width() {
        assert_eq!(attribute_width(VertexAttribute::Vec2), 8);
        assert_eq!(attribute_width(VertexAttribute::Vec3), 12);
        assert_eq!(attribute_width(VertexAttribute::Vec4), 16);
    }
}
