//! Geometry: vertex and index bytes written once at creation, read-only
//! to the GPU for the rest of their life.
//!
//! **This is a third buffer design, not a third flag on the first.**
//! `buffer.rs` already argues that the per-frame path and the image
//! upload path are different designs rather than one design with a flag.
//! Geometry is the third. A per-frame buffer is a ring — one region per
//! frame slot, a copy phase ordered behind a fence wait, a single owning
//! target — because its contents change every frame. A mesh's contents
//! never change, so it has no ring, no slot arithmetic, no copy phase and
//! no owning target, and it may be drawn by any number of items on any
//! number of targets. Every rule stated on `BufferUsage::PerFrame` would
//! have had to grow the words "unless static".
//!
//! # Immutability buys a proof, not just tidiness
//!
//! Because the bytes arrive exactly once, this module can afford to walk
//! every index and refuse the mesh if any of them points past the last
//! vertex. A buffer rewritten every frame could never pay that per frame.
//! Together with two cheaper facts — the vertex slice divides evenly by
//! its stride, and the drawing pipeline's derived stride equals the
//! mesh's — that scan makes every vertex fetch provably inside the
//! allocation.
//!
//! **That matters because nothing else here can see it.** An index past
//! the end of the vertex stream is *data*, and the validation layer does
//! not read index-buffer contents; this crate enables no GPU-assisted
//! validation. So an out-of-range index has no oracle anywhere in this
//! repository — it draws a plausible wrong picture, or reads memory the
//! draw was never given. Creation is the only instant at which both
//! halves are known and immutability guarantees neither changes
//! afterwards, which is the reason vertices and indices arrive in one
//! call rather than two.
//!
//! # What immutability does NOT buy
//!
//! It does not answer when the memory may die. [`Texture`] gets that for
//! free because the pipeline holds an `Rc` to it from creation; a mesh is
//! named per draw and **nothing holds it at record time**. So a mesh is
//! kept alive exactly the way a per-frame buffer is: a target retains a
//! clone for any slot whose recorded work references it, released only
//! when that work has provably ended. Stated here because "immutable like
//! a texture, so `Drop` needs no quiesce" is a wrong inference that reads
//! correct.
//!
//! [`Texture`]: crate::Texture
//!
//! # One allocation, two regions
//!
//! Vertices at offset zero, indices at the next four-aligned offset after
//! them, in one buffer carrying both usage flags. Two Vulkan buffers
//! would double the creation ladder, double the fault surface, and make
//! one mesh two entries in a retention table whose width is a stated
//! contract.
//!
//! # The memory class is a decision, and it is recorded elsewhere
//!
//! Host-visible and coherent, written directly — not device-local behind
//! a staging copy. That is the standing decision for static buffers, and
//! its reopening trigger is a real-GPU frame-time measurement showing
//! vertex fetch matters; the lane that gates merges is a software
//! rasterizer, where the distinction does not exist. **No public
//! signature below names a memory class**, so making that change later
//! lands entirely inside this module.
//!
//! The host writes are ordered before every later draw by the queue
//! submission itself: a submit defines a memory dependency with prior
//! host writes to mappable memory, and the memory is coherent, so there
//! is no flush and no barrier to record. This is why the geometry path
//! has no command buffer, no fence and no transfer submit at all.

use std::rc::Rc;

use ash::vk;

use crate::error::TargetError;
use crate::vk::device::{Device, DeviceShared};
use crate::vk::offscreen::{creation, pick_memory_type};
use crate::vk::texture::no_memory_type;

/// Index elements are `u32`, so the index region starts on a four-byte
/// boundary after the vertex region. Vulkan requires an index-buffer
/// bind offset to be a multiple of the index size.
const INDEX_ALIGN: usize = 4;

