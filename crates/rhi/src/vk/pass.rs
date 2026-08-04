//! The frame vocabulary: a frame is a list of passes, a pass is
//! attachments and items, and the whole list records into one submit.
//!
//! Callers compose these on their own stack — the borrows end at the
//! `render` call, so nothing here is stored across frames by anyone.

#![deny(unsafe_code)]

use std::fmt;

use crate::config::Color;
use crate::vk::pipeline::{FrameData, RenderPipeline};

/// Everything one frame needs, for either target: the passes, in order.
///
/// `#[non_exhaustive]` with a constructor, per the descriptor pattern
/// this crate uses everywhere. Frame-level fields arrive as builders on
/// this, later.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct RenderDesc<'a> {
    /// The frame: passes recorded in order, one submit.
    pub passes: &'a [Pass<'a>],
}

impl<'a> RenderDesc<'a> {
    /// A frame of `passes`, recorded in order.
    ///
    /// Positional because a frame without passes is not a
    /// partially-configured frame — an empty list is refused at
    /// `render` as a contract violation.
    #[must_use]
    pub fn new(passes: &'a [Pass<'a>]) -> Self {
        Self { passes }
    }
}

impl fmt::Debug for RenderDesc<'_> {
    /// Reports the frame's shape — how many passes, and each pass's
    /// item count — not the handles inside it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let items: Vec<usize> = self.passes.iter().map(|pass| pass.items.len()).collect();
        f.debug_struct("RenderDesc")
            .field("passes", &self.passes.len())
            .field("items_per_pass", &items)
            .finish_non_exhaustive()
    }
}

/// One pass: what it renders into, and the draws it records.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Pass<'a> {
    /// v0: exactly one, naming the target's own surface implicitly; a
    /// retained assert refuses zero or more-than-one until a later
    /// change adds image identity.
    pub color: &'a [Attachment],
    /// The target's own depth image, when Some.
    pub depth: Option<Attachment>,
    /// Draws, executed in slice order.
    pub items: &'a [Item<'a>],
}

impl<'a> Pass<'a> {
    /// A pass over `color`, drawing `items` in order, with no depth.
    #[must_use]
    pub fn new(color: &'a [Attachment], items: &'a [Item<'a>]) -> Self {
        Self {
            color,
            depth: None,
            items,
        }
    }

    /// Attach the target's depth image to this pass with `depth`'s ops.
    #[must_use]
    pub fn depth(mut self, depth: Attachment) -> Self {
        self.depth = Some(depth);
        self
    }
}

/// How one attachment is loaded and stored by a pass.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Attachment {
    /// What the pass finds in the attachment when it begins.
    pub load: LoadOp,
    /// What happens to the attachment's contents when the pass ends.
    pub store: StoreOp,
}

impl Attachment {
    /// An attachment loaded by `load` and stored by `store`.
    #[must_use]
    pub fn new(load: LoadOp, store: StoreOp) -> Self {
        Self { load, store }
    }
}

/// What a pass finds in an attachment when it begins.
///
/// A clear value without a clearing load is unrepresentable: the value
/// rides the variant that uses it.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum LoadOp {
    /// The attachment starts as `ClearValue`.
    Clear(ClearValue),
    /// The attachment keeps the previous pass's contents. Refused on a
    /// frame's first pass — every frame's first use of each attachment
    /// starts from undefined contents.
    Load,
}

/// What happens to an attachment's contents when a pass ends.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum StoreOp {
    /// The contents survive the pass.
    Store,
    /// The contents may be discarded — for attachments nothing reads
    /// afterwards.
    Discard,
}

/// The value a clearing load writes; the variant must match the
/// attachment's kind.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum ClearValue {
    /// A color attachment's clear.
    Color(Color),
    /// A depth attachment's clear — finite and in `[0, 1]`, asserted
    /// at `render`.
    Depth(f32),
}

/// One draw: a pipeline, and optionally this frame's bytes.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Item<'a> {
    /// The pipeline the draw binds.
    pub pipeline: &'a RenderPipeline,
    /// `FrameData` contained, not forked; room to grow (a
    /// first-instance or vertex-offset field) without touching existing
    /// callers.
    pub frame_data: Option<FrameData<'a>>,
}

impl<'a> Item<'a> {
    /// A draw with `pipeline` and no per-frame bytes.
    #[must_use]
    pub fn new(pipeline: &'a RenderPipeline) -> Self {
        Self {
            pipeline,
            frame_data: None,
        }
    }

    /// Carry per-frame bytes and an instanced draw in this item.
    #[must_use]
    pub fn frame_data(mut self, data: FrameData<'a>) -> Self {
        self.frame_data = Some(data);
        self
    }
}

