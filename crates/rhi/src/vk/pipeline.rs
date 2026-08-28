//! The v0 graphics pipeline: SPIR-V stages, dynamic rendering into one
//! color attachment — or none, for the depth-only shape — optionally
//! sampled-binding slots filled per item, and optionally vertex input:
//! per-vertex at binding 0, per-instance at binding 1.
//!
//! **Three pipeline shapes.** A generative pipeline's stages write their
//! own vertex list, so the count belongs to the shader and travels with
//! it in [`Shaders`]. A mesh pipeline reads a per-vertex stream, so the
//! count belongs to the geometry and arrives at the draw — which is why
//! its stages are a [`MeshShaders`] carrying no count at all rather than
//! a `Shaders` carrying one that nothing reads. A depth-only pipeline is
//! a mesh pipeline with no fragment stage and no color attachment,
//! drawing geometry into a depth-kinded render image and nothing else.

use std::fmt;
use std::rc::Rc;

use ash::vk;

use crate::error::PipelineError;
use crate::vk::binding::MAX_SAMPLED_BINDINGS;
use crate::vk::buffer::Buffer;
use crate::vk::device::{Device, DeviceShared};
use crate::vk::pass::Bindings;

/// What a pipeline renders into: a color format that must match its
/// target's, or the color-free depth-only shape. Surface-pass matches
/// are dev-build asserts beside each target; image-pass matches and
/// depth-only placement are retained contract refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetFormat {
    /// Every color render image's format: the bytes a shader wrote,
    /// stored as it wrote them.
    Rgba8Unorm,
    /// The same channel order desktop surfaces usually offer, storing
    /// exactly what was written.
    Bgra8Unorm,
    /// The offscreen target's format, and the surface's wherever one is
    /// offered: the hardware applies the sRGB transfer function on write,
    /// so a shader hands over linear light and the attachment stores the
    /// display-encoded byte.
    ///
    /// **This is what makes the working space linear.** Blending,
    /// interpolation and lighting then happen on linear values, which is
    /// where those operations mean what their arithmetic says, and the
    /// encode becomes a property of the storage rather than a step
    /// somebody has to remember to write.
    Rgba8Srgb,
    /// The same, in the channel order desktop surfaces usually offer.
    Bgra8Srgb,
    /// No color attachment at all: the format of a pipeline that draws
    /// only into a depth-kinded render image. A depth-only pipeline
    /// carries no fragment stage and must declare depth state — both
    /// asserted at creation.
    DepthOnly,
}

impl TargetFormat {
    /// The color attachment format, `None` for the depth-only shape
    /// whose color attachment list is empty.
    pub(crate) fn to_vk(self) -> Option<vk::Format> {
        match self {
            Self::Rgba8Unorm => Some(vk::Format::R8G8B8A8_UNORM),
            Self::Bgra8Unorm => Some(vk::Format::B8G8R8A8_UNORM),
            Self::Rgba8Srgb => Some(vk::Format::R8G8B8A8_SRGB),
            Self::Bgra8Srgb => Some(vk::Format::B8G8R8A8_SRGB),
            Self::DepthOnly => None,
        }
    }

    /// The byte this attachment stores for one float color channel, or
    /// `None` for a format with no color attachment at all.
    ///
    /// **This exists so an expectation can be a function of the format
    /// rather than a literal beside it.** A test that writes `[51, 102,
    /// 153, 255]` is asserting two things at once — the value it cleared
    /// to, and the rule the hardware used to store it — and only the
    /// first is the test's subject. When the storage rule changes, every
    /// such literal is wrong, and it is wrong *before* any golden
    /// bootstrap path runs: the corner assertions panic first, so no
    /// candidate image is written and a refresh that had already deleted
    /// its goldens has nothing to replace them with. Deriving the byte
    /// from the format makes that class of failure impossible to reach.
    ///
    /// A UNORM attachment stores `round(255 x channel)`, which is what
    /// makes an authored byte survive a round trip unchanged. An sRGB
    /// attachment applies the transfer function instead, so a channel of
    /// linear light lands on the byte a display expects. Callers ask the
    /// format rather than choosing, which is why flipping one constant in
    /// a test moves every expectation that test owns.
    ///
    /// Values outside `0..=1` clamp, which is what the hardware does.
    #[must_use]
    pub fn stores(self, channel: f32) -> Option<u8> {
        match self {
            Self::Rgba8Unorm | Self::Bgra8Unorm => {
                // NaN clamps to zero rather than propagating: `f32::clamp`
                // panics on a NaN bound but returns NaN for a NaN input,
                // and `as u8` would then saturate to zero silently. Saying
                // so here costs one comparison and removes the surprise.
                let channel = if channel.is_nan() { 0.0 } else { channel };
                let scaled = channel.clamp(0.0, 1.0) * 255.0;
                // `round` then truncate: the cast alone truncates, which
                // would store 50 for a channel authored as 51/255.
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped to 0..=255 and rounded on the line above"
                )]
                Some(scaled.round() as u8)
            }
            Self::Rgba8Srgb | Self::Bgra8Srgb => Some(crate::srgb::encode_u8(channel)),
            Self::DepthOnly => None,
        }
    }

    /// Whether the hardware applies the transfer function on write.
    ///
    /// Exposed so a caller holding an *authored* byte — a colour eyeballed
    /// as `#336699` rather than measured as light — can ask whether to
    /// decode it before handing it over. Asking is the supported way to
    /// find out; matching on the arms is not, because a format added later
    /// would take the wrong branch of a `matches!` without a word.
    #[must_use]
    pub fn encodes(self) -> bool {
        matches!(self, Self::Rgba8Srgb | Self::Bgra8Srgb)
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
    /// One 32-bit unsigned integer, delivered to the shader as a `uint`.
    ///
    /// **A correctness fix rather than a saving, and the distinction is
    /// worth stating because the workaround looks fine.** Without an
    /// integer format the only way to carry an integer field per vertex
    /// is to store it in a float attribute and recover it with
    /// `floatBitsToUint`. For a small integer that bit pattern has a zero
    /// exponent field — it is a denormal — and both the Vulkan spec and
    /// real drivers permit flushing denormals to zero in vertex input
    /// conversion. A software rasteriser very likely passes it and a
    /// discrete part very likely does not, which is precisely the class
    /// of defect a golden on one software lane cannot see.
    ///
    /// The other workaround, storing the integer as an exactly
    /// representable float *value* below 2^24 and reading it with
    /// `uint(attr)`, is sound but caps a packed record at 24 exact bits —
    /// so four 8-bit fields do not fit, and any such packing has to be
    /// 4x6 bits or split across components.
    Uint32,
    /// Two 32-bit unsigned integers, as a `uvec2`.
    Uint32x2,
    /// Four bytes, each normalised to `0.0..=1.0` and delivered as a
    /// `vec4`.
    ///
    /// The compact way to carry a colour, a normal or four small
    /// fractions per vertex: sixteen bytes of `Vec4` for four values that
    /// never needed more than eight bits each.
    Unorm8x4,
}

