//! The engine's only doorway to the GPU: device bring-up, render
//! targets, and the v0 clear-and-triangle draw path, over Vulkan.
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
pub use vk::device::{Device, HostAllocationStats, ValidationReport};
pub use vk::offscreen::OffscreenTarget;
pub use vk::pipeline::{PipelineDesc, RenderPipeline, TargetFormat};
#[cfg(feature = "present")]
pub use vk::swapchain::{PresentOutcome, WindowTarget};

/// The embedded v0 shaders: a colored triangle from `gl_VertexIndex`,
/// no buffers, no descriptors. Compiled offline by the pinned toolchain
/// (the record lives beside the sources); removed when the asset
/// pipeline owns shader delivery.
pub mod builtin {
    /// Vertex stage SPIR-V.
    pub static TRIANGLE_VS_SPV: &[u8] = include_bytes!("../shaders/triangle.vert.spv");
    /// Fragment stage SPIR-V.
    pub static TRIANGLE_FS_SPV: &[u8] = include_bytes!("../shaders/triangle.frag.spv");
}
