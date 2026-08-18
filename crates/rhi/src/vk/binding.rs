//! Bindings: what a draw samples, decoupled from the pipeline that
//! samples it.
//!
//! A [`Binding`] is one written descriptor set behind one of the
//! device's canonical layouts — created once, written once, never
//! rewritten. A pipeline declares how many sampled slots it reads
//! ([`PipelineDesc::sampled_bindings`](crate::PipelineDesc::sampled_bindings));
//! an item names which bindings fill them. N textures through one
//! pipeline is the point: the pipeline owns the shaders, the binding
//! owns what they read.
//!
//! # Two classes, one type
//!
//! A binding reads either an **image** — a texture or a render image,
//! through a sampler — or a **uniform block**: a per-frame buffer read
//! as structured bytes rather than as an instance stream. The two use
//! different descriptor types and so different set layouts, but
//! everything downstream of creation is identical: one set, written
//! once, named by an item, retained by the frame that named it.
//!
//! A pipeline declaring a block
//! ([`PipelineDesc::uniform_block`](crate::PipelineDesc::uniform_block))
//! reads it at **set `sampled_bindings`** — after every sampled slot, so
//! a shader that samples nothing finds its block at set zero and one
//! that samples two finds it at set two.
//!
//! The bytes are per draw and arrive with the item
//! ([`Item::uniform_data`](crate::Item::uniform_data)); the descriptor is
//! dynamic, so the one set serves every frame slot and the offset is
//! supplied when the set is bound.

use std::rc::Rc;

use ash::vk;

use crate::error::PipelineError;
use crate::vk::buffer::{Buffer, BufferInner};
use crate::vk::device::{Device, DeviceShared};
use crate::vk::pipeline::{Sampler, SamplerInner, creation};
use crate::vk::render_image::{RenderImage, RenderImageInner};
use crate::vk::texture::{Texture, TextureInner};

/// How many sampled-binding slots one pipeline may declare, and so the
/// most bindings one item may carry.
///
/// **Really the bound-set ceiling, and the name is older than that.** A
/// pipeline's sampled slots and its uniform block, if it declares one,
/// share this budget: four sampled slots is legal, and four sampled slots
/// plus a block is refused. An item then names one binding per declared
/// slot *plus* one for the block, which is still at most this many.
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
/// An input enum, so `#[non_exhaustive]`: a later source class must
/// not break downstream matchers when it arrives.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum BindingSource<'a> {
    /// A host-filled immutable texture.
    Texture(&'a Texture),
    /// A render image some pass in each frame renders into.
    ///
    /// The set is still written once, here at creation — what changes
    /// per frame is the image's *contents*, which no set write ever
    /// carries. The frame contract is what refuses sampling an image
    /// this frame has not rendered, so a binding over a render image
    /// is only as fresh as the frame that draws with it.
    Image(&'a RenderImage),
}

/// Everything a binding needs: what it reads, and how.
///
/// Opaque with constructors, rather than the crate's usual
/// `#[non_exhaustive]`-with-public-fields descriptor: this one carries a
/// pairing (an image needs a sampler, a block cannot use one) that public
/// fields would let a caller break after construction.
#[derive(Debug, Clone, Copy)]
pub struct BindingDesc<'a> {
    /// What this binding reads, and how.
    ///
    /// **Private, so the two classes cannot be mixed.** An earlier shape
    /// had `pub` fields and a doc comment claiming a caller could not
    /// reach them except through a constructor — which was false, because
    /// `#[non_exhaustive]` blocks struct literals and exhaustive matches,
    /// not assignment to a public field. A downstream crate could build a
    /// sampled descriptor and then overwrite its source with a block,
    /// producing a sampler-carrying block that only a runtime assert
    /// caught. Private fields make that state unspellable instead.
    kind: Kind<'a>,
}

