//! Batched 2D sprites over the rendering crate: one atlas, one
//! pipeline, one instanced draw per frame.
//!
//! # Contract
//!
//! - **Fill order is draw order.** Sprites composite in exactly the
//!   order pushed — painter's algorithm, no sort keys, no batches. A
//!   caller that wants order sorts before pushing.
//! - **Everything composites premultiplied.** Atlas bytes are authored,
//!   straight alpha: the hardware decodes them on sample and the
//!   fragment stage multiplies each texel's colour by its alpha. Tints
//!   are premultiplied by the caller. The pipeline composites
//!   `src + dst * (1 - src.a)`. Bytes that break either convention
//!   composite wrong, visibly, not unsafely.
//! - **All allocations happen at creation.** `begin`/`push`/`item`
//!   allocate nothing; the crate's gate measures it.
//! - **Target-agnostic.** [`SpriteRenderer::item`] returns the
//!   rendering crate's own draw item and its `color_attachment` the matching
//!   color attachment; the caller composes the frame on its own stack
//!   and hands it to whichever target it holds. This crate never
//!   renders, never presents, and never touches a window.
//! - **Zero rotation and unit scale are exact**, and quarter turns,
//!   the negative-scale mirror and flips permute an integer-cornered
//!   sprite's corners and lanes bit for bit; the sine and cosine of a
//!   turn are this crate's own, so a turned sprite packs the same
//!   corners on every platform. A region that is ever turned owes a
//!   one-texel transparent gutter ([`Region`]).
//!
//! The pure half ([`Canvas`], [`Region`], [`SubRegion`], [`Sprite`],
//! [`Instance`], the corner transform, the turn's sine and cosine, the
//! ortho and UV maps) lives apart from the device half
//! ([`SpriteRenderer`]) so the math is testable without a GPU and the
//! rendering-crate seam stays one module wide.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod fill;
mod gpu;

pub use fill::{Canvas, Instance, Region, Sprite, SubRegion};
pub use gpu::{AtlasDesc, Render2dError, SpriteRenderer};