/// The frame-shape contract, asserted identically by both targets
/// before any GPU call: the refusals that make a malformed frame a
/// named panic instead of a validation stream or a quiet wrong image.
///
/// Retained in release builds: every rule below guards either a
/// memory-ordering argument (first-pass loads of undefined contents,
/// two copies into one region) or a draw that renders differently than
/// written. The per-item device and format checks stay beside each
/// target's own device state, where they always were.
pub(crate) fn check_frame_contract(desc: &RenderDesc<'_>) {
    assert!(
        !desc.passes.is_empty(),
        "a frame needs at least one pass: an empty frame records nothing, and on the window          path would present contents nothing ever defined"
    );
    let mut depth_used = false;
    for (index, pass) in desc.passes.iter().enumerate() {
        assert!(
            pass.color.len() == 1,
            "pass {index}: v0 passes carry exactly one color attachment (the target's own \
             surface), got {}",
            pass.color.len()
        );
        let color = &pass.color[0];
        if index == 0 {
            assert!(
                !matches!(color.load, LoadOp::Load),
                "pass 0: LoadOp::Load on a frame's first pass loads undefined contents — \
                 every frame's first use of the attachment starts undefined"
            );
        }
        if let Some(depth) = &pass.depth {
            // The color first-use is always pass 0 (every pass carries
            // color); the depth first-use is the frame's first
            // depth-CARRYING pass, whatever its index — that is where
            // the walk transitions the image from UNDEFINED, so that is
            // where a Load reads garbage.
            if !depth_used {
                assert!(
                    !matches!(depth.load, LoadOp::Load),
                    "pass {index}: LoadOp::Load on the frame's first depth use loads \
                     undefined contents — the depth image transitions from UNDEFINED at \
                     its first carrying pass"
                );
            }
            depth_used = true;
        }
        if let LoadOp::Clear(value) = color.load {
            assert!(
                matches!(value, ClearValue::Color(_)),
                "pass {index}: a color attachment clears to ClearValue::Color, not \
                 ClearValue::Depth"
            );
        }
        if let Some(depth) = &pass.depth
            && let LoadOp::Clear(value) = depth.load
        {
            assert!(
                matches!(value, ClearValue::Depth(_)),
                "pass {index}: a depth attachment clears to ClearValue::Depth, not \
                 ClearValue::Color"
            );
            if let ClearValue::Depth(depth_value) = value {
                // The documented range, asserted: an out-of-range or
                // non-finite depth clear is invalid usage the driver may
                // answer with anything.
                assert!(
                    depth_value.is_finite() && (0.0..=1.0).contains(&depth_value),
                    "pass {index}: a depth clear must be finite and in [0, 1], got \
                     {depth_value}"
                );
            }
        }
        for item in pass.items {
            assert!(
                item.pipeline.depth == pass.depth.is_some(),
                "pass {index}: an item's pipeline depth state must match the pass — a \
                 depth-testing pipeline in a depthless pass (or the reverse) draws \
                 differently than written"
            );
        }
    }
    // One buffer, one item, per frame: two items naming one buffer would
    // have the second copy silently win before either draws.
    let mut seen: [Option<*const u8>; MAX_RETAINED_BUFFERS] = [None; MAX_RETAINED_BUFFERS];
    let mut count = 0usize;
    for pass in desc.passes {
        for item in pass.items {
            let Some(data) = &item.frame_data else {
                continue;
            };
            let key = std::rc::Rc::as_ptr(&data.buffer.inner).cast::<u8>();
            assert!(
                !seen[..count].contains(&Some(key)),
                "one buffer, one item, per frame: two items name the same buffer, and the \
                 second copy would silently win before either draws"
            );
            assert!(
                count < MAX_RETAINED_BUFFERS,
                "a frame carries at most {MAX_RETAINED_BUFFERS} distinct per-frame buffers"
            );
            seen[count] = Some(key);
            count += 1;
        }
    }
}

/// How many distinct per-frame buffers one frame may carry, per target
/// slot — the hard bound that keeps retention tables fixed-width and
/// the frame path allocation-free. The ninth distinct buffer is refused
/// by name in [`check_frame_contract`].
pub(crate) const MAX_RETAINED_BUFFERS: usize = 8;

impl LoadOp {
    pub(crate) fn to_vk(self) -> ash::vk::AttachmentLoadOp {
        match self {
            Self::Clear(_) => ash::vk::AttachmentLoadOp::CLEAR,
            Self::Load => ash::vk::AttachmentLoadOp::LOAD,
        }
    }
}

