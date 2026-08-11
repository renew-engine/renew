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
    Attachment, BindingDesc, BindingSource, BufferUsage, ClearValue, Color, Device, DeviceDesc,
    DeviceError, Extent, Item, LoadOp, MeshDesc, Pass, PipelineDesc, PipelineError, RenderDesc,
    Sampler, SamplerDesc, Shaders, StoreOp, TargetError, TargetFormat, Texture, TextureDesc,
    Validation, builtin,
};

const SIZE: Extent = Extent {
    width: 64,
    height: 64,
};
const CLEAR: Color = Color::new(0.0, 0.0, 0.0, 1.0);

/// The one color attachment these frames render into: cleared, stored.
fn clear(color: Color) -> [Attachment; 1] {
    [Attachment::new(
        LoadOp::Clear(ClearValue::Color(color)),
        StoreOp::Store,
    )]
}

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

/// A two-by-two atlas: the smallest thing a real upload can carry, and
/// large enough that a row-stride error would show.
const TEXEL_SIZE: Extent = Extent {
    width: 2,
    height: 2,
};
const TEXELS: [u8; 16] = [
    10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
];

/// The texture and sampler a binding needs, built with no fault armed
/// against them — the binding ladder arms calls that only
/// `create_binding` makes, so these must succeed first.
fn textured_inputs(device: &Device) -> Result<(Texture, Sampler), String> {
    let texture = device
        .create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS))
        .map_err(|error| format!("texture: {error}"))?;
    let sampler = device
        .create_sampler(&SamplerDesc::atlas())
        .map_err(|error| format!("sampler: {error}"))?;
    Ok((texture, sampler))
}

fn binding_desc<'a>(texture: &'a Texture, sampler: &'a Sampler) -> BindingDesc<'a> {
    BindingDesc::new(BindingSource::Texture(texture), sampler)
}

fn sampled_desc() -> PipelineDesc<'static> {
    PipelineDesc::new(builtin::TEXTURED, TargetFormat::Rgba8Unorm).sampled_bindings(1)
}

