//! The engine's only doorway to the GPU: device bring-up, render
//! targets, and the v0 draw path — multi-pass frames composed by the
//! caller, instanced draws, sampled textures, target-owned depth, and
//! render-to-texture through kinded render images — over Vulkan.
//!
//! # Contract
//!
//! - **The GPU API never leaks.** No Vulkan (or windowing) type appears
//!   in any public signature; consumers see only this crate's
//!   vocabulary. The one shared vocabulary with the platform's window
//!   is the standard window-handle traits, and even those stay inside
//!   the platform's opaque `NativeWindow`.
//! - **Single-threaded by contract, in the type system.** [`Device`]
//!   and everything holding one is `!Send + !Sync` by construction:
//!   Vulkan's external-synchronization rules are unrepresentable to
//!   violate. Lifting this is a future, deliberate change.
//!
//!   The two examples below are the contract, executed. They must fail
//!   to compile, and the error code is pinned so they cannot pass
//!   vacuously on a typo:
//!
//!   ```compile_fail,E0277
//!   fn needs_send<T: Send>() {}
//!   needs_send::<renew_rhi::Device>();
//!   ```
//!
//!   ```compile_fail,E0277
//!   fn needs_sync<T: Sync>() {}
//!   needs_sync::<renew_rhi::Device>();
//!   ```
//!
//!   **What this does and does not catch.** The spine is asserted, not
//!   every resource, because the resources are `!Send` for one reason —
//!   each holds an `Rc<DeviceShared>` — and a change that made them
//!   shareable would have to make that `Rc` shareable first, which these
//!   two catch. What they do not catch is a hand-written `unsafe impl
//!   Send` on one resource; that is governed by the crate's `unsafe`
//!   policy instead, which requires a written safety argument per site.
//! - **Errors are the environment's; assertions are the caller's.** A
//!   missing Vulkan runtime, a lost device, an out-of-date swapchain —
//!   recoverable results. Mixing objects across devices or handing a
//!   wrong-sized readback buffer — contract violations, asserted.
//! - **Host allocations by the driver are instrumented** through the
//!   allocation callbacks into a per-device ledger, readable via
//!   [`Device::host_allocation_stats`] — diagnostics, never control
//!   flow, and deliberately separate from the engine's own allocation
//!   accounting.
//! - **Validation is evidence.** Tests bring devices up with
//!   [`Validation::Required`]; validation messages are tallied and
//!   surfaced via [`Device::validation_report`], and the test suites
//!   fail on any error. Rendering without pixels on screen needs no
//!   window: the offscreen target exists precisely so correctness is
//!   provable headless.
//!
//! `unsafe` is confined to the `vk` backend module tree (every safe
//! module denies it) under a six-category discipline: loader entry,
//! dispatch calls, surface creation, the allocation callbacks, the
//! debug-messenger callback, and the mapped-memory read.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]
// The Vulkan backend cannot be expressed in safe Rust; the exception is
// scoped to this crate (territorially to src/vk/) and every site
// carries SAFETY.
#![allow(unsafe_code)]

mod config;
mod error;
mod spirv;
pub mod srgb;
mod vk;

pub use config::{
    AdapterInfo, AdapterKind, Color, DeviceDesc, Extent, SurfaceTransform, Validation,
};
pub use error::{DeviceError, PipelineError, TargetError};
pub use vk::binding::{Binding, BindingDesc, BindingSource, MAX_SAMPLED_BINDINGS};
pub use vk::buffer::{Buffer, BufferUsage};
pub use vk::device::{Device, HostAllocationStats, ValidationReport};
pub use vk::mesh::{Mesh, MeshDesc};
pub use vk::offscreen::OffscreenTarget;
pub use vk::pass::{
    Attachment, Bindings, ClearValue, Item, ItemList, LoadOp, MAX_FRAME_RENDER_IMAGES, Pass,
    PassTarget, RenderDesc, StoreOp, color_attachment,
};
pub use vk::pipeline::{
    AddressMode, Blend, DepthState, Filter, FrameData, MAX_PUSH_CONSTANT_BYTES,
    MAX_UNIFORM_BLOCK_BYTES, MeshShaders, PipelineDesc, RenderPipeline, Sampler, SamplerDesc,
    Shaders, TargetFormat, VertexAttribute,
};
pub use vk::render_image::{RenderImage, RenderImageDesc, RenderImageKind};
#[cfg(feature = "present")]
pub use vk::swapchain::{PresentOutcome, WindowTarget};
pub use vk::texture::{Texture, TextureContent, TextureDesc};

