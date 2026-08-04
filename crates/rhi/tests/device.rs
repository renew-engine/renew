//! Device lifecycle under `Validation::Required`: bring-up, the full
//! frame cycle, churn, drop-order freedom, and error paths — with the
//! validation layer as the correctness oracle (zero errors, mechanically
//! asserted, on every path).
//!
//! Environments without a Vulkan runtime or without the validation
//! layer skip: correctness is proven where the oracle exists.

use renew_rhi::{
    AddressMode, Attachment, BufferUsage, ClearValue, Color, DepthState, Device, DeviceDesc,
    DeviceError, Extent, Filter, FrameData, Item, LoadOp, Pass, PipelineDesc, PipelineError,
    RenderDesc, SamplerDesc, Shaders, StoreOp, TargetFormat, Validation, builtin,
};

/// The one color attachment these frames render into: cleared, stored.
fn clear(color: Color) -> [Attachment; 1] {
    [Attachment::new(
        LoadOp::Clear(ClearValue::Color(color)),
        StoreOp::Store,
    )]
}

/// `Ok(None)` is the graceful skip; other failures surface as `Err`
/// for the calling test to unwrap (test-only panics live in `#[test]`
/// bodies, where the lint allowance applies). Under `RENEW_GOLDEN=1`
/// (the CI rendering lane) a skip is a failure: that lane exists to
/// run these tests, so an environment that cannot must redden it.
fn required_device() -> Result<Option<Device>, DeviceError> {
    let strict = std::env::var_os("RENEW_GOLDEN").is_some_and(|v| v == "1");
    match Device::new(&DeviceDesc {
        app_name: "renew-rhi-device-tests",
        validation: Validation::Required,
    }) {
        Ok(device) => Ok(Some(device)),
        Err(DeviceError::LoaderUnavailable { message }) if !strict => {
            eprintln!("SKIP: no Vulkan runtime: {message}");
            Ok(None)
        }
        Err(DeviceError::ValidationUnavailable) if !strict => {
            eprintln!("SKIP: validation layer not installed");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn assert_no_validation_errors(device: &Device) {
    let report = device.validation_report();
    assert_eq!(
        report.errors, 0,
        "validation errors; first messages: {:?}",
        report.first_messages
    );
}

#[test]
fn bring_up_reports_an_adapter_and_stays_clean() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    let adapter = device.adapter();
    assert!(!adapter.name.is_empty(), "adapter must have a name");
    device.wait_idle().expect("wait_idle on a fresh device");
    device.wait_idle().expect("wait_idle is repeatable");

    let stats = device.host_allocation_stats();
    assert!(
        stats.deallocations <= stats.allocations,
        "ledger balance: {stats:?}"
    );
    assert!(
        stats.bytes_in_use <= stats.peak_bytes,
        "ledger peak: {stats:?}"
    );
    assert_no_validation_errors(&device);
}

// The probe: on the CI rendering lane (`RENEW_GOLDEN=1`) a depth format
// must exist — depth-carrying work is planned against this chain, and
// the lane exists to prove the rendering path, so an adapter that
// refuses both formats must redden it rather than silently narrowing
// what the lane proves. Elsewhere, no-format is a reportable fact.
#[test]
fn the_adapter_reports_its_depth_format() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    if let Some(name) = device.depth_format_name() {
        assert!(
            name == "D32_SFLOAT" || name == "D24_UNORM_S8_UINT",
            "the chosen format must come from the chain, got {name}"
        );
    } else {
        assert!(
            std::env::var_os("RENEW_GOLDEN").is_none_or(|v| v != "1"),
            "the rendering lane's adapter must offer a depth format"
        );
        eprintln!("SKIP: adapter offers no chain depth format");
    }
    assert_no_validation_errors(&device);
}

#[test]
fn validation_off_bring_up_works() {
    match Device::new(&DeviceDesc {
        app_name: "renew-rhi-device-tests",
        validation: Validation::Off,
    }) {
        Ok(device) => device.wait_idle().expect("wait_idle"),
        Err(DeviceError::LoaderUnavailable { message })
            if std::env::var_os("RENEW_GOLDEN").is_none_or(|v| v != "1") =>
        {
            eprintln!("SKIP: no Vulkan runtime: {message}");
        }
        Err(error) => panic!("bring-up without validation failed: {error}"),
    }
}

#[test]
fn full_frame_cycle_clear_then_triangle() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    let extent = Extent {
        width: 64,
        height: 64,
    };
    let mut target = device
        .create_offscreen_target(extent)
        .expect("offscreen target");
    let pipeline = device
        .create_pipeline(&PipelineDesc::new(
            builtin::TRIANGLE,
            TargetFormat::Rgba8Unorm,
        ))
        .expect("triangle pipeline");

    // The shape of a composed one-item frame, pinned via the
    // descriptor's Debug form. It lives here rather than in a unit
    // test because building an item requires a real pipeline, and
    // building one requires a device. The empty case is a unit test
    // beside the impl.
    let color = clear(Color::new(0.0, 0.0, 0.0, 1.0));
    let items = [Item::new(&pipeline)];
    let passes = [Pass::new(&color, &items)];
    let bound = format!("{:?}", RenderDesc::new(&passes));
    assert!(bound.contains("passes: 1"), "{bound}");
    assert!(bound.contains("items_per_pass: [1]"), "{bound}");

    target
        .render(&RenderDesc::new(&[Pass::new(&color, &[])]))
        .expect("clear-only render");
    let mut cleared = vec![0u8; target.byte_len()];
    target.read_back_into(&mut cleared);

    target
        .render(&RenderDesc::new(&passes))
        .expect("triangle render");
    let mut drawn = vec![0u8; target.byte_len()];
    target.read_back_into(&mut drawn);

    // The center pixel sits inside the triangle: the draw must have
    // changed it away from the clear value.
    let center = (64 * 32 + 32) * 4;
    assert_eq!(&cleared[center..center + 4], &[0, 0, 0, 255]);
    assert_ne!(
        &drawn[center..center + 4],
        &[0, 0, 0, 255],
        "triangle draw left the center pixel at the clear color"
    );
    // Resources torn down BEFORE the oracle reads: destruction-time
    // validation findings count.
    drop(target);
    drop(pipeline);
    assert_no_validation_errors(&device);
}

