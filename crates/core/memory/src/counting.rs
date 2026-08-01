//! The instrumented global allocator: wraps the system allocator and
//! counts. A binary opts in with `#[global_allocator]`; the engine
//! libraries never install it themselves.

use std::alloc::{GlobalAlloc, Layout, System};

use crate::counters;

/// A counting wrapper over [`System`]. Install in a binary:
///
/// `no_run` rather than `ignore`: the snippet cannot execute, because a
/// doctest harness has already installed its own global allocator — but
/// it does compile, so a rename of this type breaks the build instead of
/// silently leaving the example wrong. `ignore` would not even compile
/// it.
///
/// ```no_run
/// #[global_allocator]
/// static ALLOCATOR: renew_memory::CountingAllocator =
///     renew_memory::CountingAllocator;
/// ```
///
/// Every allocation and deallocation in the process is counted; read the
/// counters through [`counters::snapshot`]. Wrapping [`System`]
/// specifically — never the global dispatch — because the dispatch *is*
/// this type once installed.
pub struct CountingAllocator;

// SAFETY: every method delegates directly to `System`, which upholds the
// `GlobalAlloc` contract (layout fidelity, uniqueness of live blocks);
// the counter updates are relaxed atomic side effects that do not touch
// the returned memory.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded under the caller's own contract.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            counters::record_alloc(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded under the caller's own contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            counters::record_alloc(layout.size());
        }
        pointer
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded under the caller's own contract.
        let grown = unsafe { System.realloc(pointer, layout, new_size) };
        if !grown.is_null() {
            counters::record_dealloc(layout.size());
            counters::record_alloc(new_size);
        }
        grown
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        counters::record_dealloc(layout.size());
        // SAFETY: forwarded under the caller's own contract.
        unsafe { System.dealloc(pointer, layout) }
    }
}