/// The embedded v0 shaders. Most of them draw from `gl_VertexIndex` and
/// write their own vertex list, so each is bundled with the vertex count
/// its stage generates; [`MESH`] is the exception, reading a per-vertex
/// stream and taking its count from the geometry instead. Compiled
/// offline by the pinned toolchain (the record lives beside the sources);
/// removed when the asset pipeline owns shader delivery.
pub mod builtin {
    use crate::Shaders;

    /// Vertex stage SPIR-V.
    pub static TRIANGLE_VS_SPV: &[u8] = include_bytes!("../shaders/triangle.vert.spv");
    /// Fragment stage SPIR-V.
    pub static TRIANGLE_FS_SPV: &[u8] = include_bytes!("../shaders/triangle.frag.spv");

    /// What the camera mesh path fades toward with distance.
    ///
    /// **Declared here because the shader beside it is the authority.**
    /// `mesh_camera.frag` mixes toward this colour, and a caller that
    /// clears to a different one gets a fade that reads as haze sitting
    /// in front of the backdrop rather than as depth. It was written out
    /// by hand in three other places, coupled to the shader by a comment
    /// asking whoever changed one to change the rest.
    ///
    /// Linear, not sRGB — the same space the shader mixes in and the
    /// same one [`Color`](crate::Color) carries. The test beside this
    /// module reads the shader source and fails if the two disagree.
    pub const HORIZON: [f32; 3] = [0.008_568_126, 0.010_329_823, 0.015_208_514];

    /// Vertex stage SPIR-V for the plain mesh path with a texture.
    pub static MESH_TEXTURED_VS_SPV: &[u8] = include_bytes!("../shaders/mesh_textured.vert.spv");
    /// Fragment stage SPIR-V sampling set 0, binding 0 and tinting by the
    /// interpolated vertex colour.
    pub static MESH_TEXTURED_FS_SPV: &[u8] = include_bytes!("../shaders/mesh_textured.frag.spv");

