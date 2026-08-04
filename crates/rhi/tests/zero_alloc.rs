//! Mechanical enforcement of the render-path allocation contract: after
//! warmup, a steady-state frame (clear + draw + readback) performs no
//! heap allocation through the Rust global allocator.
//!
//! Driver-side host allocations are invisible here by design — they
//! route through `std::alloc::System` directly via the instrumented
//! Vulkan callbacks (and their own ledger); this test pins the engine
//! side of the line. Validation stays off: the messenger deliberately
//! allocates when it speaks, and it must not speak into the window.

use std::rc::Rc;

use renew_memory::{CountingAllocator, counters};
use renew_rhi::{
    Attachment, BufferUsage, ClearValue, Color, Device, DeviceDesc, DeviceError, Extent, FrameData,
    Item, LoadOp, Pass, PipelineDesc, RenderDesc, SamplerDesc, StoreOp, TargetFormat, TextureDesc,
    Validation, builtin,
};

/// The engine's own counting allocator, not a local copy of one.
///
/// **This file used to carry its own `GlobalAlloc` shim** — forty lines
/// of `unsafe` duplicating `renew-memory`'s, differing only in counting
/// allocations and not bytes. The windowed sibling already used the real
/// one. Two implementations of one thing is the shape a defect hides in,
/// and the duplicate was the poorer of the two: it could not report a
/// peak, so the byte figure this suite is otherwise positioned to
/// produce did not exist.
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The one color attachment these frames render into: cleared, stored.
fn clear(color: Color) -> [Attachment; 1] {
    [Attachment::new(
        LoadOp::Clear(ClearValue::Color(color)),
        StoreOp::Store,
    )]
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
#[expect(
    clippy::too_many_lines,
    reason = "one gate: fixtures, warmup, and the three measured frame shapes read top to bottom"
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
    // The frame under measurement carries per-frame bytes: a gate that
    // measured a byte-free frame would pass vacuously the moment the
    // data path allocated. The copy into the mapped region is the whole
    // point of measuring it; the instance bytes themselves live in a
    // caller array filled once, out here.
    let (instanced, buffer, instance_bytes) =
        instanced_fixture(&device).expect("instanced fixture");
    // A second per-frame buffer, so one measured frame can carry
    // several passes with several distinct buffers — the fixed-capacity
    // retention claim is gate-observed, not just typed. Built out here
    // like everything else that allocates.
    let buffer_two = device
        .create_buffer(64, BufferUsage::PerFrame)
        .expect("second per-frame buffer");
    let clear_color = Color::new(0.1, 0.2, 0.3, 1.0);
    let mut pixels = vec![0u8; target.byte_len()];

    // Warmup: first frames may lazily initialize driver state.
    for _ in 0..3 {
        let color = clear(clear_color);
        let items = [Item::new(&pipeline)];
        target
            .render(&RenderDesc::new(&[Pass::new(&color, &items)]))
            .expect("warmup frame");
        let items = [Item::new(&instanced).frame_data(FrameData::new(&buffer, &instance_bytes, 1))];
        target
            .render(&RenderDesc::new(&[Pass::new(&color, &items)]))
            .expect("warmup instanced frame");
        target.read_back_into(&mut pixels);
    }

    // The retry-until-quiet policy lives with the counters it reads;
    // this file used to open-code it, invisible to anyone grepping for
    // the helper. Both channels now: a steady frame that deallocates is
    // as loud as one that allocates.
    let verdict = counters::quiet_window(5, || {
        for _ in 0..16 {
            let color = clear(clear_color);
            let items = [Item::new(&pipeline)];
            target
                .render(&RenderDesc::new(&[Pass::new(&color, &items)]))
                .expect("steady frame");
            let items =
                [Item::new(&instanced).frame_data(FrameData::new(&buffer, &instance_bytes, 1))];
            target
                .render(&RenderDesc::new(&[Pass::new(&color, &items)]))
                .expect("steady instanced frame");
            // The multi-pass, multi-buffer frame: two passes, three
            // items, two distinct buffers retained in one frame — the
            // walk's loops and the retention table's width, measured on
            // the same zero-delta terms as the single-item frames
            // (which stay above, so a regression names its shape).
            let load = [Attachment::new(LoadOp::Load, StoreOp::Store)];
            let first_items = [
                Item::new(&pipeline),
                Item::new(&instanced).frame_data(FrameData::new(&buffer, &instance_bytes, 1)),
            ];
            let second_items =
                [
                    Item::new(&instanced).frame_data(FrameData::new(
                        &buffer_two,
                        &instance_bytes,
                        1,
                    )),
                ];
            let passes = [
                Pass::new(&color, &first_items),
                Pass::new(&load, &second_items),
            ];
            target
                .render(&RenderDesc::new(&passes))
                .expect("steady multi-item frame");
            target.read_back_into(&mut pixels);
        }
    });
    // The driver-side ledger is printed for the record, never gated:
    // driver host-allocation behavior is the driver's, not ours.
    let stats = device.host_allocation_stats();
    eprintln!("driver host-allocation ledger after steady state: {stats:?}");
    // The engine-side figure, which the local shim this file used to
    // carry could not produce. Printed for the record on the same terms
    // as the driver ledger above: never gated, because a peak is a
    // property of the whole process — this harness included — and not of
    // the frame path the assertion below actually guards.
    eprintln!(
        "engine allocation counters after steady state: {:?}",
        counters::snapshot()
    );
    if let Err(activity) = verdict {
        panic!("the render path was loud in every window (last: {activity})");
    }
}

/// The instanced pipeline, its per-frame buffer, and one packed
/// instance record, built outside the measured window.
fn instanced_fixture(
    device: &Device,
) -> Result<(renew_rhi::RenderPipeline, renew_rhi::Buffer, [u8; 24]), Box<dyn std::error::Error>> {
    let instanced = device.create_pipeline(
        &PipelineDesc::new(builtin::INSTANCED, TargetFormat::Rgba8Unorm)
            .instance_input(builtin::INSTANCED_LAYOUT),
    )?;
    let buffer = device.create_buffer(64, BufferUsage::PerFrame)?;
    let mut bytes = [0u8; 24];
    for (i, v) in [-0.5f32, -0.5, 1.0, 0.0, 0.0, 1.0].iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
    }
    Ok((instanced, buffer, bytes))
}
