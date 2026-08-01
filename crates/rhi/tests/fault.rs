//! Fault-injection scenarios: every driver-failure ladder in the
//! backend, executed for real.
//!
//! The fault layer (`tools/vk-fault-layer`) sits between this crate and
//! the driver and fails one named call per run. Two of its properties
//! shape this suite: the fault is armed when the instance is created
//! (so every scenario builds its own device with the fault already in
//! the environment), and it fires exactly once (so the call after the
//! failure succeeds — recovery needs no disarming).
//!
//! Without the layer the suite skips loudly; under `RENEW_FAULT_STRICT`
//! (the CI lane) a skip is a failure, so the lane can never pass
//! vacuously.

// Arming a fault is an environment write, `unsafe` since the 2024
// edition. Sound here: this file is a single `#[test]`, so nothing in
// the process reads or writes the environment concurrently — the one
// reader is the layer, on this thread, inside the calls below.
#![allow(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;

use renew_rhi::{
    Color, Device, DeviceDesc, DeviceError, Extent, PipelineDesc, PipelineError, TargetError,
    TargetFormat, Validation, builtin,
};

const SIZE: Extent = Extent {
    width: 64,
    height: 64,
};
const CLEAR: Color = Color::new(0.0, 0.0, 0.0, 1.0);

/// Bytes that pass this crate's structural SPIR-V gate — magic word,
/// word-aligned, non-empty — and nothing beyond it: version word zero,
/// no instructions. The validation layer rejects them itself, so
/// nothing invalid ever reaches the driver.
const IMPLAUSIBLE_SPIRV: [u8; 20] = {
    let mut bytes = [0u8; 20];
    let magic = 0x0723_0203u32.to_le_bytes();
    bytes[0] = magic[0];
    bytes[1] = magic[1];
    bytes[2] = magic[2];
    bytes[3] = magic[3];
    bytes
};

/// Records the diag channel, because two failure paths report nowhere
/// else: a teardown wait-idle that fails and validation findings at
/// instance destruction both happen after the last caller is gone, so
/// the diag record *is* the observable. Reading it here beats asserting
/// that the branch "must have" run.
struct Capture;

static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CAPTURE: Capture = Capture;

impl renew_diag::Sink for Capture {
    fn write(&self, record: &renew_diag::Record<'_>) {
        if let Ok(mut captured) = CAPTURED.lock() {
            captured.push(format!("{} {}", record.level(), record.message()));
        }
    }
}

/// Forget everything recorded so far, so the next scenario reads only
/// what it caused.
fn clear_records() {
    if let Ok(mut captured) = CAPTURED.lock() {
        captured.clear();
    }
}

/// Assert some record since the last [`clear_records`] contains
/// `needle`.
fn recorded(needle: &str) -> Verdict {
    match CAPTURED.lock() {
        Ok(captured) if captured.iter().any(|line| line.contains(needle)) => Ok(()),
        Ok(captured) => Err(format!(
            "no diagnostic contains {needle:?}; the channel carried {captured:?}"
        )),
        Err(_) => Err("the capture sink is poisoned".to_string()),
    }
}

/// The CI lane sets this: a skip becomes a failure.
fn strict() -> bool {
    std::env::var_os("RENEW_FAULT_STRICT").is_some_and(|value| value == "1")
}

/// Arm `spec` for the duration of `run`, then disarm.
fn with_fault<T>(spec: &str, run: impl FnOnce() -> T) -> T {
    // SAFETY: single-threaded by this suite's construction (one test).
    unsafe { std::env::set_var("RENEW_FAULT", spec) };
    let outcome = run();
    // SAFETY: as above.
    unsafe { std::env::remove_var("RENEW_FAULT") };
    outcome
}