    /// The plain mesh pair with a texture: **clip-space** positions,
    /// colours and texture coordinates per vertex, and a combined image
    /// sampler at set 0, binding 0.
    ///
    /// No matrix and no distance fade: positions here are already clip
    /// space, so there is no view distance to fade by. The per-vertex
    /// layout is [`MESH_LAYOUT`], shared with every other mesh pair,
    /// which is what lets one scene feed any of them.
    pub const MESH_TEXTURED: crate::MeshShaders<'static> = crate::MeshShaders {
        vertex: MESH_TEXTURED_VS_SPV,
        fragment: MESH_TEXTURED_FS_SPV,
    };

    /// Vertex stage SPIR-V for the camera mesh path with a texture.
    pub static MESH_CAMERA_TEXTURED_VS_SPV: &[u8] =
        include_bytes!("../shaders/mesh_camera_textured.vert.spv");
    /// Fragment stage SPIR-V sampling set 0, binding 0 and tinting by the
    /// interpolated vertex colour.
    pub static MESH_CAMERA_TEXTURED_FS_SPV: &[u8] =
        include_bytes!("../shaders/mesh_camera_textured.frag.spv");

    /// The camera mesh pair with a texture: **world-space** positions,
    /// colours and texture coordinates per vertex, the matrix as a
    /// 64-byte push-constant block, and a combined image sampler at
    /// set 0, binding 0.
    ///
    /// **A second pair rather than a flag on the first.** The two
    /// pipelines differ in what they bind, not only in what they compute:
    /// a pipeline over this pair declares a sampled slot and one over
    /// [`MESH_CAMERA`] does not. A uniform choosing between them would
    /// cost a fetch and a branch per fragment for a decision fixed when
    /// the pipeline was built.
    ///
    /// The per-vertex layout is [`MESH_LAYOUT`], shared with
    /// [`MESH_CAMERA`], which is what lets one scene feed either; the
    /// push block is [`MESH_CAMERA`]'s, sixty-four bytes declared the
    /// same way.
    pub const MESH_CAMERA_TEXTURED: crate::MeshShaders<'static> = crate::MeshShaders {
        vertex: MESH_CAMERA_TEXTURED_VS_SPV,
        fragment: MESH_CAMERA_TEXTURED_FS_SPV,
    };

    /// Fragment stage SPIR-V for the camera mesh path with a texture
    /// whose clear texels are thrown away rather than drawn.
    pub static MESH_CAMERA_CUTOUT_FS_SPV: &[u8] =
        include_bytes!("../shaders/mesh_camera_cutout.frag.spv");

    /// The camera mesh pair with a **cutout** texture: as
    /// [`MESH_CAMERA_TEXTURED`] in every respect a caller can see —
    /// same vertex stage, same layout, same push block, same binding —
    /// except that a fragment whose alpha falls below half is discarded
    /// instead of drawn.
    ///
    /// **What this is for.** A texture with holes in it draws as a solid
    /// rectangle on the textured pair, because that pipeline replaces the
    /// target wherever a fragment lands and writes depth while it does:
    /// the hole is opaque *and* it hides what stands behind it. Foliage,
    /// grates, fences, decals and sprites standing in a world are all
    /// that shape.
    ///
    /// **Why discarding rather than blending.** Blending fixes the colour
    /// and not the depth — a see-through fragment that still writes depth
    /// occludes whatever is drawn after it, and correcting that means
    /// sorting every draw back to front, a cost paid by every consumer
    /// for the sake of textures that are usually binary anyway. A discard
    /// needs no sorting and no ordering contract, which is what makes
    /// this the pair to reach for first and [`Blend::PremultipliedAlpha`]
    /// the one to reach for when a surface is genuinely half-there.
    ///
    /// The threshold is on the texel's alpha times the vertex colour's,
    /// so a caller can fade a whole draw to nothing rather than having it
    /// stay solid until it vanishes.
    pub const MESH_CAMERA_CUTOUT: crate::MeshShaders<'static> = crate::MeshShaders {
        vertex: MESH_CAMERA_TEXTURED_VS_SPV,
        fragment: MESH_CAMERA_CUTOUT_FS_SPV,
    };

    /// Vertex stage SPIR-V for the shadowed camera mesh path: the
    /// shared 128-byte push block in, light-space position out.
    pub static MESH_CAMERA_SHADOW_VS_SPV: &[u8] =
        include_bytes!("../shaders/mesh_camera_shadow.vert.spv");
    /// Fragment stage SPIR-V sampling the atlas at set 0 and the
    /// shadow map at set 1, dimming where the light recorded nearer.
    pub static MESH_CAMERA_SHADOW_FS_SPV: &[u8] =
        include_bytes!("../shaders/mesh_camera_shadow.frag.spv");

    /// The shadowed camera mesh pair: [`MESH_CAMERA_TEXTURED`] plus a
    /// shadow term AND a scene light. World-space positions, colours and
    /// coordinates per vertex; one 128-byte push block, exactly
    /// [`MAX_PUSH_CONSTANT_BYTES`], holding `mat4 view_projection`, the
    /// light's `vec4 light_row_0/1/2`, and `vec4 light`; the atlas at
    /// sampled slot 0 and a depth-kinded render image — the shadow map a
    /// depth-only pass rendered this frame — at slot 1. Fade constants
    /// identical to the textured pair's: two pipelines drawing one world
    /// must fade alike or the seam shows.
    ///
    /// **Three rows, not four columns, and that is what makes the light
    /// fit.** An orthographic projection over a rigid view is affine, so
    /// the light's fourth row is exactly `(0, 0, 0, 1)` and is not sent;
    /// both stages write a literal one. `mat4x3` would not do: std430
    /// pads each of its four three-component columns back to sixteen
    /// bytes, so the block would be 144 again and would not fit.
    ///
    /// The CASTER that fills the shadow map is
    /// [`MESH_CAMERA_SHADOW_CASTER_VS_SPV`], reading **this same block**
    /// and using only its light rows. One record for both halves means
    /// the map cannot be written with a light it is not sampled with.
    ///
    /// [`MAX_PUSH_CONSTANT_BYTES`]: crate::MAX_PUSH_CONSTANT_BYTES
    pub const MESH_CAMERA_SHADOW: crate::MeshShaders<'static> = crate::MeshShaders {
        vertex: MESH_CAMERA_SHADOW_VS_SPV,
        fragment: MESH_CAMERA_SHADOW_FS_SPV,
    };

    /// Vertex stage SPIR-V for the shadow CASTER: the world as the light
    /// sees it, depth only, read from the same push block the lit stage
    /// reads.
    ///
    /// **A stage of its own, where the caster used to reuse the ordinary
    /// camera vertex stage.** Sharing one block with the lit half is what
    /// stops the map being written with one light and sampled with
    /// another, and it drops a colour and a fade that a depth-only pass
    /// throws away.
    pub static MESH_CAMERA_SHADOW_CASTER_VS_SPV: &[u8] =
        include_bytes!("../shaders/mesh_camera_shadow_caster.vert.spv");

    /// The bytes both shadowed pipelines declare: a camera matrix, the
    /// light's three rows, and a scene light. Exactly
    /// [`MAX_PUSH_CONSTANT_BYTES`](crate::MAX_PUSH_CONSTANT_BYTES).
    pub const MESH_CAMERA_SHADOW_PUSH_BYTES: u32 = 128;

    /// Vertex stage SPIR-V for a full-target textured quad.
    pub static TEXTURED_VS_SPV: &[u8] = include_bytes!("../shaders/textured.vert.spv");
    /// Fragment stage SPIR-V sampling set 0, binding 0.
    pub static TEXTURED_FS_SPV: &[u8] = include_bytes!("../shaders/textured.frag.spv");

    /// Vertex stage SPIR-V for the instanced quad.
    pub static INSTANCED_VS_SPV: &[u8] = include_bytes!("../shaders/instanced.vert.spv");
    /// Fragment stage SPIR-V passing the instance colour through.
    pub static INSTANCED_FS_SPV: &[u8] = include_bytes!("../shaders/instanced.frag.spv");

    /// Vertex stage SPIR-V for the instanced quad with per-instance
    /// depth.
    pub static INSTANCED_DEPTH_VS_SPV: &[u8] =
        include_bytes!("../shaders/instanced_depth.vert.spv");
    /// Fragment stage SPIR-V passing the instance colour through.
    pub static INSTANCED_DEPTH_FS_SPV: &[u8] =
        include_bytes!("../shaders/instanced_depth.frag.spv");

    /// The instanced quad: six expanded vertices per instance, placement
    /// and colour from the one vertex buffer at instance rate. The
    /// matching layout is [`INSTANCED_LAYOUT`]; shader and slice describe
    /// the same bytes and change together.
    pub const INSTANCED: Shaders<'static> = Shaders {
        vertex: INSTANCED_VS_SPV,
        fragment: INSTANCED_FS_SPV,
        vertex_count: 6,
    };

    /// The instance layout `INSTANCED` consumes: centre, then colour.
    pub const INSTANCED_LAYOUT: &[crate::VertexAttribute] =
        &[crate::VertexAttribute::Vec2, crate::VertexAttribute::Vec4];

    /// The instanced quad with per-instance depth: six expanded
    /// vertices per instance; placement, depth and colour from the one
    /// vertex buffer at instance rate. The matching layout is
    /// [`INSTANCED_DEPTH_LAYOUT`]; shader and slice describe the same
    /// bytes and change together.
    pub const INSTANCED_DEPTH: Shaders<'static> = Shaders {
        vertex: INSTANCED_DEPTH_VS_SPV,
        fragment: INSTANCED_DEPTH_FS_SPV,
        vertex_count: 6,
    };

    /// The instance layout `INSTANCED_DEPTH` consumes: (centre.xy,
    /// depth, unused), then colour.
    pub const INSTANCED_DEPTH_LAYOUT: &[crate::VertexAttribute] =
        &[crate::VertexAttribute::Vec4, crate::VertexAttribute::Vec4];

    /// Vertex stage SPIR-V for the particle billboard.
    pub static PARTICLE_VS_SPV: &[u8] = include_bytes!("../shaders/particle.vert.spv");
    /// Fragment stage SPIR-V multiplying the atlas texel by the
    /// instance colour.
    pub static PARTICLE_FS_SPV: &[u8] = include_bytes!("../shaders/particle.frag.spv");

    /// The particle billboard: six expanded vertices per instance,
    /// facing the camera whose matrix and billboard basis arrive as a
    /// ninety-six-byte push block (the matrix, then right and up as
    /// vec4s with unused `w`). Samples set 0 binding 0. The matching
    /// instance layout is [`PARTICLE_INSTANCE_LAYOUT`]; shader and
    /// slice describe the same bytes and change together.
    pub const PARTICLE: Shaders<'static> = Shaders {
        vertex: PARTICLE_VS_SPV,
        fragment: PARTICLE_FS_SPV,
        vertex_count: 6,
    };

    /// The instance layout [`PARTICLE`] consumes: centre and size in
    /// one four-float group, a premultiplied colour, and the atlas
    /// rectangle — packing to 48 bytes.
    pub const PARTICLE_INSTANCE_LAYOUT: &[crate::VertexAttribute] = &[
        crate::VertexAttribute::Vec4,
        crate::VertexAttribute::Vec4,
        crate::VertexAttribute::Vec4,
    ];

    /// The colored triangle: three vertices, no descriptors.
    pub const TRIANGLE: Shaders<'static> = Shaders {
        vertex: TRIANGLE_VS_SPV,
        fragment: TRIANGLE_FS_SPV,
        vertex_count: 3,
    };

    /// The full-target textured quad: two triangles, sampling set 0,
    /// binding 0.
    pub const TEXTURED: Shaders<'static> = Shaders {
        vertex: TEXTURED_VS_SPV,
        fragment: TEXTURED_FS_SPV,
        vertex_count: 6,
    };

    /// Fragment stage SPIR-V sampling two slots: set 0 for the left
    /// half of the target, set 1 for the right.
    pub static TEXTURED_PAIR_FS_SPV: &[u8] = include_bytes!("../shaders/textured_pair.frag.spv");

    /// The two-slot quad: [`TEXTURED`]'s vertex stage over a fragment
    /// stage reading two sampled bindings — the shape that proves N
    /// textures share one pipeline. The target splits left/right at
    /// its vertical midline, so a wrong bind order is visibly wrong.
    pub const TEXTURED_PAIR: Shaders<'static> = Shaders {
        vertex: TEXTURED_VS_SPV,
        fragment: TEXTURED_PAIR_FS_SPV,
        vertex_count: 6,
    };

    /// Vertex stage SPIR-V reading a per-vertex stream.
    pub static MESH_VS_SPV: &[u8] = include_bytes!("../shaders/mesh.vert.spv");
    /// Fragment stage SPIR-V passing the interpolated colour through.
    pub static MESH_FS_SPV: &[u8] = include_bytes!("../shaders/mesh.frag.spv");

    /// The camera-aware mesh vertex stage: world-space positions
    /// multiplied by the matrix in its sixty-four-byte push-constant
    /// block.
    pub static MESH_CAMERA_VS_SPV: &[u8] = include_bytes!("../shaders/mesh_camera.vert.spv");

    /// The camera-aware mesh fragment stage: the vertex colour, faded
    /// with distance so a flat-shaded room reads as a space.
    pub static MESH_CAMERA_FS_SPV: &[u8] = include_bytes!("../shaders/mesh_camera.frag.spv");

    /// The mesh pair: clip-space positions and colours read per vertex,
    /// walked by an index buffer.
    ///
    /// **A [`MeshShaders`] rather than a [`Shaders`], so there is no
    /// vertex count here to be ignored.** The count belongs to the
    /// geometry on this path. The matching layout is [`MESH_LAYOUT`];
    /// shader and slice describe the same bytes and change together.
    ///
    /// [`MeshShaders`]: crate::MeshShaders
    /// [`Shaders`]: crate::Shaders
    pub const MESH: crate::MeshShaders<'static> = crate::MeshShaders {
        vertex: MESH_VS_SPV,
        fragment: MESH_FS_SPV,
    };

    /// The per-vertex layout [`MESH`] consumes: clip-space position,
    /// then colour, then a texture coordinate. Packs to **36 bytes**,
    /// which is the stride every mesh drawn by that pipeline must carry.
    ///
    /// The coordinate is carried even though this pipeline never samples
    /// anything, and `mesh.vert` says why at the declaration: the record
    /// is shared with the paths that do sample, and a pipeline describes
    /// the whole record rather than the part one shader happens to read.
    /// An attribute a shader ignores is legal; a record the pipeline
    /// mis-describes is not.
    pub const MESH_LAYOUT: &[crate::VertexAttribute] = &[
        crate::VertexAttribute::Vec3,
        crate::VertexAttribute::Vec4,
        crate::VertexAttribute::Vec2,
    ];

    /// The mesh pair with a camera: **world-space** positions and
    /// colours per vertex, multiplied by a matrix supplied once per
    /// draw as a push-constant block.
    ///
    /// **The matrix arrives as sixty-four bytes of push data.** It rode
    /// the per-instance channel before the push range existed — a
    /// per-draw constant on a per-instance road, which pinned the
    /// instance count at one and held binding 1 plus four attribute
    /// locations that real instancing wants. A pipeline built from this
    /// pair declares [`PipelineDesc::push_constant_size`]`(64)` and
    /// every item through it carries the matrix via `push_data` —
    /// column-major, the order `renew_math::Mat4` stores and a GLSL
    /// `mat4` in a push block reads, so the bytes cross unchanged.
    ///
    /// [`PipelineDesc::push_constant_size`]: crate::PipelineDesc::push_constant_size
    ///
    /// **Why this rather than transforming on the way in.** A caller
    /// that multiplied its own vertices would have to divide by `w`
    /// itself, and a triangle crossing `w = 0` cannot be divided — so it
    /// would also have to clip polygons against the near plane. Here
    /// `gl_Position` carries a real `w`, and the hardware does both.
    ///
    /// The per-vertex layout is [`MESH_LAYOUT`], unchanged. No
    /// per-instance layout: the pair declares no instance stream, and
    /// binding 1 is free for a consumer with real instances.
    pub const MESH_CAMERA: crate::MeshShaders<'static> = crate::MeshShaders {
        vertex: MESH_CAMERA_VS_SPV,
        fragment: MESH_CAMERA_FS_SPV,
    };
}