/// What a binding reads, paired with whatever reading it requires.
///
/// Not public: [`BindingSource`] is the public vocabulary for the two
/// *image* classes, and a uniform block is reached through
/// [`BindingDesc::uniform`] rather than by naming a source at all. Keeping
/// the block out of the public enum is what stops a caller from handing
/// one to [`BindingDesc::new`], which has no sensible reading.
#[derive(Debug, Clone, Copy)]
enum Kind<'a> {
    /// An image, and the sampler that reads it.
    Sampled(BindingSource<'a>, &'a Sampler),
    /// A per-frame buffer read as a uniform block.
    ///
    /// The "written once" argument, one step further: neither the set nor
    /// the *offset* in it changes per frame. The descriptor covers one
    /// slot's worth of the buffer and the slot is chosen by the dynamic
    /// offset the record path supplies, so a buffer whose bytes change
    /// every frame needs no set write at all.
    Uniform(&'a Buffer),
}

impl<'a> BindingDesc<'a> {
    /// A binding sampling `source` through `sampler`.
    ///
    /// Positional because neither has a meaningful default: a binding
    /// with nothing to read, or no way to read it, is not a
    /// partially-configured binding.
    #[must_use]
    pub fn new(source: BindingSource<'a>, sampler: &'a Sampler) -> Self {
        Self {
            kind: Kind::Sampled(source, sampler),
        }
    }

    /// A binding reading `buffer` as a uniform block.
    ///
    /// The buffer's per-frame capacity **is** the block's size, and the
    /// pipeline that reads it declares the same number
    /// ([`PipelineDesc::uniform_block`](crate::PipelineDesc::uniform_block)) —
    /// two statements of one fact, held to being equal when the item that
    /// names both is validated. Not merely "large enough": the
    /// descriptor's range is the whole per-frame capacity, so a roomier
    /// buffer would leave the shader reading a tail no frame writes.
    #[must_use]
    pub fn uniform(buffer: &'a Buffer) -> Self {
        Self {
            kind: Kind::Uniform(buffer),
        }
    }
}

/// Shared ownership of whichever source the set points at — the hold
/// is what matters, not the kind, so release sites never match on it.
pub(crate) enum SourceHold {
    /// The handles are never read through — the hold keeps the sampled
    /// image alive across a submit; the walk reads only which image.
    Texture(#[allow(dead_code, reason = "held for its Drop alone")] Rc<TextureInner>),
    Image(Rc<RenderImageInner>),
    /// The buffer a uniform binding reads. Read through, unlike the
    /// other two: the record path needs its slot stride to compute the
    /// dynamic offset, and the frame contract needs its capacity.
    Uniform(Rc<BufferInner>),
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
    /// Held so the view the set points at outlives the set, without
    /// the caller sequencing drops; read only to tell the frame walk
    /// which render image, if any, this set samples.
    source: SourceHold,
    /// As above, for the sampler handle — absent for a uniform block,
    /// which has nothing to filter.
    _sampler: Option<Rc<SamplerInner>>,
}

impl BindingInner {
    /// The render image this binding samples, if its source is one --
    /// how the frame walk learns which images a pass reads without
    /// matching on the hold anywhere else.
    pub(crate) fn sampled_render_image(&self) -> Option<&Rc<RenderImageInner>> {
        match &self.source {
            SourceHold::Image(inner) => Some(inner),
            SourceHold::Texture(_) | SourceHold::Uniform(_) => None,
        }
    }

    /// How many bytes of the block this binding's buffer holds per
    /// frame, or `None` when it reads an image.
    ///
    /// The frame contract compares it against the pipeline's declared
    /// block, so a buffer too small to hold what a shader will read is
    /// refused by name rather than discovered by the copy.
    pub(crate) fn block_capacity(&self) -> Option<usize> {
        self.uniform_buffer().map(|buffer| buffer.capacity)
    }

