//! Fault-injection scenarios: the two creation calls this crate makes,
//! failed for real, so the error arms are executed rather than trusted.
//!
//! The fault layer (`tools/vk-fault-layer`) sits between this crate and
//! the driver and fails one named call per run. The fault is armed when
//! the instance is created, so every scenario builds its own device with
//! the fault already in the environment.
//!
//! Without the layer the suite skips loudly; under `RENEW_FAULT_STRICT`
//! (the CI lane) a skip is a failure, so the lane can never pass
//! vacuously.
//!
//! **Why a driver failure and not a constructed error.** The unit tests
//! beside the code build every variant by hand and check what each one
//! says. What they cannot show is that the `?` in `new` and `upload`
//! reaches the arm the hand-built value stands for — a mistranslation
//! there produces a perfectly well-worded error about the wrong thing.
//! Only a call that really fails proves the two halves are joined.

// Arming a fault is an environment write, `unsafe` since the 2024
// edition. Sound here: this file is a single `#[test]`, so nothing in
// the process reads or writes the environment concurrently — the one
// reader is the layer, on this thread, inside the calls below.
#![allow(unsafe_code)]

use renew_render3d::{CameraRenderer, MeshRenderer, Render3dError, Scene};
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
        app_name: "renew-render3d-fault-tests",
        validation: Validation::IfAvailable,
    })
}

/// One quad, which is the smallest scene that uploads.
fn one_quad() -> Scene {
    let mut scene = Scene::new();
    scene.quad(
        [
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5],
        ],
        [1.0, 1.0, 1.0, 1.0],
    );
    scene
}

#[test]
fn every_creation_arm_reports_its_own_failure() {
    // Canary: arm a fence fault and bring up a target — the same probe
    // the rendering crate's suite uses. If the target builds anyway, the
    // layer is not in the loader's path and every scenario below would
    // "pass" by succeeding.
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
            "RENEW_FAULT_STRICT=1 but fault injection is not active — the lane exists to run these scenarios"
        );
        eprintln!("SKIP: fault injection not active");
        return;
    }

    // R1 — pipeline creation fails: the Pipeline arm, and the Display
    // still says which pipeline it was building. This is the arm a
    // depthless adapter would otherwise be needed to reach, which is
    // exactly why the depth refusal is translated rather than detected.
    arm("vkCreateGraphicsPipelines=ERROR_OUT_OF_HOST_MEMORY");
    let device = new_device().expect("device for R1");
    match MeshRenderer::new(&device, TargetFormat::Rgba8Unorm) {
        Err(error @ Render3dError::Pipeline(_)) => {
            assert!(
                error.to_string().starts_with("building the mesh pipeline:"),
                "R1: Display lost its context: {error}"
            );
        }
        other => panic!("R1: expected the pipeline failure in the Pipeline arm, got {other:?}"),
    }
    drop(device);

    // R2 — the geometry buffer fails: the Upload arm, reached only after
    // the pipeline itself was built, so this also shows the two calls do
    // not share an arm.
    arm("vkCreateBuffer=ERROR_OUT_OF_HOST_MEMORY");
    let device = new_device().expect("device for R2");
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)
        .expect("R2: the pipeline is not the armed call");
    match renderer.upload(&device, &one_quad()) {
        Err(error @ Render3dError::Upload(_)) => {
            assert!(
                error.to_string().starts_with("uploading the geometry:"),
                "R2: Display lost its context: {error}"
            );
        }
        other => panic!("R2: expected the buffer failure in the Upload arm, got {other:?}"),
    }
    drop(renderer);
    drop(device);

    // R3 — the camera pipeline fails: the same arm as R1, reached through
    // a different constructor. Separate from R1 because the two build
    // different pipelines, and a camera renderer that quietly built the
    // mesh pipeline would pass R1 and draw the wrong thing.
    arm("vkCreateGraphicsPipelines=ERROR_OUT_OF_HOST_MEMORY");
    let device = new_device().expect("device for R3");
    match CameraRenderer::new(&device, TargetFormat::Rgba8Unorm) {
        Err(error @ Render3dError::Pipeline(_)) => {
            assert!(
                error.to_string().starts_with("building the mesh pipeline:"),
                "R3: Display lost its context: {error}"
            );
        }
        other => panic!("R3: expected the pipeline failure in the Pipeline arm, got {other:?}"),
    }
    drop(device);

    // R4 — the matrix buffer fails: the arm that exists because the
    // blanket conversion would otherwise report it as a geometry upload.
    // The pipeline is built by then, so this also shows the two calls in
    // one constructor do not share an arm.
    arm("vkCreateBuffer=ERROR_OUT_OF_HOST_MEMORY");
    let device = new_device().expect("device for R4");
    match CameraRenderer::new(&device, TargetFormat::Rgba8Unorm) {
        Err(error @ Render3dError::CameraBuffer(_)) => {
            assert!(
                error
                    .to_string()
                    .starts_with("allocating the camera's matrix buffer:"),
                "R4: Display lost its context: {error}"
            );
        }
        other => panic!(
            "R4: a sixty-four-byte allocation failing must not be reported as a geometry \
             upload, got {other:?}"
        ),
    }
    drop(device);
}