#[test]
fn device_churn_three_rounds() {
    for round in 0..3 {
        let Some(device) = required_device().expect("device bring-up") else {
            return;
        };
        let mut target = device
            .create_offscreen_target(Extent {
                width: 32,
                height: 32,
            })
            .expect("offscreen target");
        let color = clear(Color::new(0.5, 0.5, 0.5, 1.0));
        target
            .render(&RenderDesc::new(&[Pass::new(&color, &[])]))
            .expect("render");
        // Teardown first, oracle second: destruction-time findings
        // count in every round.
        drop(target);
        assert_no_validation_errors(&device);
        drop(device);
        let _ = round;
    }
}

#[test]
fn resources_keep_the_device_alive_past_the_handle() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    let mut target = device
        .create_offscreen_target(Extent {
            width: 16,
            height: 16,
        })
        .expect("offscreen target");
    let pipeline = device
        .create_pipeline(&PipelineDesc::new(
            builtin::TRIANGLE,
            TargetFormat::Rgba8Unorm,
        ))
        .expect("pipeline");
    // The handle goes away; the spine lives on through the resources.
    drop(device);
    let color = clear(Color::new(0.0, 0.0, 0.0, 1.0));
    let items = [Item::new(&pipeline)];
    target
        .render(&RenderDesc::new(&[Pass::new(&color, &items)]))
        .expect("render after the device handle dropped");
    let mut pixels = vec![0u8; target.byte_len()];
    target.read_back_into(&mut pixels);
}

#[test]
fn invalid_spirv_is_rejected_per_stage() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    let bad = [0xDEu8, 0xAD, 0xBE, 0xEF];
    match device.create_pipeline(&PipelineDesc::new(
        Shaders::new(&bad, builtin::TRIANGLE_FS_SPV, 3),
        TargetFormat::Rgba8Unorm,
    )) {
        Err(PipelineError::InvalidSpirv { stage, .. }) => assert_eq!(stage, "vertex"),
        Err(other) => panic!("expected vertex rejection, got {other:?}"),
        Ok(_) => panic!("expected vertex rejection, got a pipeline"),
    }
    match device.create_pipeline(&PipelineDesc::new(
        Shaders::new(builtin::TRIANGLE_VS_SPV, &[], 3),
        TargetFormat::Rgba8Unorm,
    )) {
        Err(PipelineError::InvalidSpirv { stage, .. }) => assert_eq!(stage, "fragment"),
        Err(other) => panic!("expected fragment rejection, got {other:?}"),
        Ok(_) => panic!("expected fragment rejection, got a pipeline"),
    }
    assert_no_validation_errors(&device);
}