    /// The per-frame buffer this binding reads as a block, if it is one.
    ///
    /// How the record path learns which sets need a dynamic offset, and
    /// which buffer's slot stride to compute it from — without matching
    /// on the hold anywhere else.
    pub(crate) fn uniform_buffer(&self) -> Option<&Rc<BufferInner>> {
        match &self.source {
            SourceHold::Uniform(inner) => Some(inner),
            SourceHold::Texture(_) | SourceHold::Image(_) => None,
        }
    }
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

/// One written descriptor set — either an image and the sampler that
/// reads it, or a per-frame buffer read as a uniform block — bound behind
/// whichever of the device's two canonical layouts its class calls for.
/// Holds its device
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

impl Binding {
    /// Whether this binding reads a uniform block rather than an image.
    ///
    /// The frame contract reads it to check an item's binding list against
    /// its pipeline's declaration, position by position: a sampled slot
    /// filled with a block, or the reverse, binds a set the layout does
    /// not describe.
    ///
    /// `pub(crate)` because that is the only reader. A caller already
    /// knows which constructor it called, so exporting this would widen
    /// the public surface with a question nobody outside has to ask.
    pub(crate) fn is_uniform(&self) -> bool {
        self.inner.uniform_buffer().is_some()
    }
}

impl std::fmt::Debug for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Binding")
            .field("uniform", &self.is_uniform())
            .finish_non_exhaustive()
    }
}

/// What a source class decides: the layout the set is allocated behind,
/// the one descriptor type its pool holds, and the hold that keeps the
/// source alive. Everything after this is class-blind.
///
/// Split out of [`Device::create_binding`] because it is the whole of
/// what varies between classes, and the rest of that function is
/// pool-and-set bookkeeping every class shares.
///
/// Handles from another device would be written into this device's set
/// and read by it — undefined behaviour that no return value can report,
/// so each arm asserts, as the targets do for a pipeline built on a
/// foreign device.
fn classify(
    shared: &Rc<DeviceShared>,
    kind: Kind<'_>,
) -> (vk::DescriptorType, vk::DescriptorSetLayout, SourceHold) {
    match kind {
        Kind::Sampled(BindingSource::Texture(texture), _) => {
            debug_assert!(
                Rc::ptr_eq(shared, texture.shared()),
                "texture and binding come from different devices"
            );
            (
                vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                shared.sampled_set_layout,
                SourceHold::Texture(Rc::clone(&texture.inner)),
            )
        }
        Kind::Sampled(BindingSource::Image(image), _) => {
            debug_assert!(
                Rc::ptr_eq(shared, &image.inner.shared),
                "render image and binding come from different devices"
            );
            (
                vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                shared.sampled_set_layout,
                SourceHold::Image(Rc::clone(&image.inner)),
            )
        }
        Kind::Uniform(buffer) => {
            debug_assert!(
                Rc::ptr_eq(shared, &buffer.inner.shared),
                "buffer and binding come from different devices"
            );
            // **The descriptor's range is the buffer's per-frame
            // capacity, and a range past `maxUniformBufferRange` is
            // invalid usage.** The pipeline's declaration is held to the
            // guaranteed floor at creation; nothing held the *buffer* to
            // it, so a caller could allocate a per-frame buffer of any
            // size — perfectly legal as an instance stream — and bind it
            // as a block, where it becomes a validation error rather
            // than a refusal. Retained, because it bounds a value handed
            // to the driver.
            assert!(
                buffer.capacity() <= crate::MAX_UNIFORM_BLOCK_BYTES as usize,
                "a uniform block reads at most {} bytes (the guaranteed device minimum \
                 for a uniform buffer range), and this buffer holds {} per frame",
                crate::MAX_UNIFORM_BLOCK_BYTES,
                buffer.capacity()
            );
            (
                vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC,
                shared.uniform_set_layout,
                SourceHold::Uniform(Rc::clone(&buffer.inner)),
            )
        }
    }
}

