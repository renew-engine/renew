//! Per-frame buffers: the memory a frame's instance data rides in.
//!
//! One class of buffer lives *here* — rewritten by the host every frame,
//! read by the frame being recorded — because exactly one consumer
//! exists and an enum arm no test can reach is a hole in the coverage
//! gate, not foresight. The memory is host-visible and coherent, written
//! directly: staging a buffer that changes every frame would add a copy
//! and a submit to the steady path, which is the reason the image upload
//! path and this one are different designs, not one design with a flag.
//!
//! **Two reads, still one class.** A frame's bytes are read either as an
//! instance stream or as a uniform block, and that is a property of the
//! *draw*, not of the memory: both want host-visible coherent bytes in
//! one region per frame slot, written after the slot's fence is known
//! clear. So the buffer carries both usage bits and the slot stride
//! satisfies both alignments, rather than there being a second class
//! whose ring logic would have to be written twice.
//!
//! **That argument since produced a third design rather than a third
//! flag.** Geometry — written once at creation, drawn by any number of
//! items on any number of targets — lives in `mesh.rs`, with no ring, no
//! slot arithmetic, no copy phase and no owning target. Every rule stated
//! below would have had to grow the words "unless static" to cover it.
//!
//! # The ring lives in here, invisibly
//!
//! A frame ring means a slot's previous contents may still be feeding a
//! submit while the next frame writes. The classic answer — the caller
//! keeps one buffer per slot and writes the right one — is the published
//! protocol this crate already recorded as racing: the caller cannot
//! know when a slot is writable. So the multiplication happens here:
//! one allocation, one region per frame slot, and the target copies into
//! the region for the slot it is about to record, after that slot's
//! fence has been waited. No caller-visible slot arithmetic exists to
//! get wrong.
//!
//! # Who keeps it alive
//!
//! The caller's handle is `Rc`-backed, and a target retains a clone for
//! any slot whose recorded work references the buffer, releasing it only
//! when that work has provably ended. Dropping the last caller handle
//! after `render` returns therefore cannot free memory a submit still
//! reads — the same "without the caller having to sequence drops
//! correctly" reasoning the pipeline documents for its own resources.

use std::rc::Rc;

use ash::vk;

use crate::error::TargetError;
use crate::vk::device::{Device, DeviceShared};
// **A deliberate edge, stated here rather than hidden in a path.** This
// module is the lower half and `pass` is the frame vocabulary above it, so
// the arrow points the wrong way at first glance. It is here because the
// frame walk's module denies `unsafe` and the block copy writes through a
// mapping, so the write has to live on this side while the safe half —
// which bindings a draw writes — stays on that one. Written as an import
// so the dependency shows in the module's own header, the way
// `pipeline`'s longer-standing edge to `pass` does.
use crate::vk::offscreen::{creation, pick_memory_type};
use crate::vk::pass::{Item, MAX_RETAINED_RESOURCES, uniform_writes};
use crate::vk::texture::no_memory_type;

/// One region per frame slot, sized by the compile-time ceiling both
/// targets share. The window target's ring depth equals it today; a
/// future creation-time depth chooses at most this many.
pub(crate) const MAX_FRAME_SLOTS: usize = 2;

/// The floor per-slot regions start on. Covers every vertex attribute
/// format's alignment with room to spare.
///
/// A floor rather than the answer: a region read as a uniform block must
/// also start on the adapter's `minUniformBufferOffsetAlignment`, which
/// the guaranteed-worst-case adapter puts at 256. The stride is the
/// larger of the two, so the cost is at most `MAX_FRAME_SLOTS * 255`
/// bytes per buffer and a caller never has to know which read it will
/// be used for.
const SLOT_ALIGN: u64 = 64;