#[test]
fn interior_nul_in_the_app_name_is_sanitized_not_swallowed() {
    match Device::new(&DeviceDesc {
        app_name: "renew\0device\0tests",
        validation: Validation::Off,
    }) {
        Ok(device) => device.wait_idle().expect("wait_idle"),
        Err(DeviceError::LoaderUnavailable { message })
            if std::env::var_os("RENEW_GOLDEN").is_none_or(|v| v != "1") =>
        {
            eprintln!("SKIP: no Vulkan runtime: {message}");
        }
        Err(error) => panic!("bring-up with a NUL-bearing name failed: {error}"),
    }
}

#[test]
fn cross_device_pipeline_is_a_dev_build_contract_violation() {
    let Some(device_a) = required_device().expect("device bring-up") else {
        return;
    };
    let Some(device_b) = required_device().expect("second device bring-up") else {
        return;
    };
    let mut target = device_a
        .create_offscreen_target(Extent {
            width: 8,
            height: 8,
        })
        .expect("offscreen target");
    let foreign = device_b
        .create_pipeline(&PipelineDesc::new(
            builtin::TRIANGLE,
            TargetFormat::Rgba8Unorm,
        ))
        .expect("pipeline on the other device");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let color = clear(Color::new(0.0, 0.0, 0.0, 1.0));
        let items = [Item::new(&foreign)];
        let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
    }));
    assert!(
        outcome.is_err(),
        "mixing objects across devices must trip the dev-build contract check"
    );
}

