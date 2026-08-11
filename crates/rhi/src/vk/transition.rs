//! Pass-boundary image barriers as a pure function, and the excluded
//! sites' mask literals as named values a unit test can pin.
//!
//! The pure core covers exactly the barriers that are a function of
//! (previous use, next use) at a pass boundary. Three existing barrier
//! classes are deliberately outside it, each for a stated reason: the
//! window target's acquire-chained first-use barrier (its source stage
//! exists for semaphore chaining against the presentation engine, not
//! for the layout pair), the terminal present/transfer transitions
//! (not passes), and the offscreen host-readback buffer barrier (not
//! an image transition at all). Their masks live here as named values
//! so the sites and the pinning tests read one definition.

#![deny(unsafe_code)]

use ash::vk;

/// The masks and layouts of one image barrier, ready for a site to pour
/// into a `vk::ImageMemoryBarrier2` against its own image and aspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BarrierMasks {
    pub(crate) src_stage: vk::PipelineStageFlags2,
    pub(crate) src_access: vk::AccessFlags2,
    pub(crate) dst_stage: vk::PipelineStageFlags2,
    pub(crate) dst_access: vk::AccessFlags2,
    pub(crate) old_layout: vk::ImageLayout,
    pub(crate) new_layout: vk::ImageLayout,
}

/// How a pass uses an attachment image, as the barrier core sees it.
/// `FirstUse` variants are the per-frame undefined-contents starts; the
/// others are uses whose previous pass already wrote the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageUse {
    /// This frame's first use of the color attachment: contents
    /// undefined.
    ColorAttachmentFirstUse,
    /// A color attachment a previous pass wrote this frame.
    ColorAttachment,
    /// This frame's first use of the depth attachment: contents
    /// undefined.
    DepthAttachmentFirstUse,
    /// A depth attachment a previous pass wrote this frame.
    DepthAttachment,
    /// This frame's first use of a **render image** as a color target.
    ///
    /// Not [`Self::ColorAttachmentFirstUse`], and the difference is a
    /// real hazard: a target-owned image sits behind a per-slot fence
    /// wait, so nothing can still touch it and its source scope is
    /// empty. A render image is ONE physical image across frames in
    /// flight — the previous frame's attachment writes and sampling
    /// reads are not fence-proven when this frame first writes it, so
    /// the first-use barrier must wait on the stages that could still
    /// be using it.
    RenderColorFirstUse,
    /// This frame's first use of a render image as a depth target —
    /// [`Self::RenderColorFirstUse`]'s reasoning at the depth stages.
    RenderDepthFirstUse,
    /// A render image a pass in this frame rendered into, now read by
    /// a sampling pass. Emitted once, at the first sampling pass's
    /// boundary; the contract refuses re-targeting after sampling, so
    /// no arm leads back out.
    SampledInPass,
}