/// How long a buffer's contents live. One variant today, deliberately.
///
/// **Named for its lifetime, not for its reads.** A per-frame buffer may
/// be drawn as an instance stream or bound as a uniform block, and which
/// of those happens is decided by the pipeline and the item; the memory,
/// the ring and the copy rule are identical either way.
///
/// **Not one variant per *read*.** A per-frame buffer may be drawn as an
/// instance stream or bound as a uniform block, and which of those
/// happens is decided by the pipeline and the item, not here — the
/// memory, the ring and the copy rule are identical either way. A second
/// arm arrives with a second *lifetime*, which is what this enum is
/// actually about; geometry, the other lifetime that exists, already
/// lives in `mesh.rs` rather than as an arm here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BufferUsage {
    /// Rewritten every frame by the host; read by the frame being
    /// recorded. Mapped, host-visible, coherent.
    PerFrame,
}

/// The shared body a caller handle and a target's retain table both
/// point at. Fields are crate-visible: the render paths copy into the
/// mapping and bind the handle at a slot offset.
pub(crate) struct BufferInner {
    pub(crate) shared: Rc<DeviceShared>,
    pub(crate) buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    /// Host address of the whole mapped allocation.
    pub(crate) mapped: *mut u8,
    /// Distance between slot regions: `>= capacity`, and a multiple of
    /// `max(SLOT_ALIGN, minUniformBufferOffsetAlignment)`.
    ///
    /// **Not merely `SLOT_ALIGN`-aligned, and the difference is load-
    /// bearing.** A region read as a uniform block is reached by a dynamic
    /// offset of `slot_stride * slot`, and that offset is legal only when
    /// it is a multiple of the adapter's own uniform granularity. The whole
    /// correctness argument for the block channel rests on this field
    /// carrying the larger of the two alignments.
    /// The offscreen path is synchronous and lives in slot zero, so the
    /// only reader of a nonzero offset is the presentation path.
    #[cfg_attr(
        not(feature = "present"),
        allow(
            dead_code,
            reason = "read by the presentation path's slot addressing; the \
                      uniform-block paths that also read it are compiled \
                      unconditionally, so this only fires for a build with \
                      neither"
        )
    )]
    pub(crate) slot_stride: u64,
    /// Per-frame capacity in bytes, as the caller asked for it.
    pub(crate) capacity: usize,
    /// The one target this buffer may be used with, recorded on first
    /// use. Slot regions are semantically owned by whichever target last
    /// submitted against them; a second target's ring would race the
    /// first's reads with no fence relating them.
    pub(crate) owner: core::cell::Cell<Option<usize>>,
}

impl Drop for BufferInner {
    fn drop(&mut self) {
        // Quiesces nothing, deliberately: a target holds this `Rc` alive
        // for every slot whose recorded work references the buffer, and
        // releases it only when that work has provably ended. Directly,
        // for an instance stream the item names; transitively for a
        // uniform block, whose binding the target retains and whose
        // binding holds this — the conclusion is the same and the chain
        // is one link longer
        // (fence wait succeeded, quiesce succeeded, or device lost), so
        // this destructor is unreachable while any submit can read the
        // memory. A wait here would be the per-drop `vkDeviceWaitIdle`
        // the fence-teardown convention exists to avoid piling up.
        // SAFETY: handles live and unused — retention is the argument
        // above; the mapping dies with the memory, so no explicit unmap.
        unsafe {
            self.shared
                .device
                .destroy_buffer(self.buffer, Some(&self.shared.alloc_cbs()));
            self.shared
                .device
                .free_memory(self.memory, Some(&self.shared.alloc_cbs()));
        }
    }
}

/// A per-frame buffer. Cheap to clone a reference to; the handle owns
/// nothing the target is not also keeping alive while it is in use.
pub struct Buffer {
    pub(crate) inner: Rc<BufferInner>,
}

impl Buffer {
    /// Per-frame capacity in bytes, as fixed at creation.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }
}

impl core::fmt::Debug for Buffer {
    /// Capacity, not addresses: a Vulkan handle's value is not
    /// information, and the mapped pointer is an invitation to print it.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Buffer")
            .field("capacity", &self.inner.capacity)
            .finish_non_exhaustive()
    }
}

