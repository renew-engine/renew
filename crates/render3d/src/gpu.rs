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
    Attachment, ClearValue, Color, Device, Item, LoadOp, Mesh, MeshDesc, Pass, PipelineDesc,
    PipelineError, RenderPipeline, StoreOp, TargetError, TargetFormat, VertexAttribute, builtin,
};

use crate::scene::{Scene, VERTEX_STRIDE};

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
    /// The buffer the camera's matrix rides in could not be allocated.
    ///
    /// **Its own variant rather than [`Self::Upload`]**, which is where
    /// the blanket conversion from a target failure would have put it.
    /// The two are the same kind of refusal from the driver and different
    /// events entirely for a reader: this one happens in
    /// [`CameraRenderer::new`], before any scene exists, so reporting it
    /// as an upload of geometry describes something that had not been
    /// asked for yet. A sixty-four-byte allocation failing also means
    /// something quite different from a mesh failing — it is not a large
    /// mesh, it is a device with nothing left.
    CameraBuffer(TargetError),
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
            Self::CameraBuffer(error) => {
                write!(f, "allocating the camera's matrix buffer: {error}")
            }
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
            Self::Upload(error) | Self::CameraBuffer(error) => Some(error),
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

/// A view-projection matrix, packed for the instance stream.
///
/// Four columns of four floats, column-major — the order
/// `renew_math::Mat4` stores and the order GLSL's `mat4(c0, c1, c2, c3)`
/// takes, so the *order* crosses unchanged.
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
    bytes: [u8; 64],
}

impl Camera {
    /// Pack four column vectors.
    #[must_use]
    pub fn from_columns(columns: [[f32; 4]; 4]) -> Self {
        let mut bytes = [0u8; 64];
        let mut at = 0;
        for column in columns {
            for value in column {
                bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
                at += 4;
            }
        }
        Self { bytes }
    }

    /// The packed bytes, as the instance stream wants them.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
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
/// The constants are constants because this crate has no channel for
/// per-draw scalars that is not already carrying the camera matrix. When
/// one exists they should be the first thing to use it, at which point
/// this section describes a default rather than a fact.
pub struct CameraRenderer {
    pipeline: RenderPipeline,
    matrix: renew_rhi::Buffer,
}

impl CameraRenderer {
    /// Build the pipeline and the buffer the matrix rides in.
    ///
    /// # Errors
    ///
    /// As [`MeshRenderer::new`], plus [`Render3dError::CameraBuffer`]
    /// when the per-frame buffer the matrix is written into cannot be
    /// allocated.
    pub fn new(device: &Device, format: TargetFormat) -> Result<Self, Render3dError> {
        let pipeline = device.create_pipeline(
            &PipelineDesc::mesh(builtin::MESH_CAMERA, format, LAYOUT)
                .instance_input(builtin::MESH_CAMERA_INSTANCE_LAYOUT)
                .depth_state(renew_rhi::DepthState::read_write()),
        )?;
        // One matrix, sixty-four bytes, rewritten every frame. The
        // per-frame buffer is the crate's own answer to "written by the
        // host while an earlier frame may still be reading".
        // Mapped by hand rather than through `?`: the blanket
        // conversion from a target failure means "uploading the
        // geometry", and no geometry has been offered at this point.
        let matrix = device
            .create_buffer(64, renew_rhi::BufferUsage::PerFrame)
            .map_err(Render3dError::CameraBuffer)?;
        Ok(Self { pipeline, matrix })
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
    /// One instance: the matrix is the same for every vertex, and the
    /// instance stream is how it reaches them.
    #[must_use]
    pub fn item<'a>(&'a self, mesh: &'a Mesh, camera: &'a Camera) -> Item<'a> {
        Item::new(&self.pipeline)
            .mesh(mesh)
            .frame_data(renew_rhi::FrameData::new(&self.matrix, camera.bytes(), 1))
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

/// The colour attachment a 3D frame renders into: cleared to `clear`,
/// stored.
#[must_use]
pub fn attachment(clear: Color) -> Attachment {
    Attachment::new(LoadOp::Clear(ClearValue::Color(clear)), StoreOp::Store)
}

/// The depth attachment a 3D frame needs: cleared to the far plane,
/// discarded at the end.
///
/// **No load spelling and no clear value to choose.** Loading depth on a
/// frame's first depth use is a contract violation the rendering crate
/// refuses with a retained assertion, and one is the only clear value
/// that means "nothing is in front yet" under the fixed compare. A
/// parameter here would be a way to spell a mistake.
///
/// Discarded rather than stored because nothing reads depth after the
/// frame; a caller that grows a use for it composes its own attachment.
#[must_use]
pub fn depth_attachment() -> Attachment {
    Attachment::new(LoadOp::Clear(ClearValue::Depth(1.0)), StoreOp::Discard)
}

/// A pass over `color` drawing `items`, with depth attached.
///
/// The convenience that makes the ordinary path correct: depth is not
/// optional for 3D geometry, and this is the shape that cannot omit it.
/// The parts stay public for the frames this does not fit — a caller
/// composing 3D geometry beside a 2D overlay builds its own pass from
/// [`attachment`], [`depth_attachment`] and [`MeshRenderer::item`].
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
        assert_eq!(bytes.len(), 64, "four columns of four floats");
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
            Render3dError::CameraBuffer(TargetError::OutOfDeviceMemory {
                call: "vkAllocateMemory(matrix)",
            })
            .source()
            .is_some_and(|cause| cause.to_string().contains("vkAllocateMemory(matrix)")),
            "the matrix refusal must hand back what it wraps"
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

    /// **The camera's matrix and a mesh fail differently in words.**
    /// Both arrive as the same driver refusal, so nothing but this
    /// mapping distinguishes them, and a reader given "uploading the
    /// geometry" for a failure inside `new` would go looking at a scene
    /// that had not been built yet.
    #[test]
    fn a_matrix_buffer_failure_is_not_reported_as_a_geometry_upload() {
        let out_of_memory = || TargetError::OutOfDeviceMemory {
            call: "vkAllocateMemory",
        };
        let matrix = Render3dError::CameraBuffer(out_of_memory()).to_string();
        let geometry = Render3dError::from(out_of_memory()).to_string();
        assert!(
            matrix.contains("camera's matrix buffer"),
            "the matrix failure must name the matrix: {matrix}"
        );
        assert!(
            !matrix.contains("geometry"),
            "the matrix failure must not send a reader to look at a scene: {matrix}"
        );
        assert!(
            geometry.contains("uploading the geometry"),
            "the geometry failure keeps its own words: {geometry}"
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
                Render3dError::CameraBuffer(TargetError::OutOfDeviceMemory {
                    call: "vkAllocateMemory(matrix)",
                }),
                "the camera's matrix buffer",
            ),
        ];
        for (error, needle) in cases {
            let shown = error.to_string();
            assert!(shown.contains(needle), "`{shown}` missing `{needle}`");
        }
    }

    /// The depth attachment clears to the far plane and keeps nothing —
    /// asserted because both are decisions rather than defaults.
    #[test]
    fn the_depth_attachment_clears_far_and_discards() {
        let depth = depth_attachment();
        assert!(
            // Bit equality, the rendering crate's own precedent for this:
            // the value is a literal this code wrote and nothing computes
            // on it, so a tolerance would be looser than the truth.
            matches!(depth.load, LoadOp::Clear(ClearValue::Depth(value)) if value.to_bits() == 1.0f32.to_bits()),
            "depth clears to the far plane, or nothing is in front of anything"
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