#[cfg(test)]
mod horizon_tests {
    /// Every shader that declares `HORIZON`, so the test can check all of
    /// them rather than the one somebody thought of.
    ///
    /// **Named here rather than discovered at runtime** because the sources
    /// are `include_str!`-ed into the binary and a test cannot read the
    /// directory of a crate it was compiled from. That makes this list the
    /// weak point, so `every_shader_declaring_horizon_is_on_the_list`
    /// below holds it against the shaders that actually exist.
    const HORIZON_SHADERS: [(&str, &str); 4] = [
        (
            "mesh_camera.frag",
            include_str!("../shaders/mesh_camera.frag"),
        ),
        (
            "mesh_camera_cutout.frag",
            include_str!("../shaders/mesh_camera_cutout.frag"),
        ),
        (
            "mesh_camera_shadow.frag",
            include_str!("../shaders/mesh_camera_shadow.frag"),
        ),
        (
            "mesh_camera_textured.frag",
            include_str!("../shaders/mesh_camera_textured.frag"),
        ),
    ];

    /// Which set a shader declares its fade block at, and how many
    /// combined image samplers it declares before it.
    ///
    /// **The invariant is that these are equal.** The RHI binds the block
    /// at set `sampled_bindings`, so a shader's block belongs at exactly
    /// the set after its last sampler.
    fn fade_set_and_samplers(name: &str, source: &str) -> (u32, u32) {
        let mut samplers = 0;
        let mut block = None;
        for line in source.lines() {
            let line = line.trim_start();
            if line.contains("uniform sampler2D") {
                samplers += 1;
            }
            if line.contains("uniform Fade") {
                block = Some(
                    line.split("set = ")
                        .nth(1)
                        .and_then(|rest| rest.split(',').next())
                        .and_then(|digits| digits.trim().parse::<u32>().ok())
                        .unwrap_or_else(|| panic!("{name}'s fade block must name its set")),
                );
            }
        }
        (
            block.unwrap_or_else(|| panic!("{name} must declare a `uniform Fade` block")),
            samplers,
        )
    }

