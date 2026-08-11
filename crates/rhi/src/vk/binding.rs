//! Bindings: what a draw samples, decoupled from the pipeline that
//! samples it.
//!
//! A [`Binding`] is one written descriptor set behind the device's one
//! canonical layout — created once, written once, never rewritten. A
//! pipeline declares how many sampled slots it reads
//! ([`PipelineDesc::sampled_bindings`](crate::PipelineDesc::sampled_bindings));
//! an item names which bindings fill them. N textures through one
//! pipeline is the point: the pipeline owns the shaders, the binding
//! owns what they read.

use std::rc::Rc;

use ash::vk;

use crate::error::PipelineError;
use crate::vk::device::{Device, DeviceShared};
use crate::vk::pipeline::{Sampler, SamplerInner, creation};
use crate::vk::texture::{Texture, TextureInner};

/// How many sampled-binding slots one pipeline may declare, and so the
/// most bindings one item may carry.
///
/// A fixed ceiling rather than a `Vec`, so pipeline layouts, item
/// binding lists, and the record path's set array are all stack-sized
/// and the frame path allocates nothing — the `MAX_VERTEX_ATTRIBUTES`
/// reasoning. Four is exactly `maxBoundDescriptorSets`' guaranteed
/// floor, so a declaration this accepts is one every conformant
/// adapter accepts — and it is headroom in this tree, where every
/// consumer binds one and the widest user is the two-slot golden.
pub const MAX_SAMPLED_BINDINGS: usize = 4;

/// What a binding reads.
///
/// An input enum, so `#[non_exhaustive]`: the render-image arm arrives
/// with render-to-texture and must not break downstream matchers when
/// it does.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum BindingSource<'a> {
    /// A host-filled immutable texture.
    Texture(&'a Texture),
}

/// Everything a binding needs: what it reads, and how.
///
/// `#[non_exhaustive]` with a constructor, per the descriptor pattern
/// this crate uses everywhere.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct BindingDesc<'a> {
    /// The image this binding samples.
    pub source: BindingSource<'a>,
    /// How the image is sampled.
    pub sampler: &'a Sampler,
}

impl<'a> BindingDesc<'a> {
    /// A binding sampling `source` through `sampler`.
    ///
    /// Positional because neither has a meaningful default: a binding
    /// with nothing to read, or no way to read it, is not a
    /// partially-configured binding.
    #[must_use]
    pub fn new(source: BindingSource<'a>, sampler: &'a Sampler) -> Self {
        Self { source, sampler }
    }
}

/// The binding's owning half: the pool, the set allocated from it, and
/// shared ownership of everything the set points at.
///
/// **A pool per binding rather than one shared across the crate.** With
/// the set written once at creation and never reallocated, the pool has
/// no churn to amortise, and a pool whose lifetime is exactly that of
/// its single set needs no free list, no fragmentation story and no
/// rule about who may allocate from it when. That reasoning stops
/// holding the moment sets are allocated per frame.
pub(crate) struct BindingInner {
    pub(crate) shared: Rc<DeviceShared>,
    pool: vk::DescriptorPool,
    pub(crate) set: vk::DescriptorSet,
    /// Never read through — held so the view the set points at outlives
    /// the set, without the caller sequencing drops.
    _source: Rc<TextureInner>,
    /// As above, for the sampler handle.
    _sampler: Rc<SamplerInner>,
}

impl Drop for BindingInner {
    fn drop(&mut self) {
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // Rc; the pool was created with these callbacks; the set is
        // freed with its pool and must not be freed separately. No
        // submit still references the set: a recorded frame retains
        // this inner through the target's retention table, released
        // only after the frame's work provably ended — or by the
        // targets' best-effort teardown quiesce, the same corner every
        // retained class shares and the retention fields document.
        unsafe {
            self.shared
                .device
                .destroy_descriptor_pool(self.pool, Some(&self.shared.alloc_cbs()));
        }
    }
}