/// The pass-boundary barrier for an attachment moving `from` → `to`.
///
/// # Panics
///
/// A pair no pass boundary produces — moving *to* a first use, or
/// crossing kinds — is a contract violation, asserted.
pub(crate) fn pass_boundary(from: ImageUse, to: ImageUse) -> BarrierMasks {
    match (from, to) {
        // The frame's first color use: nothing to wait on, block the
        // color-output stage that follows.
        (ImageUse::ColorAttachmentFirstUse, ImageUse::ColorAttachment) => BarrierMasks {
            src_stage: vk::PipelineStageFlags2::NONE,
            src_access: vk::AccessFlags2::NONE,
            dst_stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            dst_access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            old_layout: vk::ImageLayout::UNDEFINED,
            new_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        },
        // Between passes: the previous pass's color writes, ordered
        // before this pass's loads and writes (a Load reads, blending
        // reads, and even a Clear write-after-writes).
        (ImageUse::ColorAttachment, ImageUse::ColorAttachment) => BarrierMasks {
            src_stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            src_access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            dst_stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            dst_access: vk::AccessFlags2::COLOR_ATTACHMENT_READ
                | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            old_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            new_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        },
        // The frame's first depth use: nothing to wait on, block the
        // fragment-test stages where depth loads, tests and writes run.
        (ImageUse::DepthAttachmentFirstUse, ImageUse::DepthAttachment) => BarrierMasks {
            src_stage: vk::PipelineStageFlags2::NONE,
            src_access: vk::AccessFlags2::NONE,
            dst_stage: vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
            dst_access: vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            old_layout: vk::ImageLayout::UNDEFINED,
            new_layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        },
        // Between depth-carrying passes: the previous pass's depth
        // writes, ordered before this pass's tests and writes.
        (ImageUse::DepthAttachment, ImageUse::DepthAttachment) => BarrierMasks {
            src_stage: vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
            src_access: vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            dst_stage: vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
            dst_access: vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            old_layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            new_layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        },
        // A render image's first frame use: the source scope covers the
        // stages a previous frame could still be running against this
        // ONE physical image, and its WRITE access besides. `UNDEFINED`
        // discards contents, but a layout transition is itself a write,
        // and ordering it against the previous frame's attachment
        // writes takes availability — execution alone is a
        // write-after-write hazard, which sync validation reported on
        // this barrier's first windowed run and this access mask fixed.
        // The previous frame's sampling reads need only the execution
        // half, which the fragment-shader stage supplies.
        (ImageUse::RenderColorFirstUse, ImageUse::ColorAttachment) => BarrierMasks {
            src_stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags2::FRAGMENT_SHADER,
            src_access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            dst_stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            dst_access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            old_layout: vk::ImageLayout::UNDEFINED,
            new_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        },
        (ImageUse::RenderDepthFirstUse, ImageUse::DepthAttachment) => BarrierMasks {
            src_stage: vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::FRAGMENT_SHADER,
            src_access: vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            dst_stage: vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
            dst_access: vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            old_layout: vk::ImageLayout::UNDEFINED,
            new_layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        },
        // Rendered this frame, sampled from here on: the writing
        // stage's results made available to fragment sampling, with the
        // layout following. Emitted at the first sampling pass's
        // boundary and never reversed — the contract refuses
        // re-targeting after sampling.
        (ImageUse::ColorAttachment, ImageUse::SampledInPass) => BarrierMasks {
            src_stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            src_access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            dst_stage: vk::PipelineStageFlags2::FRAGMENT_SHADER,
            dst_access: vk::AccessFlags2::SHADER_SAMPLED_READ,
            old_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        },
        (ImageUse::DepthAttachment, ImageUse::SampledInPass) => BarrierMasks {
            src_stage: vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
            src_access: vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            dst_stage: vk::PipelineStageFlags2::FRAGMENT_SHADER,
            dst_access: vk::AccessFlags2::SHADER_SAMPLED_READ,
            old_layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        },
        (from, to) => unreachable!("no pass boundary produces {from:?} -> {to:?}"),
    }
}

/// The window target's first-use barrier — deliberately NOT the pure
/// core's first-use masks. Its source stage is `COLOR_ATTACHMENT_OUTPUT`
/// for semaphore/queue reasons: the acquire semaphore's wait is scoped
/// to that stage, so this barrier chains after the semaphore and orders
/// the layout transition against the presentation engine's outstanding
/// reads of the image (the classic write-after-present hazard). A
/// per-target synchronization fact, not a property of the layout pair.
/// Its one call site is the presentation path, hence the headless
/// allow; the pinning test binds it on every build.
#[cfg_attr(not(feature = "present"), allow(dead_code))]
pub(crate) fn acquire_chained_color_first_use() -> BarrierMasks {
    BarrierMasks {
        src_stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        src_access: vk::AccessFlags2::NONE,
        dst_stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        dst_access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        old_layout: vk::ImageLayout::UNDEFINED,
        new_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    }
}

/// The window target's terminal transition to presentation. Not a pass:
/// the signal semaphore orders presentation, so no destination stage.
/// Headless allow as above; the pinning test binds it on every build.
#[cfg_attr(not(feature = "present"), allow(dead_code))]
pub(crate) fn terminal_present() -> BarrierMasks {
    BarrierMasks {
        src_stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        src_access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        dst_stage: vk::PipelineStageFlags2::NONE,
        dst_access: vk::AccessFlags2::NONE,
        old_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        new_layout: vk::ImageLayout::PRESENT_SRC_KHR,
    }
}

/// The offscreen target's terminal transition to its readback copy. Not
/// a pass either.
pub(crate) fn terminal_transfer_src() -> BarrierMasks {
    BarrierMasks {
        src_stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        src_access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        dst_stage: vk::PipelineStageFlags2::TRANSFER,
        dst_access: vk::AccessFlags2::TRANSFER_READ,
        old_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        new_layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
    }
}