/// The bytes of one mesh: a vertex stream, the stride that divides it,
/// and the indices that walk it.
///
/// **`#[non_exhaustive]` with a positional constructor, per the
/// descriptor pattern this crate uses everywhere** — a second vertex
/// stream, a primitive topology, or a narrower index element arrive as
/// builders touching no existing caller.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct MeshDesc<'a> {
    /// Vertex records, tightly packed, back to back.
    ///
    /// Opaque bytes: what the fields mean is the drawing pipeline's
    /// declaration, not this crate's business. The length must be a
    /// whole non-zero multiple of [`Self::vertex_stride`].
    pub vertices: &'a [u8],
    /// Distance between consecutive vertex records, in bytes.
    ///
    /// Must equal the packed stride the drawing pipeline derives from
    /// its per-vertex attribute list. That agreement is asserted where
    /// the draw is recorded rather than here, because only there are
    /// both halves in hand — and it is a retained assertion, because a
    /// stride mismatch is a fetch past the end of this allocation rather
    /// than merely a wrong picture.
    pub vertex_stride: u32,
    /// Indices into the vertex stream, in draw order.
    ///
    /// **Typed `u32` rather than bytes, and that is the whole reason
    /// there is no index-format enum.** A byte slice would need a format
    /// beside it; the two would be values that compile in any
    /// combination, and the wrong pairing draws garbage silently. Given
    /// the type, the count is `len()` and the format is unspellable. A
    /// narrower element halves this for meshes under 65 536 vertices and
    /// is a later change; its trigger is a measured index-bandwidth
    /// figure, not the arithmetic that it would be smaller.
    pub indices: &'a [u32],
}

impl<'a> MeshDesc<'a> {
    /// `vertices` divided into records of `vertex_stride` bytes, walked
    /// by `indices`.
    ///
    /// Positional because none of the three has a meaningful absence: a
    /// mesh with no vertices, no stride, or no indices is not a
    /// partially-configured mesh, it is not a mesh.
    #[must_use]
    pub fn new(vertices: &'a [u8], vertex_stride: u32, indices: &'a [u32]) -> Self {
        Self {
            vertices,
            vertex_stride,
            indices,
        }
    }
}

/// What `check_desc` proved, so the caller does not recompute it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    /// How many whole vertex records the stream holds.
    pub(crate) vertex_count: u32,
    /// Byte offset of the index region within the one allocation.
    pub(crate) index_offset: u64,
    /// Total allocation size: the index region's end.
    pub(crate) total: u64,
}

/// The descriptor contract as a value, so the release-build verdict is
/// provable: a dev build asserts long before the returned error could be
/// observed, and every test runs with assertions on.
///
/// **Every arm inspects caller-supplied data**, which is why this returns
/// `Err` rather than asserting: a mesh's bytes are content, and the
/// error-handling standard reserves assertions for our own code being
/// wrong. The index scan is the expensive arm and it is affordable
/// exactly once, here, because the bytes never change again.
pub(crate) fn check_desc(desc: &MeshDesc<'_>) -> Result<Layout, TargetError> {
    let layout = layout_for(desc.vertices.len(), desc.vertex_stride, desc.indices.len())?;
    // The scan this whole design exists to afford, and the one rule that
    // needs the bytes rather than their lengths. `>=` rather than `>`: an
    // index equal to the count addresses the record one past the end.
    if desc
        .indices
        .iter()
        .any(|&index| index >= layout.vertex_count)
    {
        return Err(TargetError::Creation {
            call: "create_mesh(index past the last vertex)",
            code: 0,
        });
    }
    Ok(layout)
}

