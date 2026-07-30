//! The Vulkan backend — the crate's only `unsafe` territory. Every
//! module under here may contain `unsafe`; everything outside denies
//! it, so the exception grant is auditable by path.
//!
//! Discipline (the granted six categories, SAFETY at every site):
//! loader entry, ash dispatch calls (liveness from the shared-spine
//! ownership chain; external synchronization from the crate-wide
//! `!Send + !Sync` contract; parameter validity by construction), the
//! one surface-creation site, the allocation-callback trio, the debug
//! messenger callback, and the one mapped-memory read site.

pub mod alloc;
pub mod debug;
pub mod device;
pub mod offscreen;
pub mod pipeline;
#[cfg(feature = "present")]
pub mod swapchain;
