//! The driver host-allocation shims: every allocation Vulkan makes on
//! the host routes through `std::alloc::System` DIRECTLY — never the
//! global dispatch, so an installed engine counting allocator can
//! neither see nor recurse into driver traffic — with a per-device
//! ledger of relaxed atomics (they synchronize with driver threads,
//! not engine threads).
//!
//! Scheme: each allocation is over-allocated by an aligned header
//! prefix recording (size, align, offset); the callbacks recover the
//! true base and layout from the header at free/realloc time. The
//! layout logic is plain safe arithmetic in [`plan`]; the pointer work
//! is confined to three small functions unit-tested directly (the
//! scheduled interpreter job drives them with fabricated layouts).

use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::alloc::{GlobalAlloc, Layout, System};

use ash::vk;

/// Per-device tally of driver host allocations. Diagnostics only.
#[derive(Debug, Default)]
pub struct AllocLedger {
    pub allocations: AtomicU64,
    pub deallocations: AtomicU64,
    pub reallocations: AtomicU64,
    pub bytes_in_use: AtomicUsize,
    pub peak_bytes: AtomicUsize,
}

/// The header stored immediately before every returned pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Header {
    size: usize,
    align: usize,
    offset: usize,
}

/// Layout arithmetic, pure and panic-free: the user pointer sits
/// `offset` bytes past the base, where `offset` is the smallest
/// multiple of `align` that fits the header. Returns `None` when the
/// request cannot be represented — the callbacks translate that into
/// an allocation failure, never a panic.
fn plan(size: usize, align: usize) -> Option<(Layout, usize)> {
    if size == 0 || !align.is_power_of_two() {
        return None;
    }
    let align = align.max(core::mem::align_of::<Header>());
    let header = core::mem::size_of::<Header>();
    let offset = header.checked_next_multiple_of(align)?;
    let total = size.checked_add(offset)?;
    let layout = Layout::from_size_align(total, align).ok()?;
    Some((layout, offset))
}

/// Allocate with the header scheme. Null on failure (the Vulkan
/// contract: null means the allocation failed).
fn allocate(ledger: &AllocLedger, size: usize, align: usize) -> *mut u8 {
    let Some((layout, offset)) = plan(size, align) else {
        return core::ptr::null_mut();
    };
    // SAFETY: `layout` has non-zero size by construction (size >= 1,
    // offset >= header size).
    let base = unsafe { System.alloc(layout) };
    if base.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: `offset < layout.size()`, so `user` is in-bounds of the
    // allocation; `user - size_of::<Header>()` is also in-bounds
    // because `offset >= size_of::<Header>()`, and it is sufficiently
    // aligned for `Header` because `offset` is a multiple of an
    // alignment >= align_of::<Header>() and the base is at least that
    // aligned.
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "offset is a multiple of an alignment >= align_of::<Header>()"
    )]
    let user = unsafe {
        let user = base.add(offset);
        user.cast::<Header>().sub(1).write(Header {
            size,
            align: layout.align(),
            offset,
        });
        user
    };
    ledger.allocations.fetch_add(1, Ordering::Relaxed);
    let in_use = ledger.bytes_in_use.fetch_add(size, Ordering::Relaxed) + size;
    ledger.peak_bytes.fetch_max(in_use, Ordering::Relaxed);
    user
}

/// Read back the header for a pointer previously returned by
/// [`allocate`].
///
/// # Safety
///
/// `user` must be a non-null pointer returned by [`allocate`] (or the
/// reallocation path) and not yet freed.
#[expect(
    clippy::cast_ptr_alignment,
    reason = "allocate stores the header at an align_of::<Header>()-aligned offset"
)]
unsafe fn header_of(user: *mut u8) -> Header {
    // SAFETY: per the function contract, a valid header sits
    // immediately before `user` (written by `allocate`).
    unsafe { user.cast::<Header>().sub(1).read() }
}

/// Free a pointer previously returned by [`allocate`].
///
/// # Safety
///
/// `user` must be a non-null pointer returned by [`allocate`] (or the
/// reallocation path) and not yet freed.
unsafe fn deallocate(ledger: &AllocLedger, user: *mut u8) {
    // SAFETY: per the function contract.
    let header = unsafe { header_of(user) };
    let total = header.size + header.offset;
    // SAFETY: `base` and the layout reconstruct exactly the allocation
    // made in `allocate` (size + offset, recorded align).
    unsafe {
        let base = user.sub(header.offset);
        System.dealloc(base, Layout::from_size_align_unchecked(total, header.align));
    }
    ledger.deallocations.fetch_add(1, Ordering::Relaxed);
    ledger
        .bytes_in_use
        .fetch_sub(header.size, Ordering::Relaxed);
}

// ---- the extern callbacks ----------------------------------------------

extern "system" fn cb_alloc(
    user_data: *mut c_void,
    size: usize,
    alignment: usize,
    _scope: vk::SystemAllocationScope,
) -> *mut c_void {
    if user_data.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: `user_data` is the `&AllocLedger` installed at device
    // creation; the ledger lives in the device spine, which outlives
    // every driver object that can call back (destruction happens
    // through the same callbacks BEFORE the spine drops the ledger).
    let ledger = unsafe { &*user_data.cast::<AllocLedger>() };
    allocate(ledger, size, alignment).cast()
}