    /// **Every camera shader reads the horizon from a block, at the set
    /// its own samplers leave free.**
    ///
    /// The colour was a `const vec3` in four shaders until E10, so a
    /// consumer whose world is warm could not say so. It could not become
    /// a push constant either, and not for want of space: this engine
    /// declares its push range for the vertex stage alone, so a fragment
    /// shader cannot read one at all.
    ///
    /// **The set index is what can drift now.** The RHI binds the block
    /// at set `sampled_bindings`, so a shader's block belongs at exactly
    /// the set after its last sampler. One set too far binds nothing and
    /// reads zeroes, which draws a world fading to black — a lighting bug
    /// to look at, and a layout mistake in fact.
    ///
    /// Checked inside each shader rather than against a table naming
    /// which pipeline uses which: a second table is a second thing to
    /// keep in step, and a shader is self-consistent or it is not.
    ///
    /// Probed by moving one block one set along: the mismatch names the
    /// shader and both numbers.
    #[test]
    fn every_camera_shader_reads_the_horizon_from_the_set_after_its_samplers() {
        for (name, source) in HORIZON_SHADERS {
            let (set, samplers) = fade_set_and_samplers(name, source);
            assert_eq!(
                set, samplers,
                "{name} declares {samplers} sampler(s) and puts its fade block at set {set};                  the RHI binds the block at set `sampled_bindings`, so those must match"
            );
            assert!(
                source.contains("fade.horizon"),
                "{name} declares a fade block and never reads it"
            );
            assert!(
                !source.contains("const vec3 HORIZON"),
                "{name} still carries the compiled-in horizon beside the block it reads"
            );
        }
    }