/// Every rule that depends only on the *lengths*, separated from the one
/// that needs the bytes.
///
/// **Split out so its refusals are reachable without allocating them.**
/// Three of the arms below fire only for slices of billions of elements,
/// which no test can build — and an arm no test can reach is a hole in
/// the coverage gate rather than foresight. Taking lengths as plain
/// numbers makes every one of them a two-line assertion, which is the
/// same reason `texture.rs` factors `required_bytes` out of its own
/// descriptor check.
fn layout_for(
    vertex_bytes: usize,
    vertex_stride: u32,
    index_count: usize,
) -> Result<Layout, TargetError> {
    if vertex_stride == 0 {
        return Err(TargetError::Creation {
            call: "create_mesh(zero vertex stride)",
            code: 0,
        });
    }
    if vertex_bytes == 0 {
        return Err(TargetError::Creation {
            call: "create_mesh(no vertices)",
            code: 0,
        });
    }
    let stride = vertex_stride as usize;
    if !vertex_bytes.is_multiple_of(stride) {
        return Err(TargetError::Creation {
            call: "create_mesh(vertex length is not a whole number of records)",
            code: 0,
        });
    }
    if index_count == 0 {
        return Err(TargetError::Creation {
            call: "create_mesh(no indices)",
            code: 0,
        });
    }
    let vertex_count = u32::try_from(vertex_bytes / stride).map_err(|_| TargetError::Creation {
        call: "create_mesh(vertex count exceeds u32)",
        code: 0,
    })?;
    // The draw's index count is a `u32`, so a longer list could not be
    // issued even if it could be allocated.
    let index_count = u32::try_from(index_count).map_err(|_| TargetError::Creation {
        call: "create_mesh(index count exceeds u32)",
        code: 0,
    })?;
    // **Computed from the two counts rather than from the byte length,
    // and every conversion here is infallible.** `vertex_count * stride`
    // is exactly `vertex_bytes` — the divisibility check above proved it
    // — and both factors are `u32`, so the product is at most
    // `(2^32 - 1)^2`, which is below `u64::MAX` by more than the rounding
    // and the index region can add. Deriving it this way rather than
    // widening the `usize` is what removes two refusal arms that the
    // count guard above had already made unreachable: after it, no
    // surviving input can overflow the alignment.
    let index_offset = (u64::from(vertex_count) * u64::from(vertex_stride)).next_multiple_of(4);
    let index_bytes = u64::from(index_count) * 4;
    // This one survives, and is reachable: the widest legal mesh puts the
    // index region past the top of a `u64`. An overflowed total would
    // hand the driver a wrapped size while the copies used the unwrapped
    // one.
    let total = index_offset
        .checked_add(index_bytes)
        .ok_or(TargetError::Creation {
            call: "create_mesh(total size overflows)",
            code: 0,
        })?;
    Ok(Layout {
        vertex_count,
        index_offset,
        total,
    })
}

/// The shared body a caller handle and a target's retain table both point
/// at. Fields are crate-visible: the record paths bind the handle and
/// issue the indexed draw.
pub(crate) struct MeshInner {
    pub(crate) shared: Rc<DeviceShared>,
    pub(crate) buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    /// Byte offset of the index region within [`Self::buffer`].
    pub(crate) index_offset: u64,
    /// Indices, which is what an indexed draw counts.
    pub(crate) index_count: u32,
    /// Whole vertex records — not a draw parameter, kept for `Debug` and
    /// for the in-bounds argument this module states.
    pub(crate) vertex_count: u32,
    /// The stride the drawing pipeline must agree with.
    pub(crate) vertex_stride: u32,
}

