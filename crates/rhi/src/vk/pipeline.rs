//! The v0 graphics pipeline: two SPIR-V stages, no vertex buffers, no
//! descriptors, dynamic rendering into one color attachment.

use std::fmt;
use std::rc::Rc;

use ash::vk;

use crate::config::Color;
use crate::error::PipelineError;
use crate::vk::device::{Device, DeviceShared};

/// The color format a pipeline renders into. Must match the target it
/// is used with (checked as a contract in dev builds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetFormat {
    /// The offscreen target's format.
    Rgba8Unorm,
    /// The common swapchain format on desktop.
    Bgra8Unorm,
}

impl TargetFormat {
    pub(crate) fn to_vk(self) -> vk::Format {
        match self {
            Self::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
            Self::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        }
    }
}

/// Pipeline construction parameters. The SPIR-V is borrowed byte
/// slices — [`crate::builtin`] provides the embedded v0 shaders.
///
/// `#[non_exhaustive]`, so it is built through [`PipelineDesc::new`]
/// rather than as a struct literal. Every field this will grow — vertex
/// input, blend state, depth state, push-constant ranges — is optional
/// with a defined absence, so each can arrive as a builder method
/// without touching a single existing caller. Without the attribute,
/// adding one field edits every construction site in the workspace, and
/// the count only rises.
///
/// **Not `Default`.** A default would have to supply empty shader bytes,
/// and empty SPIR-V is rejected by name during validation — so the
/// default value would be one that can never successfully build a
/// pipeline. A constructor taking exactly the parameters with no
/// sensible absence is the honest shape.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PipelineDesc<'a> {
    pub vertex_spirv: &'a [u8],
    pub fragment_spirv: &'a [u8],
    pub target_format: TargetFormat,
}

impl<'a> PipelineDesc<'a> {
    /// The three parameters a pipeline cannot be built without.
    ///
    /// Positional rather than a builder chain because none of these has
    /// a meaningful default: a pipeline with no vertex stage, no
    /// fragment stage, or no target format is not a partially-configured
    /// pipeline, it is not a pipeline. Optional state arrives later as
    /// builder methods on top of this.
    #[must_use]
    pub fn new(
        vertex_spirv: &'a [u8],
        fragment_spirv: &'a [u8],
        target_format: TargetFormat,
    ) -> Self {
        Self {
            vertex_spirv,
            fragment_spirv,
            target_format,
        }
    }
}

/// Everything one frame needs, for either target.
///
/// **One type, not one per target.** `RenderPipeline` already crosses
/// both targets carrying a field only one of them can satisfy, and each
/// target refuses a mismatch itself with a `debug_assert!` beside its
/// device check. So "one descriptor, target-specific fields validated at
/// the target" is the pattern this crate already uses for the closest
/// analogue it has, and splitting this one would leave two conventions
/// for one question.
///
/// **`#[non_exhaustive]`, and this is the whole point of the type.** The
/// old signature took the clear colour and the pipeline positionally,
/// which meant every frame-level parameter that arrived later broke every
/// caller. Several are already known to be coming: an in-flight policy, a
/// load operation, a viewport, a colour-space override. Each of those is
/// now a builder method that touches nothing.
///
/// Passing a field a target cannot satisfy is a contract violation rather
/// than a recoverable condition -- the caller has asked for something
/// incoherent, not something that failed -- so targets assert rather than
/// returning an error, exactly as they already do for pipeline format.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct RenderDesc<'a> {
    /// The colour the target is cleared to before drawing.
    pub clear: Color,
    /// The pipeline to draw with, or `None` to clear only.
    pub pipeline: Option<&'a RenderPipeline>,
}

impl fmt::Debug for RenderDesc<'_> {
    /// Reports *whether* a pipeline is bound, not which one.
    /// `RenderPipeline` has no `Debug` -- a Vulkan handle's address is
    /// not information -- and presence is the part a reader debugging a
    /// frame actually wants.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderDesc")
            .field("clear", &self.clear)
            .field("pipeline", &self.pipeline.map(|_| "bound"))
            .finish_non_exhaustive()
    }
}

impl<'a> RenderDesc<'a> {
    /// A frame that clears and draws nothing.
    ///
    /// The clear colour is positional because a frame has no meaningful
    /// "no clear" state today -- the load operation is unconditionally a
    /// clear in both backends. When that becomes configurable it arrives
    /// as a builder method, and this constructor keeps its meaning.
    #[must_use]
    pub fn new(clear: Color) -> Self {
        Self {
            clear,
            pipeline: None,
        }
    }

    /// Draw with this pipeline after clearing.
    #[must_use]
    pub fn pipeline(mut self, pipeline: &'a RenderPipeline) -> Self {
        self.pipeline = Some(pipeline);
        self
    }
}

/// A compiled draw pipeline. Holds its device alive; destroyed on
/// drop (after a best-effort quiesce — v0 accepts a wait-idle in cold
/// teardown paths for unconditional correctness).
pub struct RenderPipeline {
    pub(crate) shared: Rc<DeviceShared>,
    pub(crate) pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    pub(crate) format: TargetFormat,
}

