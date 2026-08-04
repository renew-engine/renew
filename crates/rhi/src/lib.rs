//! The engine's only doorway to the GPU: device bring-up, render
//! targets, and the v0 draw path from a clear through a sampled
//! texture, over Vulkan.
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
mod vk;

pub use config::{AdapterInfo, AdapterKind, Color, DeviceDesc, Extent, Validation};
pub use error::{DeviceError, PipelineError, TargetError};
pub use vk::buffer::{Buffer, BufferUsage};
pub use vk::device::{Device, HostAllocationStats, ValidationReport};
pub use vk::offscreen::OffscreenTarget;
pub use vk::pass::{Attachment, ClearValue, Item, LoadOp, Pass, RenderDesc, StoreOp};
pub use vk::pipeline::{
    AddressMode, Blend, DepthState, Filter, FrameData, InstanceAttribute, PipelineDesc,
    RenderPipeline, Sampler, SamplerDesc, Shaders, TargetFormat,
};
#[cfg(feature = "present")]
pub use vk::swapchain::{PresentOutcome, WindowTarget};
pub use vk::texture::{Texture, TextureDesc};

/// The embedded v0 shaders, each bundled with the vertex count its
/// vertex stage generates: a colored triangle, and a full-target quad
/// that samples one texture. Both draw from `gl_VertexIndex` with no
/// vertex buffers. Compiled offline by the pinned toolchain (the record
/// lives beside the sources); removed when the asset pipeline owns
/// shader delivery.
pub mod builtin {
    use crate::Shaders;

    /// Vertex stage SPIR-V.
    pub static TRIANGLE_VS_SPV: &[u8] = include_bytes!("../shaders/triangle.vert.spv");
    /// Fragment stage SPIR-V.
    pub static TRIANGLE_FS_SPV: &[u8] = include_bytes!("../shaders/triangle.frag.spv");

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
    pub const INSTANCED_LAYOUT: &[crate::InstanceAttribute] = &[
        crate::InstanceAttribute::Vec2,
        crate::InstanceAttribute::Vec4,
    ];

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
    pub const INSTANCED_DEPTH_LAYOUT: &[crate::InstanceAttribute] = &[
        crate::InstanceAttribute::Vec4,
        crate::InstanceAttribute::Vec4,
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
}
