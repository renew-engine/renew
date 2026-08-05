//! The v0 graphics pipeline: two SPIR-V stages, dynamic rendering into
//! one color attachment, optionally one sampled texture, and optionally
//! vertex input — per-vertex at binding 0, per-instance at binding 1.
//!
//! **Two pipeline shapes, and which one a pipeline is decides where its
//! vertex count comes from.** A generative pipeline's stages write their
//! own vertex list, so the count belongs to the shader and travels with
//! it in [`Shaders`]. A mesh pipeline reads a per-vertex stream, so the
//! count belongs to the geometry and arrives at the draw — which is why
//! its stages are a [`MeshShaders`] carrying no count at all rather than
//! a `Shaders` carrying one that nothing reads.

use std::fmt;
use std::rc::Rc;

use ash::vk;

use crate::error::PipelineError;
use crate::vk::buffer::Buffer;
use crate::vk::device::{Device, DeviceShared};
use crate::vk::texture::Texture;

/// The color format a pipeline renders into. Must match the target it
/// is used with (checked as a contract in dev builds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetFormat {
    /// The offscreen target's format.
    Rgba8Unorm,
    /// The common swapchain format on desktop.
    Bgra8Unorm,
}

impl TargetFormat {
    pub(crate) fn to_vk(self) -> vk::Format {
        match self {
            Self::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
            Self::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        }
    }
}

/// One vertex attribute, in declaration order.
///
/// Closed and small on purpose: these are the shapes the paths in this
/// crate consume today, and a format nobody binds is an enum arm no test
/// can reach. Offsets and locations are derived from position in the
/// slice — the caller declares an order, never arithmetic.
///
/// **Named for what it is rather than for the rate it arrives at.** It
/// described per-instance data only while per-instance data was the only
/// kind; a per-vertex stream reads the same formats through the same
/// descriptions, and a type named `InstanceAttribute` sitting in a
/// per-vertex list would misdescribe its own module.
///
/// **Deliberately not `#[non_exhaustive]`**, following the resolution
/// that removed the attribute from this module's sibling enums: growing
/// it should be a compile error at every in-tree match, naming each site
/// that must handle the new format, rather than routing it silently into
/// a wildcard arm no test can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexAttribute {
    /// Two 32-bit floats.
    Vec2,
    /// Three 32-bit floats — a position, which is the one shape a mesh
    /// cannot express as any of the others without padding every vertex.
    Vec3,
    /// Four 32-bit floats.
    Vec4,
}

impl VertexAttribute {
    pub(crate) fn byte_len(self) -> u32 {
        match self {
            Self::Vec2 => 8,
            Self::Vec3 => 12,
            Self::Vec4 => 16,
        }
    }

    pub(crate) fn format(self) -> vk::Format {
        match self {
            Self::Vec2 => vk::Format::R32G32_SFLOAT,
            Self::Vec3 => vk::Format::R32G32B32_SFLOAT,
            Self::Vec4 => vk::Format::R32G32B32A32_SFLOAT,
        }
    }
}

/// The buffer binding a per-vertex stream is bound at.
///
/// **Fixed rather than derived from which streams a pipeline declares.**
/// Vulkan's input rate is a property of a binding, so two rates need two
/// bindings; assigning them by rate rather than by presence means the
/// number is a constant every reader can check, and the pipeline builder
/// and both record paths read this one definition instead of three
/// agreeing literals. A pipeline declaring only per-instance input leaves
/// binding 0 undeclared, which is legal — bindings need not be dense —
/// and changes no GLSL anywhere, because a shader declares locations, not
/// bindings.
pub(crate) const VERTEX_BINDING: u32 = 0;

/// The buffer binding a per-instance stream is bound at. See
/// [`VERTEX_BINDING`] for why these are constants rather than derived.
pub(crate) const INSTANCE_BINDING: u32 = 1;

/// Pipeline construction parameters. The SPIR-V is borrowed byte
/// slices — [`crate::builtin`] provides the embedded v0 shaders.
///
/// `#[non_exhaustive]`, so it is built through [`PipelineDesc::new`]
/// rather than as a struct literal. Every field this will grow — vertex
/// input, blend state, depth state, push-constant ranges — is optional
/// with a defined absence, so each can arrive as a builder method
/// without touching a single existing caller. Without the attribute,
/// adding one field edits every construction site in the workspace, and
/// the count only rises.
///
/// **Not `Default`.** A default would have to supply empty shader bytes,
/// and empty SPIR-V is rejected by name during validation — so the
/// default value would be one that can never successfully build a
/// pipeline. A constructor taking exactly the parameters with no
/// sensible absence is the honest shape.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PipelineDesc<'a> {
    pub vertex_spirv: &'a [u8],
    pub fragment_spirv: &'a [u8],
    pub target_format: TargetFormat,
    /// How many vertices the vertex stage generates for one draw.
    ///
    /// With no per-vertex buffer, the vertex list is written into the
    /// shader, so its length is a property of the shader and not of the
    /// frame -- which is why it is set here and the caller never passes
    /// a count to a draw. Asking a stage for more vertices than it has
    /// indexes past the end of its own constant array.
    ///
    /// **Ignored by a pipeline that declares [`Self::vertex_input`]**,
    /// whose count comes from the geometry instead. The two are the
    /// mutually exclusive answers to one question, and which answers it
    /// is decided by whether a per-vertex layout is declared.
    pub vertex_count: u32,
    /// Per-vertex input, or `None` for the shaders that write their
    /// vertex list into the source.
    ///
    /// Declaring this makes the pipeline a *mesh* pipeline: it may only
    /// be drawn by an item naming geometry, and the draw becomes indexed
    /// with its count taken from that geometry. The frame contract
    /// refuses the mismatch by name before any GPU call, the same way it
    /// refuses a depth-testing pipeline in a depthless pass.
    pub vertex_input: Option<&'a [VertexAttribute]>,
    /// Per-instance input, or `None`. Bytes here advance once per
    /// instance rather than once per vertex — for the shaders that
    /// expand corners from `gl_VertexIndex`, this is the only stream
    /// they read.
    pub instance_input: Option<&'a [VertexAttribute]>,
    /// How this pipeline's output combines with the target's contents.
    /// [`Blend::Opaque`] — no blending — unless the builder says
    /// otherwise.
    pub blend: Blend,
    /// The texture this pipeline samples, and how.
    ///
    /// **Bound here rather than after creation, and that is the whole
    /// design.** The descriptor set is written once, before the pipeline
    /// can be used, so it can never be rewritten while a submit that
    /// reads it is still running -- the rule a post-creation setter
    /// would need is not merely satisfied but unstatable. The cost is
    /// that two textures mean two pipelines over identical shaders,
    /// which is free for one atlas and is not free for a material
    /// system.
    ///
    /// Shared ownership because the pipeline must keep both alive for
    /// as long as the set points at them, without the caller having to
    /// sequence drops correctly.
    pub texture: Option<(Rc<Texture>, Rc<Sampler>)>,
    /// Depth testing, or `None` for the depth-free pipelines every
    /// depthless pass records. A pipeline carrying this can only draw
    /// inside a pass that carries a depth attachment, and the reverse —
    /// the targets assert the match by name.
    pub depth_state: Option<DepthState>,
}