impl Drop for RenderPipeline {
    fn drop(&mut self) {
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // Rc; wait-idle guarantees no submitted work still references
        // the pipeline; handles were created with these callbacks.
        unsafe {
            let _ = self.shared.device.device_wait_idle();
            self.shared
                .device
                .destroy_pipeline(self.pipeline, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .destroy_pipeline_layout(self.layout, Some(&self.shared.alloc_cbs()));
        }
    }
}

fn creation(call: &'static str, code: vk::Result) -> PipelineError {
    PipelineError::Creation {
        call,
        code: code.as_raw(),
    }
}

impl Device {
    /// Build a draw pipeline for a target of `desc.target_format`.
    ///
    /// # Errors
    ///
    /// [`PipelineError::InvalidSpirv`] when either byte slice fails the
    /// structural checks; [`PipelineError::Creation`] when the driver
    /// rejects a creation call.
    #[expect(
        clippy::too_many_lines,
        reason = "the fixed-function state block is one declaration list; splitting it scatters what belongs together"
    )]
    pub fn create_pipeline(
        &self,
        desc: &PipelineDesc<'_>,
    ) -> Result<RenderPipeline, PipelineError> {
        let shared = &self.shared;
        let vs_words = crate::spirv::words_from_bytes("vertex", desc.vertex_spirv)?;
        let fs_words = crate::spirv::words_from_bytes("fragment", desc.fragment_spirv)?;

        // SAFETY: category 2 (ash dispatch): device live via the spine;
        // the word slices outlive the calls; callbacks' ledger outlives
        // the modules (destroyed below in every path).
        let vs = unsafe {
            shared.device.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&vs_words),
                Some(&shared.alloc_cbs()),
            )
        }
        .map_err(|code| creation("vkCreateShaderModule(vertex)", code))?;
        // SAFETY: as above.
        let fs = match unsafe {
            shared.device.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&fs_words),
                Some(&shared.alloc_cbs()),
            )
        } {
            Ok(module) => module,
            Err(code) => {
                // SAFETY: `vs` was just created, unused elsewhere.
                unsafe {
                    shared
                        .device
                        .destroy_shader_module(vs, Some(&shared.alloc_cbs()));
                }
                return Err(creation("vkCreateShaderModule(fragment)", code));
            }
        };
        // Both modules exist past this point; this frees them on every
        // exit (the pipeline retains what it needs — modules are only
        // creation-time inputs).
        let destroy_modules = || {
            // SAFETY: both modules live and unused after pipeline
            // creation resolves.
            unsafe {
                shared
                    .device
                    .destroy_shader_module(vs, Some(&shared.alloc_cbs()));
                shared
                    .device
                    .destroy_shader_module(fs, Some(&shared.alloc_cbs()));
            }
        };

        // SAFETY: category 2: device live; the default (empty) layout
        // info borrows nothing.
        let layout = match unsafe {
            shared.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default(),
                Some(&shared.alloc_cbs()),
            )
        } {
            Ok(layout) => layout,
            Err(code) => {
                destroy_modules();
                return Err(creation("vkCreatePipelineLayout", code));
            }
        };

        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vs)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs)
                .name(c"main"),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = [desc.target_format.to_vk()];
        let mut rendering =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);

        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);

        // SAFETY: category 2: device live; every array the create info
        // references is a local outliving the call.
        let created = unsafe {
            shared.device.create_graphics_pipelines(
                vk::PipelineCache::null(),
                &[info],
                Some(&shared.alloc_cbs()),
            )
        };
        destroy_modules();
        // A rejected create info and an empty result set are the same
        // outcome here: no pipeline, and a layout nobody owns. One info
        // in, one pipeline out is the driver contract — and ash sizes
        // the result vector from the create-info count, so the empty
        // case is unreachable rather than merely unlikely; folding it
        // into the failure path keeps it diagnosable instead of
        // asserted, at no cost.
        let (built, code) = match created {
            Ok(pipelines) => (pipelines.into_iter().next(), vk::Result::ERROR_UNKNOWN),
            Err((_partial, code)) => (None, code),
        };
        let Some(pipeline) = built else {
            // SAFETY: layout live, no pipeline retained it.
            unsafe {
                shared
                    .device
                    .destroy_pipeline_layout(layout, Some(&shared.alloc_cbs()));
            }
            return Err(creation("vkCreateGraphicsPipelines", code));
        };

        Ok(RenderPipeline {
            shared: Rc::clone(shared),
            pipeline,
            layout,
            format: desc.target_format,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unbound case, asserted on its content rather than merely run.
    ///
    /// A test that only *calls* `Debug` satisfies a line-coverage gate
    /// while proving nothing -- which is the failure mode a 100% gate
    /// invites. This asserts the two claims the impl actually makes: the
    /// clear colour is reported, and the pipeline field says whether one
    /// is bound rather than naming a handle.
    #[test]
    fn the_debug_form_reports_an_unbound_pipeline_as_none() {
        let desc = RenderDesc::new(Color::new(0.25, 0.5, 0.75, 1.0));
        let shown = format!("{desc:?}");
        assert!(shown.contains("RenderDesc"), "{shown}");
        assert!(
            shown.contains("0.25"),
            "the clear colour should be visible: {shown}"
        );
        assert!(shown.contains("pipeline: None"), "{shown}");
        // `finish_non_exhaustive` renders the trailing `..`, which is the
        // signal to a reader that the struct grows.
        assert!(shown.contains(".."), "{shown}");
    }
}