fn pipeline_desc() -> PipelineDesc<'static> {
    PipelineDesc::new(builtin::TRIANGLE, TargetFormat::Rgba8Unorm)
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
    verdicts.push(bringup_case(
        "A5 sampled-set-layout/out-of-host-memory",
        "vkCreateDescriptorSetLayout=ERROR_OUT_OF_HOST_MEMORY",
        |got| match got {
            Err(DeviceError::OutOfHostMemory {
                call: "vkCreateDescriptorSetLayout",
            }) => Ok(()),
            other => Err(wrong(
                "",
                "OutOfHostMemory(vkCreateDescriptorSetLayout)",
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
    // The depth half of the bring-up unwinder, ordinal-pinned: the
    // depth image is created last in the build, after the color image
    // (create/bind/view ordinal 1) and the readback allocation
    // (vkAllocateMemory ordinal 2), so its calls are the second — or
    // for memory the third — of their names. Runs only where the
    // adapter offers a depth format; a depthless adapter never makes
    // ordinal 2 of these calls, and the guard keeps the armed fault
    // from silently outliving the scenario.
    let depth_ladder: &[(&str, &str, &str, bool)] = &[
        (
            "B12",
            "vkCreateImage=ERROR_OUT_OF_HOST_MEMORY@2",
            "vkCreateImage(depth)",
            false,
        ),
        (
            "B13",
            "vkAllocateMemory=ERROR_OUT_OF_DEVICE_MEMORY@3",
            "vkAllocateMemory(depth)",
            true,
        ),
        (
            "B14",
            "vkBindImageMemory=ERROR_OUT_OF_HOST_MEMORY@2",
            "vkBindImageMemory(depth)",
            false,
        ),
        (
            "B15",
            "vkCreateImageView=ERROR_OUT_OF_HOST_MEMORY@2",
            "vkCreateImageView(depth)",
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
            let color = clear(CLEAR);
            recovered
                .render(&RenderDesc::new(&[Pass::new(&color, &[])]))
                .map_err(|error| format!("{name}: recovery render failed: {error}"))
        }));
    }
    for &(name, fault, call, device_memory) in depth_ladder {
        verdicts.push(device_case(name, fault, |device| {
            if device.depth_format_name().is_none() {
                eprintln!("SKIP {name}: adapter offers no depth format");
                return Ok(());
            }
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
            let color = clear(CLEAR);
            recovered
                .render(&RenderDesc::new(&[Pass::new(&color, &[])]))
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
            let color = clear(CLEAR);
            let items = [Item::new(&pipeline)];
            target
                .render(&RenderDesc::new(&[Pass::new(&color, &items)]))
                .map_err(|error| format!("{name}: recovery render failed: {error}"))
        }));
    }

    // C5 sits outside the loop above because it exercises a different
    // constructor: the ladder's body builds a pipeline, and a sampler
    // is built by its own call. Recovery is asserted the same way --
    // an armed fault fires once, so the second attempt must succeed.
    verdicts.push(device_case(
        "C5",
        "vkCreateSampler=ERROR_OUT_OF_HOST_MEMORY",
        |device| {
            match device.create_sampler(&SamplerDesc::atlas()) {
                Err(PipelineError::Creation {
                    call: "vkCreateSampler",
                    ..
                }) => {}
                Err(other) => return Err(wrong("C5", "Creation(vkCreateSampler)", &other)),
                Ok(_) => return Err("C5: the sampler was created despite the fault".to_owned()),
            }
            let recovered = device
                .create_sampler(&SamplerDesc::atlas())
                .map_err(|error| format!("C5: recovery sampler failed: {error}"))?;
            // `Debug` is asserted here rather than in the device
            // suite: that suite skips wherever the validation layer is
            // absent, which is most environments.
            let shown = format!("{recovered:?}");
            if !shown.starts_with("Sampler") {
                return Err(format!("C5: unexpected Debug form: {shown}"));
            }
            Ok(())
        },
    ));

    // ---- C7-C8 · binding ladder --------------------------------------
    // A binding is the only thing that allocates descriptors now, so
    // these arm the fault and then build one — retargeted from pipeline
    // creation when the pool and set moved out of it. Each is a
    // distinct creation call inside `create_binding`, and each must
    // leave the device able to build the same binding on a second
    // attempt. C6 (descriptor-set-layout creation) moved to the
    // bring-up ladder as A5: the layout is the device spine's one
    // shared object, created with the device, so arming that call
    // fails bring-up rather than binding creation.
    let binding_ladder: &[(&str, &str, &str)] = &[
        (
            "C7",
            "vkCreateDescriptorPool=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateDescriptorPool",
        ),
        // C8 fails *after* the pool exists, which is the case that has
        // to unwind it again — nothing else reaches that cleanup.
        (
            "C8",
            "vkAllocateDescriptorSets=ERROR_OUT_OF_HOST_MEMORY",
            "vkAllocateDescriptorSets",
        ),
    ];
    for &(name, fault, call) in binding_ladder {
        verdicts.push(device_case(name, fault, |device| {
            let (texture, sampler) = textured_inputs(device)
                .map_err(|error| format!("{name}: textured inputs: {error}"))?;
            match device.create_binding(&binding_desc(&texture, &sampler)) {
                Err(PipelineError::Creation { call: got, .. }) if got == call => {}
                Err(other) => return Err(wrong(name, &format!("Creation({call})"), &other)),
                Ok(_) => return Err(format!("{name}: the binding was built despite the fault")),
            }
            device
                .create_binding(&binding_desc(&texture, &sampler))
                .map(|_| ())
                .map_err(|error| format!("{name}: recovery binding failed: {error}"))
        }));
    }

    // ---- C9-C10 · slot-declaring pipeline ladder ---------------------
    // The same two calls the untextured ladder above arms, through a
    // pipeline that declares a sampled slot — the path whose layout
    // list is non-empty. The descriptor pool and set moved to the
    // binding ladder; what is left to prove here is that a declaring
    // pipeline's failures unwind cleanly and a second attempt succeeds.
    let slot_ladder: &[(&str, &str, &str)] = &[
        (
            "C9",
            "vkCreatePipelineLayout=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreatePipelineLayout",
        ),
        (
            "C10",
            "vkCreateGraphicsPipelines=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateGraphicsPipelines",
        ),
    ];
    for &(name, fault, call) in slot_ladder {
        verdicts.push(device_case(name, fault, |device| {
            match device.create_pipeline(&sampled_desc()) {
                Err(PipelineError::Creation { call: got, .. }) if got == call => {}
                Err(other) => return Err(wrong(name, &format!("Creation({call})"), &other)),
                Ok(_) => return Err(format!("{name}: the build succeeded despite the fault")),
            }
            device
                .create_pipeline(&sampled_desc())
                .map(|_| ())
                .map_err(|error| format!("{name}: recovery build failed: {error}"))
        }));
    }

    // ---- T · texture upload ladder ---------------------------------
    // Every fallible call `create_texture` makes, in the order it makes
    // them. The upload is synchronous and owns transient staging state,
    // so each failure must both surface the right call and leave
    // nothing behind — which the validation layer, consulted after each
    // case by `device_case`, is what actually proves.
    let texture_ladder: &[(&str, &str, &str, bool)] = &[
        (
            "T1",
            "vkCreateImage=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateImage",
            false,
        ),
        (
            "T2",
            "vkAllocateMemory=ERROR_OUT_OF_DEVICE_MEMORY",
            "vkAllocateMemory(texture)",
            true,
        ),
        (
            "T3",
            "vkBindImageMemory=ERROR_OUT_OF_HOST_MEMORY",
            "vkBindImageMemory",
            false,
        ),
        (
            "T4",
            "vkCreateImageView=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateImageView",
            false,
        ),
        (
            "T5",
            "vkCreateBuffer=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateBuffer",
            false,
        ),
        (
            "T6",
            "vkAllocateMemory=ERROR_OUT_OF_HOST_MEMORY@2",
            "vkAllocateMemory(staging)",
            false,
        ),
        (
            "T7",
            "vkBindBufferMemory=ERROR_OUT_OF_HOST_MEMORY",
            "vkBindBufferMemory",
            false,
        ),
        (
            "T8",
            "vkMapMemory=ERROR_OUT_OF_HOST_MEMORY",
            "vkMapMemory",
            false,
        ),
        (
            "T9",
            "vkCreateCommandPool=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateCommandPool",
            false,
        ),
        (
            "T10",
            "vkAllocateCommandBuffers=ERROR_OUT_OF_HOST_MEMORY",
            "vkAllocateCommandBuffers",
            false,
        ),
        (
            "T11",
            "vkCreateFence=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateFence",
            false,
        ),
        (
            "T12",
            "vkBeginCommandBuffer=ERROR_OUT_OF_HOST_MEMORY",
            "vkBeginCommandBuffer",
            false,
        ),
        (
            "T13",
            "vkEndCommandBuffer=ERROR_OUT_OF_HOST_MEMORY",
            "vkEndCommandBuffer",
            false,
        ),
        (
            "T14",
            "vkQueueSubmit2=ERROR_OUT_OF_HOST_MEMORY",
            "vkQueueSubmit2",
            false,
        ),
        (
            "T15",
            "vkWaitForFences=ERROR_OUT_OF_HOST_MEMORY",
            "vkWaitForFences(texture upload)",
            false,
        ),
    ];
    // T16 is the same call reporting the one non-error outcome that is
    // still a failure for us: the upload did not finish in time. It sits
    // outside the table because its verdict is a different variant — a
    // timeout is not a creation error, and folding it in would hide the
    // case where a driver merely needs longer.
    verdicts.push(device_case("T16", "vkWaitForFences=TIMEOUT", |device| {
        match device.create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS)) {
            Err(TargetError::Timeout {
                call: "vkWaitForFences(texture upload)",
            }) => {}
            Err(other) => {
                return Err(wrong(
                    "T16",
                    "Timeout(vkWaitForFences(texture upload))",
                    &other,
                ));
            }
            Ok(_) => {
                return Err("T16: the upload reported success despite timing out".to_owned());
            }
        }
        device
            .create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS))
            .map(|_| ())
            .map_err(|error| format!("T16: recovery upload failed: {error}"))
    }));

    // T17 is the one submit failure that must do more than report: a
    // lost device has to be recorded on the shared spine, or every
    // later render passes its own guard and submits to a dead device.
    // Asserted through a second call, because the poison flag is not
    // public and its whole purpose is what the *next* call does.
    verdicts.push(device_case(
        "T17",
        "vkQueueSubmit2=ERROR_DEVICE_LOST",
        |device| {
            match device.create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS)) {
                Err(TargetError::DeviceLost) => {}
                Err(other) => return Err(wrong("T17", "DeviceLost", &other)),
                Ok(_) => return Err("T17: the upload survived a lost device".to_owned()),
            }
            match device.create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS)) {
                Err(TargetError::DeviceLost) => Ok(()),
                Err(other) => Err(wrong("T17", "DeviceLost on the next call", &other)),
                Ok(_) => {
                    Err("T17: the device was not poisoned, so the next upload proceeded".to_owned())
                }
            }
        },
    ));

    // T18 is the same loss reported one call later. The submit succeeds
    // and the *wait* discovers the device is gone, which is a separate
    // arm with its own quiesce — T17 cannot reach it, and a loss that
    // only surfaces at the fence is the ordinary way a GPU hang is
    // reported.
    verdicts.push(device_case(
        "T18",
        "vkWaitForFences=ERROR_DEVICE_LOST",
        |device| {
            match device.create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS)) {
                Err(TargetError::DeviceLost) => {}
                Err(other) => return Err(wrong("T18", "DeviceLost", &other)),
                Ok(_) => return Err("T18: the upload survived a lost device".to_owned()),
            }
            match device.create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS)) {
                Err(TargetError::DeviceLost) => Ok(()),
                Err(other) => Err(wrong("T18", "DeviceLost on the next call", &other)),
                Ok(_) => {
                    Err("T18: the device was not poisoned, so the next upload proceeded".to_owned())
                }
            }
        },
    ));
    for &(name, fault, call, device_memory) in texture_ladder {
        verdicts.push(device_case(name, fault, |device| {
            match device.create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS)) {
                Err(error) => {
                    let matched = if device_memory {
                        matches!(&error, TargetError::OutOfDeviceMemory { call: got } if *got == call)
                    } else {
                        matches!(&error, TargetError::Creation { call: got, .. } if *got == call)
                    };
                    if !matched {
                        return Err(wrong(name, call, &error));
                    }
                }
                Ok(_) => return Err(format!("{name}: the upload succeeded despite the fault")),
            }
            device
                .create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS))
                .map(|_| ())
                .map_err(|error| format!("{name}: recovery upload failed: {error}"))
        }));
    }

    // ---- B · per-frame buffer ladder --------------------------------
    // Every fallible call `create_buffer` makes, in order. Each case
    // must surface the right call, leave nothing behind (the validation
    // layer consulted after each case proves the unwinder ran), and
    // recover: the same creation succeeds once the fault clears. No
    // ordinals: within one `create_buffer` each interposed call runs
    // exactly once, and the pinned `@2` scenarios elsewhere drive paths
    // that never construct a buffer, so their counts are undisturbed.
    let buffer_ladder: &[(&str, &str, &str)] = &[
        (
            "PB1",
            "vkCreateBuffer=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateBuffer",
        ),
        (
            "PB2",
            "vkAllocateMemory=ERROR_OUT_OF_HOST_MEMORY",
            "vkAllocateMemory",
        ),
        (
            "PB3",
            "vkBindBufferMemory=ERROR_OUT_OF_HOST_MEMORY",
            "vkBindBufferMemory",
        ),
        ("PB4", "vkMapMemory=ERROR_OUT_OF_HOST_MEMORY", "vkMapMemory"),
    ];
    for &(name, fault, call) in buffer_ladder {
        verdicts.push(device_case(name, fault, |device| {
            match device.create_buffer(64, BufferUsage::PerFrame) {
                Err(error) => {
                    if !matches!(&error, TargetError::Creation { call: got, .. } if *got == call) {
                        return Err(wrong(name, call, &error));
                    }
                }
                Ok(_) => {
                    return Err(format!("{name}: creation succeeded despite the fault"));
                }
            }
            device
                .create_buffer(64, BufferUsage::PerFrame)
                .map(|_| ())
                .map_err(|error| format!("{name}: recovery creation failed: {error}"))
        }));
    }

    // PB5 is not a ladder rung: the ladder above fails one driver call
    // and checks which one is named, while this checks the refusal that
    // happens before any call at all.
    //
    // **It is the one scenario here whose absence was a real defect.**
    // `create_buffer` had no poison gate while all seven other resource
    // entry points did, and none of the four calls it makes lists
    // `VK_ERROR_DEVICE_LOST` among its return codes -- so on a dead
    // device it did not fail loudly or even quietly, it *succeeded*, and
    // handed back a live buffer. The loss is induced through a texture
    // upload, the same way T17 induces it, because poisoning needs a
    // submit and creating a buffer performs none.
    verdicts.push(device_case(
        "PB5",
        "vkQueueSubmit2=ERROR_DEVICE_LOST",
        |device| {
            match device.create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS)) {
                Err(TargetError::DeviceLost) => {}
                Err(other) => return Err(wrong("PB5", "DeviceLost", &other)),
                Ok(_) => return Err("PB5: the upload survived a lost device".to_owned()),
            }
            match device.create_buffer(64, BufferUsage::PerFrame) {
                Err(TargetError::DeviceLost) => Ok(()),
                Err(other) => Err(wrong("PB5", "DeviceLost from create_buffer", &other)),
                Ok(_) => Err(
                    "PB5: a per-frame buffer was created on a lost device, which is the \
                     defect this scenario exists for"
                        .to_owned(),
                ),
            }
        },
    ));

    // ---- MB · mesh buffer ladder --------------------------------------------
    // Every fallible call `create_mesh` makes, in order, on the same
    // terms as the buffer ladder above: surface the right call, leave
    // nothing behind, and recover.
    //
    // **The no-ordinal justification carries over rather than being
    // assumed.** `create_mesh` performs each interposed call exactly
    // once — it maps host-visible memory and copies into it, with no
    // staging buffer, no second allocation and no transfer submit. Had
    // it staged, `vkCreateBuffer` and `vkAllocateMemory` would each run
    // twice per creation, which would silently re-aim every pinned `@2`
    // scenario in this file.
    let mesh_ladder: &[(&str, &str, &str)] = &[
        (
            "MB1",
            "vkCreateBuffer=ERROR_OUT_OF_HOST_MEMORY",
            "vkCreateBuffer",
        ),
        (
            "MB2",
            "vkAllocateMemory=ERROR_OUT_OF_HOST_MEMORY",
            "vkAllocateMemory(mesh)",
        ),
        (
            "MB3",
            "vkBindBufferMemory=ERROR_OUT_OF_HOST_MEMORY",
            "vkBindBufferMemory",
        ),
        ("MB4", "vkMapMemory=ERROR_OUT_OF_HOST_MEMORY", "vkMapMemory"),
    ];
    let mesh_bytes = [0u8; 28 * 3];
    let mesh_indices = [0u32, 1, 2];
    for &(name, fault, call) in mesh_ladder {
        verdicts.push(device_case(name, fault, |device| {
            let desc = MeshDesc::new(&mesh_bytes, 28, &mesh_indices);
            match device.create_mesh(&desc) {
                Err(error) => {
                    if !matches!(&error, TargetError::Creation { call: got, .. } if *got == call) {
                        return Err(wrong(name, call, &error));
                    }
                }
                Ok(_) => {
                    return Err(format!("{name}: creation succeeded despite the fault"));
                }
            }
            device
                .create_mesh(&desc)
                .map(|_| ())
                .map_err(|error| format!("{name}: recovery creation failed: {error}"))
        }));
    }

    // MB5: the poison gate on mesh creation. Every resource constructor
    // in the crate refuses on a lost device before touching the driver,
    // and this proves the mesh one does — reached the way T17 reaches its
    // own, by losing the device on an unrelated upload first and then
    // asking. The four rungs above cannot get here: they inject
    // out-of-host-memory, which fails a call without poisoning anything.
    verdicts.push(device_case(
        "MB5",
        "vkQueueSubmit2=ERROR_DEVICE_LOST",
        |device| {
            match device.create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS)) {
                Err(TargetError::DeviceLost) => {}
                Err(other) => return Err(wrong("MB5", "DeviceLost", &other)),
                Ok(_) => return Err("MB5: the upload survived a lost device".to_owned()),
            }
            let desc = MeshDesc::new(&mesh_bytes, 28, &mesh_indices);
            match device.create_mesh(&desc) {
                Err(TargetError::DeviceLost) => Ok(()),
                Err(other) => Err(wrong("MB5", "DeviceLost from create_mesh", &other)),
                Ok(_) => Err("MB5: a mesh was built on a lost device".to_owned()),
            }
        },
    ));

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
            let color = clear(CLEAR);
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
                Err(error) => creation_named(name, call, &error)?,
                Ok(()) => return Err(format!("{name}: the frame succeeded despite the fault")),
            }
            // Not wedged, not poisoned: the next frame goes through.
            target
                .render(&RenderDesc::new(&[Pass::new(&color, &[])]))
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
            let color = clear(CLEAR);
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
                Err(TargetError::DeviceLost) => {}
                other => return Err(wrong("D5", "DeviceLost", &other)),
            }
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
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
            let color = clear(CLEAR);
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
                Err(error) => timeout_named("D6", "vkWaitForFences", &error)?,
                Ok(()) => return Err("D6: the frame succeeded despite the fault".to_string()),
            }
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
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
            let color = clear(CLEAR);
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
                Err(TargetError::DeviceLost) => {}
                other => return Err(wrong("D7", "DeviceLost", &other)),
            }
            // Wedged first, so the wedge answer wins over the poison.
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
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
            let color = clear(CLEAR);
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
                Err(error) => creation_named("D8", "vkResetFences", &error)?,
                Ok(()) => return Err("D8: the frame succeeded despite the fault".to_string()),
            }
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
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
    // E12 fails after the device exists: the unwinder has the device
    // itself to take back down beside the instance and messenger.
    verdicts.push(bringup_case(
        "E12 sampled-set-layout/initialization-failed",
        "vkCreateDescriptorSetLayout=ERROR_INITIALIZATION_FAILED",
        |got| match got {
            Err(DeviceError::Creation {
                call: "vkCreateDescriptorSetLayout",
                ..
            }) => Ok(()),
            other => Err(wrong(
                "",
                "Creation(vkCreateDescriptorSetLayout)",
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
            let color = clear(CLEAR);
            target
                .render(&RenderDesc::new(&[Pass::new(&color, &[])]))
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
                Shaders::new(&IMPLAUSIBLE_SPIRV, builtin::TRIANGLE_FS_SPV, 3),
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
            let color = clear(CLEAR);
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
                Err(error) => creation_named("F1", "vkWaitForFences", &error)?,
                Ok(()) => return Err("F1: the frame succeeded despite the fault".to_string()),
            }
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
                Err(error) => {
                    timeout_named("F1", "target wedged by an earlier incomplete frame", &error)
                }
                Ok(()) => Err("F1: a wedged target rendered".to_string()),
            }
        },
    ));

    // F2: the wedge arm with per-frame bytes in flight. A timed-out
    // tail wait returns with the submit possibly still reading the
    // bound vertex buffer, and the caller's borrow ends at that return
    // — dropping the handle must free nothing the submit can touch,
    // which is the retention table's whole job. The scenario is the
    // arm's exercise: wedge, drop, and the run ends without a crash and
    // validation-clean (device_case's closing check).
    verdicts.push(device_case(
        "F2 wedge/retains-frame-buffers",
        "vkWaitForFences=TIMEOUT",
        |device| {
            let mut target = device
                .create_offscreen_target(SIZE)
                .map_err(|error| format!("F2: target: {error}"))?;
            let instanced = device
                .create_pipeline(
                    &PipelineDesc::new(builtin::INSTANCED, TargetFormat::Rgba8Unorm)
                        .instance_input(builtin::INSTANCED_LAYOUT),
                )
                .map_err(|error| format!("F2: instanced pipeline: {error}"))?;
            let buffer = device
                .create_buffer(24, renew_rhi::BufferUsage::PerFrame)
                .map_err(|error| format!("F2: buffer: {error}"))?;
            let bytes = [0u8; 24];
            let color = clear(CLEAR);
            let items =
                [Item::new(&instanced).frame_data(renew_rhi::FrameData::new(&buffer, &bytes, 1))];
            match target.render(&RenderDesc::new(&[Pass::new(&color, &items)])) {
                Err(error) => timeout_named("F2", "vkWaitForFences", &error)?,
                Ok(()) => return Err("F2: the frame succeeded despite the fault".to_string()),
            }
            // The caller is done with its handle; the target is not.
            drop(buffer);
            match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
                Err(error) => {
                    timeout_named("F2", "target wedged by an earlier incomplete frame", &error)?;
                }
                Ok(()) => return Err("F2: a wedged target rendered".to_string()),
            }
            // Drop's quiesce is what finally proves the work ended and
            // releases the retained memory.
            drop(target);
            Ok(())
        },
    ));

    // ---- E continued: every silent teardown discard, logged ---------
    // The layer arms when an instance is created, so each case wraps
    // its whole scenario in `with_fault` exactly as E6 does, and each
    // window's first wait-idle is reasoned out beside its drop. The
    // no-ordinal spec is first-match: one wait-idle per window is
    // faulted, the rest run clean — and E6's own premise holds in ITS
    // window untouched, because every case here owns a separate one.

    // E9: the offscreen target's teardown wait-idle fails — the diag
    // record is the only observable.
    verdicts.push(with_fault(
        "vkDeviceWaitIdle=ERROR_OUT_OF_HOST_MEMORY",
        || {
            teardown_case("E9 offscreen-teardown/wait-idle-failure", |device| {
                let target = device
                    .create_offscreen_target(SIZE)
                    .map_err(|error| format!("target: {error}"))?;
                clear_records();
                // Nothing before this drop performed a wait-idle
                // (bring-up and target creation make none), so the
                // target's own Drop owns the window's first — the
                // faulted one; the spine's follows, unfaulted.
                drop(target);
                recorded("wait-idle at offscreen teardown failed")?;
                drop(device);
                Ok(())
            })
        },
    ));

    // E10: the pipeline's teardown wait-idle fails — same contract.
    verdicts.push(with_fault(
        "vkDeviceWaitIdle=ERROR_OUT_OF_HOST_MEMORY",
        || {
            teardown_case("E10 pipeline-teardown/wait-idle-failure", |device| {
                let pipeline = device
                    .create_pipeline(&pipeline_desc())
                    .map_err(|error| format!("pipeline: {error}"))?;
                clear_records();
                // Pipeline creation performs no wait-idle: its Drop
                // owns the window's first — the faulted one.
                drop(pipeline);
                recorded("wait-idle at pipeline teardown failed")?;
                drop(device);
                Ok(())
            })
        },
    ));

    // E11: the upload machinery's guarded teardown wait-idle fails —
    // logged, and the texture itself still comes out whole (the wait is
    // best-effort cleanup, not part of the upload's correctness).
    verdicts.push(with_fault(
        "vkDeviceWaitIdle=ERROR_OUT_OF_HOST_MEMORY",
        || {
            teardown_case("E11 upload-teardown/wait-idle-failure", |device| {
                clear_records();
                // The upload's fence-guarded teardown inside
                // create_texture performs the window's first wait-idle
                // (the upload's fence wait is vkWaitForFences, another
                // name) — the faulted one.
                let texture = device
                    .create_texture(&TextureDesc::new(TEXEL_SIZE, &TEXELS))
                    .map_err(|error| {
                        format!("the upload must survive a logged failure: {error}")
                    })?;
                recorded("wait-idle at upload teardown failed")?;
                drop(texture);
                drop(device);
                Ok(())
            })
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