/// One written descriptor set: an image and the sampler that reads it,
/// bound behind the device's one canonical layout. Holds its device
/// alive; destroyed on drop.
///
/// # Contract
///
/// A binding's set is written once, at creation, and never rewritten —
/// so no rule about rewriting a set a submit still reads is needed,
/// because the operation does not exist. What the set points at is kept
/// alive by shared ownership: the caller may drop the source texture
/// and sampler the moment this exists, and may drop this mid-frame —
/// the frame that named it retains it until the work provably ends.
pub struct Binding {
    pub(crate) inner: Rc<BindingInner>,
}

impl std::fmt::Debug for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Binding").finish_non_exhaustive()
    }
}

impl Device {
    /// Create a binding: one descriptor pool sized for exactly one set,
    /// the set, and one write pointing it at the source through the
    /// sampler.
    ///
    /// # Errors
    ///
    /// [`PipelineError::Creation`] naming the Vulkan call that refused —
    /// pool creation or set allocation. Nothing half-built survives an
    /// error.
    pub fn create_binding(&self, desc: &BindingDesc<'_>) -> Result<Binding, PipelineError> {
        let shared = &self.shared;
        let BindingSource::Texture(texture) = desc.source;
        // Handles from another device would be written into this
        // device's set and read by it — undefined behaviour that no
        // return value can report, so it asserts, as the targets do for
        // a pipeline built on a foreign device.
        debug_assert!(
            Rc::ptr_eq(shared, texture.shared()),
            "texture and binding come from different devices"
        );
        debug_assert!(
            Rc::ptr_eq(shared, &desc.sampler.inner.shared),
            "sampler and binding come from different devices"
        );
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(&sizes);
        // SAFETY: category 2 (ash dispatch): device live via the spine;
        // the size array is a local outliving the call. (The same
        // argument covers every dispatch call in this function.)
        let pool = unsafe {
            shared
                .device
                .create_descriptor_pool(&pool_info, Some(&shared.alloc_cbs()))
        }
        .map_err(|code| creation("vkCreateDescriptorPool", code))?;

        // The spine's one shared layout: layout identity is what makes
        // this set compatible with every pipeline slot that declares
        // sampled bindings.
        let set_layouts = [shared.sampled_set_layout];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&set_layouts);
        // SAFETY: pool and layout live; the layout array is a local.
        let allocated = unsafe { shared.device.allocate_descriptor_sets(&alloc_info) };
        // One layout in, one set out is the driver contract, and ash
        // sizes the result from the layout count — so the empty case is
        // unreachable rather than unlikely. Folded into the failure path
        // to keep it diagnosable instead of asserted, at no cost.
        let (set, code) = match allocated {
            Ok(sets) => (sets.into_iter().next(), vk::Result::ERROR_UNKNOWN),
            Err(code) => (None, code),
        };
        let Some(set) = set else {
            // SAFETY: nothing was allocated from the pool, so no submit
            // can reference it.
            unsafe {
                shared
                    .device
                    .destroy_descriptor_pool(pool, Some(&shared.alloc_cbs()));
            }
            return Err(creation("vkAllocateDescriptorSets", code));
        };

        let image_info = [vk::DescriptorImageInfo::default()
            .sampler(desc.sampler.inner.sampler)
            .image_view(texture.inner.view)
            // The upload left the image in this layout and nothing
            // changes it afterwards, immutability being the texture
            // contract.
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let writes = [vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info)];
        // SAFETY: the set is live and not in use by any submit — nothing
        // can have been recorded against a binding that does not exist
        // yet. The write array and the image info it points at are
        // locals outliving the call.
        unsafe { shared.device.update_descriptor_sets(&writes, &[]) };

        Ok(Binding {
            inner: Rc::new(BindingInner {
                shared: Rc::clone(shared),
                pool,
                set,
                _source: Rc::clone(&texture.inner),
                _sampler: Rc::clone(&desc.sampler.inner),
            }),
        })
    }
}