/// How a pipeline's output is combined with what the target already
/// holds.
///
/// An input enum, so `#[non_exhaustive]`: a third mode later must not be
/// a breaking change for downstream matchers. Both variants are bound by
/// tests where they live — the default by every existing pipeline, the
/// premultiplied mode by the sprite path that exists for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Blend {
    /// Blending disabled; the draw's output replaces the target's
    /// contents wherever a fragment lands. Today's behavior, and the
    /// default.
    Opaque,
    /// `src + dst * (1 - src.a)`, color and alpha alike — compositing
    /// for sources whose color is already multiplied by their alpha.
    /// The premultiplied convention is the caller's obligation; bytes
    /// that are not premultiplied composite wrong, visibly, not
    /// unsafely.
    PremultipliedAlpha,
}

/// A vertex/fragment pair and the number of vertices its vertex stage
/// generates.
///
/// **The three travel together because they are only correct together.**
/// A stage that reads no per-vertex buffer writes its vertex list into
/// the shader, so the count belongs to that shader and to no other.
/// Passed separately they are two safe values that compile in any
/// combination: too low and the draw renders part of the geometry, too
/// high and the stage indexes past the end of its own constant array.
/// Bundled, the mismatch cannot be spelled.
///
/// **A stage that *does* read a per-vertex buffer takes [`MeshShaders`]
/// instead**, because its premise is the opposite one: the count belongs
/// to the geometry, so there is none here to bundle.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Shaders<'a> {
    /// Vertex stage SPIR-V.
    pub vertex: &'a [u8],
    /// Fragment stage SPIR-V.
    pub fragment: &'a [u8],
    /// How many vertices [`Self::vertex`] generates for one draw.
    pub vertex_count: u32,
}

impl<'a> Shaders<'a> {
    /// A stage pair and its vertex count.
    #[must_use]
    pub fn new(vertex: &'a [u8], fragment: &'a [u8], vertex_count: u32) -> Self {
        Self {
            vertex,
            fragment,
            vertex_count,
        }
    }
}

/// A vertex/fragment pair whose vertex stage reads a per-vertex stream.
///
/// **A second bundle type rather than a count-carrying one with the
/// count ignored.** [`Shaders`] bundles a vertex count because a stage
/// that writes its own vertex list owns that number. A mesh stage does
/// not: the count belongs to the geometry, and arrives with it at the
/// draw. Given a single bundle, a mesh pipeline would have to carry a
/// number nothing reads — and a caller writing `Shaders::new(vs, fs, 6)`
/// for a mesh pipeline would get a silently ignored `6`. Two types remove
/// that from the constructors, which is the same reasoning that makes a
/// clear value ride the load op that uses it.
///
/// **What this does not do, stated because the stronger claim is the
/// tempting one:** it does not make the bad value *unspellable*.
/// `PipelineDesc`'s fields are `pub`, and `#[non_exhaustive]` blocks
/// struct-literal construction from outside the crate rather than
/// assignment to a field of a value already built — so a caller can still
/// set `vertex_count` on a mesh descriptor, or clear `vertex_input` on
/// one. Both are then caught at `render` by the frame contract rather
/// than by the compiler. Closing that would mean private fields with
/// accessors across every descriptor in this crate, which one pipeline
/// shape does not justify.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct MeshShaders<'a> {
    /// Vertex stage SPIR-V, declaring per-vertex inputs.
    pub vertex: &'a [u8],
    /// Fragment stage SPIR-V.
    pub fragment: &'a [u8],
}

impl<'a> MeshShaders<'a> {
    /// A stage pair that reads geometry.
    #[must_use]
    pub fn new(vertex: &'a [u8], fragment: &'a [u8]) -> Self {
        Self { vertex, fragment }
    }
}

impl<'a> PipelineDesc<'a> {
    /// The two things a pipeline cannot be built without.
    ///
    /// Positional rather than a builder chain because neither has a
    /// meaningful default: a pipeline with no shaders, or with no target
    /// format, is not a partially-configured pipeline, it is not a
    /// pipeline. Optional state arrives later as builder methods on top
    /// of this.
    #[must_use]
    pub fn new(shaders: Shaders<'a>, target_format: TargetFormat) -> Self {
        Self {
            vertex_spirv: shaders.vertex,
            fragment_spirv: shaders.fragment,
            target_format,
            vertex_count: shaders.vertex_count,
            blend: Blend::Opaque,
            texture: None,
            vertex_input: None,
            instance_input: None,
            depth_state: None,
        }
    }

