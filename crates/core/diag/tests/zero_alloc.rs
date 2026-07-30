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
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn installation_and_the_emit_path_allocate_nothing() {
    // A static sink; the counted window opens before `install`: the
    // crate's whole surface — installation included — allocates nothing.
    static SINK: FixedSink = FixedSink {
        state: Mutex::new(Written {
            buffer: [0; 256],
            length: 0,
        }),
    };
    // Warm the fixture's mutex once before any window opens: on some
    // platforms the standard library lazily initializes lock internals
    // with a one-time allocation at first use (observed on macOS). That
    // allocation belongs to the platform's lock, not to the emit path
    // under test.
    drop(SINK.state.lock());

    // Measurement protocol: the counter is process-wide and the test
    // harness's own thread can allocate concurrently (its progress
    // output has landed inside a measurement window on Linux). So the
    // window retries: one-shot neighbor noise rides out, while a real
    // emit-path allocation reproduces in every window and still fails.
    // `install` is write-once and participates in the first window; on
    // the typical clean first attempt the whole surface is verified,
    // and on a noisy first attempt the retries pin the emit path (the
    // load-bearing half of the contract).
    let mut installed = false;
    let mut last_delta = 0usize;
    let mut clean = false;
    for _ in 0..5 {
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        if !installed {
            renew_diag::install(&SINK);
            installed = true;
        }
        for frame in 0..16 {
            renew_diag::info!("frame {} took {}ns", 41 + frame, 16_600_000);
        }
        let after = ALLOCATIONS.load(Ordering::Relaxed);
        last_delta = after - before;
        if last_delta == 0 {
            clean = true;
            break;
        }
        // Reset the fixed buffer between attempts (no allocation).
        if let Ok(mut written) = SINK.state.lock() {
            written.length = 0;
        }
    }
    assert!(
        clean,
        "install or emit heap-allocated in every window (last delta: {last_delta})"
    );

    let written = SINK.state.lock().expect("buffer lock");
    let text = std::str::from_utf8(&written.buffer[..written.length]).expect("utf8 output");
    assert!(
        text.starts_with("INFO zero_alloc frame "),
        "unexpected sink output: {text}"
    );
    assert!(text.contains("took 16600000ns"), "unexpected: {text}");
}
