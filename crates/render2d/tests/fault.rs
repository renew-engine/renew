//! Fault-injection scenarios: every rhi creation call `SpriteRenderer::
//! new` makes, failed for real, so the error arms are executed rather
//! than trusted.
//!
//! The fault layer (`tools/vk-fault-layer`) sits between the rendering
//! crate and the driver and fails one named call per run. The fault is
//! armed when the instance is created, so every scenario builds its own
//! device with the fault already in the environment.
//!
//! Without the layer the suite skips loudly; under `RENEW_FAULT_STRICT`
//! (the CI lane) a skip is a failure, so the lane can never pass
//! vacuously.

// Arming a fault is an environment write, `unsafe` since the 2024
// edition. Sound here: this file is a single `#[test]`, so nothing in
// the process reads or writes the environment concurrently — the one
// reader is the layer, on this thread, inside the calls below.
#![allow(unsafe_code)]

use renew_render2d::{AtlasDesc, Canvas, Render2dError, SpriteRenderer};
use renew_rhi::{Device, DeviceDesc, DeviceError, Extent, TargetError, TargetFormat, Validation};

/// The CI lane sets this: a skip becomes a failure.
fn strict() -> bool {
    std::env::var_os("RENEW_FAULT_STRICT").is_some_and(|value| value == "1")
}

fn arm(directive: &str) {
    // SAFETY: single-test binary; no concurrent environment access —
    // see the file-top note.
    unsafe { std::env::set_var("RENEW_FAULT", directive) };
}

fn new_device() -> Result<Device, DeviceError> {
    Device::new(&DeviceDesc {
        app_name: "renew-render2d-fault-tests",
        validation: Validation::IfAvailable,
    })
}

/// Build a renderer against `device`, expecting failure; returns the
/// error for the scenario to classify. Canvas and capacity travel in
/// from the test body, where refusing their construction may panic.
fn build(
    device: &Device,
    canvas: Canvas,
    capacity: core::num::NonZeroU32,
) -> Result<SpriteRenderer, Render2dError> {
    let atlas: [u8; 16] = [255; 16];
    SpriteRenderer::new(
        device,
        &AtlasDesc::new(
            Extent {
                width: 2,
                height: 2,
            },
            &atlas,
        ),
        canvas,
        TargetFormat::Rgba8Unorm,
        capacity,
    )
}

#[test]
fn every_creation_arm_reports_its_own_failure() {
    let canvas = Canvas::new(64, 64).expect("nonzero canvas");
    let capacity = core::num::NonZeroU32::new(4).expect("nonzero capacity");
    // Canary: arm a fence fault and bring up a target — the same probe
    // the rendering crate's suite uses. If the target builds anyway,
    // the layer is not in the loader's path.
    arm("vkCreateFence=ERROR_OUT_OF_HOST_MEMORY");
    let armed = match new_device() {
        Ok(device) => match device.create_offscreen_target(Extent {
            width: 4,
            height: 4,
        }) {
            Err(TargetError::Creation {
                call: "vkCreateFence",
                ..
            }) => true,
            Ok(_) => false,
            Err(other) => {
                eprintln!("canary: unexpected target error: {other:?}");
                false
            }
        },
        Err(DeviceError::LoaderUnavailable { message }) => {
            assert!(
                !strict(),
                "RENEW_FAULT_STRICT=1 but no Vulkan runtime: {message}"
            );
            eprintln!("SKIP: no Vulkan runtime: {message}");
            return;
        }
        Err(error) => {
            eprintln!("canary: device bring-up failed: {error}");
            false
        }
    };
    if !armed {
        assert!(
            !strict(),
            "RENEW_FAULT_STRICT=1 but fault injection is not active — \
             the lane exists to run these scenarios"
        );
        eprintln!("SKIP: fault injection not active");
        return;
    }

    // R1 — the atlas upload fails: the Target arm, from create_texture.
    arm("vkCreateImage=ERROR_OUT_OF_HOST_MEMORY");
    let device = new_device().expect("device for R1");
    match build(&device, canvas, capacity) {
        Err(Render2dError::Target(_)) => {}
        other => panic!("R1: expected the texture failure in the Target arm, got {other:?}"),
    }
    drop(device);

    // R2 — the sampler fails: the Pipeline arm, from create_sampler.
    arm("vkCreateSampler=ERROR_OUT_OF_HOST_MEMORY");
    let device = new_device().expect("device for R2");
    match build(&device, canvas, capacity) {
        Err(Render2dError::Pipeline(_)) => {}
        other => panic!("R2: expected the sampler failure in the Pipeline arm, got {other:?}"),
    }
    drop(device);

    // R3 — pipeline creation fails: the Pipeline arm, from
    // create_pipeline, and the error's Display carries the context.
    arm("vkCreateGraphicsPipelines=ERROR_OUT_OF_HOST_MEMORY");
    let device = new_device().expect("device for R3");
    match build(&device, canvas, capacity) {
        Err(error @ Render2dError::Pipeline(_)) => {
            assert!(
                error
                    .to_string()
                    .starts_with("building the sprite pipeline:"),
                "R3: Display lost its context: {error}"
            );
        }
        other => panic!("R3: expected the pipeline failure in the Pipeline arm, got {other:?}"),
    }
    drop(device);

    // R4 — the per-frame buffer fails: the Target arm, from
    // create_buffer, after every earlier resource succeeded.
    arm("vkCreateBuffer=ERROR_OUT_OF_HOST_MEMORY");
    let device = new_device().expect("device for R4");
    match build(&device, canvas, capacity) {
        Err(error @ Render2dError::Target(_)) => {
            assert!(
                error
                    .to_string()
                    .starts_with("building the sprite renderer's resources:"),
                "R4: Display lost its context: {error}"
            );
        }
        other => panic!("R4: expected the buffer failure in the Target arm, got {other:?}"),
    }
    drop(device);

    // Disarm for any later process reuse.
    // SAFETY: same single-test argument as `arm`.
    unsafe { std::env::remove_var("RENEW_FAULT") };
}