    /// Combine output with the target per `blend` instead of replacing
    /// it. The premultiplied convention, where chosen, is the caller's
    /// obligation on every byte the pipeline samples or tints.
    #[must_use]
    pub fn blend(mut self, blend: Blend) -> Self {
        self.blend = blend;
        self
    }

    /// A mesh pipeline: stages that read a per-vertex stream of
    /// `layout`, drawing geometry supplied per item.
    ///
    /// **The layout is positional, not a builder, and the shaders are a
    /// different type from the generative ones.** Both follow from the
    /// same fact: a mesh pipeline's vertex count comes from the geometry,
    /// so there is no count to supply and no meaningful pipeline without
    /// a layout. Between them, "a mesh pipeline carrying a vertex count"
    /// and "a mesh pipeline with no per-vertex layout" are values that
    /// cannot be written down rather than mistakes that are documented.
    ///
    /// Locations and offsets are derived from position in `layout`; the
    /// shader's `location(n)` list and this slice describe the same bytes
    /// or the draw reads garbage, which is why the mesh builtin and its
    /// layout slice live beside each other. The packed sum of the
    /// attributes is the stride every mesh drawn by this pipeline must
    /// have — a disagreement fetches past the end of the mesh's
    /// allocation, so it is refused by a retained assertion where the
    /// draw is recorded.
    #[must_use]
    pub fn mesh(
        shaders: MeshShaders<'a>,
        target_format: TargetFormat,
        layout: &'a [VertexAttribute],
    ) -> Self {
        Self {
            vertex_spirv: shaders.vertex,
            fragment_spirv: shaders.fragment,
            target_format,
            // Never read on this path: the draw takes its count from the
            // geometry. Zero rather than a sentinel because no caller can
            // supply it and nothing consults it.
            vertex_count: 0,
            blend: Blend::Opaque,
            texture: None,
            vertex_input: Some(layout),
            instance_input: None,
            depth_state: None,
        }
    }

    /// Declare per-instance vertex input, in order. Locations and
    /// offsets are derived from position; the shader's `location(n)`
    /// list and this slice describe the same layout or the draw reads
    /// garbage, which is why the instanced builtin and its layout slice
    /// live beside each other.
    ///
    /// Where a pipeline declares both, per-vertex locations come first
    /// and per-instance locations continue after them — one location
    /// space across two bindings, as Vulkan requires.
    #[must_use]
    pub fn instance_input(mut self, attributes: &'a [VertexAttribute]) -> Self {
        self.instance_input = Some(attributes);
        self
    }

    /// Sample `texture` through `sampler`, at set 0 binding 0.
    ///
    /// The fragment stage must declare a matching combined image
    /// sampler; a pipeline given a texture its shader does not sample
    /// is not an error, merely a set nothing reads.
    #[must_use]
    pub fn texture(mut self, texture: Rc<Texture>, sampler: Rc<Sampler>) -> Self {
        self.texture = Some((texture, sampler));
        self
    }

    /// Test (and per `depth`, write) the pass's depth attachment. The
    /// compare op is `LESS_OR_EQUAL` in v0.
    #[must_use]
    pub fn depth_state(mut self, depth: DepthState) -> Self {
        self.depth_state = Some(depth);
        self
    }
}

/// Per-frame bytes and the instanced draw they feed.
///
/// **`#[non_exhaustive]` with a constructor, per the descriptor pattern
/// this crate uses everywhere** -- room to grow (a first-instance or
/// vertex-offset field) without touching existing callers.
///
/// The bytes are written into the buffer's region for the frame being
/// recorded, *after* the point where "no submit is reading this region"
/// is a proven fact on the target recording it. The draw stays counts
/// and offsets: `instances` here, the vertex count from the pipeline's
/// shaders, the slot offset chosen by the target.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct FrameData<'a> {
    /// The buffer whose current-slot region receives `bytes`.
    pub buffer: &'a Buffer,
    /// This frame's data. Length must be at most the buffer's per-frame
    /// capacity; over-length is refused by a retained assertion, never
    /// truncated -- a truncated instance is a quiet wrong draw.
    pub bytes: &'a [u8],
    /// Instance count for the draw.
    pub instances: u32,
}

impl<'a> FrameData<'a> {
    /// `bytes` for this frame, feeding `instances` instances.
    #[must_use]
    pub fn new(buffer: &'a Buffer, bytes: &'a [u8], instances: u32) -> Self {
        Self {
            buffer,
            bytes,
            instances,
        }
    }
}

/// Whether a pipeline tests and writes the pass's depth attachment.
///
/// `#[non_exhaustive]` with constructors: the compare op is fixed
/// `LESS_OR_EQUAL` in v0 and arrives as a builder when a consumer needs
/// another.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct DepthState {
    /// Fragments failing the depth test are discarded.
    pub test: bool,
    /// Surviving fragments write their depth.
    pub write: bool,
}

impl DepthState {
    /// Test against and write the depth attachment — the common case.
    #[must_use]
    pub fn read_write() -> Self {
        Self {
            test: true,
            write: true,
        }
    }

    /// Test without writing — geometry that respects depth but leaves
    /// no footprint in it.
    #[must_use]
    pub fn test_only() -> Self {
        Self {
            test: true,
            write: false,
        }
    }
}

/// How a sampled image is filtered and addressed.
///
/// **`#[non_exhaustive]` with a constructor, per the descriptor pattern
/// this crate now uses everywhere** -- filter and address mode are the
/// two a sprite atlas needs, and mip mode, anisotropy and border colour
/// arrive as builders touching no caller.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct SamplerDesc {
    /// How texels are chosen when the sample lands between them.
    pub filter: Filter,
    /// What happens outside `[0, 1]`.
    pub address: AddressMode,
}