impl StoreOp {
    pub(crate) fn to_vk(self) -> ash::vk::AttachmentStoreOp {
        match self {
            Self::Store => ash::vk::AttachmentStoreOp::STORE,
            Self::Discard => ash::vk::AttachmentStoreOp::DONT_CARE,
        }
    }
}

/// The color clear an attachment's load op carries, or a zeroed value
/// for `Load` (the driver ignores it). The kind mismatch is refused by
/// [`check_frame_contract`] before any conversion runs.
pub(crate) fn vk_clear_color(attachment: &Attachment) -> ash::vk::ClearValue {
    match attachment.load {
        LoadOp::Clear(ClearValue::Color(color)) => ash::vk::ClearValue {
            color: ash::vk::ClearColorValue {
                float32: [color.r, color.g, color.b, color.a],
            },
        },
        _ => ash::vk::ClearValue::default(),
    }
}

/// The depth clear an attachment's load op carries, or a zeroed value
/// for `Load`. Kind mismatches are refused before conversion, as above.
pub(crate) fn vk_clear_depth(attachment: &Attachment) -> ash::vk::ClearValue {
    match attachment.load {
        LoadOp::Clear(ClearValue::Depth(depth)) => ash::vk::ClearValue {
            depth_stencil: ash::vk::ClearDepthStencilValue { depth, stencil: 0 },
        },
        _ => ash::vk::ClearValue::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Debug form reports shape, not handles — pinned on content so
    /// the claim cannot rot into a mere smoke call.
    #[test]
    fn the_debug_form_reports_the_frames_shape() {
        let color = [Attachment::new(
            LoadOp::Clear(ClearValue::Color(Color::new(0.0, 0.0, 0.0, 1.0))),
            StoreOp::Store,
        )];
        let passes = [Pass::new(&color, &[]), Pass::new(&color, &[])];
        let shown = format!("{:?}", RenderDesc::new(&passes));
        assert!(shown.contains("RenderDesc"), "{shown}");
        assert!(shown.contains("passes: 2"), "{shown}");
        assert!(shown.contains("items_per_pass: [0, 0]"), "{shown}");
        // `finish_non_exhaustive` renders the trailing `..`, the signal
        // that the struct grows.
        assert!(shown.contains(".."), "{shown}");
    }

    /// The op conversions, both arms of each, with no device involved.
    #[test]
    fn every_op_maps_to_its_vulkan_spelling() {
        let clear = LoadOp::Clear(ClearValue::Color(Color::new(0.0, 0.0, 0.0, 1.0)));
        assert_eq!(clear.to_vk(), ash::vk::AttachmentLoadOp::CLEAR);
        assert_eq!(LoadOp::Load.to_vk(), ash::vk::AttachmentLoadOp::LOAD);
        assert_eq!(StoreOp::Store.to_vk(), ash::vk::AttachmentStoreOp::STORE);
        assert_eq!(
            StoreOp::Discard.to_vk(),
            ash::vk::AttachmentStoreOp::DONT_CARE
        );
    }

    /// The clear values ride their variants into the raw union. The
    /// module denies `unsafe`; this test alone allows it to read back
    /// the union arms the converters write.
    #[test]
    #[allow(unsafe_code)]
    fn clear_values_survive_conversion() {
        let color = Attachment::new(
            LoadOp::Clear(ClearValue::Color(Color::new(0.25, 0.5, 0.75, 1.0))),
            StoreOp::Store,
        );
        // SAFETY: reading the union arm the converter just wrote.
        let raw = unsafe { vk_clear_color(&color).color.float32 };
        // Bit equality: the converter moves the values, it never does
        // arithmetic on them.
        assert_eq!(
            raw.map(f32::to_bits),
            [0.25f32, 0.5, 0.75, 1.0].map(f32::to_bits)
        );
        let depth = Attachment::new(LoadOp::Clear(ClearValue::Depth(0.5)), StoreOp::Discard);
        // SAFETY: as above.
        let raw = unsafe { vk_clear_depth(&depth).depth_stencil };
        assert_eq!(raw.depth.to_bits(), 0.5f32.to_bits());
        assert_eq!(raw.stencil, 0);
        // A Load carries no clear: the converters hand the driver a
        // zeroed value it is required to ignore.
        let load = Attachment::new(LoadOp::Load, StoreOp::Store);
        // SAFETY: as above.
        let raw = unsafe { vk_clear_depth(&load).depth_stencil };
        assert_eq!(raw.depth.to_bits(), 0.0f32.to_bits());
    }
}
