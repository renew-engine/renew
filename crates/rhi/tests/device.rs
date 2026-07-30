//! Device lifecycle under `Validation::Required`: bring-up, the full
//! frame cycle, churn, drop-order freedom, and error paths — with the
//! validation layer as the correctness oracle (zero errors, mechanically
//! asserted, on every path).
//!
//! Environments without a Vulkan runtime or without the validation
//! layer skip: correctness is proven where the oracle exists.

use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, PipelineDesc, PipelineError, TargetFormat,
    Validation, builtin,
};

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
        .create_pipeline(&PipelineDesc {
            vertex_spirv: builtin::TRIANGLE_VS_SPV,
            fragment_spirv: builtin::TRIANGLE_FS_SPV,
            target_format: TargetFormat::Rgba8Unorm,
        })
        .expect("triangle pipeline");

    let clear = Color::new(0.0, 0.0, 0.0, 1.0);
    target.render(clear, None).expect("clear-only render");
    let mut cleared = vec![0u8; target.byte_len()];
    target.read_back_into(&mut cleared);

    target
        .render(clear, Some(&pipeline))
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
        target
            .render(Color::new(0.5, 0.5, 0.5, 1.0), None)
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
        .create_pipeline(&PipelineDesc {
            vertex_spirv: builtin::TRIANGLE_VS_SPV,
            fragment_spirv: builtin::TRIANGLE_FS_SPV,
            target_format: TargetFormat::Rgba8Unorm,
        })
        .expect("pipeline");
    // The handle goes away; the spine lives on through the resources.
    drop(device);
    target
        .render(Color::new(0.0, 0.0, 0.0, 1.0), Some(&pipeline))
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
    match device.create_pipeline(&PipelineDesc {
        vertex_spirv: &bad,
        fragment_spirv: builtin::TRIANGLE_FS_SPV,
        target_format: TargetFormat::Rgba8Unorm,
    }) {
        Err(PipelineError::InvalidSpirv { stage, .. }) => assert_eq!(stage, "vertex"),
        Err(other) => panic!("expected vertex rejection, got {other:?}"),
        Ok(_) => panic!("expected vertex rejection, got a pipeline"),
    }
    match device.create_pipeline(&PipelineDesc {
        vertex_spirv: builtin::TRIANGLE_VS_SPV,
        fragment_spirv: &[],
        target_format: TargetFormat::Rgba8Unorm,
    }) {
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
        .create_pipeline(&PipelineDesc {
            vertex_spirv: builtin::TRIANGLE_VS_SPV,
            fragment_spirv: builtin::TRIANGLE_FS_SPV,
            target_format: TargetFormat::Rgba8Unorm,
        })
        .expect("pipeline on the other device");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = target.render(Color::new(0.0, 0.0, 0.0, 1.0), Some(&foreign));
    }));
    assert!(
        outcome.is_err(),
        "mixing objects across devices must trip the dev-build contract check"
    );
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