impl SamplerDesc {
    /// The atlas default: nearest, clamped.
    ///
    /// **Nearest rather than linear, and this is a decision rather than
    /// an omission.** It is the single parameter deciding whether a
    /// sprite atlas comes out crisp or blurred. Nearest keeps it exact,
    /// and keeps reference-image comparisons meaningful: those compare
    /// bytes, and linear filtering makes the bytes depend on how a
    /// particular adapter interpolates rather than on the engine.
    /// Clamped because an atlas has no meaning outside its own edges --
    /// a wrapped sample reads a neighbouring sprite, which is a bug that
    /// looks like a rendering artifact.
    #[must_use]
    pub fn atlas() -> Self {
        Self {
            filter: Filter::Nearest,
            address: AddressMode::ClampToEdge,
        }
    }
}

/// Texel selection between sample points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// The nearest texel. Exact, and what a sprite atlas wants.
    Nearest,
    /// Bilinear blend of the four surrounding texels.
    Linear,
}

/// What a sample outside `[0, 1]` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressMode {
    /// The edge texel, repeated.
    ClampToEdge,
    /// The image, tiled.
    Repeat,
}

impl Filter {
    fn to_vk(self) -> vk::Filter {
        match self {
            Self::Nearest => vk::Filter::NEAREST,
            Self::Linear => vk::Filter::LINEAR,
        }
    }
}

impl AddressMode {
    fn to_vk(self) -> vk::SamplerAddressMode {
        match self {
            Self::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
            Self::Repeat => vk::SamplerAddressMode::REPEAT,
        }
    }
}

/// A sampler. Holds its device alive; destroyed on drop.
///
/// # Contract
///
/// A sampler must outlive every descriptor set that references it.
///
/// **The caller cannot violate that, because they do not hold the
/// obligation.** [`PipelineDesc::texture`] takes shared ownership, and
/// the [`RenderPipeline`] keeps its clone for as long as the descriptor
/// set exists — so a caller who drops their own handle mid-frame takes
/// nothing away from the set. `Drop` therefore needs no quiesce: it can
/// only run once the pipeline has released its clone, which happens
/// inside `RenderPipeline`'s own `Drop`, after that has waited for the
/// device to go idle and destroyed the pool.
pub struct Sampler {
    pub(crate) shared: Rc<DeviceShared>,
    pub(crate) sampler: vk::Sampler,
}

impl fmt::Debug for Sampler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sampler").finish_non_exhaustive()
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // Rc; the handle was created with these callbacks; the owner of
        // any descriptor set referencing it has already quiesced.
        unsafe {
            self.shared
                .device
                .destroy_sampler(self.sampler, Some(&self.shared.alloc_cbs()));
        }
    }
}

/// The descriptor state of a textured pipeline: one layout, one pool
/// sized for exactly one set, and that set.
///
/// **A pool per pipeline rather than one shared across the crate.** With
/// the set written once at creation and never reallocated, the pool has
/// no churn to amortise, and a pool whose lifetime is exactly that of
/// its single set needs no free list, no fragmentation story and no
/// rule about who may allocate from it when. That reasoning stops
/// holding the moment sets are allocated per frame.
#[derive(Clone, Copy)]
struct Descriptors {
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
}

impl Descriptors {
    /// Destroy the pool and the layout. The set is freed with the pool
    /// and must not be freed separately.
    ///
    /// # Safety
    ///
    /// No submit referencing the set may still be running.
    unsafe fn destroy(self, shared: &DeviceShared) {
        // SAFETY: forwarded to the caller; both handles were created by
        // `create_descriptors` with these callbacks.
        unsafe {
            shared
                .device
                .destroy_descriptor_pool(self.pool, Some(&shared.alloc_cbs()));
            shared
                .device
                .destroy_descriptor_set_layout(self.set_layout, Some(&shared.alloc_cbs()));
        }
    }
}

/// A compiled draw pipeline. Holds its device alive; destroyed on
/// drop (after a best-effort quiesce — v0 accepts a wait-idle in cold
/// teardown paths for unconditional correctness).
pub struct RenderPipeline {
    pub(crate) shared: Rc<DeviceShared>,
    pub(crate) pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    pub(crate) format: TargetFormat,
    pub(crate) vertex_count: u32,
    /// Whether this pipeline reads a per-vertex stream — that is, whether
    /// it is a mesh pipeline. The frame contract asserts it matches
    /// whether the item names geometry, exactly as it does for depth.
    pub(crate) vertex_input: bool,
    /// Packed stride of the per-vertex stream; zero when there is none.
    /// The record path asserts a mesh's stride equals it, because a
    /// disagreement fetches past the end of the mesh's allocation.
    pub(crate) vertex_stride: u32,
    /// Whether this pipeline carries depth state — the targets assert
    /// it matches the pass it draws in.
    pub(crate) depth: bool,
    descriptors: Option<Descriptors>,
    /// Kept alive because the descriptor set points at them. Never read
    /// through — the set holds the handles the GPU uses, and these hold
    /// the right to keep those handles valid.
    _bound: Option<(Rc<Texture>, Rc<Sampler>)>,
}