    /// The list above is the thing that can go stale, so it is checked
    /// against the directory rather than trusted.
    ///
    /// A shader added tomorrow that fades to the horizon and is not listed
    /// would leave the drift check silently narrower than it reads.
    ///
    /// The filesystem ban this crate carries is about the *renderer* never
    /// reading a file while it draws. Reading this crate's own shader
    /// directory to check a list against it is a different act, at a
    /// different time, and is named rather than exempted silently.
    #[test]
    #[allow(
        clippy::disallowed_methods,
        reason = "a test reading its own crate's sources is not the renderer reading files"
    )]
    fn every_shader_reading_a_fade_block_is_on_the_list() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        let mut declaring: Vec<String> = std::fs::read_dir(&dir)
            .expect("the shader directory is beside the crate")
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.extension()? != "frag" && path.extension()? != "vert" {
                    return None;
                }
                let source = std::fs::read_to_string(&path).ok()?;
                source
                    .lines()
                    .any(|line| line.contains("uniform Fade"))
                    .then(|| path.file_name()?.to_str().map(str::to_owned))
                    .flatten()
            })
            .collect();
        declaring.sort();

        let mut listed: Vec<String> = HORIZON_SHADERS
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        listed.sort();

        assert_eq!(
            declaring, listed,
            "the shaders reading a fade block and the list the drift check walks have diverged"
        );
    }

    /// The two shadowed vertex stages that share one push block, and the
    /// members they must both declare, in order, with the bytes each
    /// costs.
    ///
    /// **The block is exactly the guaranteed push ceiling, and the fit is
    /// the whole design.** A camera matrix, the light's three rows, and a
    /// scene light come to 128; the naive layout — two full matrices and
    /// a colour — is 144 and does not fit, which is why no path carried a
    /// light and a shadow at once before. Anything that grows a member
    /// breaks the path silently, so the shape is pinned here rather than
    /// left to two files agreeing by habit.
    const SHADOW_BLOCK_MEMBERS: [(&str, &str, u32); 5] = [
        ("mat4", "view_projection", 64),
        ("vec4", "light_row_0", 16),
        ("vec4", "light_row_1", 16),
        ("vec4", "light_row_2", 16),
        ("vec4", "light", 16),
    ];

    /// Both stages reading that block.
    const SHADOW_BLOCK_SHADERS: [(&str, &str); 2] = [
        (
            "mesh_camera_shadow.vert",
            include_str!("../shaders/mesh_camera_shadow.vert"),
        ),
        (
            "mesh_camera_shadow_caster.vert",
            include_str!("../shaders/mesh_camera_shadow_caster.vert"),
        ),
    ];

    /// The `type name;` pairs inside a shader's `push_constant` block, in
    /// declaration order, with comments and blank lines dropped.
    fn declared_push_members(name: &str, source: &str) -> Vec<(String, String)> {
        let open = source
            .find("layout(push_constant) uniform Matrices {")
            .unwrap_or_else(|| panic!("{name} must declare a push_constant block named Matrices"));
        // `unwrap_or_else` rather than `let ... else`, matching the
        // sibling parser above: the refusal is the same, and this shape
        // keeps the never-taken arm inside the expression rather than on
        // a line of its own that no well-formed shader can reach.
        let body_start = open
            + source[open..]
                .find('{')
                .unwrap_or_else(|| panic!("{name}'s push block must have a body"))
            + 1;
        let body_end = body_start
            + source[body_start..]
                .find('}')
                .unwrap_or_else(|| panic!("{name}'s push block must be closed"));
        source[body_start..body_end]
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("//"))
            .map(|line| {
                let statement = line.trim_end_matches(';');
                let (kind, member) = statement
                    .split_once(char::is_whitespace)
                    .unwrap_or_else(|| panic!("{name}: `{line}` is not `type name;`"));
                (kind.trim().to_owned(), member.trim().to_owned())
            })
            .collect()
    }

    /// **Both shadowed stages declare one block, member for member, in
    /// one order.**
    ///
    /// They are two files that must agree byte for byte: the caster
    /// rasterizes the depth the lit stage compares against, so a member
    /// that moved in one and not the other would put the comparison at
    /// the wrong offset and shift every shadow. Nothing else in the
    /// toolchain checks it — SPIR-V is embedded as bytes and no stage
    /// reflection exists here.
    ///
    /// Probed by reordering two members in one stage, by renaming one,
    /// and by replacing the three rows with a `mat4x3`: each fails.
    #[test]
    fn the_shadowed_shaders_declare_one_push_block() {
        let expected: Vec<(String, String)> = SHADOW_BLOCK_MEMBERS
            .iter()
            .map(|(kind, member, _)| ((*kind).to_owned(), (*member).to_owned()))
            .collect();
        for (name, source) in SHADOW_BLOCK_SHADERS {
            assert_eq!(
                declared_push_members(name, source),
                expected,
                "{name}'s push block is not the shared shadow block"
            );
        }
        let total: u32 = SHADOW_BLOCK_MEMBERS.iter().map(|(_, _, bytes)| bytes).sum();
        assert_eq!(
            total,
            crate::builtin::MESH_CAMERA_SHADOW_PUSH_BYTES,
            "the members do not sum to the declared push range"
        );
        assert_eq!(
            total,
            crate::MAX_PUSH_CONSTANT_BYTES,
            "the block is meant to be exactly the guaranteed ceiling"
        );

        // **Where the light is applied, pinned in text — with the
        // comments stripped first.** No pixel probe can tell a light
        // multiplied in the vertex stage from one multiplied in the
        // fragment stage, so this substring is the only guard. Searching
        // the raw source would let a commented-out line satisfy it:
        // `// fragment_colour = vertex_colour * matrices.light;` above a
        // line that drops the multiply keeps a naive `contains` green.
        //
        // Both halves of the family are pinned, not just the shadowed
        // one. They are spelled differently — one parenthesises the
        // vertex colour — so each is matched on the parts that carry the
        // meaning: the destination, the source, and the light.
        for (name, source, receiver) in [
            (
                "mesh_camera_shadow.vert",
                SHADOW_BLOCK_SHADERS[0].1,
                "matrices.light",
            ),
            (
                "mesh_camera.vert",
                include_str!("../shaders/mesh_camera.vert"),
                "camera.light",
            ),
        ] {
            let code: String = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            let applied = code
                .split_once("fragment_colour =")
                .is_some_and(|(_, rest)| {
                    let statement = rest.split(';').next().unwrap_or("");
                    statement.contains("vertex_colour") && statement.contains(receiver)
                });
            assert!(
                applied,
                "{name} must apply the scene light to the vertex colour in the vertex \
                 stage, as every camera path does — a world drawn half by each must dim \
                 alike"
            );
        }
    }

    /// Every shader that reads the shared shadow block is on the list
    /// above, and none of them spells the light's rows as a `mat4x3`.
    ///
    /// **A directory read, because a list is only as good as its
    /// completeness** — the sibling of `every_shader_declaring_horizon_is_on_the_list`,
    /// for the same reason. The `mat4x3` ban is the trap this layout
    /// invites: that type looks like the same saving and is not, because
    /// std430 pads each of its four three-component columns back to
    /// sixteen bytes — 64 again, and the block back to 144, while the
    /// host packs 48 and the shader reads padding.
    ///
    /// Probed by adding a third shader that reads `light_row_0` without
    /// listing it, and by writing `mat4x3` in any shader: each fails.
    #[test]
    #[allow(
        clippy::disallowed_methods,
        reason = "a test reading its own crate's sources is not the renderer reading files"
    )]
    fn every_shader_reading_the_shadow_block_is_on_the_list() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        let mut reading: Vec<String> = Vec::new();
        let mut with_mat4x3: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("the shader directory is beside the crate") {
            let path = entry.expect("a readable directory entry").path();
            let is_source = path
                .extension()
                .is_some_and(|kind| kind == "vert" || kind == "frag");
            if !is_source {
                continue;
            }
            let name = path
                .file_name()
                .expect("a named file")
                .to_string_lossy()
                .into_owned();
            let source = std::fs::read_to_string(&path).expect("a readable shader");
            if source.contains("light_row_0") {
                reading.push(name.clone());
            }
            // The word, not a comment mentioning it: the bans below are
            // about declarations, and this file's own comments explain
            // why the type is wrong.
            // A filter rather than an `if` that pushes: the push can only
            // run when a shader breaks the ban, so as a branch it is
            // untaken by design and reads as uncovered forever. Collected
            // this way the predicate runs on every shader and the result
            // is simply empty.
            with_mat4x3.extend(
                source
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .any(|line| line.contains("mat4x3"))
                    .then_some(name),
            );
        }
        reading.sort();
        let mut listed: Vec<String> = SHADOW_BLOCK_SHADERS
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        listed.sort();
        assert_eq!(
            reading, listed,
            "every shader reading the shadow block must be on SHADOW_BLOCK_SHADERS"
        );
        assert!(
            with_mat4x3.is_empty(),
            "mat4x3 saves nothing under std430 — each of its four columns pads back to \
             sixteen bytes — and using it here silently returns the block to 144: {with_mat4x3:?}"
        );
    }
}
