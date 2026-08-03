//! Per-frame buffers: the memory a frame's instance data rides in.
//!
//! One class of buffer exists — rewritten by the host every frame, read
//! by the frame being recorded — because exactly one consumer exists and
//! an enum arm no test can reach is a hole in the coverage gate, not
//! foresight. The memory is host-visible and coherent, written directly:
//! staging a buffer that changes every frame would add a copy and a
//! submit to the steady path, which is the reason the image upload path
//! and this one are different designs, not one design with a flag.
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
use crate::vk::offscreen::{creation, pick_memory_type};
use crate::vk::texture::no_memory_type;

/// One region per frame slot, sized by the compile-time ceiling both
/// targets share. The window target's ring depth equals it today; a
/// future creation-time depth chooses at most this many.
pub(crate) const MAX_FRAME_SLOTS: usize = 2;

/// Per-slot regions start on this alignment. Covers every vertex
/// attribute format's alignment with room to spare; the cost is at most
/// `MAX_FRAME_SLOTS * 63` bytes per buffer.
const SLOT_ALIGN: u64 = 64;

/// What a buffer is for. One variant today, deliberately: the only
/// consumer draws instanced quads from per-frame data, and an arm no
/// test can reach is a hole in the coverage gate, not foresight.
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
    /// Distance between slot regions, `>= capacity`, `SLOT_ALIGN`ed.
    /// The offscreen path is synchronous and lives in slot zero, so the
    /// only reader of a nonzero offset is the presentation path.
    #[cfg_attr(
        not(feature = "present"),
        allow(
            dead_code,
            reason = "read only by the presentation path's slot addressing"
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
        // Quiesces nothing, deliberately: a target retains a clone of
        // this `Rc` for every slot whose recorded work references the
        // buffer and releases it only when that work has provably ended
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
    /// [`TargetError`] naming the Vulkan call that refused.
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

        let shared = Rc::clone(&self.shared);
        let mut partial = Partial {
            shared: Rc::clone(&shared),
            buffer: None,
            memory: None,
        };

        let per_slot = (capacity as u64).div_ceil(SLOT_ALIGN) * SLOT_ALIGN;
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

        let info = vk::BufferCreateInfo::default()
            .size(total)
            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
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
