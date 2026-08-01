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

/// How a sampled image is filtered and addressed.
///
/// **`#[non_exhaustive]` with a constructor, per the descriptor pattern
/// this crate now uses everywhere** -- filter and address mode are the
/// two a sprite atlas needs, and mip mode, anisotropy and border colour
/// arrive as builders touching no caller.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct SamplerDesc {
    /// How texels are chosen when the sample lands between them.
    pub filter: Filter,
    /// What happens outside `[0, 1]`.
    pub address: AddressMode,
}

impl SamplerDesc {
    /// The atlas default: nearest, clamped.
    ///
    /// **Nearest rather than linear, and this is a decision rather than
    /// an omission.** It is the single parameter deciding whether a
    /// sprite atlas comes out crisp or blurred. Nearest keeps it exact,
    /// and keeps reference-image comparisons meaningful: those compare
    /// bytes, and linear filtering makes the bytes depend on how a
    /// particular adapter interpolates rather than on the engine.
    /// Clamped because an atlas has no meaning outside its own edges --
    /// a wrapped sample reads a neighbouring sprite, which is a bug that
    /// looks like a rendering artifact.
    #[must_use]
    pub fn atlas() -> Self {
        Self {
            filter: Filter::Nearest,
            address: AddressMode::ClampToEdge,
        }
    }
}

/// Texel selection between sample points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Filter {
    /// The nearest texel. Exact, and what a sprite atlas wants.
    Nearest,
    /// Bilinear blend of the four surrounding texels.
    Linear,
}

/// What a sample outside `[0, 1]` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddressMode {
    /// The edge texel, repeated.
    ClampToEdge,
    /// The image, tiled.
    Repeat,
}

impl Filter {
    fn to_vk(self) -> vk::Filter {
        match self {
            Self::Nearest => vk::Filter::NEAREST,
            Self::Linear => vk::Filter::LINEAR,
        }
    }
}

impl AddressMode {
    fn to_vk(self) -> vk::SamplerAddressMode {
        match self {
            Self::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
            Self::Repeat => vk::SamplerAddressMode::REPEAT,
        }
    }
}

/// A sampler. Holds its device alive; destroyed on drop.
///
/// # Contract
///
/// A sampler must outlive every descriptor set that references it.
///
/// **Nothing can reference one yet** -- there is no API that binds a
/// sampler to anything -- so today the contract is satisfied vacuously,
/// and `Drop` needs no quiesce: no submit can name a handle no submit
/// can reach. Whatever gains the ability to bind one owns keeping it
/// alive, by holding it rather than by asking the caller to sequence
/// drops. Stated now because the reason `Drop` is bare is the emptiness
/// of that set, and a later binding API that does not hold its sampler
/// would silently turn a vacuous guarantee into a dangling handle.
pub struct Sampler {
    pub(crate) shared: Rc<DeviceShared>,
    pub(crate) sampler: vk::Sampler,
}

impl fmt::Debug for Sampler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sampler").finish_non_exhaustive()
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // Rc; the handle was created with these callbacks; the owner of
        // any descriptor set referencing it has already quiesced.
        unsafe {
            self.shared
                .device
                .destroy_sampler(self.sampler, Some(&self.shared.alloc_cbs()));
        }
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
    /// Create a sampler.
    ///
    /// # Errors
    ///
    /// [`PipelineError::Creation`] if the driver refuses the sampler.
    pub fn create_sampler(&self, desc: &SamplerDesc) -> Result<Sampler, PipelineError> {
        let shared = &self.shared;
        let address = desc.address.to_vk();
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(desc.filter.to_vk())
            .min_filter(desc.filter.to_vk())
            // NEAREST rather than LINEAR: with no mip levels created,
            // the mode only decides how a single level is selected, and
            // nearest keeps the choice from mattering at all.
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            // No anisotropy: it is a device feature that must be enabled
            // at device creation, and this crate enables none. Asking
            // for it here without that is a validation error.
            .anisotropy_enable(false)
            .unnormalized_coordinates(false);
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // Rc; the info struct is a local outliving this call; the crate
        // is structurally `!Send + !Sync`, so external synchronisation
        // holds.
        let sampler = unsafe {
            shared
                .device
                .create_sampler(&info, Some(&shared.alloc_cbs()))
        }
        .map_err(|code| creation("vkCreateSampler", code))?;
        Ok(Sampler {
            shared: Rc::clone(shared),
            sampler,
        })
    }

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

    /// The converters, both arms of each, with no device involved.
    ///
    /// **These live here rather than in the device suite deliberately.**
    /// That suite demands `Validation::Required` and skips wherever the
    /// validation layer is absent, which is most environments. A
    /// conversion whose only exercise sits behind a skipped test is an
    /// untested conversion on every machine but the few that happen to
    /// have the layer installed.
    #[test]
    fn every_filter_and_address_mode_maps_to_its_vulkan_spelling() {
        assert_eq!(Filter::Nearest.to_vk(), vk::Filter::NEAREST);
        assert_eq!(Filter::Linear.to_vk(), vk::Filter::LINEAR);
        assert_eq!(
            AddressMode::ClampToEdge.to_vk(),
            vk::SamplerAddressMode::CLAMP_TO_EDGE
        );
        assert_eq!(AddressMode::Repeat.to_vk(), vk::SamplerAddressMode::REPEAT);
    }

    /// The atlas preset is a decision, so it is asserted rather than
    /// assumed: linear filtering would make a sampled golden's bytes
    /// depend on how an adapter interpolates, and the golden lane
    /// compares bytes.
    #[test]
    fn the_atlas_preset_is_nearest_and_clamped() {
        let desc = SamplerDesc::atlas();
        assert_eq!(desc.filter, Filter::Nearest);
        assert_eq!(desc.address, AddressMode::ClampToEdge);
    }

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
