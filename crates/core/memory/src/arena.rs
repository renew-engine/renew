//! Linear (bump) arena: allocate forward through a fixed block, hand out
//! disjoint references, reclaim everything at once with [`LinearArena::reset`].

use core::cell::Cell;
use core::marker::PhantomData;
use core::mem::{align_of, size_of};
use core::ptr::NonNull;
use std::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};

/// The arena's base alignment: every allocation address is aligned to
/// this, so any type with `align_of::<T>() <= BASE_ALIGN` can live here.
/// Types wanting more get `None` (none exist in the engine today).
const BASE_ALIGN: usize = 16;

/// Zero-sized type whose only job is carrying [`BASE_ALIGN`], so the
/// empty arena's dangling base is aligned *by construction* — no
/// fallible conversion, no fallback that could quietly regress.
#[repr(align(16))]
struct BaseAligned;

const _: () = assert!(core::mem::align_of::<BaseAligned>() == BASE_ALIGN);

/// A fixed-capacity bump allocator for `Copy` data.
///
/// Allocation takes `&self` and returns `&mut T` — sound because every
/// allocation hands out a *disjoint* region of the arena's storage, and
/// the arena itself never touches handed-out bytes again until
/// [`LinearArena::reset`], which takes `&mut self` and therefore cannot
/// run while any allocation is still borrowed.
///
/// Storage is held as a raw base pointer (never re-borrowed as a whole),
/// so handed-out references are the only references into the buffer.
///
/// `Copy`-only in v0: the arena never runs destructors, and `Copy` types
/// have none to run. Deliberately neither `Send` nor `Sync`: arenas are
/// single-thread context objects.
///
/// ```compile_fail
/// fn requires_sync<T: Sync>() {}
/// requires_sync::<renew_memory::LinearArena>();
/// ```
///
/// ```compile_fail
/// fn requires_send<T: Send>() {}
/// requires_send::<renew_memory::LinearArena>();
/// ```
pub struct LinearArena {
    base: NonNull<u8>,
    capacity: usize,
    offset: Cell<usize>,
    high_water: Cell<usize>,
    /// The arena owns its buffer like a `Box<[u8]>` (freed in `Drop`).
    _owns: PhantomData<Box<[u8]>>,
}