impl Drop for RenderPipeline {
    fn drop(&mut self) {
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // Rc; wait-idle guarantees no submitted work still references
        // the pipeline or its descriptor set; handles were created with
        // these callbacks. The set is destroyed before the texture and
        // sampler it points at, which `_bound` releases after this
        // returns.
        unsafe {
            // Best-effort quiesce; failure is logged, never a panic (D5)
            // — the diag record is the only observable this path has.
            if let Err(code) = self.shared.device.device_wait_idle() {
                renew_diag::error!(
                    target: "renew-rhi",
                    "wait-idle at pipeline teardown failed: {code:?}"
                );
            }
            self.shared
                .device
                .destroy_pipeline(self.pipeline, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .destroy_pipeline_layout(self.layout, Some(&self.shared.alloc_cbs()));
            if let Some(descriptors) = self.descriptors {
                descriptors.destroy(&self.shared);
            }
        }
    }
}

impl RenderPipeline {
    /// Bind this pipeline's descriptor set, if it has one.
    ///
    /// **One implementation, called by both targets.** The two record
    /// paths are otherwise independent, and a bind duplicated across
    /// them is a correctness rule maintained in two places — the set
    /// index and bind point have to agree with the pipeline layout
    /// built here, not with whichever target is being read.
    ///
    /// # Safety
    ///
    /// `cmd` must be recording, and its command pool must belong to
    /// this pipeline's device.
    pub(crate) unsafe fn bind_descriptors(&self, cmd: vk::CommandBuffer) {
        let Some(descriptors) = self.descriptors else {
            return;
        };
        // SAFETY: forwarded to the caller for the command buffer; the
        // layout and set were created together by `create_pipeline` and
        // are live for as long as `self` is; set 0 is the only set the
        // layout declares.
        unsafe {
            self.shared.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[descriptors.set],
                &[],
            );
        }
    }
}

/// Create the layout, pool and set for one combined image sampler, and
/// write it to point at `texture` through `sampler`.
fn create_descriptors(
    shared: &DeviceShared,
    texture: &Texture,
    sampler: &Sampler,
) -> Result<Descriptors, PipelineError> {
    // Handles from another device would be written into this device's
    // set and used by it — undefined behaviour that no return value can
    // report, so it asserts, as the two targets already do for a
    // pipeline built on a foreign device.
    debug_assert!(
        core::ptr::eq(shared, Rc::as_ptr(texture.shared())),
        "texture and pipeline come from different devices"
    );
    debug_assert!(
        core::ptr::eq(shared, Rc::as_ptr(&sampler.shared)),
        "sampler and pipeline come from different devices"
    );
    let bindings = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
    let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    // SAFETY: category 2 (ash dispatch): device live via the spine; the
    // binding array is a local outliving the call. (The same argument
    // covers every dispatch call in this function.)
    let set_layout = unsafe {
        shared
            .device
            .create_descriptor_set_layout(&layout_info, Some(&shared.alloc_cbs()))
    }
    .map_err(|code| creation("vkCreateDescriptorSetLayout", code))?;

    let sizes = [vk::DescriptorPoolSize::default()
        .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)];
    let pool_info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(1)
        .pool_sizes(&sizes);
    // SAFETY: device live; the size array is a local.
    let pool = match unsafe {
        shared
            .device
            .create_descriptor_pool(&pool_info, Some(&shared.alloc_cbs()))
    } {
        Ok(pool) => pool,
        Err(code) => {
            // SAFETY: layout live, nothing retained it.
            unsafe {
                shared
                    .device
                    .destroy_descriptor_set_layout(set_layout, Some(&shared.alloc_cbs()));
            }
            return Err(creation("vkCreateDescriptorPool", code));
        }
    };

    let set_layouts = [set_layout];
    let alloc_info = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(pool)
        .set_layouts(&set_layouts);
    // SAFETY: pool and layout live; the layout array is a local.
    let allocated = unsafe { shared.device.allocate_descriptor_sets(&alloc_info) };
    // One layout in, one set out is the driver contract, and ash sizes
    // the result from the layout count — so the empty case is
    // unreachable rather than unlikely. Folded into the failure path to
    // keep it diagnosable instead of asserted, at no cost.
    let (set, code) = match allocated {
        Ok(sets) => (sets.into_iter().next(), vk::Result::ERROR_UNKNOWN),
        Err(code) => (None, code),
    };
    let Some(set) = set else {
        let partial = Descriptors {
            set_layout,
            pool,
            set: vk::DescriptorSet::null(),
        };
        // SAFETY: nothing was allocated from the pool, so no submit can
        // reference it.
        unsafe { partial.destroy(shared) };
        return Err(creation("vkAllocateDescriptorSets", code));
    };

    let image_info = [vk::DescriptorImageInfo::default()
        .sampler(sampler.sampler)
        .image_view(texture.view)
        // The upload left the image in this layout and nothing changes
        // it afterwards, immutability being the texture contract.
        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
    let writes = [vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .image_info(&image_info)];
    // SAFETY: the set is live and not in use by any submit — nothing can
    // have been recorded against a pipeline that does not exist yet. The
    // write array and the image info it points at are locals outliving
    // the call.
    unsafe { shared.device.update_descriptor_sets(&writes, &[]) };

    Ok(Descriptors {
        set_layout,
        pool,
        set,
    })
}

fn creation(call: &'static str, code: vk::Result) -> PipelineError {
    PipelineError::Creation {
        call,
        code: code.as_raw(),
    }
}

/// How many attributes one pipeline may declare across both streams.
///
/// A fixed ceiling rather than a `Vec`, so building a pipeline layout
/// allocates nothing and the arrays below are stack-sized. Sixteen is
/// four times what any consumer in this tree declares and comfortably
/// under `maxVertexInputAttributes`, whose guaranteed floor is sixteen —
/// so a layout this accepts is one every conformant adapter accepts.
pub(crate) const MAX_VERTEX_ATTRIBUTES: usize = 16;

/// The Vulkan vertex-input description a pair of attribute lists
/// produces, plus the per-vertex stride the record path checks a mesh
/// against.
pub(crate) struct VertexInputLayout {
    pub(crate) bindings: [vk::VertexInputBindingDescription; 2],
    pub(crate) binding_count: usize,
    pub(crate) attributes: [vk::VertexInputAttributeDescription; MAX_VERTEX_ATTRIBUTES],
    pub(crate) attribute_count: usize,
    /// Packed sum of the per-vertex attributes; zero when none are
    /// declared.
    pub(crate) vertex_stride: u32,
}

