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
    /// Every camera fragment shader, which is every shader that fades.
    ///
    /// **Named here rather than discovered at runtime** because the
    /// sources are `include_str!`-ed into the binary and a test cannot
    /// read the directory of a crate it was compiled from. That makes this
    /// list the weak point, so `no_shader_compiles_the_horizon_in` below
    /// holds it against the shaders that actually exist.
    const FADING_SHADERS: [(&str, &str); 4] = [
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

    /// **Every shader that fades reads the colour rather than knowing it.**
    ///
    /// This replaces a pair of tests that checked the Rust constant and
    /// four compiled-in copies of it had not drifted apart. They cannot
    /// drift now, because there is one copy and the shaders no longer hold
    /// it — which is the better answer to the same question, and it is why
    /// those tests are gone rather than adapted.
    ///
    /// What can still go wrong is the opposite: somebody reintroduces a
    /// constant, the fade stops honouring what the caller asked for, and
    /// nothing looks wrong until a caller clears to a colour that is not
    /// this crate's default.
    #[test]
    fn every_fading_shader_reads_the_horizon_from_its_caller() {
        for (name, source) in FADING_SHADERS {
            assert!(
                source.contains("uniform Air {"),
                "{name} fades with distance but declares no Air block, so whatever it fades \
                 toward is not what the caller asked for"
            );
            assert!(
                source.contains("air.horizon.rgb"),
                "{name} declares an Air block and does not fade toward it"
            );
        }
    }

    /// The list above is the thing that can go stale, so it is checked
    /// against the directory rather than trusted — and the same walk
    /// catches a compiled-in horizon coming back anywhere.
    ///
    /// The filesystem ban this crate carries is about the *renderer* never
    /// reading a file while it draws. Reading this crate's own shader
    /// directory is a different act, at a different time, and is named
    /// rather than exempted silently.
    #[test]
    #[allow(
        clippy::disallowed_methods,
        reason = "a test reading its own crate's sources is not the renderer reading files"
    )]
    fn no_shader_compiles_the_horizon_in() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        let mut fading: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("the shader directory is beside the crate") {
            let Ok(path) = entry.map(|entry| entry.path()) else {
                continue;
            };
            if path
                .extension()
                .is_none_or(|kind| kind != "frag" && kind != "vert")
            {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            assert!(
                !source
                    .lines()
                    .any(|line| line.starts_with("const vec3 HORIZON")),
                "{name} compiles a horizon in, so it cannot fade toward what its caller clears \
                 to — the colour belongs in the Air block"
            );
            if source.contains("uniform Air {") {
                fading.push(name);
            }
        }
        fading.sort();

        let mut listed: Vec<String> = FADING_SHADERS
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        listed.sort();

        assert_eq!(
            fading, listed,
            "the shaders reading an Air block and the list the check above walks have diverged"
        );
    }
}