impl Device {
    /// Create a binding: one descriptor pool sized for exactly one set,
    /// the set, and one write pointing it at its source — an image
    /// through a sampler, or a per-frame buffer as a uniform block.
    ///
    /// # Errors
    ///
    /// [`PipelineError::Creation`] naming the Vulkan call that refused —
    /// pool creation or set allocation. Nothing half-built survives an
    /// error.
    ///
    /// # Panics
    ///
    /// A descriptor whose source and sampler disagree — an image with no
    /// sampler, or a uniform block with one. Reachable only by building
    /// a [`BindingDesc`] some way other than its two constructors, which
    /// pair them correctly; asserted rather than returned because the
    /// pairing is a caller mistake with no partial reading.
    ///
    /// A uniform source whose buffer holds more per frame than
    /// [`MAX_UNIFORM_BLOCK_BYTES`](crate::MAX_UNIFORM_BLOCK_BYTES): the
    /// descriptor's range is that capacity, and a range past the
    /// guaranteed `maxUniformBufferRange` is invalid usage rather than a
    /// slow path.
    pub fn create_binding(&self, desc: &BindingDesc<'_>) -> Result<Binding, PipelineError> {
        let shared = &self.shared;
        let (descriptor_type, set_layout, source_hold) = classify(shared, desc.kind);
        // **No pairing assert any more.** An image binding is sampled
        // and a block is not, and the descriptor's private `Kind`
        // carries the sampler alongside the source it belongs to — so
        // the mixed state the old runtime check caught is one the type
        // cannot hold. Making an illegal state unspellable beats
        // catching it, and it retires a public variant whose only use
        // was to trip that check.
        if let Kind::Sampled(_, sampler) = desc.kind {
            debug_assert!(
                Rc::ptr_eq(shared, &sampler.inner.shared),
                "sampler and binding come from different devices"
            );
        }
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(descriptor_type)
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

        // One of the spine's canonical layouts: layout identity is what
        // makes this set compatible with every pipeline slot of its
        // class.
        let set_layouts = [set_layout];
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

        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(descriptor_type);
        // The two info arrays are locals of this scope rather than of
        // their arms, because the write borrows whichever it is given
        // and both must outlive the update below.
        let image_info = [vk::DescriptorImageInfo::default()
            .sampler(match desc.kind {
                Kind::Sampled(_, sampler) => sampler.inner.sampler,
                Kind::Uniform(_) => vk::Sampler::null(),
            })
            .image_view(match &source_hold {
                SourceHold::Texture(inner) => inner.view,
                SourceHold::Image(inner) => inner.view,
                SourceHold::Uniform(_) => vk::ImageView::null(),
            })
            // The layout every sampled read sees. A texture's upload
            // left it here and immutability keeps it; a render image is
            // *brought* here by the frame walk's sampling transition
            // before any draw recorded against this set runs.
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let buffer_info = [vk::DescriptorBufferInfo::default()
            .buffer(match &source_hold {
                SourceHold::Uniform(inner) => inner.buffer,
                SourceHold::Texture(_) | SourceHold::Image(_) => vk::Buffer::null(),
            })
            // Offset zero and one slot's worth of range: the descriptor
            // describes the *shape* of a slot and the record path adds
            // `slot * stride` as the dynamic offset. A range of
            // WHOLE_SIZE would make the last slot's offset push the
            // window past the end of the allocation, which is a usage
            // violation rather than a wrong picture.
            .offset(0)
            .range(match &source_hold {
                SourceHold::Uniform(inner) => inner.capacity as u64,
                SourceHold::Texture(_) | SourceHold::Image(_) => vk::WHOLE_SIZE,
            })];
        let writes = [match &source_hold {
            SourceHold::Uniform(_) => write.buffer_info(&buffer_info),
            SourceHold::Texture(_) | SourceHold::Image(_) => write.image_info(&image_info),
        }];
        // SAFETY: the set is live and not in use by any submit — nothing
        // can have been recorded against a binding that does not exist
        // yet. The write array and the info it points at are locals
        // outliving the call.
        unsafe { shared.device.update_descriptor_sets(&writes, &[]) };

        Ok(Binding {
            inner: Rc::new(BindingInner {
                shared: Rc::clone(shared),
                pool,
                set,
                source: source_hold,
                _sampler: match desc.kind {
                    Kind::Sampled(_, sampler) => Some(Rc::clone(&sampler.inner)),
                    Kind::Uniform(_) => None,
                },
            }),
        })
    }
}