/// Every frame-shape refusal, driven. The contract asserts fire before
/// any fence is waited or written, so one target serves every case and
/// stays reusable after each panic — proven at the end by rendering a
/// well-formed frame through the same target.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one refusal per contract clause; the list is the point"
)]
fn malformed_frames_are_refused_by_name() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    let mut target = device
        .create_offscreen_target(Extent {
            width: 8,
            height: 8,
        })
        .expect("offscreen target");
    let pipeline = device
        .create_pipeline(&PipelineDesc::new(
            builtin::TRIANGLE,
            TargetFormat::Rgba8Unorm,
        ))
        .expect("triangle pipeline");
    let black = Color::new(0.0, 0.0, 0.0, 1.0);

    let mut refused = |label: &str, frame: &dyn Fn(&mut renew_rhi::OffscreenTarget)| {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            frame(&mut target);
        }));
        assert!(outcome.is_err(), "{label}: the contract must refuse this");
    };

    refused("empty passes", &|target| {
        let _ = target.render(&RenderDesc::new(&[]));
    });
    refused("zero color attachments", &|target| {
        let _ = target.render(&RenderDesc::new(&[Pass::new(&[], &[])]));
    });
    refused("two color attachments", &|target| {
        let color = Attachment::new(LoadOp::Clear(ClearValue::Color(black)), StoreOp::Store);
        let two = [color, color];
        let _ = target.render(&RenderDesc::new(&[Pass::new(&two, &[])]));
    });
    refused("first-pass color Load", &|target| {
        let color = [Attachment::new(LoadOp::Load, StoreOp::Store)];
        let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &[])]));
    });
    refused("first-pass depth Load", &|target| {
        let color = clear(black);
        let pass = Pass::new(&color, &[]).depth(Attachment::new(LoadOp::Load, StoreOp::Discard));
        let _ = target.render(&RenderDesc::new(&[pass]));
    });
    refused("a depth clear on a color attachment", &|target| {
        let color = [Attachment::new(
            LoadOp::Clear(ClearValue::Depth(1.0)),
            StoreOp::Store,
        )];
        let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &[])]));
    });
    refused("a color clear on a depth attachment", &|target| {
        let color = clear(black);
        let pass = Pass::new(&color, &[]).depth(Attachment::new(
            LoadOp::Clear(ClearValue::Color(black)),
            StoreOp::Discard,
        ));
        let _ = target.render(&RenderDesc::new(&[pass]));
    });
    refused(
        "a depth-free pipeline in a depth-carrying pass",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&pipeline)];
            let pass = Pass::new(&color, &items).depth(Attachment::new(
                LoadOp::Clear(ClearValue::Depth(1.0)),
                StoreOp::Discard,
            ));
            let _ = target.render(&RenderDesc::new(&[pass]));
        },
    );
    if device.depth_format_name().is_some() {
        let depth_pipeline = device
            .create_pipeline(
                &PipelineDesc::new(builtin::TRIANGLE, TargetFormat::Rgba8Unorm)
                    .depth_state(DepthState::read_write()),
            )
            .expect("depth pipeline");
        refused("a depth-testing pipeline in a depthless pass", &|target| {
            let color = clear(black);
            let items = [Item::new(&depth_pipeline)];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        });
    } else {
        eprintln!("SKIP: no depth format, the depth-pipeline half of the mismatch is untestable");
    }
    let instanced = device
        .create_pipeline(
            &PipelineDesc::new(builtin::INSTANCED, TargetFormat::Rgba8Unorm)
                .instance_input(builtin::INSTANCED_LAYOUT),
        )
        .expect("instanced pipeline");
    let buffer = device
        .create_buffer(64, BufferUsage::PerFrame)
        .expect("per-frame buffer");
    let bytes = [0u8; 24];
    refused("two items naming one buffer", &|target| {
        let color = clear(black);
        let items = [
            Item::new(&instanced).frame_data(FrameData::new(&buffer, &bytes, 1)),
            Item::new(&instanced).frame_data(FrameData::new(&buffer, &bytes, 1)),
        ];
        let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
    });
    let many: Vec<renew_rhi::Buffer> = (0..9)
        .map(|_| {
            device
                .create_buffer(64, BufferUsage::PerFrame)
                .expect("boundary buffer")
        })
        .collect();
    refused("a ninth distinct buffer", &|target| {
        let color = clear(black);
        let items: Vec<Item<'_>> = many
            .iter()
            .map(|buffer| Item::new(&instanced).frame_data(FrameData::new(buffer, &bytes, 1)))
            .collect();
        let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
    });

    // The refusals fired before any GPU work: the same target still
    // renders a well-formed frame, and validation stayed silent.
    let color = clear(black);
    let items = [Item::new(&pipeline)];
    target
        .render(&RenderDesc::new(&[Pass::new(&color, &items)]))
        .expect("the target survives every refusal");
    assert_no_validation_errors(&device);
}

#[test]
fn wrong_readback_length_is_a_retained_contract_check() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    let target = device
        .create_offscreen_target(Extent {
            width: 8,
            height: 8,
        })
        .expect("offscreen target");
    let mut wrong = vec![0u8; 7];
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        target.read_back_into(&mut wrong);
    }));
    assert!(outcome.is_err(), "short readback buffer must be rejected");
}

#[test]
fn samplers_are_created_and_dropped_without_validation_complaint() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    // Every combination the descriptor can express, not just the
    // preset: `atlas()` picks one corner of a two-by-two space, and a
    // filter or address mode that no test ever hands to the driver is
    // an untested conversion however well-covered its line is.
    for filter in [Filter::Nearest, Filter::Linear] {
        for address in [AddressMode::ClampToEdge, AddressMode::Repeat] {
            let mut desc = SamplerDesc::atlas();
            desc.filter = filter;
            desc.address = address;
            drop(device.create_sampler(&desc).expect("sampler"));
        }
    }
    assert_no_validation_errors(&device);
}

#[test]
fn zero_capacity_buffer_is_a_retained_contract_check() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    // Retained in release: the capacity bounds every later copy into the
    // mapping, so the refusal survives builds with debug assertions off,
    // exactly as the readback length guard does.
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = device.create_buffer(0, renew_rhi::BufferUsage::PerFrame);
    }));
    assert!(
        refused.is_err(),
        "a zero-capacity per-frame buffer must refuse, not allocate"
    );
}
