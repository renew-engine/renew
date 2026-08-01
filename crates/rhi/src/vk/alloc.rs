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
///
/// **A zero-size request is planned, not refused, and the distinction
/// is load-bearing.** `NULL` is how a host allocator reports *failure*,
/// so refusing a zero-size request tells the driver it is out of
/// memory. Drivers do make them: one that records commands into a
/// deferred queue allocates a trailing array per command, and an array
/// of zero dynamic offsets is a zero-byte request. The total is never
/// zero anyway — the header sits below every allocation — so a unique,
/// freeable pointer costs nothing, which is also what `malloc` returns
/// for `malloc(0)` and what drivers are written against.
fn plan(size: usize, align: usize) -> Option<(Layout, usize)> {
    if !align.is_power_of_two() {
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
    allocate_with(ledger, size, align, |layout| {
        // SAFETY: `layout` has non-zero size by construction (size >= 1,
        // offset >= header size).
        unsafe { System.alloc(layout) }
    })
}

/// The body of [`allocate`], over a caller-supplied allocator.
///
/// The seam exists for one reason: a host that REFUSES an allocation is
/// the branch below, and no portable request provokes a refusal —
/// Linux over-commits an absurd size, Windows serves an absurd
/// alignment, and an optimizing build may fold the null check away
/// entirely. Injecting the refusal is the only way to prove the code
/// handles it. The parameter is generic, so the real path monomorphizes
/// to exactly what it was before.
fn allocate_with(
    ledger: &AllocLedger,
    size: usize,
    align: usize,
    alloc: impl FnOnce(Layout) -> *mut u8,
) -> *mut u8 {
    let Some((layout, offset)) = plan(size, align) else {
        return core::ptr::null_mut();
    };
    let base = alloc(layout);
    if base.is_null() {
        return core::ptr::null_mut();
    }
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "offset is a multiple of an alignment >= align_of::<Header>()"
    )]
    // SAFETY: `offset < layout.size()`, so `user` is in-bounds of the
    // allocation; `user - size_of::<Header>()` is also in-bounds
    // because `offset >= size_of::<Header>()`, and it is sufficiently
    // aligned for `Header` because `offset` is a multiple of an
    // alignment >= align_of::<Header>() and the base is at least that
    // aligned.
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

    /// The ledger pointer the callbacks are always handed in production.
    fn user_data_of(ledger: &AllocLedger) -> *mut c_void {
        core::ptr::from_ref(ledger).cast_mut().cast()
    }

    /// A zero-size request must produce a real pointer, because `NULL`
    /// means *failure* to a driver rather than *nothing*.
    ///
    /// This is not hypothetical tidiness: a driver that records commands
    /// into a deferred queue allocates one trailing array per command,
    /// and binding a descriptor set with no dynamic offsets asks for an
    /// array of zero of them. Refusing it makes the driver believe the
    /// host is out of memory, and it reports that at the end of
    /// recording — nowhere near the call that caused it.
    #[test]
    fn a_zero_size_request_is_served_rather_than_refused() {
        let ledger = AllocLedger::default();
        let first = allocate(&ledger, 0, 8);
        let second = allocate(&ledger, 0, 8);
        assert!(!first.is_null(), "zero size must not read as failure");
        assert!(!second.is_null());
        assert_ne!(first, second, "each request owns a distinct address");
        assert_eq!(ledger.allocations.load(Ordering::Relaxed), 2);
        assert_eq!(
            ledger.bytes_in_use.load(Ordering::Relaxed),
            0,
            "zero bytes were asked for, so zero are in use"
        );
        // SAFETY: both pointers came from `allocate` and are unfreed.
        unsafe {
            deallocate(&ledger, first);
            deallocate(&ledger, second);
        }
        assert_eq!(ledger.deallocations.load(Ordering::Relaxed), 2);
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn plan_rejects_the_unplannable() {
        assert!(plan(16, 3).is_none(), "non-power-of-two align");
        assert!(plan(usize::MAX, 8).is_none(), "overflow");
        // Adding the header does not overflow here, but the total is
        // still past what `Layout` will represent — a separate refusal.
        assert!(
            plan(usize::MAX - 1024, 8).is_none(),
            "unrepresentable layout"
        );
    }

    #[test]
    fn an_unplannable_request_is_a_null_return_not_a_panic() {
        let ledger = AllocLedger::default();
        assert!(allocate(&ledger, 16, 3).is_null(), "non-power-of-two align");
        assert!(allocate(&ledger, usize::MAX, 8).is_null(), "overflow");
        assert_eq!(
            ledger.allocations.load(Ordering::Relaxed),
            0,
            "a refusal is not an allocation"
        );
    }

    /// A host that refuses the allocation must yield null and leave
    /// the ledger untouched — Vulkan reads null as "allocation failed".
    /// The refusal is INJECTED: no portable request provokes a real
    /// one (Linux over-commits an absurd size, Windows serves an absurd
    /// alignment, and a release build can fold the null check away), so
    /// a test that asks the host to refuse is a test that passes for
    /// the wrong reason on some machine.
    #[test]
    fn a_refused_host_allocation_is_a_null_return() {
        let ledger = AllocLedger::default();
        let refused = allocate_with(&ledger, 64, 8, |_| core::ptr::null_mut());
        assert!(refused.is_null(), "a refusal must surface as null");
        assert_eq!(
            ledger.allocations.load(Ordering::Relaxed),
            0,
            "a refusal is not an allocation"
        );
        assert_eq!(
            ledger.bytes_in_use.load(Ordering::Relaxed),
            0,
            "a refusal reserves no bytes"
        );
    }

    #[test]
    fn round_trips_write_read_and_balance_the_ledger() {
        let ledger = AllocLedger::default();
        let ptr = allocate(&ledger, 100, 32);
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize % 32, 0, "returned pointer respects align");
        // SAFETY: (test) writing within the 100 bytes just allocated.
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
        let user_data = user_data_of(&ledger);
        let first = cb_alloc(user_data, 8, 8, vk::SystemAllocationScope::OBJECT);
        assert!(!first.is_null());
        // SAFETY: (test) writing within the 8 bytes just allocated.
        unsafe { core::ptr::write_bytes(first.cast::<u8>(), 0x5A, 8) };
        let grown = cb_realloc(user_data, first, 64, 8, vk::SystemAllocationScope::OBJECT);
        assert!(!grown.is_null());
        // SAFETY: (test) the first 8 bytes were copied by realloc.
        unsafe {
            for index in 0..8 {
                assert_eq!(*grown.cast::<u8>().add(index), 0x5A);
            }
        }
        cb_free(user_data, grown);
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
        assert_eq!(ledger.reallocations.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn realloc_of_a_null_original_is_an_allocation() {
        let ledger = AllocLedger::default();
        let user_data = user_data_of(&ledger);
        let fresh = cb_realloc(
            user_data,
            core::ptr::null_mut(),
            32,
            8,
            vk::SystemAllocationScope::OBJECT,
        );
        assert!(!fresh.is_null());
        assert_eq!(ledger.allocations.load(Ordering::Relaxed), 1);
        assert_eq!(
            ledger.reallocations.load(Ordering::Relaxed),
            0,
            "nothing was moved, so nothing was reallocated"
        );
        cb_free(user_data, fresh);
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn realloc_to_zero_frees_and_reports_null() {
        let ledger = AllocLedger::default();
        let user_data = user_data_of(&ledger);
        let live = cb_alloc(user_data, 32, 8, vk::SystemAllocationScope::OBJECT);
        assert!(!live.is_null());
        let gone = cb_realloc(user_data, live, 0, 8, vk::SystemAllocationScope::OBJECT);
        assert!(gone.is_null(), "a zero-size reallocation owns nothing");
        assert_eq!(ledger.deallocations.load(Ordering::Relaxed), 1);
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_refused_realloc_leaves_the_original_alive() {
        let ledger = AllocLedger::default();
        let user_data = user_data_of(&ledger);
        let live = cb_alloc(user_data, 32, 8, vk::SystemAllocationScope::OBJECT);
        assert!(!live.is_null());
        // Vulkan's contract: when reallocation fails the caller still
        // owns the original, so the ledger must show it untouched.
        let refused = cb_realloc(user_data, live, 64, 3, vk::SystemAllocationScope::OBJECT);
        assert!(
            refused.is_null(),
            "a non-power-of-two align cannot be served"
        );
        assert_eq!(ledger.deallocations.load(Ordering::Relaxed), 0);
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 32);
        cb_free(user_data, live);
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn the_callbacks_refuse_rather_than_dereference_a_null_ledger() {
        let scope = vk::SystemAllocationScope::OBJECT;
        let nothing = core::ptr::null_mut();
        assert!(cb_alloc(nothing, 16, 8, scope).is_null());
        assert!(cb_realloc(nothing, nothing, 16, 8, scope).is_null());

        let ledger = AllocLedger::default();
        let user_data = user_data_of(&ledger);
        let live = cb_alloc(user_data, 16, 8, scope);
        assert!(!live.is_null());
        cb_free(nothing, live);
        assert_eq!(
            ledger.deallocations.load(Ordering::Relaxed),
            0,
            "a free with no ledger cannot account for anything"
        );
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 16);
        // Freeing null is legal by the Vulkan contract, and a no-op.
        cb_free(user_data, core::ptr::null_mut());
        assert_eq!(ledger.deallocations.load(Ordering::Relaxed), 0);
        cb_free(user_data, live);
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn the_installed_struct_carries_the_ledger_and_three_working_shims() {
        let ledger = AllocLedger::default();
        let installed = callbacks(&ledger);
        assert!(
            core::ptr::eq(
                installed.p_user_data.cast_const().cast::<AllocLedger>(),
                core::ptr::from_ref(&ledger)
            ),
            "the driver must be handed this ledger, not a copy"
        );
        let alloc = installed.pfn_allocation.expect("allocation shim");
        let realloc = installed.pfn_reallocation.expect("reallocation shim");
        let free = installed.pfn_free.expect("free shim");
        let scope = vk::SystemAllocationScope::OBJECT;
        // SAFETY: (test) the arguments are this struct's own user_data
        // and pointers these very shims returned, which is exactly the
        // contract the driver honors.
        unsafe {
            let ptr = alloc(installed.p_user_data, 24, 16, scope);
            assert!(!ptr.is_null());
            let grown = realloc(installed.p_user_data, ptr, 48, 16, scope);
            assert!(!grown.is_null());
            free(installed.p_user_data, grown);
        }
        assert_eq!(ledger.allocations.load(Ordering::Relaxed), 2);
        assert_eq!(ledger.reallocations.load(Ordering::Relaxed), 1);
        assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
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
            // SAFETY: (test) first and last byte of the allocation.
            unsafe {
                *ptr = 1;
                *ptr.add(size - 1) = 2;
                deallocate(&ledger, ptr);
            }
            prop_assert_eq!(ledger.bytes_in_use.load(Ordering::Relaxed), 0);
        }
    }
}