/// Derive bindings, attributes and the per-vertex stride from what a
/// pipeline declares.
///
/// **One location space across two bindings, per-vertex first.** Vulkan
/// numbers shader input locations globally, not per binding, so the two
/// lists cannot both start at zero. Per-vertex first is arbitrary but
/// fixed, and it is what keeps an instance-only pipeline's locations at
/// `0..n` — which is why every existing shader and its committed SPIR-V
/// are untouched by this becoming two streams.
///
/// # Panics
///
/// Declaring more than [`MAX_VERTEX_ATTRIBUTES`] across both lists is a
/// contract violation, asserted: the arrays are fixed-width, so the
/// alternative is a silently truncated layout whose draw reads the wrong
/// bytes.
pub(crate) fn vertex_input_layout(
    per_vertex: Option<&[VertexAttribute]>,
    per_instance: Option<&[VertexAttribute]>,
) -> VertexInputLayout {
    let vertex = per_vertex.unwrap_or(&[]);
    let instance = per_instance.unwrap_or(&[]);
    assert!(
        vertex.len() + instance.len() <= MAX_VERTEX_ATTRIBUTES,
        "a pipeline declares at most {MAX_VERTEX_ATTRIBUTES} vertex attributes across both \
         streams, got {} per-vertex and {} per-instance",
        vertex.len(),
        instance.len()
    );

    let mut layout = VertexInputLayout {
        bindings: [vk::VertexInputBindingDescription::default(); 2],
        binding_count: 0,
        attributes: [vk::VertexInputAttributeDescription::default(); MAX_VERTEX_ATTRIBUTES],
        attribute_count: 0,
        vertex_stride: 0,
    };
    let mut location = 0u32;
    for (attributes, binding, rate) in [
        (vertex, VERTEX_BINDING, vk::VertexInputRate::VERTEX),
        (instance, INSTANCE_BINDING, vk::VertexInputRate::INSTANCE),
    ] {
        if attributes.is_empty() {
            continue;
        }
        let mut stride = 0u32;
        for attribute in attributes {
            layout.attributes[layout.attribute_count] =
                vk::VertexInputAttributeDescription::default()
                    .location(location)
                    .binding(binding)
                    .format(attribute.format())
                    .offset(stride);
            layout.attribute_count += 1;
            location += 1;
            stride += attribute.byte_len();
        }
        layout.bindings[layout.binding_count] = vk::VertexInputBindingDescription::default()
            .binding(binding)
            .stride(stride)
            .input_rate(rate);
        layout.binding_count += 1;
        if binding == VERTEX_BINDING {
            layout.vertex_stride = stride;
        }
    }
    layout
}

impl Device {
    /// Create a sampler.
    ///
    /// # Errors
    ///
    /// [`PipelineError::Creation`] if the driver refuses the sampler.
    pub fn create_sampler(&self, desc: &SamplerDesc) -> Result<Sampler, PipelineError> {
        let shared = &self.shared;
        let address = desc.address.to_vk();
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(desc.filter.to_vk())
            .min_filter(desc.filter.to_vk())
            // NEAREST rather than LINEAR: with no mip levels created,
            // the mode only decides how a single level is selected, and
            // nearest keeps the choice from mattering at all.
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            // No anisotropy: it is a device feature that must be enabled
            // at device creation, and this crate enables none. Asking
            // for it here without that is a validation error.
            .anisotropy_enable(false)
            .unnormalized_coordinates(false);
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // Rc; the info struct is a local outliving this call; the crate
        // is structurally `!Send + !Sync`, so external synchronisation
        // holds.
        let sampler = unsafe {
            shared
                .device
                .create_sampler(&info, Some(&shared.alloc_cbs()))
        }
        .map_err(|code| creation("vkCreateSampler", code))?;
        Ok(Sampler {
            shared: Rc::clone(shared),
            sampler,
        })
    }