/// Creation-failure unwinder, the texture path's convention: fields fill
/// as calls succeed, and dropping it destroys exactly what exists so an
/// early `?` cannot leak a handle. Disarmed by `take`-ing on success.
struct Partial {
    shared: Rc<DeviceShared>,
    buffer: Option<vk::Buffer>,
    memory: Option<vk::DeviceMemory>,
}

impl Drop for Partial {
    fn drop(&mut self) {
        // SAFETY: only handles this construction created exist here, and
        // nothing has been submitted that could reference them.
        unsafe {
            if let Some(memory) = self.memory.take() {
                self.shared
                    .device
                    .free_memory(memory, Some(&self.shared.alloc_cbs()));
            }
            if let Some(buffer) = self.buffer.take() {
                self.shared
                    .device
                    .destroy_buffer(buffer, Some(&self.shared.alloc_cbs()));
            }
        }
    }
}

impl Device {
    /// A buffer holding `capacity` bytes of per-frame data.
    ///
    /// Capacity is per frame and fixed here — this is where the
    /// allocation happens, so this is where the bound belongs. The
    /// allocation itself is one region per frame slot; which region a
    /// frame writes is the target's business, decided after the slot's
    /// fence has been waited, and never the caller's.
    ///
    /// # Errors
    ///
    /// [`TargetError`] naming the Vulkan call that refused;
    /// [`TargetError::DeviceLost`] on a poisoned device.
    ///
    /// Among those refusals is *no suitable memory type*, which this path
    /// can now reach on an adapter it previously could not: the buffer
    /// carries both the vertex and the uniform usage bits, so the memory
    /// types the driver will accept for it may be narrower than for a
    /// vertex-only buffer. Unlikely on desktop, not impossible under a
    /// translation layer.
    ///
    /// # Panics
    ///
    /// A zero `capacity` is a contract violation and panics through a
    /// retained assertion — an empty per-frame region has no meaning a
    /// draw could consume, and the capacity bounds every later copy, so
    /// the check survives release builds.
    pub fn create_buffer(
        &self,
        capacity: usize,
        usage: BufferUsage,
    ) -> Result<Buffer, TargetError> {
        // One variant exists; naming it keeps the parameter honest and
        // the call sites readable, and the match is where variant two
        // will land its differences.
        let BufferUsage::PerFrame = usage;
        // Contract violations are assertions, and this one is retained
        // in release per the memory-safety-boundary rule: the capacity
        // bounds every later copy into the mapping.
        assert!(capacity > 0, "a per-frame buffer needs a non-zero capacity");

        // The larger of the two alignments this memory must satisfy: the
        // vertex floor above, and the adapter's own uniform-offset
        // granularity, because a dynamic offset that is not a multiple
        // of it is a usage violation rather than a slow path.
        let align = SLOT_ALIGN.max(self.shared.uniform_offset_alignment.max(1));
        let per_slot = (capacity as u64).div_ceil(align) * align;
        let total = per_slot.checked_mul(MAX_FRAME_SLOTS as u64);
        // Same boundary, same retained refusal: an overflowed size would
        // hand the driver a wrapped number with the caller's bytes later
        // copied against the unwrapped one.
        assert!(
            total.is_some(),
            "per-frame capacity overflows the allocation size"
        );
        let slot_stride = per_slot;
        let total = total.unwrap_or(0);

        // Gated like every other resource constructor in this crate, and
        // after the contract checks for the same reason they are: a
        // malformed request is reported as malformed even on a dead
        // device, because that is the caller's bug either way.
        //
        // **The gate is the only thing that refuses here.** None of the
        // four driver calls below lists `VK_ERROR_DEVICE_LOST` among its
        // return codes, so without this the likely outcome after a loss
        // was not a laundered error — it was success, handing back a live
        // buffer on a dead device while every sibling refused.
        if self.shared.lost.poisoned() {
            return Err(TargetError::DeviceLost);
        }

        let shared = Rc::clone(&self.shared);
        let mut partial = Partial {
            shared: Rc::clone(&shared),
            buffer: None,
            memory: None,
        };

        let info = vk::BufferCreateInfo::default()
            .size(total)
            .usage(vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::UNIFORM_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: device live; info local.
        let buffer = unsafe {
            shared
                .device
                .create_buffer(&info, Some(&shared.alloc_cbs()))
        }
        .map_err(|code| creation("vkCreateBuffer", code))?;
        partial.buffer = Some(buffer);

        // SAFETY: buffer live.
        let requirements = unsafe { shared.device.get_buffer_memory_requirements(buffer) };
        // SAFETY: instance and physical device live via the spine.
        let memory_properties = unsafe {
            shared
                .instance
                .get_physical_device_memory_properties(shared.physical)
        };
        let memory_type = pick_memory_type(
            &memory_properties,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| no_memory_type("per-frame buffer memory type"))?;

        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type);
        // SAFETY: device live; info local.
        let memory = unsafe {
            shared
                .device
                .allocate_memory(&alloc, Some(&shared.alloc_cbs()))
        }
        .map_err(|code| creation("vkAllocateMemory", code))?;
        partial.memory = Some(memory);

        // SAFETY: buffer and memory live; offset 0 within an allocation
        // sized from this buffer's own requirements.
        unsafe { shared.device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|code| creation("vkBindBufferMemory", code))?;

        // SAFETY: memory live, HOST_VISIBLE, not already mapped;
        // WHOLE_SIZE maps the full allocation, which stays mapped for
        // the buffer's whole life — HOST_COHERENT, so writes need no
        // flush and the map needs no re-establishment per frame.
        let mapped = unsafe {
            shared
                .device
                .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
        }
        .map_err(|code| creation("vkMapMemory", code))?;

        // Success: hand everything to the real owner and disarm the
        // unwinder by forgetting its fields were ever set.
        partial.buffer = None;
        partial.memory = None;
        drop(partial);

        Ok(Buffer {
            inner: Rc::new(BufferInner {
                shared,
                buffer,
                memory,
                mapped: mapped.cast::<u8>(),
                slot_stride,
                capacity,
                owner: core::cell::Cell::new(None),
            }),
        })
    }
}