impl VertexAttribute {
    pub(crate) fn byte_len(self) -> u32 {
        match self {
            Self::Vec2 | Self::Uint32x2 => 8,
            Self::Vec3 => 12,
            Self::Vec4 => 16,
            Self::Uint32 | Self::Unorm8x4 => 4,
        }
    }

    pub(crate) fn format(self) -> vk::Format {
        match self {
            Self::Vec2 => vk::Format::R32G32_SFLOAT,
            Self::Vec3 => vk::Format::R32G32B32_SFLOAT,
            Self::Vec4 => vk::Format::R32G32B32A32_SFLOAT,
            Self::Uint32 => vk::Format::R32_UINT,
            Self::Uint32x2 => vk::Format::R32G32_UINT,
            Self::Unorm8x4 => vk::Format::R8G8B8A8_UNORM,
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
/// rather than as a struct literal. Every field it has grown — vertex
/// input, blend state, depth state, and the push-constant range — is
/// optional with a defined absence, and each arrived as a builder
/// method touching no existing caller, which is the attribute paying
/// for itself. Without it, adding one field edits every construction
/// site in the workspace, and the count only rises.
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
    /// Which side of a triangle this pipeline draws. See [`Facing`].
    pub facing: Facing,
    /// How many sampled-binding slots this pipeline's shaders read;
    /// zero for none.
    ///
    /// Slot `i` is descriptor set `i`, binding 0, one combined image
    /// sampler each — the crate's sampled set layout, repeated. What
    /// fills the slots is not named here: an item names its
    /// [`Binding`](crate::Binding)s per draw, and the frame contract
    /// refuses a count mismatch by name before any GPU call. That is
    /// what lets N textures share one pipeline.
    ///
    /// At most [`MAX_SAMPLED_BINDINGS`](crate::MAX_SAMPLED_BINDINGS),
    /// asserted at creation — and one fewer than that when the pipeline
    /// also declares a [`Self::uniform_block`], because the block spends
    /// one of the same guaranteed sets.
    pub sampled_bindings: u32,
    /// Depth testing, or `None` for the depth-free pipelines every
    /// depthless pass records. A pipeline carrying this can only draw
    /// inside a pass that carries a depth attachment, and the reverse —
    /// the targets assert the match by name.
    pub depth_state: Option<DepthState>,
    /// Vertex-stage push-constant range, in bytes; zero for none.
    ///
    /// The per-draw constant channel: bytes recorded into the command
    /// stream per item, never a buffer, never a retention slot. At most
    /// [`MAX_PUSH_CONSTANT_BYTES`] and a multiple of four — anything
    /// else is a contract violation, asserted at creation. An item
    /// drawn through a declaring pipeline must carry exactly this many
    /// bytes; presence and length are both frame-contract refusals.
    pub push_constant_size: u32,
    /// Uniform block size in bytes; zero for none.
    ///
    /// **The push range's larger sibling.** A push range is capped at
    /// [`MAX_PUSH_CONSTANT_BYTES`] because that is what the device
    /// guarantees; anything bigger — two matrices and a light, a light
    /// list, a material table — has to travel in memory instead. A
    /// declaring pipeline reads it as a dynamic uniform buffer at
    /// **set [`Self::sampled_bindings`]**, after every sampled slot, so
    /// a shader that samples nothing finds its block at set zero.
    ///
    /// At most [`MAX_UNIFORM_BLOCK_BYTES`] and a multiple of sixteen —
    /// std140 rounds a block up to a vec4 boundary, so a size that is
    /// not is one the shader does not agree with. An item drawn through
    /// a declaring pipeline must carry exactly this many bytes of
    /// uniform data; presence and length are both frame-contract
    /// refusals, exactly as for push data.
    pub uniform_block: u32,
}

/// The push-constant ceiling: Vulkan's guaranteed minimum
/// `maxPushConstantsSize`, so a range this accepts is one every
/// conformant adapter accepts — the same reasoning that sizes
/// [`MAX_VERTEX_ATTRIBUTES`]. Larger device limits exist and are
/// deliberately not queried: a range sized to one machine's limit is a
/// pipeline that fails to build on another's.
pub const MAX_PUSH_CONSTANT_BYTES: u32 = 128;

/// Refuse a push-constant declaration outside what the spec guarantees.
/// A pure function so both rules are unit-tested without a device.
///
/// # Panics
///
/// Over [`MAX_PUSH_CONSTANT_BYTES`], or not a multiple of four (the
/// spec's granularity) — caller mistakes, asserted rather than
/// returned: the environment did not decline, the declaration was
/// never valid anywhere.
pub(crate) fn validate_push_constant_size(size: u32) {
    assert!(
        size <= MAX_PUSH_CONSTANT_BYTES,
        "a push-constant range is at most {MAX_PUSH_CONSTANT_BYTES} bytes (the guaranteed \
         device minimum), got {size}"
    );
    assert!(
        size.is_multiple_of(4),
        "a push-constant range is a multiple of four bytes (the spec's granularity), got {size}"
    );
}

/// The largest uniform block a pipeline may declare.
///
/// `maxUniformBufferRange`'s guaranteed floor is 16384 bytes, so a
/// declaration this accepts is one every conformant adapter accepts —
/// the same reasoning [`MAX_SAMPLED_BINDINGS`] uses for bound sets and
/// [`MAX_PUSH_CONSTANT_BYTES`] for the push range. It is 128 times the
/// push ceiling, which is the whole point of the channel.
pub const MAX_UNIFORM_BLOCK_BYTES: u32 = 16_384;

/// Refuse a uniform-block declaration outside what the spec guarantees,
/// or one std140 cannot describe.
///
/// A pure function so the rule is unit-tested without a device, like its
/// two siblings.
///
/// # Panics
///
/// Over [`MAX_UNIFORM_BLOCK_BYTES`], or not a multiple of sixteen. Both
/// are caller mistakes asserted rather than returned: the ceiling was
/// never valid anywhere, and a block whose size is not a vec4 multiple
/// is one the shader's own layout rules disagree with — which draws a
/// wrong picture instead of raising anything.
pub(crate) fn validate_uniform_block(bytes: u32) {
    assert!(
        bytes <= MAX_UNIFORM_BLOCK_BYTES,
        "a pipeline declares at most {MAX_UNIFORM_BLOCK_BYTES} bytes of uniform block \
         (the guaranteed device minimum for a uniform buffer range), got {bytes}"
    );
    assert!(
        bytes.is_multiple_of(16),
        "a uniform block is a multiple of sixteen bytes, because std140 rounds one up \
         to a vec4 boundary and a shader would read the rounded size, got {bytes}"
    );
}

/// Refuse a sampled-slot declaration outside what the spec guarantees.
/// A pure function so the rule is unit-tested without a device, like
/// its push-constant sibling above.
///
/// # Panics
///
/// Over [`MAX_SAMPLED_BINDINGS`] slots — a caller mistake, asserted
/// rather than returned: the ceiling is the guaranteed device minimum
/// for bound sets, so the declaration was never valid anywhere.
pub(crate) fn validate_sampled_bindings(count: u32) {
    let declared = count as usize;
    assert!(
        declared <= MAX_SAMPLED_BINDINGS,
        "a pipeline declares at most {MAX_SAMPLED_BINDINGS} sampled bindings (the guaranteed          device minimum for bound sets), got {declared}"
    );
}

/// Which side of a triangle a pipeline draws.
///
/// **Every pipeline drew both, and for closed geometry that is half the
/// rasterisation thrown away.** A triangle whose back is turned to the
/// camera is either inside a solid, where nothing can see it, or behind
/// one, where the depth test discards it *after* it has been rasterised,
/// shaded and tested at full price. Dropping it before the rasteriser is
/// what `cull_mode` is for, and it costs nothing: the hardware decides
/// from the sign of the triangle's screen-space area.
///
/// **Per pipeline rather than per device, and that is the whole design.**
/// Closed solids want their backs dropped. Foliage, water surfaces,
/// banners, decals and anything else a viewer is meant to see from
/// behind want both sides — and culling those makes geometry vanish from
/// one direction only, which is the hardest kind of rendering fault to
/// notice because half the time it looks right.
///
/// **Front is counter-clockwise in clip space** — the winding a
/// right-handed mesher gives the outside of a solid. So `Front` keeps
/// what you can see and drops what is inside the rock, which is the only
/// reading of the name that is any use.
///
/// Getting that took a deliberate choice, and it is worth knowing about
/// before writing a mesher against this. Vulkan applies its front-face
/// rule in **framebuffer** space, where Y points down, so a
/// counter-clockwise clip-space triangle arrives at the rasteriser wound
/// clockwise. Declaring the intuitive `COUNTER_CLOCKWISE` therefore
/// culls precisely the geometry a caller can see. The rasteriser state
/// declares `CLOCKWISE` to undo that, once, here — rather than leaving
/// every caller of this crate to discover the flip by watching their
/// world turn inside out.
///
/// An input enum, so `#[non_exhaustive]`: a fourth mode later must not be
/// a breaking change for downstream matchers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Facing {
    /// Draw both sides. Today's behavior, and the default — because it
    /// is the answer that is never *wrong*, only sometimes wasteful, and
    /// a default that can make geometry disappear is not a default.
    Both,
    /// Draw only triangles wound counter-clockwise on screen: the
    /// outside of a closed solid.
    Front,
    /// Draw only the ones wound clockwise: the inside of a volume, a
    /// skybox seen from within, a cutaway.
    Back,
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
    /// `src + dst`, color and alpha alike — light on light, for
    /// sources that accumulate rather than occlude: glows, sparks, the
    /// particle shapes that read as energy.
    ///
    /// **Order-independent by arithmetic**: saturating addition is
    /// commutative, so draw order cannot change the picture — which is
    /// what lets unsorted batches of additive geometry stay
    /// byte-stable under any submission order, and what makes this the
    /// recommended mode where sorting has not been paid for. The
    /// premultiplied convention still applies to the source bytes; the
    /// difference from [`Self::PremultipliedAlpha`] is only the
    /// destination factor.
    Additive,
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
/// **A fragment-carrying pair that *does* read a per-vertex buffer
/// takes [`MeshShaders`] instead**, because its premise is the opposite
/// one: the count belongs to the geometry, so there is none here to
/// bundle. (The depth-only shape reads a per-vertex stream too, and
/// bundles nothing — one stage has no pair to carry.)
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
            facing: Facing::Both,
            sampled_bindings: 0,
            uniform_block: 0,
            vertex_input: None,
            instance_input: None,
            depth_state: None,
            push_constant_size: 0,
        }
    }

    /// Draw only one side of each triangle. See [`Facing`].
    #[must_use]
    pub const fn facing(mut self, facing: Facing) -> Self {
        self.facing = facing;
        self
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
            facing: Facing::Both,
            sampled_bindings: 0,
            uniform_block: 0,
            vertex_input: Some(layout),
            instance_input: None,
            depth_state: None,
            push_constant_size: 0,
        }
    }

    /// A depth-only mesh pipeline: one vertex stage over a per-vertex
    /// stream, no fragment stage, no color attachment — the shape that
    /// renders geometry into a depth-kinded render image and nothing
    /// else.
    ///
    /// **No fragment shader parameter, and that is the point**: with no
    /// color attachment there is nothing for one to write, and
    /// rasterization without a fragment stage still tests and writes
    /// depth — the whole draw. Depth state must still be declared (a
    /// builder call), because a depth-only pipeline that neither tests
    /// nor writes depth does nothing at all; creation asserts that,
    /// and that this shape carries no fragment bytes. (The reverse —
    /// empty fragment bytes on a color pipeline — stays the
    /// recoverable `InvalidSpirv` refusal it has always been.)
    ///
    /// The vertex stage may write outputs no stage consumes — reusing a
    /// full mesh pair's vertex stage is expected, not clever.
    #[must_use]
    pub fn depth_mesh(vertex_spirv: &'a [u8], layout: &'a [VertexAttribute]) -> Self {
        Self {
            vertex_spirv,
            // Structurally absent rather than optional: the depth-only
            // format is what licenses the emptiness, and creation
            // asserts the pairing in both directions.
            fragment_spirv: &[],
            target_format: TargetFormat::DepthOnly,
            vertex_count: 0,
            blend: Blend::Opaque,
            facing: Facing::Both,
            sampled_bindings: 0,
            uniform_block: 0,
            vertex_input: Some(layout),
            instance_input: None,
            depth_state: None,
            push_constant_size: 0,
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

    /// Declare `count` sampled-binding slots: descriptor sets
    /// `0..count`, one combined image sampler at binding 0 each.
    ///
    /// The shaders must declare matching combined image samplers at
    /// those set indices; a declared slot the shader never reads is not
    /// an error, merely a set nothing samples. Every item drawn through
    /// this pipeline must then name exactly `count`
    /// [`Binding`](crate::Binding)s — or `count + 1` when the pipeline
    /// also declares a [`Self::uniform_block`], the extra one last.
    /// Presence, count and class are frame-contract refusals, the same way
    /// push data matches its range.
    ///
    /// At most [`MAX_SAMPLED_BINDINGS`](crate::MAX_SAMPLED_BINDINGS),
    /// refused at creation rather than quietly clamped.
    #[must_use]
    pub fn sampled_bindings(mut self, count: u32) -> Self {
        self.sampled_bindings = count;
        self
    }

    /// Declare a uniform block of `bytes` bytes, read at set
    /// [`Self::sampled_bindings`] — the per-draw channel for data too
    /// large for a push range.
    ///
    /// The shader declares a matching `layout(std140) uniform` block at
    /// that set, binding zero. Every item drawn through this pipeline
    /// must then name one more [`Binding`](crate::Binding) than it has
    /// sampled slots — the extra one built with
    /// [`BindingDesc::uniform`](crate::BindingDesc::uniform) — and carry
    /// exactly `bytes` bytes of
    /// [`uniform_data`](crate::Item::uniform_data).
    ///
    /// **Visible to the vertex *and* fragment stages**, unlike the push
    /// range beside it, which is vertex-only. A fragment shader can read a
    /// block and cannot read a push constant, which is often the deciding
    /// difference between the two channels.
    ///
    /// At most [`MAX_UNIFORM_BLOCK_BYTES`] and a multiple of sixteen;
    /// anything else is refused at creation rather than quietly clamped.
    /// Sampled slots and the block share the guaranteed four bound sets,
    /// so a pipeline declaring the full complement of sampled slots cannot
    /// also declare a block.
    #[must_use]
    pub fn uniform_block(mut self, bytes: u32) -> Self {
        self.uniform_block = bytes;
        self
    }

    /// Test (and per `depth`, write) the pass's depth attachment. The
    /// compare op is `GREATER_OR_EQUAL`: depth is reversed, nearer is
    /// larger, and the far plane is zero.
    #[must_use]
    pub fn depth_state(mut self, depth: DepthState) -> Self {
        self.depth_state = Some(depth);
        self
    }

    /// Declare a vertex-stage push-constant range of `bytes` bytes —
    /// the per-draw constant channel, for values that change every
    /// draw and belong to no instance stream.
    ///
    /// At most [`MAX_PUSH_CONSTANT_BYTES`] and a multiple of four;
    /// anything else is refused at creation rather than quietly
    /// clamped. Every item drawn through this pipeline must then carry
    /// exactly `bytes` bytes of push data — the frame contract refuses
    /// a missing, surplus, or mis-sized push by name.
    #[must_use]
    pub fn push_constant_size(mut self, bytes: u32) -> Self {
        self.push_constant_size = bytes;
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
/// `GREATER_OR_EQUAL` — **depth is reversed**, the engine's single
/// convention: nearer is larger, the far plane is zero, depth clears to
/// zero. Reversed rather than conventional because a perspective
/// mapping is hyperbolic either way, and the conventional direction
/// lands most of a scene at depths near 1.0 — where f32 spacing is at
/// its coarsest — starving the far field of distinct values until
/// distant surfaces z-fight. Reversed, the hyperbola's dense end and
/// the float format's dense end coincide, and a consumer's regression
/// test measures the difference in representable steps rather than
/// asserting it. (The fade-fog defect the depth range caused earlier
/// was a different cost — hyperbolic compression fools a depth-driven
/// fade under either convention, which is why fades read clip `w`.)
/// A compare-op builder arrives when a consumer genuinely needs
/// another op — a knob with no consumer is a knob that exists to be
/// wrong.
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

/// The sampler's owning half, split like the texture's and for the
/// same reason: a binding holds the inner alive while callers pass
/// borrows. The contract lives on [`Sampler`], where its reader is.
pub(crate) struct SamplerInner {
    pub(crate) shared: Rc<DeviceShared>,
    pub(crate) sampler: vk::Sampler,
}

/// A sampler. Holds its device alive; destroyed on drop.
///
/// # Contract
///
/// A sampler must outlive every descriptor set that references it.
///
/// **The caller cannot violate that, because they do not hold the
/// obligation.** [`Device::create_binding`](crate::Device::create_binding)
/// takes shared ownership of the inner half, and the
/// [`Binding`](crate::Binding) keeps its clone for as long as the
/// descriptor set exists — so a caller who drops their own handle the
/// moment the binding is built takes nothing away from the set. `Drop`
/// therefore needs no quiesce: it can only run once every binding has
/// released its clone, which each one's own `Drop` proves happens after
/// the last submit that read the set.
pub struct Sampler {
    pub(crate) inner: Rc<SamplerInner>,
}

impl fmt::Debug for Sampler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sampler").finish_non_exhaustive()
    }
}

impl Drop for SamplerInner {
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
    /// Declared vertex-stage push-constant range in bytes; zero for
    /// none. The frame contract asserts every item's push data matches
    /// it exactly, the same way it matches depth state to the pass.
    pub(crate) push_constant_size: u32,
    /// Declared sampled-binding slot count; zero for none. The frame
    /// contract asserts every item names exactly this many bindings,
    /// the same way push data matches its declared range.
    pub(crate) sampled_bindings: u32,
    /// Declared uniform block size in bytes; zero for none. The frame
    /// contract asserts every item's uniform data matches it exactly,
    /// and that the item names a uniform binding exactly when this is
    /// non-zero.
    pub(crate) uniform_block: u32,
}

impl Drop for RenderPipeline {
    fn drop(&mut self) {
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // Rc; wait-idle guarantees no submitted work still references
        // the pipeline; handles were created with these callbacks.
        unsafe {
            // Best-effort quiesce; failure is logged, never a panic
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
        }
    }
}

impl RenderPipeline {
    /// Bind an item's named bindings as descriptor sets `0..count`,
    /// with `slot`'s dynamic offset for any uniform block among them.
    ///
    /// **One implementation, called by both targets.** The two record
    /// paths are otherwise independent, and a bind duplicated across
    /// them is a correctness rule maintained in two places — the set
    /// indices and bind point have to agree with the pipeline layout
    /// built here, not with whichever target is being read.
    ///
    /// `slot` is the frame slot the caller is recording into: the
    /// offscreen target is synchronous and always passes zero, and the
    /// presentation target passes the frame it is about to record. A
    /// uniform binding's descriptor covers one slot's worth of its
    /// buffer starting at zero, so the offset is what selects the region
    /// this frame's bytes were copied into — the same arithmetic the
    /// copy phase used, kept here rather than in either target so the
    /// two cannot drift.
    ///
    /// # Safety
    ///
    /// `cmd` must be recording, and its command pool must belong to
    /// this pipeline's device. The frame contract proved the binding
    /// count equals this pipeline's declared slot count, and the
    /// target's retention table holds every named binding alive past
    /// the submit.
    pub(crate) unsafe fn bind_bindings(
        &self,
        cmd: vk::CommandBuffer,
        bindings: &Bindings<'_>,
        slot: usize,
    ) {
        let mut sets = [vk::DescriptorSet::null(); MAX_SAMPLED_BINDINGS];
        // One entry per DYNAMIC descriptor, in set order — which here
        // means at most one, and only when the pipeline declares a
        // block. An array of the same width so the frame path allocates
        // nothing.
        let mut offsets = [0u32; MAX_SAMPLED_BINDINGS];
        let mut count = 0usize;
        let mut dynamic = 0usize;
        for binding in bindings.iter() {
            sets[count] = binding.inner.set;
            if let Some(buffer) = binding.inner.uniform_buffer() {
                // **Asserted, not saturated.** An out-of-range dynamic
                // offset is refused only where the validation layer is
                // on; with it off — which is every shipping consumer —
                // it is undefined behaviour, so a saturating cast trades
                // a diagnosable panic for a silent wrong read. The bound
                // is real: the stride is a rounded per-frame capacity and
                // the slot count is `MAX_FRAME_SLOTS`, so the product is
                // far inside u32 for any block this crate accepts.
                let at = buffer.slot_stride * slot as u64;
                let narrowed = u32::try_from(at);
                // Retained, because it bounds a value handed straight to
                // the driver — the rule this crate states wherever a
                // length or an offset crosses that boundary. Asserted
                // rather than saturated: an out-of-range dynamic offset is
                // refused only where the validation layer is on, and with
                // it off it is undefined behaviour, so saturating would
                // trade a diagnosable stop for a silent wrong read. The
                // fallback below is unreachable past this line.
                assert!(
                    narrowed.is_ok(),
                    "a uniform block's slot offset ({at}) is past what a dynamic offset can                      address; the buffer's per-frame capacity is too large for the ring"
                );
                offsets[dynamic] = narrowed.unwrap_or_default();
                dynamic += 1;
            }
            count += 1;
        }
        // No empty-list arm: the record paths call this only for an
        // item carrying bindings, and the contract proved that list's
        // length equals the pipeline's declared, non-zero slot count.
        // SAFETY: forwarded to the caller for the command buffer; the
        // layout is live for as long as `self` is; each set is live via
        // the binding the item names, retained by the target; slot `i`
        // is set `i`, exactly the layout list `create_pipeline` built;
        // the slice is non-empty by the contract argument above; the
        // offset array has exactly one entry per dynamic descriptor in
        // the bound sets, in set order, which is what the call requires.
        unsafe {
            self.shared.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &sets[..count],
                &offsets[..dynamic],
            );
        }
    }

    /// Record an item's push-constant bytes into `cmd`.
    ///
    /// **One implementation, called by both targets**, for the reason
    /// [`Self::bind_bindings`] states: the stage flags and offset
    /// must agree with the one range the layout built here declares,
    /// not with whichever target happens to be recording.
    ///
    /// # Safety
    ///
    /// `cmd` must be recording, and its command pool must belong to
    /// this pipeline's device. `bytes` must be exactly the declared
    /// range — the frame contract refused anything else before
    /// recording began.
    pub(crate) unsafe fn push_frame_constants(&self, cmd: vk::CommandBuffer, bytes: &[u8]) {
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // Rc; the command buffer is forwarded to the caller; the layout
        // is live for as long as `self` is; the vertex stage flag and
        // zero offset match the one range `create_pipeline` declares,
        // and the contract proved `bytes` is its exact length.
        unsafe {
            self.shared.device.cmd_push_constants(
                cmd,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytes,
            );
        }
    }
}