    /// Build a draw pipeline for a target of `desc.target_format`.
    ///
    /// # Errors
    ///
    /// [`PipelineError::InvalidSpirv`] when either byte slice fails the
    /// structural checks; [`PipelineError::Creation`] when the driver
    /// rejects a creation call.
    #[expect(
        clippy::too_many_lines,
        reason = "the fixed-function state block is one declaration list; splitting it scatters what belongs together"
    )]
    pub fn create_pipeline(
        &self,
        desc: &PipelineDesc<'_>,
    ) -> Result<RenderPipeline, PipelineError> {
        let shared = &self.shared;
        // Checked before anything is created so this failure path owns
        // nothing. The environment declined, not the caller — a Result,
        // never an assert.
        let depth_format = match desc.depth_state {
            Some(_) => match shared.depth_format {
                Some(format) => Some(format),
                None => {
                    return Err(PipelineError::DepthUnsupported {
                        chain: crate::vk::depth::CHAIN_NAMES,
                    });
                }
            },
            None => None,
        };
        let vs_words = crate::spirv::words_from_bytes("vertex", desc.vertex_spirv)?;
        let fs_words = crate::spirv::words_from_bytes("fragment", desc.fragment_spirv)?;

        // SAFETY: category 2 (ash dispatch): device live via the spine;
        // the word slices outlive the calls; callbacks' ledger outlives
        // the modules (destroyed below in every path).
        let vs = unsafe {
            shared.device.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&vs_words),
                Some(&shared.alloc_cbs()),
            )
        }
        .map_err(|code| creation("vkCreateShaderModule(vertex)", code))?;
        // SAFETY: as above.
        let fs = match unsafe {
            shared.device.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&fs_words),
                Some(&shared.alloc_cbs()),
            )
        } {
            Ok(module) => module,
            Err(code) => {
                // SAFETY: `vs` was just created, unused elsewhere.
                unsafe {
                    shared
                        .device
                        .destroy_shader_module(vs, Some(&shared.alloc_cbs()));
                }
                return Err(creation("vkCreateShaderModule(fragment)", code));
            }
        };
        // Both modules exist past this point; this frees them on every
        // exit (the pipeline retains what it needs — modules are only
        // creation-time inputs).
        let destroy_modules = || {
            // SAFETY: both modules live and unused after pipeline
            // creation resolves.
            unsafe {
                shared
                    .device
                    .destroy_shader_module(vs, Some(&shared.alloc_cbs()));
                shared
                    .device
                    .destroy_shader_module(fs, Some(&shared.alloc_cbs()));
            }
        };

        let descriptors = match &desc.texture {
            Some((texture, sampler)) => match create_descriptors(shared, texture, sampler) {
                Ok(descriptors) => Some(descriptors),
                Err(error) => {
                    destroy_modules();
                    return Err(error);
                }
            },
            None => None,
        };

        // An untextured pipeline keeps the empty layout it has always
        // had; a textured one declares the single set the crate defines.
        let set_layouts: &[vk::DescriptorSetLayout] = match &descriptors {
            Some(descriptors) => core::slice::from_ref(&descriptors.set_layout),
            None => &[],
        };
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(set_layouts);
        // SAFETY: category 2: device live; the layout array is a local
        // outliving the call.
        let layout = match unsafe {
            shared
                .device
                .create_pipeline_layout(&layout_info, Some(&shared.alloc_cbs()))
        } {
            Ok(layout) => layout,
            Err(code) => {
                destroy_modules();
                if let Some(descriptors) = descriptors {
                    // SAFETY: no pipeline exists, so no submit can
                    // reference the set.
                    unsafe { descriptors.destroy(shared) };
                }
                return Err(creation("vkCreatePipelineLayout", code));
            }
        };

        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vs)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs)
                .name(c"main"),
        ];
        // With neither stream declared this stays the empty state every
        // generative pipeline was built with. The layout itself is a
        // pure function, so its cross-stream location numbering — the
        // part that compiles and binds happily while reading the wrong
        // bytes — is unit-tested without a device.
        let vertex_layout = vertex_input_layout(desc.vertex_input, desc.instance_input);
        let mut vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        if vertex_layout.binding_count > 0 {
            vertex_input = vertex_input
                .vertex_binding_descriptions(&vertex_layout.bindings[..vertex_layout.binding_count])
                .vertex_attribute_descriptions(
                    &vertex_layout.attributes[..vertex_layout.attribute_count],
                );
        }
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_attachments = [match desc.blend {
            Blend::Opaque => vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA),
            Blend::PremultipliedAlpha => vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD),
        }];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = [desc.target_format.to_vk()];
        let mut rendering =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);
        if let Some(format) = depth_format {
            rendering = rendering.depth_attachment_format(format);
        }
        // Compare op fixed LESS_OR_EQUAL in v0; another op arrives as a
        // builder on `DepthState` when a consumer needs it.
        let depth_stencil = desc.depth_state.map(|state| {
            vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(state.test)
                .depth_write_enable(state.write)
                .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL)
        });

        let mut info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        if let Some(depth_stencil) = &depth_stencil {
            info = info.depth_stencil_state(depth_stencil);
        }

        // SAFETY: category 2: device live; every array the create info
        // references is a local outliving the call.
        let created = unsafe {
            shared.device.create_graphics_pipelines(
                vk::PipelineCache::null(),
                &[info],
                Some(&shared.alloc_cbs()),
            )
        };
        destroy_modules();
        // A rejected create info and an empty result set are the same
        // outcome here: no pipeline, and a layout nobody owns. One info
        // in, one pipeline out is the driver contract — and ash sizes
        // the result vector from the create-info count, so the empty
        // case is unreachable rather than merely unlikely; folding it
        // into the failure path keeps it diagnosable instead of
        // asserted, at no cost.
        let (built, code) = match created {
            Ok(pipelines) => (pipelines.into_iter().next(), vk::Result::ERROR_UNKNOWN),
            Err((_partial, code)) => (None, code),
        };
        let Some(pipeline) = built else {
            // SAFETY: layout live, no pipeline retained it; and no
            // pipeline exists to have referenced the descriptor set.
            unsafe {
                shared
                    .device
                    .destroy_pipeline_layout(layout, Some(&shared.alloc_cbs()));
                if let Some(descriptors) = descriptors {
                    descriptors.destroy(shared);
                }
            }
            return Err(creation("vkCreateGraphicsPipelines", code));
        };

        Ok(RenderPipeline {
            shared: Rc::clone(shared),
            pipeline,
            layout,
            format: desc.target_format,
            vertex_count: desc.vertex_count,
            vertex_input: desc.vertex_input.is_some(),
            vertex_stride: vertex_layout.vertex_stride,
            depth: desc.depth_state.is_some(),
            descriptors,
            _bound: desc.texture.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The converters, both arms of each, with no device involved.
    ///
    /// **These live here rather than in the device suite deliberately.**
    /// That suite demands `Validation::Required` and skips wherever the
    /// validation layer is absent, which is most environments. A
    /// conversion whose only exercise sits behind a skipped test is an
    /// untested conversion on every machine but the few that happen to
    /// have the layer installed.
    #[test]
    fn every_filter_and_address_mode_maps_to_its_vulkan_spelling() {
        assert_eq!(Filter::Nearest.to_vk(), vk::Filter::NEAREST);
        assert_eq!(Filter::Linear.to_vk(), vk::Filter::LINEAR);
        assert_eq!(
            AddressMode::ClampToEdge.to_vk(),
            vk::SamplerAddressMode::CLAMP_TO_EDGE
        );
        assert_eq!(AddressMode::Repeat.to_vk(), vk::SamplerAddressMode::REPEAT);
    }

    /// The atlas preset is a decision, so it is asserted rather than
    /// assumed: linear filtering would make a sampled golden's bytes
    /// depend on how an adapter interpolates, and the golden lane
    /// compares bytes.
    #[test]
    fn the_atlas_preset_is_nearest_and_clamped() {
        let desc = SamplerDesc::atlas();
        assert_eq!(desc.filter, Filter::Nearest);
        assert_eq!(desc.address, AddressMode::ClampToEdge);
    }

    /// Every attribute's size and Vulkan spelling, all three arms. Here
    /// rather than in the device suite for the reason the filter test
    /// above states: that suite skips wherever the validation layer is
    /// absent, which is most machines.
    #[test]
    fn every_vertex_attribute_maps_to_its_vulkan_spelling() {
        assert_eq!(VertexAttribute::Vec2.byte_len(), 8);
        assert_eq!(VertexAttribute::Vec3.byte_len(), 12);
        assert_eq!(VertexAttribute::Vec4.byte_len(), 16);
        assert_eq!(VertexAttribute::Vec2.format(), vk::Format::R32G32_SFLOAT);
        assert_eq!(VertexAttribute::Vec3.format(), vk::Format::R32G32B32_SFLOAT);
        assert_eq!(
            VertexAttribute::Vec4.format(),
            vk::Format::R32G32B32A32_SFLOAT
        );
    }

    /// **The two streams share one location space, and this is where
    /// getting it wrong would be invisible.** A layout with repeated or
    /// gapped locations builds a pipeline the driver accepts and that
    /// reads the wrong bytes; no image oracle would name the cause. So
    /// the numbering is pinned directly, for all four declaration shapes.
    #[test]
    fn the_two_streams_share_one_location_space() {
        let vertex = [VertexAttribute::Vec3, VertexAttribute::Vec4];
        let instance = [VertexAttribute::Vec2, VertexAttribute::Vec4];

        let neither = vertex_input_layout(None, None);
        assert_eq!(neither.binding_count, 0, "no streams, no bindings");
        assert_eq!(neither.attribute_count, 0);
        assert_eq!(neither.vertex_stride, 0);

        // Instance-only is what every pipeline in the tree declared
        // before meshes existed: locations must still start at zero, or
        // the committed SPIR-V beside them would stop matching.
        let only_instance = vertex_input_layout(None, Some(&instance));
        assert_eq!(only_instance.binding_count, 1);
        assert_eq!(only_instance.bindings[0].binding, INSTANCE_BINDING);
        assert_eq!(only_instance.bindings[0].stride, 24);
        assert_eq!(
            only_instance.bindings[0].input_rate,
            vk::VertexInputRate::INSTANCE
        );
        assert_eq!(only_instance.attributes[0].location, 0);
        assert_eq!(only_instance.attributes[1].location, 1);
        assert_eq!(only_instance.attributes[1].offset, 8);
        assert_eq!(
            only_instance.vertex_stride, 0,
            "no per-vertex stream means no stride for a mesh to match"
        );

        let only_vertex = vertex_input_layout(Some(&vertex), None);
        assert_eq!(only_vertex.binding_count, 1);
        assert_eq!(only_vertex.bindings[0].binding, VERTEX_BINDING);
        assert_eq!(
            only_vertex.bindings[0].input_rate,
            vk::VertexInputRate::VERTEX
        );
        assert_eq!(only_vertex.vertex_stride, 28, "vec3 + vec4");
        assert_eq!(only_vertex.attributes[1].offset, 12);

        let both = vertex_input_layout(Some(&vertex), Some(&instance));
        assert_eq!(both.binding_count, 2);
        assert_eq!(both.attribute_count, 4);
        assert_eq!(both.vertex_stride, 28);
        // Per-vertex first, per-instance continuing after it: one
        // ascending run with no repeat and no gap.
        let locations: Vec<u32> = both.attributes[..4].iter().map(|a| a.location).collect();
        assert_eq!(locations, vec![0, 1, 2, 3], "one location space");
        let bindings: Vec<u32> = both.attributes[..4].iter().map(|a| a.binding).collect();
        assert_eq!(
            bindings,
            vec![
                VERTEX_BINDING,
                VERTEX_BINDING,
                INSTANCE_BINDING,
                INSTANCE_BINDING
            ],
            "each attribute reads from its own stream's binding"
        );
        // Offsets restart per stream: they are within a binding, not
        // across the pair.
        let offsets: Vec<u32> = both.attributes[..4].iter().map(|a| a.offset).collect();
        assert_eq!(offsets, vec![0, 12, 0, 8]);
    }

    /// The fixed-width arrays make over-declaring a truncation rather
    /// than a reallocation, so it is refused by name instead.
    #[test]
    fn more_attributes_than_the_arrays_hold_are_refused() {
        let many = [VertexAttribute::Vec2; MAX_VERTEX_ATTRIBUTES];
        assert!(
            std::panic::catch_unwind(|| vertex_input_layout(Some(&many), Some(&many))).is_err(),
            "twice the ceiling across both streams must refuse, not truncate"
        );
        assert!(
            std::panic::catch_unwind(|| vertex_input_layout(Some(&many), None)).is_ok(),
            "exactly the ceiling is legal"
        );
    }

    /// The two constructors are decisions, so they are asserted: the
    /// common case tests and writes, the read-only case tests without
    /// leaving a footprint.
    #[test]
    fn the_depth_state_constructors_mean_what_they_say() {
        let both = DepthState::read_write();
        assert!(both.test && both.write);
        let read = DepthState::test_only();
        assert!(read.test && !read.write);
    }
}
