//! Indexed 3D geometry over the rendering crate: one mesh pipeline,
//! depth-tested, indexed draws in submission order.
//!
//! # Contract
//!
//! - **Push order is index order is draw order.** Quads are drawn in
//!   exactly the order pushed. No sort, no batching, no depth pre-pass —
//!   so two scenes built by the same calls produce byte-identical
//!   buffers, and a frame drawn twice is the same image.
//! - **Depth is not optional.** The pipeline tests and writes depth, and
//!   [`pass`] always attaches it. A 3D frame drawn without depth is a
//!   wrong picture that looks plausible, which is a worse failure than
//!   one that refuses.
//! - **An adapter with no depth format is refused by name**, before
//!   anything is created, carrying the format chain that was tried.
//! - **Positions are clip space.** There is no camera here; a caller
//!   drawing a world transforms on its own side until one exists.
//! - **Target-agnostic.** [`MeshRenderer::item`] returns the rendering
//!   crate's own draw item and [`attachment`] the matching colour
//!   attachment; the caller composes the frame on its own stack and
//!   hands it to whichever target it holds. This crate never renders,
//!   never presents, and never touches a window.
//!
//! The pure half ([`Scene`]) lives apart from the device half
//! ([`MeshRenderer`]) so the packing and the numbering are testable
//! without an adapter, and the rendering-crate seam stays one module
//! wide.
//!
//! # What v0 does not do
//!
//! No camera or projection, no textures, no window, no image writing,
//! and no meshing of anything — a caller supplies quads already in clip
//! space. Each of those is a later step with its own trigger, recorded
//! rather than left to be guessed at.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod gpu;
mod scene;

pub use gpu::{
    Camera, CameraRenderer, MeshRenderer, Render3dError, attachment, depth_attachment, pass,
};
pub use scene::Scene;
