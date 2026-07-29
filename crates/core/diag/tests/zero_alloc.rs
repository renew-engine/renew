//! Mechanical enforcement of the crate's zero-allocation contract: the
//! emit path — record assembly, slot lookup, sink dispatch, and message
//! formatting into sink-owned storage — performs no heap allocation.
//! Own process: this test owns the slot and the global allocator.

// `unsafe` is required to implement a global allocator; this test-only
// shim wraps the system allocator unchanged and only adds a counter.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use renew_diag::{Record, Sink};

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// Counts every allocation while delegating to the system allocator.
struct CountingAllocator;

// SAFETY: every method delegates directly to `System`, which upholds the
// `GlobalAlloc` contract; the counter is a relaxed atomic side effect.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarded under the caller's own contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarded under the caller's own contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarded under the caller's own contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded under the caller's own contract.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Fixed-capacity text buffer a sink can format into without allocating.
struct Written {
    buffer: [u8; 256],
    length: usize,
}

impl std::fmt::Write for Written {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let bytes = text.as_bytes();
        let end = self.length + bytes.len();
        if end > self.buffer.len() {
            return Err(std::fmt::Error);
        }
        self.buffer[self.length..end].copy_from_slice(bytes);
        self.length = end;
        Ok(())
    }
}

struct FixedSink {
    state: Mutex<Written>,
}

impl Sink for FixedSink {
    fn write(&self, record: &Record<'_>) {
        if let Ok(mut written) = self.state.lock() {
            let _ = write!(
                written,
                "{} {} {}",
                record.level(),
                record.target(),
                record.message()
            );
        }
    }
}

#[test]
fn installation_and_the_emit_path_allocate_nothing() {
    // A static sink, and the counted window opens before `install`: the
    // crate's whole surface — installation included — allocates nothing.
    static SINK: FixedSink = FixedSink {
        state: Mutex::new(Written {
            buffer: [0; 256],
            length: 0,
        }),
    };
    // Warm the fixture's mutex once before the window opens: on some
    // platforms the standard library lazily initializes lock internals
    // with a one-time allocation at first use (observed on macOS). That
    // allocation belongs to the platform's lock, not to the emit path
    // under test.
    drop(SINK.state.lock());
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    renew_diag::install(&SINK);
    renew_diag::info!("frame {} took {}ns", 41, 16_600_000);
    let after = ALLOCATIONS.load(Ordering::Relaxed);
    assert_eq!(after - before, 0, "install or emit heap-allocated");

    let written = SINK.state.lock().expect("buffer lock");
    let text = std::str::from_utf8(&written.buffer[..written.length]).expect("utf8 output");
    assert_eq!(text, "INFO zero_alloc frame 41 took 16600000ns");
}
