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

use renew_render3d::{
    CameraRenderer, MeshRenderer, Render3dError, Scene, ShadowedCameraRenderer,
    TexturedCameraRenderer, TexturedMeshRenderer,
};
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
    match MeshRenderer::new(&device, TargetFormat::Rgba8Srgb) {
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
    let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Srgb)
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
    match CameraRenderer::new(&device, TargetFormat::Rgba8Srgb) {
        Err(error @ Render3dError::Pipeline(_)) => {
            assert!(
                error.to_string().starts_with("building the mesh pipeline:"),
                "R3: Display lost its context: {error}"
            );
        }
        other => panic!("R3: expected the pipeline failure in the Pipeline arm, got {other:?}"),
    }
    drop(device);

    // R4 — the camera constructor allocates no buffer, proven by arming
    // the FIRST buffer allocation to fail (the layer fails the
    // ordinal-th occurrence of a named call, not every occurrence — so
    // any buffer the constructor created would be occurrence one and
    // die). This scenario used to drive a `CameraBuffer` arm: the
    // constructor owned a sixty-four-byte per-frame buffer for the
    // matrix, and this fault reached it. The matrix rides push
    // constants now, recorded into the command stream per draw — so
    // construction must SUCCEED. Non-vacuous because R2 above armed the
    // identical directive and it bit: the directive works, and this
    // constructor simply never makes the call.
    arm("vkCreateBuffer=ERROR_OUT_OF_HOST_MEMORY");
    let device = new_device().expect("device for R4");
    match CameraRenderer::new(&device, TargetFormat::Rgba8Srgb) {
        Ok(renderer) => drop(renderer),
        Err(error) => panic!(
            "R4: the camera constructor owns no buffer, so a buffer fault must not reach \
             it — it failed with {error:?}"
        ),
    }
    drop(device);

    textured_constructors_report_their_own_failures();
}

/// Four white texels: the smallest texture a renderer will accept, and
/// these scenarios are about the refusal rather than the picture.
const WHITE: [u8; 16] = [255; 16];

/// The textured constructors' failure arms.
///
/// **A function rather than a second `#[test]`.** Arming a fault is an
/// environment write, and the safety argument at the top of this file is
/// that there is exactly one test in the binary — a sibling test would
/// run on another thread and both would be writing `RENEW_FAULT` while
/// the other read it. Called from the one test, this runs on that test's
/// thread and the argument holds.
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test scaffolding: the allowance clippy grants a #[test] body does not reach a               function it calls, and this is the same assertion code the test beside it uses"
)]
fn textured_constructors_report_their_own_failures() {
    /// A constructor under test: hand it a device, get back why it
    /// refused.
    type Build = fn(&Device) -> Option<Render3dError>;

    // R5 and R6 — the two textured constructors fail their pipeline the
    // same way the untextured ones do. Separate scenarios because each
    // builds a different pipeline from different shaders, and a
    // constructor that quietly built the wrong one would pass its
    // sibling's test and then draw the wrong thing.
    let small = Extent {
        width: 2,
        height: 2,
    };
    let camera: Build = |device| {
        TexturedCameraRenderer::new(
            device,
            TargetFormat::Rgba8Srgb,
            Extent {
                width: 2,
                height: 2,
            },
            &WHITE,
        )
        .err()
    };
    let plain: Build = |device| {
        TexturedMeshRenderer::new(
            device,
            TargetFormat::Rgba8Srgb,
            Extent {
                width: 2,
                height: 2,
            },
            &WHITE,
        )
        .err()
    };

    for (label, build) in [("R5", camera), ("R6", plain)] {
        arm("vkCreateGraphicsPipelines=ERROR_OUT_OF_HOST_MEMORY");
        let device = new_device().expect("the device should come up with only a pipeline armed");
        match build(&device) {
            Some(error @ Render3dError::Pipeline(_)) => {
                assert!(
                    error.to_string().starts_with("building the mesh pipeline:"),
                    "{label}: Display lost its context: {error}"
                );
            }
            other => {
                panic!("{label}: expected the pipeline failure in the Pipeline arm, got {other:?}")
            }
        }
        drop(device);
    }

    // R8 and R9 — the shadowed constructor builds TWO pipelines, and
    // each one's refusal must surface rather than being swallowed by
    // the other's success. The ordinal is what separates them: the
    // caster is created first, the lit pipeline second, so failing the
    // first occurrence proves the caster's unwind and failing the
    // second proves the lit one's. A constructor that built them in
    // the other order, or that ignored the first result, would pass
    // one of these and fail the other.
    for (label, ordinal) in [("R8", 1u32), ("R9", 2)] {
        arm(&format!(
            "vkCreateGraphicsPipelines=ERROR_OUT_OF_HOST_MEMORY@{ordinal}"
        ));
        let device = new_device().expect("the device should come up with only a pipeline armed");
        match ShadowedCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, small, &WHITE, 64) {
            Err(error @ Render3dError::Pipeline(_)) => {
                assert!(
                    error.to_string().starts_with("building the mesh pipeline:"),
                    "{label}: Display lost its context: {error}"
                );
            }
            other => panic!(
                "{label}: expected pipeline {ordinal} to fail into the Pipeline arm, got {other:?}"
            ),
        }
        drop(device);
    }

    // R10 — the shadow map itself is refused. The map is a render
    // image, not a texture, and its own failure must not be reported
    // as one: a reader sent to "creating the texture" would go looking
    // at the atlas, which is fine.
    arm("vkCreateImage=ERROR_OUT_OF_HOST_MEMORY@2");
    let device = new_device().expect("device for R10");
    match ShadowedCameraRenderer::new(&device, TargetFormat::Rgba8Srgb, small, &WHITE, 64) {
        Err(error @ Render3dError::Texture(_)) => {
            assert!(
                error.to_string().starts_with("creating the texture:"),
                "R10: Display lost its context: {error}"
            );
        }
        other => panic!("R10: expected the shadow map's failure to surface, got {other:?}"),
    }
    drop(device);

    // R7 — the image itself is refused. Its own arm, because it happens
    // before any scene exists, and reporting it as a geometry upload
    // would send a reader to look at something nobody has offered.
    arm("vkCreateImage=ERROR_OUT_OF_HOST_MEMORY");
    let device = new_device().expect("device for R7");
    match TexturedMeshRenderer::new(&device, TargetFormat::Rgba8Srgb, small, &WHITE) {
        Err(error @ Render3dError::Texture(_)) => {
            assert!(
                error.to_string().starts_with("creating the texture:"),
                "R7: Display lost its context: {error}"
            );
        }
        other => panic!("R7: expected the image failure in the Texture arm, got {other:?}"),
    }
    drop(device);
}