impl LinearArena {
    /// A new arena backed by `capacity` bytes acquired up front from the
    /// process's global allocator, aligned to [`BASE_ALIGN`] so offset
    /// alignment is address alignment. The arena never grows.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let base = if capacity == 0 {
            // A dangling-but-BASE_ALIGN-aligned base: the empty arena
            // still hands out (zero-sized) references whose addresses
            // must satisfy the alignment the API promises. Infallible by
            // construction via the aligned marker type.
            NonNull::<BaseAligned>::dangling().cast::<u8>()
        } else {
            let layout = Self::layout(capacity);
            // SAFETY: the layout has non-zero size (capacity > 0) and a
            // valid power-of-two alignment.
            let pointer = unsafe { alloc_zeroed(layout) };
            let Some(pointer) = NonNull::new(pointer) else {
                // Allocation-failure policy: abort.
                handle_alloc_error(layout)
            };
            pointer
        };
        Self {
            base,
            capacity,
            offset: Cell::new(0),
            high_water: Cell::new(0),
            _owns: PhantomData,
        }
    }

    fn layout(capacity: usize) -> Layout {
        match Layout::from_size_align(capacity, BASE_ALIGN) {
            Ok(layout) => layout,
            // A capacity no allocator can represent cannot be satisfied:
            // abort, per the allocation-failure policy. Falling back to a
            // smaller layout here would silently desynchronize the
            // recorded capacity from the real buffer.
            Err(_) => handle_alloc_error(Layout::new::<u8>()),
        }
    }

    /// Allocate `value` in the arena, or `None` when the remaining
    /// capacity (after alignment) cannot hold it.
    // The shared-borrow signature is the point of an arena: each call
    // returns a reference to a *disjoint* region (see the type docs), so
    // the lint's aliasing concern does not apply.
    #[allow(clippy::mut_from_ref)]
    #[must_use]
    pub fn alloc<T: Copy>(&self, value: T) -> Option<&mut T> {
        let start = self.aligned_offset::<T>()?;
        let end = start.checked_add(size_of::<T>())?;
        if end > self.capacity {
            return None;
        }
        self.advance(end);
        // SAFETY: `start..end` lies inside the owned buffer (bounds
        // checked above) and is aligned for `T` (`aligned_offset`). The
        // region is disjoint from every previously returned region
        // because the offset only moves forward until `reset`, which
        // requires exclusive access. All references into the buffer are
        // derived from the raw base pointer, so no whole-buffer borrow
        // ever aliases them.
        unsafe {
            let slot = self.base.as_ptr().add(start).cast::<T>();
            slot.write(value);
            Some(&mut *slot)
        }
    }

    /// Allocate a copy of `values`, or `None` when it cannot fit.
    // See `alloc` for why the shared-borrow signature is sound.
    #[allow(clippy::mut_from_ref)]
    #[must_use]
    pub fn alloc_slice<T: Copy>(&self, values: &[T]) -> Option<&mut [T]> {
        let bytes = size_of::<T>().checked_mul(values.len())?;
        let start = self.aligned_offset::<T>()?;
        let end = start.checked_add(bytes)?;
        if end > self.capacity {
            return None;
        }
        self.advance(end);
        // SAFETY: same bounds/alignment/disjointness argument as `alloc`.
        // The copy regions cannot overlap: the destination begins at the
        // freshly claimed offset, while any arena-derived source lies
        // strictly below it (older allocations only) — and non-arena
        // sources are disjoint trivially.
        unsafe {
            let slot = self.base.as_ptr().add(start).cast::<T>();
            core::ptr::copy_nonoverlapping(values.as_ptr(), slot, values.len());
            Some(core::slice::from_raw_parts_mut(slot, values.len()))
        }
    }

    /// Reclaim everything. Requires exclusive access, so it cannot run
    /// while any allocation is still borrowed.
    pub fn reset(&mut self) {
        self.offset.set(0);
    }

    /// Bytes currently allocated (including alignment padding).
    #[must_use]
    pub fn used(&self) -> usize {
        self.offset.get()
    }

    /// The high-water mark: the largest `used` value ever reached,
    /// surviving `reset` — budget evidence.
    #[must_use]
    pub fn high_water(&self) -> usize {
        self.high_water.get()
    }

    /// Total backing capacity in bytes.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    fn aligned_offset<T>(&self) -> Option<usize> {
        let align = align_of::<T>();
        if align > BASE_ALIGN {
            // The base pointer only guarantees BASE_ALIGN; a type wanting
            // more cannot be placed correctly at any offset.
            return None;
        }
        let offset = self.offset.get();
        let misalignment = offset % align;
        if misalignment == 0 {
            Some(offset)
        } else {
            offset.checked_add(align - misalignment)
        }
    }

    fn advance(&self, end: usize) {
        self.offset.set(end);
        self.high_water.set(self.high_water.get().max(end));
    }
}