/// Implicit layers (overlays, vendor shims) sit above this suite and
/// some of them RETRY calls that fail — an injected fault would then be
/// papered over by a layer nobody asked for, and the scenario would
/// silently measure the overlay instead of the backend. Observed on a
/// desktop with a vendor present layer and a capture hook installed.
/// The loader reads this when an instance is created, so setting it
/// before the first device is enough.
fn silence_implicit_layers() {
    // SAFETY: single-threaded; see the module note.
    unsafe { std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~") };
}

/// Run `body` with the validation layer struck from the loader's list,
/// then restore the suite's baseline. The loader reads this when an
/// instance is created, and drops a disabled layer from enumeration
/// too — which is exactly the "layer is not installed" condition
/// `Validation::Required` exists to report.
fn without_the_validation_layer<T>(run: impl FnOnce() -> T) -> T {
    // SAFETY: single-threaded by this suite's construction (one test).
    unsafe {
        std::env::set_var(
            "VK_LOADER_LAYERS_DISABLE",
            "~implicit~,VK_LAYER_KHRONOS_validation",
        );
    }
    let outcome = run();
    silence_implicit_layers();
    outcome
}

fn new_device() -> Result<Device, DeviceError> {
    Device::new(&DeviceDesc {
        app_name: "renew-rhi-fault-tests",
        validation: Validation::IfAvailable,
    })
}

fn pipeline_desc() -> PipelineDesc<'static> {
    PipelineDesc::new(
        builtin::TRIANGLE_VS_SPV,
        builtin::TRIANGLE_FS_SPV,
        TargetFormat::Rgba8Unorm,
    )
}

/// A scenario failure, carrying its own name.
type Verdict = Result<(), String>;

fn wrong<T: std::fmt::Debug>(name: &str, expected: &str, got: &T) -> String {
    if name.is_empty() {
        format!("expected {expected}, got {got:?}")
    } else {
        format!("{name}: expected {expected}, got {got:?}")
    }
}

/// Assert a target error is `Creation` naming `call`.
fn creation_named(name: &str, call: &str, got: &TargetError) -> Verdict {
    match got {
        TargetError::Creation { call: got_call, .. } if *got_call == call => Ok(()),
        other => Err(wrong(name, &format!("Creation({call})"), other)),
    }
}

/// Assert a target error is `Timeout` naming `call`.
fn timeout_named(name: &str, call: &str, got: &TargetError) -> Verdict {
    match got {
        TargetError::Timeout { call: got_call } if *got_call == call => Ok(()),
        other => Err(wrong(name, &format!("Timeout({call})"), other)),
    }
}

/// Every scenario ends here: the layer never makes validation dirty,
/// because the faults it injects are returned instead of reaching the
/// driver — anything the layer left half-built is the backend's own
/// unwinder to clean up, and validation is the judge of that.
fn validation_clean(name: &str, device: &Device) -> Verdict {
    let report = device.validation_report();
    if report.errors == 0 {
        Ok(())
    } else {
        Err(format!(
            "{name}: {} validation error(s) after the fault path; first: {:?}",
            report.errors, report.first_messages
        ))
    }
}

/// Bring-up scenario: the fault hits `Device::new` itself.
fn bringup_case(
    name: &str,
    fault: &str,
    check: impl FnOnce(Result<Device, DeviceError>) -> Verdict,
) -> Verdict {
    with_fault(fault, || check(new_device())).map_err(|error| format!("{name}: {error}"))
}

/// Post-bring-up scenario: the device must come up (its fault targets a
/// later call), the body runs the repro, validation must be clean.
fn device_case(name: &str, fault: &str, body: impl FnOnce(&Device) -> Verdict) -> Verdict {
    with_fault(fault, || {
        let device = new_device().map_err(|error| format!("{name}: device bring-up: {error}"))?;
        body(&device)?;
        validation_clean(name, &device)
    })
}

/// Teardown scenario: the body owns the device, because the path under
/// test is the spine's own `Drop` — it runs only when the last handle
/// goes, and validation cannot be consulted afterwards.
fn teardown_case(name: &str, body: impl FnOnce(Device) -> Verdict) -> Verdict {
    let device = new_device().map_err(|error| format!("{name}: device bring-up: {error}"))?;
    body(device).map_err(|error| format!("{name}: {error}"))
}