pub(crate) fn creation(call: &'static str, code: vk::Result) -> PipelineError {
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
            inner: Rc::new(SamplerInner {
                shared: Rc::clone(shared),
                sampler,
            }),
        })
    }

    /// Build a draw pipeline for a target of `desc.target_format`.
    ///
    /// # Errors
    ///
    /// [`PipelineError::InvalidSpirv`] when either byte slice fails the
    /// structural checks; [`PipelineError::Creation`] when the driver
    /// rejects a creation call.
    ///
    /// # Panics
    ///
    /// In a dev build, on a malformed declaration — a push-constant
    /// range outside the guaranteed ceiling or granularity; more sampled
    /// slots than
    /// [`MAX_SAMPLED_BINDINGS`](crate::MAX_SAMPLED_BINDINGS); a uniform
    /// block past [`MAX_UNIFORM_BLOCK_BYTES`] or not a multiple of
    /// sixteen; or sampled slots and a block together spending more than
    /// the guaranteed bound sets. All are contract violations valid on no
    /// adapter, asserted before anything is created so the panic owns
    /// nothing.
    #[expect(
        clippy::too_many_lines,
        reason = "the fixed-function state block is one declaration list; splitting it scatters what belongs together"
    )]
    pub fn create_pipeline(
        &self,
        desc: &PipelineDesc<'_>,
    ) -> Result<RenderPipeline, PipelineError> {
        let shared = &self.shared;
        // Contract, not environment: the ceiling is the spec's
        // guaranteed minimum, so a size this refuses was never valid on
        // any adapter — asserted, like the vertex-attribute ceiling,
        // and before anything is created so the panic owns nothing.
        validate_push_constant_size(desc.push_constant_size);
        // The same reasoning as the push-constant ceiling, checked by
        // the same pure-validator shape — before anything is created,
        // so the panic owns nothing.
        validate_sampled_bindings(desc.sampled_bindings);
        validate_uniform_block(desc.uniform_block);
        let sampled_slots = desc.sampled_bindings as usize;
        let uniform_slots = usize::from(desc.uniform_block > 0);
        // Bound sets are one budget, not two: the guaranteed floor for
        // `maxBoundDescriptorSets` is what `MAX_SAMPLED_BINDINGS` names,
        // and a block spends one of them.
        assert!(
            sampled_slots + uniform_slots <= MAX_SAMPLED_BINDINGS,
            "a pipeline binds at most {MAX_SAMPLED_BINDINGS} descriptor sets (the guaranteed \
             device minimum), and a uniform block spends one: {sampled_slots} sampled plus \
             {uniform_slots} block"
        );
        // The depth-only pairing: the format is what licenses the
        // missing fragment stage, and a depth-only pipeline without
        // depth state does nothing at all. One direction only — an
        // empty fragment slice on a color pipeline stays the
        // recoverable InvalidSpirv refusal it has always been.
        let depth_only = desc.target_format == TargetFormat::DepthOnly;
        assert!(
            !depth_only || desc.fragment_spirv.is_empty(),
            "a depth-only pipeline carries no fragment stage — its color-attachment list \
             is empty, so there is nothing for one to write"
        );
        assert!(
            !depth_only || desc.depth_state.is_some(),
            "a depth-only pipeline must declare depth state: with no color attachment and \
             no depth test or write, the draw does nothing"
        );
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
        let fs_words = if depth_only {
            None
        } else {
            Some(crate::spirv::words_from_bytes(
                "fragment",
                desc.fragment_spirv,
            )?)
        };

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
        let fs = match &fs_words {
            None => None,
            // SAFETY: as above.
            Some(fs_words) => match unsafe {
                shared.device.create_shader_module(
                    &vk::ShaderModuleCreateInfo::default().code(fs_words),
                    Some(&shared.alloc_cbs()),
                )
            } {
                Ok(module) => Some(module),
                Err(code) => {
                    // SAFETY: `vs` was just created, unused elsewhere.
                    unsafe {
                        shared
                            .device
                            .destroy_shader_module(vs, Some(&shared.alloc_cbs()));
                    }
                    return Err(creation("vkCreateShaderModule(fragment)", code));
                }
            },
        };
        // Every created module exists past this point; this frees them
        // on every exit (the pipeline retains what it needs — modules
        // are only creation-time inputs).
        let destroy_modules = || {
            // SAFETY: modules live and unused after pipeline creation
            // resolves.
            unsafe {
                shared
                    .device
                    .destroy_shader_module(vs, Some(&shared.alloc_cbs()));
                if let Some(fs) = fs {
                    shared
                        .device
                        .destroy_shader_module(fs, Some(&shared.alloc_cbs()));
                }
            }
        };

        // A pipeline with no sets keeps the empty layout it has always
        // had; a declaring one lists the crate's canonical set layouts,
        // sampled slots first and the uniform block last — set `i` is
        // slot `i`, which is the agreement `bind_bindings` records
        // against and the order a shader's set indices must follow.
        //
        // **Sampled first so no existing shader moves.** Every set index
        // written in GLSL in this tree today is a sampler's, and putting
        // the block after them leaves all of them where they are.
        let mut slot_layouts = [shared.sampled_set_layout; MAX_SAMPLED_BINDINGS];
        if uniform_slots > 0 {
            slot_layouts[sampled_slots] = shared.uniform_set_layout;
        }
        let set_layouts: &[vk::DescriptorSetLayout] =
            &slot_layouts[..sampled_slots + uniform_slots];
        // One vertex-stage range at offset zero, present exactly when
        // the descriptor declares a size — the record path pushes with
        // the same stage flags and offset, one definition apart.
        let push_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(desc.push_constant_size)];
        let mut layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(set_layouts);
        if desc.push_constant_size > 0 {
            layout_info = layout_info.push_constant_ranges(&push_ranges);
        }
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
                return Err(creation("vkCreatePipelineLayout", code));
            }
        };

        let mut stages = [vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vs)
            .name(c"main"); 2];
        let mut stage_count = 1usize;
        if let Some(fs) = fs {
            stages[1] = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs)
                .name(c"main");
            stage_count = 2;
        }
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
            .cull_mode(match desc.facing {
                Facing::Both => vk::CullModeFlags::NONE,
                // Cull the BACK to draw the front, and the front to draw
                // the back: the flag names what is *discarded*, and
                // reading it as what is kept is the mistake this match
                // exists to keep away from callers.
                Facing::Front => vk::CullModeFlags::BACK,
                Facing::Back => vk::CullModeFlags::FRONT,
            })
            // **Clockwise, and that is not a typo.** Vulkan applies
            // this in *framebuffer* space, where Y points down — so a
            // triangle written counter-clockwise in clip space, which
            // is how every right-handed mesher winds the outside of a
            // solid, arrives at the rasteriser wound clockwise.
            // Declaring COUNTER_CLOCKWISE here would make
            // `Facing::Front` cull exactly the geometry a caller can
            // see and keep the geometry inside the rock.
            //
            // It had no effect at all until `Facing` existed, because
            // the cull mode was always NONE. So this is a free choice
            // made once, here, rather than a Y-flip exported to every
            // caller of this crate for ever.
            //
            // `a_pipeline_draws_the_side_of_a_triangle_it_was_asked_for`
            // is what holds it: it renders one triangle both ways round
            // under all three modes, and the mirror pair is what would
            // catch this being flipped back.
            .front_face(vk::FrontFace::CLOCKWISE)
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
            // The one difference from the arm above is the destination
            // factor: the target keeps everything it has, and the
            // source adds to it.
            Blend::Additive => vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(vk::BlendFactor::ONE)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                .alpha_blend_op(vk::BlendOp::ADD),
        }];
        // No color attachment, no blend entry: the blend list's length
        // must equal the attachment count.
        let blend_count = usize::from(!depth_only);
        let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(&blend_attachments[..blend_count]);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        // A depth-only pipeline's color list is empty — the same shape
        // its passes render with. Everything else declares its one
        // color format.
        let mut formats = [vk::Format::UNDEFINED; 1];
        let mut format_count = 0usize;
        if let Some(format) = desc.target_format.to_vk() {
            formats[0] = format;
            format_count = 1;
        }
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&formats[..format_count]);
        if let Some(format) = depth_format {
            rendering = rendering.depth_attachment_format(format);
        }
        // Reversed depth: nearer is larger, so the compare keeps the
        // larger value. Fixed rather than configurable; another op
        // arrives as a builder on `DepthState` when a consumer needs it.
        let depth_stencil = desc.depth_state.map(|state| {
            vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(state.test)
                .depth_write_enable(state.write)
                .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL)
        });

        let mut info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages[..stage_count])
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
            // SAFETY: layout live, no pipeline retained it.
            unsafe {
                shared
                    .device
                    .destroy_pipeline_layout(layout, Some(&shared.alloc_cbs()));
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
            push_constant_size: desc.push_constant_size,
            sampled_bindings: desc.sampled_bindings,
            uniform_block: desc.uniform_block,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::TargetFormat;

    /// Every format answers both questions the type promises.
    ///
    /// **One test over every arm rather than one per arm**, because what
    /// is being pinned is that the set is total: a format added later gets
    /// a Vulkan format and an honest answer about the transfer function,
    /// or this fails. `encodes` exists precisely so a caller does not
    /// `matches!` on the arms itself — a promise worth a test, since it is
    /// public API that nothing in this tree calls.
    #[test]
    fn every_target_format_maps_and_says_whether_it_encodes() {
        let all = [
            (TargetFormat::Rgba8Unorm, false),
            (TargetFormat::Bgra8Unorm, false),
            (TargetFormat::Rgba8Srgb, true),
            (TargetFormat::Bgra8Srgb, true),
        ];
        for (format, encodes) in all {
            assert!(format.to_vk().is_some(), "{format:?} has no Vulkan format");
            assert_eq!(
                format.encodes(),
                encodes,
                "{format:?} disagreed about the transfer function"
            );
        }
    }

    use super::*;

    /// The store rule, at the values an expectation is built from.
    ///
    /// The load-bearing case is the round trip: a channel authored as
    /// `b / 255` must store back as exactly `b`. Every golden clear in the
    /// tree is written that way, so if this rounds down instead of to
    /// nearest, `51 / 255` stores as 50 and each of those tests fails by
    /// one — which is the shape of failure that reads as a driver
    /// difference and is not one.
    #[test]
    fn a_unorm_target_stores_an_authored_byte_unchanged() {
        for byte in 0..=255u8 {
            let channel = f32::from(byte) / 255.0;
            assert_eq!(
                TargetFormat::Rgba8Unorm.stores(channel),
                Some(byte),
                "channel {channel} came from byte {byte}"
            );
        }
    }

    /// Both colour formats store the same way; only their channel order
    /// differs, and that is the caller's business rather than this rule's.
    #[test]
    fn both_colour_formats_agree_on_the_rule() {
        for byte in [0u8, 1, 51, 128, 254, 255] {
            let channel = f32::from(byte) / 255.0;
            assert_eq!(
                TargetFormat::Rgba8Unorm.stores(channel),
                TargetFormat::Bgra8Unorm.stores(channel)
            );
        }
    }

    /// A depth-only target has no colour attachment, so there is no byte
    /// to name — reported rather than invented.
    #[test]
    fn a_depth_only_target_stores_no_colour() {
        assert_eq!(TargetFormat::DepthOnly.stores(0.5), None);
    }

    /// Out of range clamps the way the hardware does, and a NaN lands on
    /// zero rather than propagating into a byte by way of a saturating
    /// cast nobody wrote down.
    #[test]
    fn out_of_range_clamps_and_a_nan_is_zero() {
        assert_eq!(TargetFormat::Rgba8Unorm.stores(-1.0), Some(0));
        assert_eq!(TargetFormat::Rgba8Unorm.stores(2.0), Some(255));
        assert_eq!(TargetFormat::Rgba8Unorm.stores(f32::NAN), Some(0));
        assert_eq!(TargetFormat::Rgba8Unorm.stores(f32::INFINITY), Some(255));
        assert_eq!(TargetFormat::Rgba8Unorm.stores(f32::NEG_INFINITY), Some(0));
    }

    /// Rounding is to nearest, not toward zero: the midpoint between two
    /// stored bytes must land on the upper one.
    #[test]
    fn the_rule_rounds_to_nearest_rather_than_truncating() {
        assert_eq!(TargetFormat::Rgba8Unorm.stores(0.5 / 255.0), Some(1));
        assert_eq!(TargetFormat::Rgba8Unorm.stores(0.49 / 255.0), Some(0));
    }

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

    /// Every attribute's size and Vulkan spelling, every arm. Here rather
    /// than in the device suite for the reason the filter test above
    /// states: that suite skips wherever the validation layer is absent,
    /// which is most machines.
    ///
    /// **Exhaustive by construction.** The expectation is a `match`, so a
    /// seventh format is a compile error in this test before it can be a
    /// wrong stride in a vertex buffer — which is a silent failure that
    /// reads the wrong bytes and draws a plausible wrong picture.
    #[test]
    fn every_vertex_attribute_maps_to_its_vulkan_spelling() {
        const EVERY: [VertexAttribute; 6] = [
            VertexAttribute::Vec2,
            VertexAttribute::Vec3,
            VertexAttribute::Vec4,
            VertexAttribute::Uint32,
            VertexAttribute::Uint32x2,
            VertexAttribute::Unorm8x4,
        ];
        fn expected(attribute: VertexAttribute) -> (u32, vk::Format) {
            match attribute {
                VertexAttribute::Vec2 => (8, vk::Format::R32G32_SFLOAT),
                VertexAttribute::Vec3 => (12, vk::Format::R32G32B32_SFLOAT),
                VertexAttribute::Vec4 => (16, vk::Format::R32G32B32A32_SFLOAT),
                VertexAttribute::Uint32 => (4, vk::Format::R32_UINT),
                VertexAttribute::Uint32x2 => (8, vk::Format::R32G32_UINT),
                VertexAttribute::Unorm8x4 => (4, vk::Format::R8G8B8A8_UNORM),
            }
        }
        for attribute in EVERY {
            let (bytes, format) = expected(attribute);
            assert_eq!(attribute.byte_len(), bytes, "{attribute:?} byte length");
            assert_eq!(attribute.format(), format, "{attribute:?} Vulkan format");
        }
    }

    /// **No two attributes share a Vulkan format**, which the table above
    /// cannot catch on its own: it restates the implementation, so a
    /// copy-paste that gave `Uint32x2` the single-component spelling
    /// would be wrong in both places and agree with itself.
    ///
    /// Injectivity is the property that does not restate anything. Two
    /// formats colliding means one attribute reads the other's component
    /// count, which is a stride the pipeline accepts and a buffer the
    /// shader misreads.
    #[test]
    fn no_two_vertex_attributes_share_a_vulkan_format() {
        const EVERY: [VertexAttribute; 6] = [
            VertexAttribute::Vec2,
            VertexAttribute::Vec3,
            VertexAttribute::Vec4,
            VertexAttribute::Uint32,
            VertexAttribute::Uint32x2,
            VertexAttribute::Unorm8x4,
        ];
        for (index, one) in EVERY.iter().enumerate() {
            for other in EVERY.iter().skip(index + 1) {
                assert_ne!(
                    one.format(),
                    other.format(),
                    "{one:?} and {other:?} claim the same Vulkan format"
                );
            }
        }
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

    /// The mesh stage pair carries exactly the two blobs and nothing
    /// else.
    ///
    /// **Here because the builtin is a const struct literal**, so
    /// `MeshShaders::new` — the constructor a caller with its own shaders
    /// uses — is reached by nothing else in the tree. An untested public
    /// constructor is the coverage gate's business as much as the
    /// reader's.
    #[test]
    fn the_mesh_stage_pair_carries_its_two_blobs() {
        let vertex = [1u8, 2, 3, 4];
        let fragment = [5u8, 6, 7, 8];
        let shaders = MeshShaders::new(&vertex, &fragment);
        assert_eq!(shaders.vertex, &vertex);
        assert_eq!(shaders.fragment, &fragment);
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

    /// Both push-constant rules, both sides of each, with no device
    /// involved — the same reasoning that keeps the layout numbering
    /// tests here rather than behind a device skip.
    #[test]
    fn push_constant_sizes_outside_the_spec_floor_are_refused() {
        // Legal: absent, the smallest real range, and exactly the
        // ceiling — the guaranteed minimum every adapter honours.
        validate_push_constant_size(0);
        validate_push_constant_size(4);
        validate_push_constant_size(MAX_PUSH_CONSTANT_BYTES);
        // Over the ceiling: valid on some adapters, which is exactly
        // why it is refused — a pipeline that builds on one machine
        // and fails on another is the portability bug this floor
        // exists to prevent.
        assert!(
            std::panic::catch_unwind(|| validate_push_constant_size(MAX_PUSH_CONSTANT_BYTES + 4))
                .is_err(),
            "over the guaranteed minimum must refuse"
        );
        // Off-granularity: the spec requires four-byte multiples.
        assert!(
            std::panic::catch_unwind(|| validate_push_constant_size(66)).is_err(),
            "a non-multiple-of-four range must refuse"
        );
    }

    /// The sampled-slot ceiling, both sides, with no device involved —
    /// the push-constant test's reasoning, one rule over.
    /// **The block validator's two refusals, neither of which any device
    /// test reaches.** Every `uniform_block` call in the tree passes the
    /// same legal 192, and the coverage gate cannot see the gap: the happy
    /// path runs on every pipeline creation, so both assertion lines count
    /// as covered while neither failing branch is ever taken. Its sibling
    /// below is unit-tested for exactly this reason, and this one's own
    /// docstring claimed to be. It was not.
    ///
    /// Probed by widening each bound in turn — raising the ceiling, and
    /// dropping the multiple-of-sixteen rule: the matching case stops
    /// panicking and this fails.
    #[test]
    fn uniform_block_sizes_outside_the_spec_floor_are_refused() {
        validate_uniform_block(0);
        validate_uniform_block(16);
        validate_uniform_block(MAX_UNIFORM_BLOCK_BYTES);
        assert!(
            std::panic::catch_unwind(|| validate_uniform_block(MAX_UNIFORM_BLOCK_BYTES + 16))
                .is_err(),
            "a block past the guaranteed uniform range was accepted"
        );
        // Not a multiple of sixteen: std140 rounds a block up to a vec4
        // boundary, so the shader would read a size the declaration does
        // not name. Two independent rules, so a legal-sized non-multiple
        // and a small non-multiple are both refused.
        for rounded in [24, 4, MAX_UNIFORM_BLOCK_BYTES - 8] {
            assert!(
                std::panic::catch_unwind(move || validate_uniform_block(rounded)).is_err(),
                "a {rounded}-byte block, which std140 would round, was accepted"
            );
        }
    }

    #[test]
    fn sampled_slot_counts_outside_the_spec_floor_are_refused() {
        // Legal: absent, one slot, and exactly the ceiling — the
        // guaranteed minimum every adapter honours. The ceiling is
        // four; the literal cast is a lint formality, not a risk.
        #[allow(clippy::cast_possible_truncation)]
        const CEILING: u32 = MAX_SAMPLED_BINDINGS as u32;
        validate_sampled_bindings(0);
        validate_sampled_bindings(1);
        validate_sampled_bindings(CEILING);
        // Over the ceiling: valid on some adapters, which is exactly
        // why it is refused — the portability bug the floor prevents.
        assert!(
            std::panic::catch_unwind(|| validate_sampled_bindings(CEILING + 1)).is_err(),
            "over the guaranteed minimum must refuse"
        );
    }

    /// The builder writes the field it names; the descriptor starts
    /// with no range.
    #[test]
    fn the_push_constant_builder_declares_the_range() {
        let desc = PipelineDesc::new(
            Shaders::new(&[1, 2, 3, 4], &[5, 6, 7, 8], 3),
            TargetFormat::Rgba8Unorm,
        );
        assert_eq!(desc.push_constant_size, 0, "no range unless declared");
        let desc = desc.push_constant_size(64);
        assert_eq!(desc.push_constant_size, 64);
    }
}