/// Which block buffers a frame has already written, so a block named by
/// several items is copied once rather than once per item.
///
/// **The frame contract makes this an optimisation rather than a
/// correctness fix**: it has already refused two items carrying *different*
/// bytes for one buffer, so every repeat writes identical bytes to an
/// identical address and skipping it cannot change what the GPU reads.
///
/// It is worth having anyway, because the shape it saves is the common
/// one. A camera block read by every draw in a frame is the motivating
/// consumer: sixty-four draws over a sixteen-kilobyte block was a megabyte
/// of host writes into one region, and the memory is whatever
/// `HOST_VISIBLE | HOST_COHERENT` type the adapter offered first — which on
/// some configurations is uncached and slow to write twice.
///
/// Fixed width, sized by the frame's own distinct-resource ceiling, so the
/// frame path still allocates nothing.
pub(crate) struct BlockWrites {
    seen: [Option<*const u8>; MAX_RETAINED_RESOURCES],
    count: usize,
}

impl BlockWrites {
    /// Nothing written yet — one per `render` call.
    pub(crate) const fn new() -> Self {
        Self {
            seen: [None; crate::vk::pass::MAX_RETAINED_RESOURCES],
            count: 0,
        }
    }

    /// Claim `key`, answering whether this frame had already written it.
    ///
    /// A full table answers "already written", which is the safe direction
    /// and unreachable besides: the frame contract bounded the distinct
    /// count below this width before any of this ran.
    fn first_time(&mut self, key: *const u8) -> bool {
        if self.seen[..self.count].contains(&Some(key)) {
            return false;
        }
        let Some(slot) = self.seen.get_mut(self.count) else {
            return false;
        };
        *slot = Some(key);
        self.count += 1;
        true
    }
}