/// The whole suite, serial by construction: arming a fault is process
/// state, so exactly one test owns it.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario table; splitting it would scatter the ladder it walks"
)]
fn every_driver_failure_ladder_behaves() {
    silence_implicit_layers();
    renew_diag::install(&CAPTURE);

    // ---- canary: is the layer actually in the chain? ---------------
    let armed = with_fault("vkCreateFence=ERROR_OUT_OF_HOST_MEMORY", || {
        match new_device() {
            Ok(device) => match device.create_offscreen_target(SIZE) {
                // The layer failed the fence: faults are live.
                Err(TargetError::Creation {
                    call: "vkCreateFence",
                    ..
                }) => Some(true),
                // A target built despite the armed fault: no layer.
                Ok(_) => Some(false),
                Err(other) => {
                    eprintln!("canary: unexpected target error: {other:?}");
                    Some(false)
                }
            },
            Err(DeviceError::LoaderUnavailable { message }) => {
                eprintln!("no Vulkan runtime: {message}");
                None
            }
            Err(error) => {
                eprintln!("canary: device bring-up failed: {error}");
                Some(false)
            }
        }
    });
    match armed {
        Some(true) => {}
        other => {
            assert!(
                !strict(),
                "RENEW_FAULT_STRICT=1 but fault injection is not active \
                 (canary: {other:?}) — the lane exists to run these scenarios"
            );
            eprintln!("SKIP: fault injection not active (canary: {other:?})");
            return;
        }
    }

    // Whether the validation layer is installed at all: the scenarios
    // that walk the debug-messenger path or read validation's verdict
    // have nothing to observe without it, and say so rather than
    // passing vacuously.
    let validation_available = match new_device() {
        Ok(device) => device.validation_active(),
        Err(error) => {
            eprintln!("probe: device bring-up failed: {error}");
            false
        }
    };
    assert!(
        validation_available || !strict(),
        "RENEW_FAULT_STRICT=1 but the validation layer is not active — every          zero-validation-errors assertion below would be a claim about nothing.          Check that the layer search path still names the SDK's layer directory."
    );
    if !validation_available {
        eprintln!("note: no validation layer; the E4/E7 scenarios are skipped");
    }

    let mut verdicts: Vec<Verdict> = Vec::with_capacity(32);

    // ---- A · device bring-up ---------------------------------------
    verdicts.push(bringup_case(
        "A1 instance/incompatible-driver",
        "vkCreateInstance=ERROR_INCOMPATIBLE_DRIVER",
        |got| match got {
            Err(DeviceError::LoaderUnavailable { .. }) => Ok(()),
            other => Err(wrong("", "LoaderUnavailable", &other.map(|_| "a device"))),
        },
    ));
    verdicts.push(bringup_case(
        "A2 instance/out-of-host-memory",
        "vkCreateInstance=ERROR_OUT_OF_HOST_MEMORY",
        |got| match got {
            Err(DeviceError::OutOfHostMemory {
                call: "vkCreateInstance",
            }) => Ok(()),
            other => Err(wrong(
                "",
                "OutOfHostMemory(vkCreateInstance)",
                &other.map(|_| "a device"),
            )),
        },
    ));
    verdicts.push(bringup_case(
        "A3 enumerate-physical-devices",
        "vkEnumeratePhysicalDevices=ERROR_OUT_OF_HOST_MEMORY",
        |got| match got {
            Err(DeviceError::Creation {
                call: "vkEnumeratePhysicalDevices",
                ..
            }) => Ok(()),
            other => Err(wrong(
                "",
                "Creation(vkEnumeratePhysicalDevices)",
                &other.map(|_| "a device"),
            )),
        },
    ));
    verdicts.push(bringup_case(
        "A4 create-device/out-of-host-memory",
        "vkCreateDevice=ERROR_OUT_OF_HOST_MEMORY",
        |got| match got {
            Err(DeviceError::OutOfHostMemory {
                call: "vkCreateDevice",
            }) => Ok(()),
            other => Err(wrong(
                "",
                "OutOfHostMemory(vkCreateDevice)",
                &other.map(|_| "a device"),
            )),
        },
    ));

    // ---- B · offscreen bring-up unwinder ---------------------------
    // Each: the build fails at the named call, then a second build
    // succeeds (the fault is spent) — proving the unwinder left the
    // device usable and leaked nothing validation can see.
    let offscreen_ladder: &[(&str, &str, &str, bool)] = &[
        (
            "B1",
            "vkCreateImage=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateImage",
            false,
        ),
        (
            "B2",
            "vkAllocateMemory=ERROR_OUT_OF_DEVICE_MEMORY",
            "vkAllocateMemory(image)",
            true,
        ),
        (
            "B3",
            "vkBindImageMemory=ERROR_OUT_OF_HOST_MEMORY",
            "vkBindImageMemory",
            false,
        ),
        (
            "B4",
            "vkCreateImageView=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateImageView",
            false,
        ),
        (
            "B5",
            "vkCreateBuffer=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateBuffer",
            false,
        ),
        (
            "B6",
            "vkAllocateMemory=ERROR_OUT_OF_DEVICE_MEMORY@2",
            "vkAllocateMemory(readback)",
            true,
        ),
        (
            "B7",
            "vkBindBufferMemory=ERROR_OUT_OF_HOST_MEMORY",
            "vkBindBufferMemory",
            false,
        ),
        (
            "B8",
            "vkMapMemory=ERROR_OUT_OF_HOST_MEMORY",
            "vkMapMemory",
            false,
        ),
        (
            "B9",
            "vkCreateCommandPool=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateCommandPool",
            false,
        ),
        (
            "B10",
            "vkAllocateCommandBuffers=ERROR_OUT_OF_HOST_MEMORY",
            "vkAllocateCommandBuffers",
            false,
        ),
        (
            "B11",
            "vkCreateFence=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateFence",
            false,
        ),
    ];
    for &(name, fault, call, device_memory) in offscreen_ladder {
        verdicts.push(device_case(name, fault, |device| {
            let failed = device.create_offscreen_target(SIZE);
            match failed {
                Err(error) => {
                    if device_memory {
                        match &error {
                            TargetError::OutOfDeviceMemory { call: got } if *got == call => {}
                            other => {
                                return Err(wrong(
                                    name,
                                    &format!("OutOfDeviceMemory({call})"),
                                    other,
                                ));
                            }
                        }
                    } else {
                        creation_named(name, call, &error)?;
                    }
                }
                Ok(_) => return Err(format!("{name}: the build succeeded despite the fault")),
            }
            // The fault is spent: the unwinder left a working device.
            let mut recovered = device
                .create_offscreen_target(SIZE)
                .map_err(|error| format!("{name}: recovery build failed: {error}"))?;
            if recovered.extent() != SIZE {
                return Err(wrong(
                    name,
                    "a recovery target of the requested size",
                    &recovered.extent(),
                ));
            }
            recovered
                .render(CLEAR, None)
                .map_err(|error| format!("{name}: recovery render failed: {error}"))
        }));
    }

    // ---- C · pipeline unwinder -------------------------------------
    let pipeline_ladder: &[(&str, &str, &str)] = &[
        (
            "C1",
            "vkCreateShaderModule=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateShaderModule(vertex)",
        ),
        (
            "C2",
            "vkCreateShaderModule=ERROR_OUT_OF_HOST_MEMORY@2",
            "vkCreateShaderModule(fragment)",
        ),
        (
            "C3",
            "vkCreatePipelineLayout=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreatePipelineLayout",
        ),
        (
            "C4",
            "vkCreateGraphicsPipelines=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateGraphicsPipelines",
        ),
    ];
    for &(name, fault, call) in pipeline_ladder {
        verdicts.push(device_case(name, fault, |device| {
            match device.create_pipeline(&pipeline_desc()) {
                Err(PipelineError::Creation { call: got, .. }) if got == call => {}
                Err(other) => return Err(wrong(name, &format!("Creation({call})"), &other)),
                Ok(_) => return Err(format!("{name}: the build succeeded despite the fault")),
            }
            let pipeline = device
                .create_pipeline(&pipeline_desc())
                .map_err(|error| format!("{name}: recovery build failed: {error}"))?;
            let mut target = device
                .create_offscreen_target(SIZE)
                .map_err(|error| format!("{name}: recovery target failed: {error}"))?;
            target
                .render(CLEAR, Some(&pipeline))
                .map_err(|error| format!("{name}: recovery render failed: {error}"))
        }));
    }

    // ---- D · offscreen render ladder -------------------------------
    // D1-D3: the frame fails before submission, so nothing is in
    // flight: the target stays usable and the next frame succeeds.
    let recoverable_render: &[(&str, &str, &str)] = &[
        (
            "D1",
            "vkResetCommandBuffer=ERROR_OUT_OF_HOST_MEMORY",
            "vkResetCommandBuffer",
        ),
        (
            "D2",
            "vkBeginCommandBuffer=ERROR_OUT_OF_HOST_MEMORY",
            "vkBeginCommandBuffer",
        ),
        (
            "D3",
            "vkEndCommandBuffer=ERROR_OUT_OF_HOST_MEMORY",
            "vkEndCommandBuffer",
        ),
        (
            "D4",
            "vkQueueSubmit2=ERROR_OUT_OF_HOST_MEMORY",
            "vkQueueSubmit2",
        ),
    ];
    for &(name, fault, call) in recoverable_render {
        verdicts.push(device_case(name, fault, |device| {
            let mut target = device
                .create_offscreen_target(SIZE)
                .map_err(|error| format!("{name}: target: {error}"))?;
            match target.render(CLEAR, None) {
                Err(error) => creation_named(name, call, &error)?,
                Ok(()) => return Err(format!("{name}: the frame succeeded despite the fault")),
            }
            // Not wedged, not poisoned: the next frame goes through.
            target
                .render(CLEAR, None)
                .map_err(|error| format!("{name}: recovery frame failed: {error}"))?;
            let mut pixels = vec![0u8; target.byte_len()];
            target.read_back_into(&mut pixels);
            Ok(())
        }));
    }

    // D5: submission reports device loss — the device poisons and every
    // later operation on it fails fast.
    verdicts.push(device_case(
        "D5 submit/device-lost",
        "vkQueueSubmit2=ERROR_DEVICE_LOST",
        |device| {
            let mut target = device
                .create_offscreen_target(SIZE)
                .map_err(|error| format!("D5: target: {error}"))?;
            match target.render(CLEAR, None) {
                Err(TargetError::DeviceLost) => {}
                other => return Err(wrong("D5", "DeviceLost", &other)),
            }
            match target.render(CLEAR, None) {
                Err(TargetError::DeviceLost) => {}
                other => return Err(wrong("D5", "DeviceLost on the next frame", &other)),
            }
            match device.wait_idle() {
                Err(DeviceError::DeviceLost) => {}
                other => return Err(wrong("D5", "DeviceLost from wait_idle", &other)),
            }
            match device.create_offscreen_target(SIZE) {
                Err(TargetError::DeviceLost) => Ok(()),
                Ok(_) => Err("D5: a poisoned device still built a target".to_string()),
                Err(other) => Err(wrong("D5", "DeviceLost from create", &other)),
            }
        },
    ));

    // D6: the fence wait times out — work may still be in flight, so
    // the target wedges and refuses to hand out pixels.
    verdicts.push(device_case(
        "D6 fence/timeout-wedges",
        "vkWaitForFences=TIMEOUT",
        |device| {
            let mut target = device
                .create_offscreen_target(SIZE)
                .map_err(|error| format!("D6: target: {error}"))?;
            match target.render(CLEAR, None) {
                Err(error) => timeout_named("D6", "vkWaitForFences", &error)?,
                Ok(()) => return Err("D6: the frame succeeded despite the fault".to_string()),
            }
            match target.render(CLEAR, None) {
                Err(error) => {
                    timeout_named("D6", "target wedged by an earlier incomplete frame", &error)?;
                }
                Ok(()) => return Err("D6: a wedged target rendered".to_string()),
            }
            let mut pixels = vec![0u8; target.byte_len()];
            // The contract check below is supposed to panic; keep the
            // default hook's output out of a passing run's log.
            let hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let read = catch_unwind(AssertUnwindSafe(|| target.read_back_into(&mut pixels)));
            std::panic::set_hook(hook);
            if read.is_ok() {
                return Err("D6: a wedged target handed out pixels".to_string());
            }
            Ok(())
        },
    ));

    // D7: the fence wait reports device loss — poison AND wedge.
    verdicts.push(device_case(
        "D7 fence/device-lost",
        "vkWaitForFences=ERROR_DEVICE_LOST",
        |device| {
            let mut target = device
                .create_offscreen_target(SIZE)
                .map_err(|error| format!("D7: target: {error}"))?;
            match target.render(CLEAR, None) {
                Err(TargetError::DeviceLost) => {}
                other => return Err(wrong("D7", "DeviceLost", &other)),
            }
            // Wedged first, so the wedge answer wins over the poison.
            match target.render(CLEAR, None) {
                Err(error) => {
                    timeout_named("D7", "target wedged by an earlier incomplete frame", &error)?;
                }
                Ok(()) => return Err("D7: a wedged target rendered".to_string()),
            }
            match device.wait_idle() {
                Err(DeviceError::DeviceLost) => Ok(()),
                other => Err(wrong("D7", "DeviceLost from wait_idle", &other)),
            }
        },
    ));

    // D8: resetting the fence fails after a completed frame — the
    // fence's state is unknown, so the target wedges rather than
    // submitting against it again.
    verdicts.push(device_case(
        "D8 reset-fences/wedges",
        "vkResetFences=ERROR_OUT_OF_HOST_MEMORY",
        |device| {
            let mut target = device
                .create_offscreen_target(SIZE)
                .map_err(|error| format!("D8: target: {error}"))?;
            match target.render(CLEAR, None) {
                Err(error) => creation_named("D8", "vkResetFences", &error)?,
                Ok(()) => return Err("D8: the frame succeeded despite the fault".to_string()),
            }
            match target.render(CLEAR, None) {
                Err(error) => {
                    timeout_named("D8", "target wedged by an earlier incomplete frame", &error)
                }
                Ok(()) => Err("D8: a wedged target rendered".to_string()),
            }
        },
    ));

    // D9: an explicit wait-idle reporting loss poisons the device.
    verdicts.push(device_case(
        "D9 wait-idle/device-lost",
        "vkDeviceWaitIdle=ERROR_DEVICE_LOST",
        |device| {
            match device.wait_idle() {
                Err(DeviceError::DeviceLost) => {}
                other => return Err(wrong("D9", "DeviceLost", &other)),
            }
            match device.create_offscreen_target(SIZE) {
                Err(TargetError::DeviceLost) => Ok(()),
                Ok(_) => Err("D9: a poisoned device still built a target".to_string()),
                Err(other) => Err(wrong("D9", "DeviceLost from create", &other)),
            }
        },
    ));

    // ---- E · the rest of the bring-up ladder ------------------------
    // A1/A2 and A4 pin the two codes bring-up translates specially;
    // E1/E2 pin the fall-through that carries every other code
    // through with the call that produced it.
    verdicts.push(bringup_case(
        "E1 instance/initialization-failed",
        "vkCreateInstance=ERROR_INITIALIZATION_FAILED",
        |got| match got {
            Err(DeviceError::Creation {
                call: "vkCreateInstance",
                ..
            }) => Ok(()),
            other => Err(wrong(
                "",
                "Creation(vkCreateInstance)",
                &other.map(|_| "a device"),
            )),
        },
    ));
    verdicts.push(bringup_case(
        "E2 create-device/initialization-failed",
        "vkCreateDevice=ERROR_INITIALIZATION_FAILED",
        |got| match got {
            Err(DeviceError::Creation {
                call: "vkCreateDevice",
                ..
            }) => Ok(()),
            other => Err(wrong(
                "",
                "Creation(vkCreateDevice)",
                &other.map(|_| "a device"),
            )),
        },
    ));
    // E3 fails between the instance and the device: the unwinder has an
    // instance (and possibly a messenger) to take back down.
    verdicts.push(bringup_case(
        "E3 enumerate-device-extensions",
        "vkEnumerateDeviceExtensionProperties=ERROR_OUT_OF_HOST_MEMORY",
        |got| match got {
            Err(DeviceError::Creation {
                call: "vkEnumerateDeviceExtensionProperties",
                ..
            }) => Ok(()),
            other => Err(wrong(
                "",
                "Creation(vkEnumerateDeviceExtensionProperties)",
                &other.map(|_| "a device"),
            )),
        },
    ));
    if validation_available {
        // E4: the instance is up but its messenger is not, so the
        // instance must come back down before the error is returned.
        verdicts.push(bringup_case(
            "E4 debug-messenger",
            "vkCreateDebugUtilsMessengerEXT=ERROR_OUT_OF_HOST_MEMORY",
            |got| match got {
                Err(DeviceError::Creation {
                    call: "vkCreateDebugUtilsMessengerEXT",
                    ..
                }) => Ok(()),
                other => Err(wrong(
                    "",
                    "Creation(vkCreateDebugUtilsMessengerEXT)",
                    &other.map(|_| "a device"),
                )),
            },
        ));
    }

    // E5: a wait-idle that fails without losing the device — reported
    // with its call, and the device keeps working (D9 covers the loss).
    verdicts.push(device_case(
        "E5 wait-idle/out-of-host-memory",
        "vkDeviceWaitIdle=ERROR_OUT_OF_HOST_MEMORY",
        |device| {
            match device.wait_idle() {
                Err(DeviceError::Creation {
                    call: "vkDeviceWaitIdle",
                    ..
                }) => {}
                other => return Err(wrong("E5", "Creation(vkDeviceWaitIdle)", &other)),
            }
            device.wait_idle().map_err(|error| {
                format!("E5: a device that was never lost stopped working: {error}")
            })?;
            let mut target = device
                .create_offscreen_target(SIZE)
                .map_err(|error| format!("E5: target: {error}"))?;
            target
                .render(CLEAR, None)
                .map_err(|error| format!("E5: frame after a failed wait-idle: {error}"))
        },
    ));

    // E6: the teardown wait-idle fails. There is no caller left to
    // return to, so the diag channel is the whole contract.
    verdicts.push(with_fault(
        "vkDeviceWaitIdle=ERROR_OUT_OF_HOST_MEMORY",
        || {
            teardown_case("E6 teardown/wait-idle-failure", |device| {
                // Nothing has quiesced this device yet, so the spine's own
                // teardown owns the first (and only) wait-idle.
                clear_records();
                drop(device);
                recorded("wait-idle at teardown failed")
            })
        },
    ));

    if validation_available {
        // E7: validation findings must still be reported when the last
        // caller is gone — the layer's own leak check at instance
        // destruction lands in exactly that window.
        verdicts.push(teardown_case("E7 teardown/validation-report", |device| {
            match device.create_pipeline(&PipelineDesc::new(
                &IMPLAUSIBLE_SPIRV,
                builtin::TRIANGLE_FS_SPV,
                TargetFormat::Rgba8Unorm,
            )) {
                Err(PipelineError::Creation { .. }) => {}
                Err(other) => return Err(wrong("", "Creation(vkCreateShaderModule)", &other)),
                Ok(_) => return Err("the layer accepted implausible SPIR-V".to_string()),
            }
            let report = device.validation_report();
            if report.errors == 0 {
                return Err("implausible SPIR-V drew no validation error".to_string());
            }
            clear_records();
            drop(device);
            recorded("validation reported")
        }));
    }

    // E8: `Validation::Required` with the layer struck from the
    // loader's list — the one policy that refuses to build a device it
    // cannot police.
    verdicts.push(without_the_validation_layer(|| {
        match Device::new(&DeviceDesc {
            app_name: "renew-rhi-fault-tests",
            validation: Validation::Required,
        }) {
            Err(DeviceError::ValidationUnavailable) => Ok(()),
            other => Err(wrong(
                "E8 validation/required-but-absent",
                "ValidationUnavailable",
                &other.map(|_| "a device"),
            )),
        }
    }));

    // ---- F · the rest of the frame ladder ---------------------------
    // D6 and D7 cover the fence wait timing out and losing the device;
    // F1 is every other way it can fail: reported with its call, and
    // still a wedge, because the submit is no less in flight for the
    // wait having failed differently.
    verdicts.push(device_case(
        "F1 fence-wait/out-of-host-memory",
        "vkWaitForFences=ERROR_OUT_OF_HOST_MEMORY",
        |device| {
            let mut target = device
                .create_offscreen_target(SIZE)
                .map_err(|error| format!("F1: target: {error}"))?;
            match target.render(CLEAR, None) {
                Err(error) => creation_named("F1", "vkWaitForFences", &error)?,
                Ok(()) => return Err("F1: the frame succeeded despite the fault".to_string()),
            }
            match target.render(CLEAR, None) {
                Err(error) => {
                    timeout_named("F1", "target wedged by an earlier incomplete frame", &error)
                }
                Ok(()) => Err("F1: a wedged target rendered".to_string()),
            }
        },
    ));

    // ---- report -----------------------------------------------------
    let failures: Vec<String> = verdicts.into_iter().filter_map(Result::err).collect();
    assert!(
        failures.is_empty(),
        "{} fault scenario(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