impl Drop for MeshInner {
    fn drop(&mut self) {
        // Quiesces nothing, deliberately, and NOT on the texture's
        // immutability argument — that one does not transfer. A texture
        // survives because the pipeline holds an `Rc` to it from
        // creation; nothing holds a mesh at record time. What makes this
        // destructor unreachable while a submit can read the memory is
        // retention: a target keeps a clone of this `Rc` for every slot
        // whose recorded work references the mesh, and releases it only
        // when that work has provably ended. A wait here would be the
        // per-drop `vkDeviceWaitIdle` the fence-teardown convention
        // exists to avoid piling up.
        //
        // SAFETY: category 2 (ash dispatch): device live via the spine
        // `Rc`; both handles were created here with these callbacks and
        // are destroyed once; the mapping was released at creation, so
        // there is nothing to unmap.
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

/// Geometry the GPU can draw. Holds its device alive; destroyed on drop.
///
/// # Contract
///
/// A mesh's bytes never change after creation, and a mesh may only be
/// drawn by a pipeline whose per-vertex stride equals the stride it was
/// built with.
///
/// **Immutability is structural rather than promised.** The mapping is
/// released before this value exists, so no pointer into the memory
/// survives anywhere in the process — there is no write to forbid.
///
/// Cheap to clone a reference to, and a mesh may be named by any number
/// of items, in any number of passes, on any number of targets. That is
/// the difference from a per-frame buffer, whose slot regions belong to
/// whichever target last submitted against them.
pub struct Mesh {
    pub(crate) inner: Rc<MeshInner>,
}

impl Mesh {
    /// Whole vertex records the stream holds.
    #[must_use]
    pub fn vertex_count(&self) -> u32 {
        self.inner.vertex_count
    }

    /// Indices, which is how many the draw issues.
    #[must_use]
    pub fn index_count(&self) -> u32 {
        self.inner.index_count
    }

    /// Bytes between consecutive vertex records, as fixed at creation.
    #[must_use]
    pub fn vertex_stride(&self) -> u32 {
        self.inner.vertex_stride
    }
}

impl core::fmt::Debug for Mesh {
    /// Counts, not addresses: a Vulkan handle's value is not
    /// information.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Mesh")
            .field("vertex_count", &self.inner.vertex_count)
            .field("index_count", &self.inner.index_count)
            .field("vertex_stride", &self.inner.vertex_stride)
            .finish_non_exhaustive()
    }
}

/// Creation-failure unwinder, the convention both other buffer paths
/// use: fields fill as calls succeed, and dropping it destroys exactly
/// what exists so an early `?` cannot leak a handle. Disarmed by clearing
/// its fields on success.
/// **No `mapped` flag, unlike the offscreen target's unwinder.** There
/// nothing fallible follows the map, either — but the mapping lives for
/// the target's whole life, so its `Drop` must unmap. Here the mapping is
/// released inside `create_mesh` before the handle exists, and no
/// fallible step sits between the map and that release, so a flag would
/// have exactly one value at every point this type can be dropped: a
/// branch no test could reach.
struct Partial {
    shared: Rc<DeviceShared>,
    buffer: Option<vk::Buffer>,
    memory: Option<vk::DeviceMemory>,
}

impl Drop for Partial {
    fn drop(&mut self) {
        // SAFETY: category 2 (ash dispatch): only handles this
        // construction created exist here, each made with these
        // callbacks, and nothing has been submitted that could reference
        // them. Freeing memory that is still mapped is defined — the
        // mapping ends with the allocation — and on every path that
        // reaches here it is not mapped anyway.
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
    /// Create a mesh and fill it from host bytes.
    ///
    /// The bytes are copied during this call and never read again, so
    /// `desc` may be dropped the moment it returns.
    ///
    /// # Errors
    ///
    /// [`TargetError::Creation`] if `desc` is malformed — a zero stride,
    /// an empty stream, a vertex length that is not a whole number of
    /// records, **an index pointing past the last vertex**, or arithmetic
    /// that overflows — each naming the rule it broke, so a caller learns
    /// which one rather than that creation failed. Driver refusals carry
    /// the Vulkan call that returned them;
    /// [`TargetError::OutOfDeviceMemory`] when an allocation fails for
    /// want of device memory; [`TargetError::DeviceLost`] on a poisoned
    /// device.
    ///
    /// # Panics
    ///
    /// In a dev build, if `desc` is malformed. The returned error is the
    /// release-build verdict for the same condition.
    pub fn create_mesh(&self, desc: &MeshDesc<'_>) -> Result<Mesh, TargetError> {
        // Fatal in dev builds; in release, where the assertion is
        // compiled out, the same verdict is returned instead. Every value
        // in the message is bound first, so no call hides in a region
        // that runs only when the assertion fails.
        let checked = check_desc(desc);
        let vertices = desc.vertices.len();
        let stride = desc.vertex_stride;
        let indices = desc.indices.len();
        debug_assert!(
            checked.is_ok(),
            "a mesh needs a non-zero stride dividing its vertex bytes and indices inside its \
             vertex count; got {vertices} bytes at stride {stride} with {indices} indices"
        );
        let layout = checked?;
        // Gated like every other resource constructor in this crate.
        // `create_buffer` is the one that does not, which is a recorded
        // inconsistency rather than a precedent to copy forward.
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
            .size(layout.total)
            // Both usages on one buffer: the two regions live in one
            // allocation, and a bind names the region by offset.
            .usage(vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::INDEX_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: category 2 (ash dispatch): device live via the spine;
        // the create info is a local outliving the call. (The same
        // argument covers every dispatch call in this function.)
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
        .ok_or_else(|| no_memory_type("mesh memory type"))?;

        // The driver's own size, never host arithmetic: `layout.total`
        // is what the regions need, and this is what the allocation must
        // be.
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type);
        // SAFETY: device live; info local.
        let memory = unsafe {
            shared
                .device
                .allocate_memory(&alloc, Some(&shared.alloc_cbs()))
        }
        .map_err(|code| creation("vkAllocateMemory(mesh)", code))?;
        partial.memory = Some(memory);

        // SAFETY: buffer and memory live; offset 0 within an allocation
        // sized from this buffer's own requirements.
        unsafe { shared.device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|code| creation("vkBindBufferMemory", code))?;

        // SAFETY: memory live, HOST_VISIBLE, not already mapped;
        // WHOLE_SIZE maps the full allocation.
        let mapped = unsafe {
            shared
                .device
                .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
        }
        .map_err(|code| creation("vkMapMemory", code))?;

        // SAFETY: a mapped-memory write, carrying its argument in full
        // rather than a category label — the same shape the crate's three
        // existing mapped writes use, the granted category naming only
        // the readback site. The mapping covers
        // the whole allocation, whose size is at least `layout.total`;
        // `check_desc` proved the vertex bytes end at or before
        // `index_offset` and the index bytes end at `total`, so both
        // writes stay inside it; source and destination cannot overlap,
        // the mapping being a fresh device allocation; the memory is
        // HOST_COHERENT, so no flush is needed, and the submit that later
        // reads it defines a memory dependency with these host writes.
        // No slice over driver memory outlives this block.
        unsafe {
            let base = mapped.cast::<u8>();
            std::ptr::copy_nonoverlapping(desc.vertices.as_ptr(), base, desc.vertices.len());
            // `u32` has no padding, and the GPU reads the same
            // native-endian bytes the host wrote.
            std::ptr::copy_nonoverlapping(
                desc.indices.as_ptr().cast::<u8>(),
                base.add(usize::try_from(layout.index_offset).unwrap_or(usize::MAX)),
                desc.indices.len() * INDEX_ALIGN,
            );
        }

        // **The unmap is load-bearing, not tidiness.** After it, no
        // pointer into this allocation exists anywhere in the process,
        // which is what makes "the bytes never change" a structural fact
        // rather than a promise.
        // SAFETY: memory live and mapped by this function; no slice over
        // it survives the block above.
        unsafe { shared.device.unmap_memory(memory) };

        // Success: hand everything to the real owner and disarm the
        // unwinder by forgetting its fields were ever set.
        partial.buffer = None;
        partial.memory = None;
        drop(partial);

        Ok(Mesh {
            inner: Rc::new(MeshInner {
                shared,
                buffer,
                memory,
                index_offset: layout.index_offset,
                index_count: u32::try_from(desc.indices.len()).unwrap_or(u32::MAX),
                vertex_count: layout.vertex_count,
                vertex_stride: desc.vertex_stride,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every malformed descriptor names itself in the error, so a caller
    /// reading the failure learns which rule they broke rather than that
    /// creation failed. No device involved, so this runs everywhere —
    /// including under the sanitizer and Miri lanes.
    #[test]
    fn every_malformed_mesh_is_rejected_by_name() {
        let bytes = [0u8; 24];
        let good = [0u32, 1, 2];

        let cases: [(MeshDesc<'_>, &str); 6] = [
            (
                MeshDesc::new(&bytes, 0, &good),
                "create_mesh(zero vertex stride)",
            ),
            (MeshDesc::new(&[], 8, &good), "create_mesh(no vertices)"),
            (
                MeshDesc::new(&bytes[..23], 8, &good),
                "create_mesh(vertex length is not a whole number of records)",
            ),
            (MeshDesc::new(&bytes, 8, &[]), "create_mesh(no indices)"),
            (
                MeshDesc::new(&bytes, 8, &[3]),
                "create_mesh(index past the last vertex)",
            ),
            (
                MeshDesc::new(&bytes, 8, &[u32::MAX]),
                "create_mesh(index past the last vertex)",
            ),
        ];
        for (desc, expected) in cases {
            match check_desc(&desc) {
                Err(TargetError::Creation { call, code: 0 }) => {
                    assert_eq!(call, expected, "wrong rule named for {desc:?}");
                }
                other => panic!("{desc:?} should be refused as {expected}, got {other:?}"),
            }
        }
    }

    /// The scan's boundary, pinned on both sides on a stream of more than
    /// one vertex so an off-by-one cannot pass by coincidence.
    #[test]
    fn the_last_vertex_is_addressable_and_the_one_after_it_is_not() {
        let bytes = [0u8; 24];
        let layout = check_desc(&MeshDesc::new(&bytes, 8, &[0, 1, 2]))
            .expect("three records, indices 0..=2");
        assert_eq!(layout.vertex_count, 3);
        assert!(
            check_desc(&MeshDesc::new(&bytes, 8, &[0, 1, 3])).is_err(),
            "index 3 addresses a fourth record that does not exist"
        );
    }

    /// The index region starts four-aligned, which Vulkan requires of an
    /// index-buffer bind offset. Pinned for a ragged vertex length, where
    /// padding is actually inserted, as well as for one already aligned.
    #[test]
    fn the_index_region_starts_four_aligned() {
        let aligned = [0u8; 84];
        let layout = check_desc(&MeshDesc::new(&aligned, 28, &[0, 1, 2]))
            .expect("28 * 3 is already 4-aligned");
        assert_eq!(layout.index_offset, 84);
        assert_eq!(layout.total, 84 + 12);

        let ragged = [0u8; 18];
        let layout = check_desc(&MeshDesc::new(&ragged, 6, &[0, 1, 2])).expect("6 * 3 = 18");
        assert_eq!(
            layout.index_offset, 20,
            "18 rounds up to the next multiple of 4"
        );
        assert_eq!(layout.total, 32);
    }

    /// The counts are checked arithmetic, and every refusal is reached.
    ///
    /// **This is what the length/bytes split buys.** Each case below needs
    /// a slice of billions of elements to reach through the public API;
    /// as plain numbers they are one line each, so none of these arms is
    /// an exemption in the coverage manifest.
    #[test]
    fn every_arithmetic_limit_is_refused_by_name() {
        const U32_MAX: usize = u32::MAX as usize;
        let cases: [(usize, u32, usize, &str); 3] = [
            // More whole records than a `u32` can count.
            (
                usize::MAX / 2 + 1,
                1,
                3,
                "create_mesh(vertex count exceeds u32)",
            ),
            // More indices than an indexed draw can issue.
            (
                28,
                28,
                usize::MAX / 2,
                "create_mesh(index count exceeds u32)",
            ),
            // The widest legal mesh: `u32::MAX` records of `u32::MAX`
            // bytes puts the index region past the top of a `u64`. Both
            // counts are individually legal, which is what makes this the
            // one arithmetic refusal that survives the guards above.
            (
                U32_MAX * U32_MAX,
                u32::MAX,
                U32_MAX,
                "create_mesh(total size overflows)",
            ),
        ];
        for (vertex_bytes, stride, index_count, expected) in cases {
            let outcome = layout_for(vertex_bytes, stride, index_count);
            assert!(
                matches!(&outcome, Err(TargetError::Creation { call, code: 0 }) if *call == expected),
                "({vertex_bytes}, {stride}, {index_count}) should be refused as {expected}, \
                 got {outcome:?}"
            );
        }
        // And the ordinary case still computes, so the guards above are
        // not simply refusing everything.
        let layout = layout_for(84, 28, 3).expect("three records of 28 bytes, three indices");
        assert_eq!(layout.vertex_count, 3);
        assert_eq!(layout.total, 96);
    }

    /// The index scan's boundary through the public descriptor, which is
    /// the half `layout_for` cannot see.
    #[test]
    fn the_scan_reads_the_indices_and_not_only_their_count() {
        let bytes = [0u8; 8];
        assert!(check_desc(&MeshDesc::new(&bytes, 1, &[7])).is_ok());
        assert!(
            check_desc(&MeshDesc::new(&bytes, 1, &[8])).is_err(),
            "eight records means indices 0..=7"
        );
    }
}