impl Drop for LinearArena {
    fn drop(&mut self) {
        if self.capacity > 0 {
            // SAFETY: `base` was returned by `alloc_zeroed` with exactly
            // this layout in `with_capacity`, and is freed exactly once;
            // `&mut self` guarantees no outstanding allocation borrows.
            unsafe {
                dealloc(self.base.as_ptr(), Self::layout(self.capacity));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_and_are_disjoint() {
        let arena = LinearArena::with_capacity(256);
        let a = arena.alloc(41u32).expect("fits");
        let b = arena.alloc(42u32).expect("fits");
        *a += 1;
        assert_eq!(*a, 42);
        assert_eq!(*b, 42);
        assert!(!core::ptr::eq(a, b));
    }

    #[test]
    fn allocations_are_aligned() {
        let arena = LinearArena::with_capacity(256);
        let _byte = arena.alloc(1u8).expect("fits");
        let word = arena.alloc(2u64).expect("fits");
        assert_eq!((core::ptr::from_mut(word) as usize) % align_of::<u64>(), 0);
        // The padding shows up in `used`.
        assert_eq!(arena.used(), 16);
    }

    #[test]
    fn exhaustion_returns_none_and_leaves_state_sane() {
        let arena = LinearArena::with_capacity(8);
        assert!(arena.alloc(1u64).is_some());
        assert!(arena.alloc(2u64).is_none());
        assert_eq!(arena.used(), 8);
        assert!(arena.alloc(3u8).is_none());
    }

    #[test]
    fn reset_reclaims_but_high_water_survives() {
        let mut arena = LinearArena::with_capacity(64);
        let _ = arena.alloc([0u8; 48]).expect("fits");
        assert_eq!(arena.high_water(), 48);
        arena.reset();
        assert_eq!(arena.used(), 0);
        assert_eq!(arena.high_water(), 48);
        assert!(arena.alloc([0u8; 64]).is_some());
        assert_eq!(arena.high_water(), 64);
    }

    #[test]
    fn slices_copy_in_and_read_back() {
        let arena = LinearArena::with_capacity(64);
        let source = [1u16, 2, 3, 5, 8];
        let copied = arena.alloc_slice(&source).expect("fits");
        assert_eq!(copied, &source);
        copied[0] = 99;
        assert_eq!(copied[0], 99);
    }

    #[test]
    fn zero_sized_types_cost_nothing() {
        let arena = LinearArena::with_capacity(4);
        for _ in 0..1000 {
            assert!(arena.alloc(()).is_some());
        }
        assert_eq!(arena.used(), 0);
    }

    #[test]
    fn zero_capacity_arena_refuses_politely() {
        let arena = LinearArena::with_capacity(0);
        assert!(arena.alloc(1u8).is_none());
        assert_eq!(arena.capacity(), 0);
    }

    #[test]
    fn the_empty_arena_still_honors_alignment_for_zero_sized_leases() {
        // Regression: the dangling base must carry BASE_ALIGN, or these
        // 100%-safe calls materialize misaligned references (UB).
        #[repr(C, align(16))]
        #[derive(Clone, Copy)]
        struct WideZst;

        let arena = LinearArena::with_capacity(0);
        let lease = arena.alloc(WideZst).expect("zero-sized always fits");
        assert_eq!((core::ptr::from_mut(lease) as usize) % 16, 0);

        let empty: &mut [u64] = arena.alloc_slice(&[]).expect("empty always fits");
        assert_eq!((empty.as_ptr() as usize) % 8, 0);
        assert!(empty.is_empty());
    }

    #[test]
    fn base_alignment_is_honored_up_to_the_documented_bound() {
        #[repr(C, align(16))]
        #[derive(Clone, Copy)]
        struct Wide([u8; 16]);

        #[repr(C, align(32))]
        #[derive(Clone, Copy)]
        struct TooWide([u8; 32]);

        let arena = LinearArena::with_capacity(64);
        let _nudge = arena.alloc(1u8).expect("fits");
        let wide = arena.alloc(Wide([7; 16])).expect("fits at align 16");
        assert_eq!((core::ptr::from_mut(wide) as usize) % 16, 0);
        // Beyond the base alignment there is no correct placement.
        assert!(arena.alloc(TooWide([0; 32])).is_none());
    }

    /// `alloc_slice` runs its own alignment check, so it needs its own
    /// proof: an element type the base pointer cannot align is refused
    /// there too, and a refusal must be free — a partially advanced
    /// offset would strand bytes no allocation ever gets to use.
    #[test]
    fn over_aligned_slice_elements_are_refused_like_scalars() {
        #[repr(C, align(32))]
        #[derive(Clone, Copy)]
        struct TooWide([u8; 32]);

        let arena = LinearArena::with_capacity(256);
        // Even the EMPTY slice is refused: the arena would still be
        // promising an address it cannot align, and the length never
        // enters that question.
        assert!(arena.alloc_slice::<TooWide>(&[]).is_none());
        assert!(arena.alloc_slice(&[TooWide([7; 32]); 2]).is_none());
        assert_eq!(arena.used(), 0, "a refusal must consume nothing");
        assert_eq!(arena.high_water(), 0);

        // And the arena is untouched for the types it CAN align.
        let ok = arena.alloc_slice(&[1u128, 2]).expect("align 16 fits");
        assert_eq!(ok, &[1u128, 2]);
        assert_eq!(arena.used(), 32);
    }
}
