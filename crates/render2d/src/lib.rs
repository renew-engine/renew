//! Batched 2D sprites over the rendering crate: one atlas, one
//! pipeline, one instanced draw per frame.
//!
//! # Contract
//!
//! - **Fill order is draw order.** Sprites composite in exactly the
//!   order pushed — painter's algorithm, no sort keys, no batches. A
//!   caller that wants order sorts before pushing.
//! - **Everything is premultiplied.** Atlas texels and tints alike
//!   carry their alpha multiplied into their color channels; the
//!   pipeline composites `src + dst * (1 - src.a)`. Bytes that break
//!   the convention composite wrong, visibly, not unsafely.
//! - **All allocations happen at creation.** `begin`/`push`/`item`
//!   allocate nothing; the crate's gate measures it.
//! - **Target-agnostic.** [`SpriteRenderer::item`] returns the
//!   rendering crate's own draw item and its `color_attachment` the matching
//!   color attachment; the caller composes the frame on its own stack
//!   and hands it to whichever target it holds. This crate never
//!   renders, never presents, and never touches a window.
//!
//! The pure half ([`Canvas`], [`Region`], [`Sprite`], [`Instance`], the
//! ortho and UV maps) lives apart from the device half
//! ([`SpriteRenderer`]) so the math is testable without a GPU and the
//! rendering-crate seam stays one module wide.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod fill;
mod gpu;

pub use fill::{Canvas, Instance, Region, Sprite};
pub use gpu::{AtlasDesc, Render2dError, SpriteRenderer};
