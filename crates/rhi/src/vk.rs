//! The Vulkan backend — the crate's only `unsafe` territory. Every
//! module under here may contain `unsafe`; everything outside denies
//! it, so the boundary is auditable by path. (This root file itself
//! carries none — review-verified, and every block crate-wide must
//! carry `// SAFETY:` by lint.)
//!
//! The six-category discipline, SAFETY at every site: loader entry,
//! ash dispatch calls (liveness from the shared-spine ownership chain;
//! external synchronization from the crate-wide `!Send + !Sync`
//! contract; parameter validity by construction), the one
//! surface-creation site, the allocation-callback trio, the debug
//! messenger callback, and the one mapped-memory read site.

pub mod alloc;
pub mod buffer;
pub mod debug;
pub mod device;
pub mod offscreen;
pub mod pipeline;
#[cfg(feature = "present")]
pub mod swapchain;
pub mod texture;