extern "system" fn cb_realloc(
    user_data: *mut c_void,
    original: *mut c_void,
    size: usize,
    alignment: usize,
    scope: vk::SystemAllocationScope,
) -> *mut c_void {
    if user_data.is_null() {
        return core::ptr::null_mut();
    }
    if original.is_null() {
        return cb_alloc(user_data, size, alignment, scope);
    }
    // SAFETY: as in `cb_alloc` for the ledger; `original` is a live
    // pointer previously returned by these callbacks (the Vulkan
    // contract for pfnReallocation).
    let ledger = unsafe { &*user_data.cast::<AllocLedger>() };
    if size == 0 {
        // SAFETY: `original` is live per the callback contract.
        unsafe { deallocate(ledger, original.cast()) };
        return core::ptr::null_mut();
    }
    let fresh = allocate(ledger, size, alignment);
    if fresh.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: `original` is live per the callback contract; the copy
    // length is bounded by both allocations' recorded sizes.
    unsafe {
        let old = header_of(original.cast());
        core::ptr::copy_nonoverlapping(original.cast::<u8>(), fresh, old.size.min(size));
        deallocate(ledger, original.cast());
    }
    ledger.reallocations.fetch_add(1, Ordering::Relaxed);
    fresh.cast()
}

extern "system" fn cb_free(user_data: *mut c_void, memory: *mut c_void) {
    if user_data.is_null() || memory.is_null() {
        // Vulkan may free null; a no-op by contract.
        return;
    }
    // SAFETY: as in `cb_alloc`; `memory` is a live pointer previously
    // returned by these callbacks (the Vulkan contract for pfnFree).
    let ledger = unsafe { &*user_data.cast::<AllocLedger>() };
    // SAFETY: `memory` is live per the callback contract.
    unsafe { deallocate(ledger, memory.cast()) };
}

/// Build the callback structure pointing at a ledger. The returned
/// value borrows the ledger for its lifetime parameter, which keeps
/// the pointer's validity visible to the compiler at every use site.
pub fn callbacks(ledger: &AllocLedger) -> vk::AllocationCallbacks<'_> {
    vk::AllocationCallbacks::default()
        .user_data(core::ptr::from_ref(ledger).cast_mut().cast())
        .pfn_allocation(Some(cb_alloc))
        .pfn_reallocation(Some(cb_realloc))
        .pfn_free(Some(cb_free))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::RngSeed;

    #[test]
    fn plan_rejects_the_unplannable() {
        assert!(plan(0, 8).is_none(), "zero size");
        assert!(plan(16, 3).is_none(), "non-power-of-two align");
        assert!(plan(usize::MAX, 8).is_none(), "overflow");
    }

    #[test]
    fn round_trips_write_read_and_balance_the_ledger() {
        let ledger = AllocLedger::default();
        let ptr = allocate(&ledger, 100, 32);
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize % 32, 0, "returned pointer respects align");
        // SAFETY (test): writing within the 100 bytes just allocated.
        unsafe {
            core::ptr::write_bytes(ptr, 0xAB, 100);
            assert_eq!(*ptr, 0xAB);
            assert_eq!(*ptr.add(99), 0xAB);
            deallocate(&ledger, ptr);
        }
        assert_eq!(ledger.allocations.load(Ordering::Relaxed), 1);
        assert_eq!(ledger.deallocations.load(Ordering::Relaxed), 1);
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
        assert!(ledger.peak_bytes.load(Ordering::Relaxed) >= 100);
    }

    #[test]
    fn realloc_preserves_content_through_the_callback_path() {
        let ledger = AllocLedger::default();
        let user_data = core::ptr::from_ref(&ledger).cast_mut().cast::<c_void>();
        let first = cb_alloc(user_data, 8, 8, vk::SystemAllocationScope::OBJECT);
        assert!(!first.is_null());
        // SAFETY (test): writing within the 8 bytes just allocated.
        unsafe { core::ptr::write_bytes(first.cast::<u8>(), 0x5A, 8) };
        let grown = cb_realloc(user_data, first, 64, 8, vk::SystemAllocationScope::OBJECT);
        assert!(!grown.is_null());
        // SAFETY (test): the first 8 bytes were copied by realloc.
        unsafe {
            for index in 0..8 {
                assert_eq!(*grown.cast::<u8>().add(index), 0x5A);
            }
        }
        cb_free(user_data, grown);
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
        assert_eq!(ledger.reallocations.load(Ordering::Relaxed), 1);
    }

    proptest! {
        // Fixed RNG seed: same inputs on every run and machine.
        #![proptest_config(ProptestConfig {
            rng_seed: RngSeed::Fixed(0x0000_A110),
            cases: if cfg!(miri) { 16 } else { 128 },
            ..ProptestConfig::default()
        })]

        #[test]
        fn any_size_align_pair_round_trips(
            size in 1usize..4096,
            align_pow in 0u32..8,
        ) {
            let align = 1usize << align_pow;
            let ledger = AllocLedger::default();
            let ptr = allocate(&ledger, size, align);
            prop_assert!(!ptr.is_null());
            prop_assert_eq!(ptr as usize % align, 0);
            // SAFETY (test): first and last byte of the allocation.
            unsafe {
                *ptr = 1;
                *ptr.add(size - 1) = 2;
                deallocate(&ledger, ptr);
            }
            prop_assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
        }
    }
}