/// Copy an item's uniform blocks into `slot`'s region of the buffers
/// their bindings read.
///
/// **One implementation, called by both targets**, for the reason the
/// binding bind is one: the slot arithmetic here and the dynamic offset
/// there are one agreement, and two copies of it drift. `owner` is the
/// calling target's identity, for the one-target rule per-frame buffers
/// carry.
///
/// It lives here rather than beside the frame walk because the walk's
/// module denies `unsafe` and this writes through a mapping; the safe
/// half — which bindings a draw writes — stays there as
/// [`uniform_writes`](crate::vk::pass::uniform_writes).
///
/// # Panics
///
/// Bytes longer than the buffer's per-frame capacity — retained, because
/// the length bounds a write into mapped device memory. The frame
/// contract already refused the case by name; this is the memory-safety
/// boundary behind it.
///
/// # Safety
///
/// Two obligations, and the second went unstated until a review asked
/// for it.
///
/// - No submit may be reading `slot`'s regions: the tail wait on the
///   offscreen path, the per-slot fence wait on the presentation one. The
///   caller must also hold every named binding alive past the submit it is
///   about to record.
/// - **`slot` must be less than [`MAX_FRAME_SLOTS`].** The pointer
///   arithmetic below is bounded by it and by nothing else, so a caller
///   passing a larger slot writes past the allocation. The presentation
///   target's `FRAMES_IN_FLIGHT` and this module's `MAX_FRAME_SLOTS` are
///   two independent literals with no compile-time link between them —
///   raising one without the other turns this into an out-of-bounds write
///   into mapped memory, which is why the bound is now named here and
///   checked below rather than left to the two constants agreeing.
pub(crate) unsafe fn write_uniform_blocks(
    item: &Item<'_>,
    shared: &Rc<DeviceShared>,
    owner: usize,
    slot: usize,
    written: &mut BlockWrites,
) {
    debug_assert!(
        slot < MAX_FRAME_SLOTS,
        "a frame slot past the ring's depth would write outside the allocation"
    );
    for (buffer, bytes) in uniform_writes(item) {
        if !written.first_time(Rc::as_ptr(buffer).cast::<u8>()) {
            // Written by an earlier item this frame, with bytes the
            // contract proved identical. Copying again would be the same
            // memcpy to the same address.
            continue;
        }
        debug_assert!(
            Rc::ptr_eq(&buffer.shared, shared),
            "uniform buffer and target come from different devices"
        );
        match buffer.owner.get() {
            None => buffer.owner.set(Some(owner)),
            // **Retained, unlike the instance path's twin.** Two targets
            // writing one buffer's slot regions race with no fence
            // relating them — a torn read and a wrong picture, reported
            // by nothing. That was survivable while per-frame buffers
            // were instance streams, which are target-local in practice;
            // a block is the opposite, exactly the resource an app shares
            // between a window and an offscreen preview. Debug-only here
            // would leave the case it was written for unguarded in the
            // builds that ship.
            //
            // The token is the target's address, which cannot tell a
            // moved target from a different one — recorded as debt rather
            // than fixed here, because a monotonic target id is a change
            // to both targets and to the instance path beside them.
            Some(held) => assert!(
                held == owner,
                "a per-frame buffer belongs to one target: its slot regions are owned by \
                 whichever target last submitted against them"
            ),
        }
        assert!(
            bytes.len() <= buffer.capacity,
            "uniform data exceeds the block buffer's per-frame capacity"
        );
        // The stride is alignment-rounded and the slot count is two, so
        // the product is far inside usize on every supported target.
        #[allow(clippy::cast_possible_truncation)]
        let at = (buffer.slot_stride * slot as u64) as usize;
        // SAFETY: the mapping covers `slot_stride * MAX_FRAME_SLOTS`
        // bytes; `slot < MAX_FRAME_SLOTS` and the assert above bounds the
        // length within one region, so the write stays inside the
        // allocation and cannot touch a neighbouring slot. The caller's
        // obligation covers the "no submit reads this" half, and the
        // memory is HOST_COHERENT, so no flush.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.mapped.add(at), bytes.len());
        }
    }
}