/// The stage/access masks of the offscreen host-readback *buffer*
/// barrier — not an image transition at all, so no layouts to carry.
/// Returned in `BarrierMasks` shape with `UNDEFINED` layouts standing
/// for "no layout", so the pinning test reads one vocabulary.
pub(crate) fn host_readback() -> BarrierMasks {
    BarrierMasks {
        src_stage: vk::PipelineStageFlags2::TRANSFER,
        src_access: vk::AccessFlags2::TRANSFER_WRITE,
        dst_stage: vk::PipelineStageFlags2::HOST,
        dst_access: vk::AccessFlags2::HOST_READ,
        old_layout: vk::ImageLayout::UNDEFINED,
        new_layout: vk::ImageLayout::UNDEFINED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole reachable table, every field pinned. A wrong source
    /// stage produces identical pixels on a CPU rasterizer, so the
    /// image goldens cannot be the oracle for these masks — this table
    /// is.
    #[test]
    fn the_pass_boundary_table_is_pinned_field_by_field() {
        let first_color =
            pass_boundary(ImageUse::ColorAttachmentFirstUse, ImageUse::ColorAttachment);
        assert_eq!(first_color.src_stage, vk::PipelineStageFlags2::NONE);
        assert_eq!(first_color.src_access, vk::AccessFlags2::NONE);
        assert_eq!(
            first_color.dst_stage,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
        );
        assert_eq!(
            first_color.dst_access,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
        );
        assert_eq!(first_color.old_layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(
            first_color.new_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );

        let between_color = pass_boundary(ImageUse::ColorAttachment, ImageUse::ColorAttachment);
        assert_eq!(
            between_color.src_stage,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
        );
        assert_eq!(
            between_color.src_access,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
        );
        assert_eq!(
            between_color.dst_stage,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
        );
        assert_eq!(
            between_color.dst_access,
            vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
        );
        assert_eq!(
            between_color.old_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );
        assert_eq!(
            between_color.new_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );

        let tests_stages = vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;
        let depth_rw = vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
            | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE;

        let first_depth =
            pass_boundary(ImageUse::DepthAttachmentFirstUse, ImageUse::DepthAttachment);
        assert_eq!(first_depth.src_stage, vk::PipelineStageFlags2::NONE);
        assert_eq!(first_depth.src_access, vk::AccessFlags2::NONE);
        assert_eq!(first_depth.dst_stage, tests_stages);
        assert_eq!(first_depth.dst_access, depth_rw);
        assert_eq!(first_depth.old_layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(
            first_depth.new_layout,
            vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
        );

        let between_depth = pass_boundary(ImageUse::DepthAttachment, ImageUse::DepthAttachment);
        assert_eq!(between_depth.src_stage, tests_stages);
        assert_eq!(
            between_depth.src_access,
            vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE
        );
        assert_eq!(between_depth.dst_stage, tests_stages);
        assert_eq!(between_depth.dst_access, depth_rw);
        assert_eq!(
            between_depth.old_layout,
            vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
        );
        assert_eq!(
            between_depth.new_layout,
            vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
        );
    }

    /// The render-image arms, pinned like the rest of the table. The
    /// source STAGES on the first uses are the point: an execution-only
    /// wait on whatever a previous frame could still be running against
    /// the one physical image. A CPU rasterizer draws identical pixels
    /// with these masks wrong, so this table is the oracle.
    #[test]
    fn the_render_image_arms_are_pinned_field_by_field() {
        let tests_stages = vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;
        let depth_rw = vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
            | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE;

        let render_color = pass_boundary(ImageUse::RenderColorFirstUse, ImageUse::ColorAttachment);
        assert_eq!(
            render_color.src_stage,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags2::FRAGMENT_SHADER
        );
        assert_eq!(
            render_color.src_access,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
        );
        assert_eq!(
            render_color.dst_stage,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
        );
        assert_eq!(
            render_color.dst_access,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
        );
        assert_eq!(render_color.old_layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(
            render_color.new_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );

        let render_depth = pass_boundary(ImageUse::RenderDepthFirstUse, ImageUse::DepthAttachment);
        assert_eq!(
            render_depth.src_stage,
            tests_stages | vk::PipelineStageFlags2::FRAGMENT_SHADER
        );
        assert_eq!(
            render_depth.src_access,
            vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE
        );
        assert_eq!(render_depth.dst_stage, tests_stages);
        assert_eq!(render_depth.dst_access, depth_rw);
        assert_eq!(render_depth.old_layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(
            render_depth.new_layout,
            vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
        );

        let color_sampled = pass_boundary(ImageUse::ColorAttachment, ImageUse::SampledInPass);
        assert_eq!(
            color_sampled.src_stage,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
        );
        assert_eq!(
            color_sampled.src_access,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
        );
        assert_eq!(
            color_sampled.dst_stage,
            vk::PipelineStageFlags2::FRAGMENT_SHADER
        );
        assert_eq!(
            color_sampled.dst_access,
            vk::AccessFlags2::SHADER_SAMPLED_READ
        );
        assert_eq!(
            color_sampled.old_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );
        assert_eq!(
            color_sampled.new_layout,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        );

        let depth_sampled = pass_boundary(ImageUse::DepthAttachment, ImageUse::SampledInPass);
        assert_eq!(depth_sampled.src_stage, tests_stages);
        assert_eq!(
            depth_sampled.src_access,
            vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE
        );
        assert_eq!(
            depth_sampled.dst_stage,
            vk::PipelineStageFlags2::FRAGMENT_SHADER
        );
        assert_eq!(
            depth_sampled.dst_access,
            vk::AccessFlags2::SHADER_SAMPLED_READ
        );
        assert_eq!(
            depth_sampled.old_layout,
            vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
        );
        assert_eq!(
            depth_sampled.new_layout,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        );
    }

    /// The excluded sites' literals, pinned exactly as they ship today.
    /// These are the barriers the pure core deliberately does not own;
    /// a change to any mask here is a change to a synchronization
    /// argument and must be its own reviewed decision.
    #[test]
    fn the_excluded_sites_keep_their_exact_mask_literals() {
        let acquire = acquire_chained_color_first_use();
        assert_eq!(
            acquire.src_stage,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            "the semaphore-chaining source stage is the reason this site is outside the core"
        );
        assert_eq!(acquire.src_access, vk::AccessFlags2::NONE);
        assert_eq!(
            acquire.dst_stage,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
        );
        assert_eq!(acquire.dst_access, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
        assert_eq!(acquire.old_layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(
            acquire.new_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );

        let present = terminal_present();
        assert_eq!(
            present.src_stage,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
        );
        assert_eq!(present.src_access, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
        assert_eq!(present.dst_stage, vk::PipelineStageFlags2::NONE);
        assert_eq!(present.dst_access, vk::AccessFlags2::NONE);
        assert_eq!(
            present.old_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );
        assert_eq!(present.new_layout, vk::ImageLayout::PRESENT_SRC_KHR);

        let transfer = terminal_transfer_src();
        assert_eq!(
            transfer.src_stage,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT
        );
        assert_eq!(
            transfer.src_access,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
        );
        assert_eq!(transfer.dst_stage, vk::PipelineStageFlags2::TRANSFER);
        assert_eq!(transfer.dst_access, vk::AccessFlags2::TRANSFER_READ);
        assert_eq!(
            transfer.old_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );
        assert_eq!(transfer.new_layout, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);

        let host = host_readback();
        assert_eq!(host.src_stage, vk::PipelineStageFlags2::TRANSFER);
        assert_eq!(host.src_access, vk::AccessFlags2::TRANSFER_WRITE);
        assert_eq!(host.dst_stage, vk::PipelineStageFlags2::HOST);
        assert_eq!(host.dst_access, vk::AccessFlags2::HOST_READ);
    }

    /// The unreachable pairs are refused, not silently mapped: proving
    /// the contract's edge rather than assuming it.
    #[test]
    fn a_pair_no_pass_boundary_produces_is_refused() {
        let result = std::panic::catch_unwind(|| {
            pass_boundary(ImageUse::ColorAttachment, ImageUse::DepthAttachment)
        });
        assert!(
            result.is_err(),
            "kind-crossing pairs are contract violations"
        );
        let result = std::panic::catch_unwind(|| {
            pass_boundary(ImageUse::ColorAttachment, ImageUse::ColorAttachmentFirstUse)
        });
        assert!(
            result.is_err(),
            "moving to a first use is a contract violation"
        );
        // No arm leads back out of sampling: the contract refuses
        // re-targeting after a sampling pass, so the table has nothing
        // to answer with.
        let result = std::panic::catch_unwind(|| {
            pass_boundary(ImageUse::SampledInPass, ImageUse::ColorAttachment)
        });
        assert!(
            result.is_err(),
            "re-targeting after sampling is a contract violation"
        );
    }
}
