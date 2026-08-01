//! Mechanical enforcement of the render-path allocation contract: after
//! warmup, a steady-state frame (clear + draw + readback) performs no
//! heap allocation through the Rust global allocator.
//!
//! Driver-side host allocations are invisible here by design — they
//! route through `std::alloc::System` directly via the instrumented
//! Vulkan callbacks (and their own ledger); this test pins the engine
//! side of the line. Validation stays off: the messenger deliberately
//! allocates when it speaks, and it must not speak into the window.

// `unsafe` is required to implement a global allocator; this test-only
// shim wraps the system allocator unchanged and only adds a counter.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, PipelineDesc, RenderDesc, SamplerDesc,
    TargetFormat, TextureDesc, Validation, builtin,
};

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

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn steady_state_frames_allocate_nothing() {
    let device = match Device::new(&DeviceDesc {
        app_name: "renew-rhi-zero-alloc",
        validation: Validation::Off,
    }) {
        Ok(device) => device,
        Err(DeviceError::LoaderUnavailable { message })
            if std::env::var_os("RENEW_GOLDEN").is_none_or(|v| v != "1") =>
        {
            eprintln!("SKIP: no Vulkan runtime: {message}");
            return;
        }
        Err(error) => panic!("device bring-up failed: {error}"),
    };
    let mut target = device
        .create_offscreen_target(Extent {
            width: 64,
            height: 64,
        })
        .expect("offscreen target");
    // **A textured pipeline, deliberately, and it is the stronger
    // measurement rather than a different one.** Inside the measured
    // window the textured path does everything the untextured path does
    // — bind pipeline, viewport, scissor, draw — and then one thing
    // more: it binds a descriptor set. So a textured frame allocating
    // nothing implies an untextured frame allocating nothing, while the
    // reverse says nothing at all. Measuring the triangle left the bind
    // covered by reading the code rather than by the gate written to
    // judge it.
    //
    // The texture and sampler are built here, outside the window: their
    // creation allocates freely, and only the per-frame path is under
    // test.
    let texels: [u8; 16] = [
        10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
    ];
    let texture = Rc::new(
        device
            .create_texture(&TextureDesc::new(
                Extent {
                    width: 2,
                    height: 2,
                },
                &texels,
            ))
            .expect("atlas upload"),
    );
    let sampler = Rc::new(
        device
            .create_sampler(&SamplerDesc::atlas())
            .expect("sampler"),
    );
    let pipeline = device
        .create_pipeline(
            &PipelineDesc::new(builtin::TEXTURED, TargetFormat::Rgba8Unorm)
                .texture(texture, sampler),
        )
        .expect("textured pipeline");
    let clear = Color::new(0.1, 0.2, 0.3, 1.0);
    let mut pixels = vec![0u8; target.byte_len()];

    // Warmup: first frames may lazily initialize driver state.
    for _ in 0..3 {
        target
            .render(&RenderDesc::new(clear).pipeline(&pipeline))
            .expect("warmup frame");
        target.read_back_into(&mut pixels);
    }

    // Measurement protocol: the counter is process-wide and the test
    // harness's own thread can allocate concurrently. So the window
    // retries: one-shot neighbor noise rides out, while a real
    // render-path allocation reproduces in every window and still
    // fails.
    let mut last_delta = 0usize;
    let mut observed_zero = false;
    for _ in 0..5 {
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        for _ in 0..16 {
            target
                .render(&RenderDesc::new(clear).pipeline(&pipeline))
                .expect("steady frame");
            target.read_back_into(&mut pixels);
        }
        let after = ALLOCATIONS.load(Ordering::Relaxed);
        last_delta = after - before;
        if last_delta == 0 {
            observed_zero = true;
            break;
        }
    }
    // The driver-side ledger is printed for the record, never gated:
    // driver host-allocation behavior is the driver's, not ours.
    let stats = device.host_allocation_stats();
    eprintln!("driver host-allocation ledger after steady state: {stats:?}");
    assert!(
        observed_zero,
        "the render path heap-allocated in every window (last delta: {last_delta})"
    );
}
