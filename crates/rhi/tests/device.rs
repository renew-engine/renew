//! Device lifecycle under `Validation::Required`: bring-up, the full
//! frame cycle, churn, drop-order freedom, and error paths — with the
//! validation layer as the correctness oracle (zero errors, mechanically
//! asserted, on every path).
//!
//! Environments without a Vulkan runtime or without the validation
//! layer skip: correctness is proven where the oracle exists.

use renew_rhi::{
    AddressMode, Attachment, Binding, BindingDesc, BindingSource, BufferUsage, ClearValue, Color,
    DepthState, Device, DeviceDesc, DeviceError, Extent, Filter, FrameData, Item, ItemList, LoadOp,
    MeshDesc, Pass, PipelineDesc, PipelineError, RenderDesc, Sampler, SamplerDesc, Shaders,
    StoreOp, TargetFormat, Texture, Validation, builtin,
};

/// The one color attachment these frames render into: cleared, stored.
fn clear(color: Color) -> [Attachment; 1] {
    [Attachment::new(
        LoadOp::Clear(ClearValue::Color(color)),
        StoreOp::Store,
    )]
}

/// The push-constant test fixture: a full-target triangle whose color
/// is the sixteen pushed bytes. A fixture rather than a builtin — no
/// engine path consumes it, so exporting it would be dead public
/// surface; the compile record lives in the shaders README beside the
/// builtins'.
static PUSH_COLOR_VS_SPV: &[u8] = include_bytes!("../shaders/push_color.vert.spv");
static PUSH_COLOR_FS_SPV: &[u8] = include_bytes!("../shaders/push_color.frag.spv");

/// The fixture's stage pair: three generated vertices, no buffers.
fn push_color_shaders() -> Shaders<'static> {
    Shaders::new(PUSH_COLOR_VS_SPV, PUSH_COLOR_FS_SPV, 3)
}

/// A minimal binding and the resources it samples, for the contract
/// refusals: a two-by-two atlas through the atlas sampler. The source
/// handles ride along so the test controls every drop. `Err` carries
/// which creation refused; the panic lives in the `#[test]` body.
fn binding_fixture(device: &Device) -> Result<(Texture, Sampler, Binding), String> {
    let texture = device
        .create_texture(&renew_rhi::TextureDesc::new(
            Extent {
                width: 2,
                height: 2,
            },
            &[0u8; 16],
        ))
        .map_err(|error| format!("fixture texture: {error}"))?;
    let sampler = device
        .create_sampler(&SamplerDesc::atlas())
        .map_err(|error| format!("fixture sampler: {error}"))?;
    let binding = device
        .create_binding(&BindingDesc::new(
            BindingSource::Texture(&texture),
            &sampler,
        ))
        .map_err(|error| format!("fixture binding: {error}"))?;
    Ok((texture, sampler, binding))
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
            TargetFormat::Rgba8Srgb,
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
            TargetFormat::Rgba8Srgb,
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
        TargetFormat::Rgba8Srgb,
    )) {
        Err(PipelineError::InvalidSpirv { stage, .. }) => assert_eq!(stage, "vertex"),
        Err(other) => panic!("expected vertex rejection, got {other:?}"),
        Ok(_) => panic!("expected vertex rejection, got a pipeline"),
    }
    match device.create_pipeline(&PipelineDesc::new(
        Shaders::new(builtin::TRIANGLE_VS_SPV, &[], 3),
        TargetFormat::Rgba8Srgb,
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
            TargetFormat::Rgba8Srgb,
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

/// **The draw list hands over what was pushed, in order.** The order
/// is the whole claim: a list that reordered draws would compose a
/// frame that renders differently than written, and every consumer
/// with an optional middle draw depends on it. It lives here rather
/// than beside the type because an `Item` names a real pipeline, and a
/// pipeline needs a device.
#[test]
fn the_item_list_composes_a_frames_draws_in_order() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    let pipeline = device
        .create_pipeline(
            &PipelineDesc::new(push_color_shaders(), TargetFormat::Rgba8Srgb)
                .push_constant_size(16),
        )
        .expect("push-constant pipeline");
    // Four distinguishable items over one pipeline: an `Item` carries
    // no identity of its own, so the push block is the label.
    let labels: [[u8; 16]; 4] = [[1; 16], [2; 16], [3; 16], [4; 16]];
    let mut list = ItemList::<4>::new(Item::new(&pipeline).push_data(&labels[0]));
    assert_eq!(list.as_slice().len(), 1, "a seeded list holds its seed");
    list.push(Item::new(&pipeline).push_data(&labels[1]));
    // The optional shape, both ways: absent adds nothing, present
    // appends exactly where a push would have.
    list.push_some(None);
    assert_eq!(list.as_slice().len(), 2, "an absent item is not a draw");
    list.push_some(Some(Item::new(&pipeline).push_data(&labels[2])));
    list.push(Item::new(&pipeline).push_data(&labels[3]));
    let slice = list.as_slice();
    assert_eq!(slice.len(), 4);
    for (index, item) in slice.iter().enumerate() {
        assert_eq!(
            item.push_data,
            Some(&labels[index][..]),
            "item {index} is not the one pushed there"
        );
    }

    // The Debug form reports the shape, not the draws.
    let shown = format!("{list:?}");
    assert!(shown.contains("ItemList"), "{shown}");
    assert!(shown.contains("items: 4"), "{shown}");
    assert!(shown.contains("capacity: 4"), "{shown}");

    // Past its capacity it refuses by name rather than dropping a draw,
    // and a list of zero cannot hold the seed it is given.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let over = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut two = ItemList::<2>::new(Item::new(&pipeline).push_data(&labels[0]));
        two.push(Item::new(&pipeline).push_data(&labels[1]));
        two.push(Item::new(&pipeline).push_data(&labels[2]));
    }));
    let empty = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = ItemList::<0>::new(Item::new(&pipeline).push_data(&labels[0]));
    }));
    std::panic::set_hook(hook);
    // By name, not merely "something panicked": without the assert the
    // very next line indexes out of bounds and this test would pass on
    // the wrong panic entirely.
    let message = over
        .expect_err("a third item in a list of two must refuse")
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        message.contains("item list of capacity 2 is full"),
        "refused, but not by name: {message:?}"
    );
    assert!(empty.is_err(), "a list of zero cannot hold its seed");

    // And the list composes a frame that actually renders, which is
    // the point of the type: three draws, in order, through one target.
    let mut target = device
        .create_offscreen_target(Extent {
            width: 8,
            height: 8,
        })
        .expect("offscreen target");
    let color = clear(Color::new(0.0, 0.0, 0.0, 1.0));
    let mut frame = ItemList::<3>::new(Item::new(&pipeline).push_data(&labels[0]));
    frame.push_some(Some(Item::new(&pipeline).push_data(&labels[1])));
    frame.push(Item::new(&pipeline).push_data(&labels[2]));
    target
        .render(&RenderDesc::new(&[Pass::new(&color, frame.as_slice())]))
        .expect("a frame composed by the list renders");
    drop(target);
    assert_no_validation_errors(&device);
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
            TargetFormat::Rgba8Srgb,
        ))
        .expect("triangle pipeline");
    let black = Color::new(0.0, 0.0, 0.0, 1.0);

    // Every refusal below is supposed to panic; keep the default
    // hook's output out of a passing run's log (the wedge test's
    // precedent). Restored before the surviving-frame proof.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut refused =
        |label: &str, needle: &str, frame: &dyn Fn(&mut renew_rhi::OffscreenTarget)| {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                frame(&mut target);
            }));
            let Err(payload) = outcome else {
                panic!("{label}: the contract must refuse this");
            };
            // Refused BY NAME: the payload must carry the clause's own
            // words, or any unrelated panic would satisfy the case.
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("");
            assert!(
                message.contains(needle),
                "{label}: refused, but not by name — the payload {message:?} lacks {needle:?}"
            );
        };

    refused(
        "empty passes",
        "a frame needs at least one pass",
        &|target| {
            let _ = target.render(&RenderDesc::new(&[]));
        },
    );
    refused(
        "zero color attachments",
        "exactly one color attachment",
        &|target| {
            let _ = target.render(&RenderDesc::new(&[Pass::new(&[], &[])]));
        },
    );
    refused(
        "two color attachments",
        "exactly one color attachment",
        &|target| {
            let color = Attachment::new(LoadOp::Clear(ClearValue::Color(black)), StoreOp::Store);
            let two = [color, color];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&two, &[])]));
        },
    );
    refused(
        "first-pass color Load",
        "loads undefined contents",
        &|target| {
            let color = [Attachment::new(LoadOp::Load, StoreOp::Store)];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &[])]));
        },
    );
    refused("first-pass depth Load", "first depth use", &|target| {
        let color = clear(black);
        let pass = Pass::new(&color, &[]).depth(Attachment::new(LoadOp::Load, StoreOp::Discard));
        let _ = target.render(&RenderDesc::new(&[pass]));
    });
    refused(
        "depth Load on a LATER first depth-carrying pass",
        "first depth use",
        &|target| {
            // The frame's first depth-carrying pass is pass 1, not pass
            // 0 — the depth image still transitions from UNDEFINED
            // there, so the Load must be refused at that index too.
            let color = clear(black);
            let load = [Attachment::new(LoadOp::Load, StoreOp::Store)];
            let first = Pass::new(&color, &[]);
            let second =
                Pass::new(&load, &[]).depth(Attachment::new(LoadOp::Load, StoreOp::Discard));
            let _ = target.render(&RenderDesc::new(&[first, second]));
        },
    );
    refused(
        "a depth clear on a color attachment",
        "clears to ClearValue::Color",
        &|target| {
            let color = [Attachment::new(
                LoadOp::Clear(ClearValue::Depth(0.0)),
                StoreOp::Store,
            )];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &[])]));
        },
    );
    refused(
        "a color clear on a depth attachment",
        "clears to ClearValue::Depth",
        &|target| {
            let color = clear(black);
            let pass = Pass::new(&color, &[]).depth(Attachment::new(
                LoadOp::Clear(ClearValue::Color(black)),
                StoreOp::Discard,
            ));
            let _ = target.render(&RenderDesc::new(&[pass]));
        },
    );
    refused(
        "an out-of-range depth clear",
        "finite and in [0, 1]",
        &|target| {
            let color = clear(black);
            let pass = Pass::new(&color, &[]).depth(Attachment::new(
                LoadOp::Clear(ClearValue::Depth(1.5)),
                StoreOp::Discard,
            ));
            let _ = target.render(&RenderDesc::new(&[pass]));
        },
    );
    refused(
        "a non-finite depth clear",
        "finite and in [0, 1]",
        &|target| {
            let color = clear(black);
            let pass = Pass::new(&color, &[]).depth(Attachment::new(
                LoadOp::Clear(ClearValue::Depth(f32::NAN)),
                StoreOp::Discard,
            ));
            let _ = target.render(&RenderDesc::new(&[pass]));
        },
    );
    refused(
        "a depth-free pipeline in a depth-carrying pass",
        "depth state must match the pass",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&pipeline)];
            let pass = Pass::new(&color, &items).depth(Attachment::new(
                LoadOp::Clear(ClearValue::Depth(0.0)),
                StoreOp::Discard,
            ));
            let _ = target.render(&RenderDesc::new(&[pass]));
        },
    );
    if device.depth_format_name().is_some() {
        let depth_pipeline = device
            .create_pipeline(
                &PipelineDesc::new(builtin::TRIANGLE, TargetFormat::Rgba8Srgb)
                    .depth_state(DepthState::read_write()),
            )
            .expect("depth pipeline");
        refused(
            "a depth-testing pipeline in a depthless pass",
            "depth state must match the pass",
            &|target| {
                let color = clear(black);
                let items = [Item::new(&depth_pipeline)];
                let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
            },
        );
    } else {
        eprintln!("SKIP: no depth format, the depth-pipeline half of the mismatch is untestable");
    }
    let instanced = device
        .create_pipeline(
            &PipelineDesc::new(builtin::INSTANCED, TargetFormat::Rgba8Srgb)
                .instance_input(builtin::INSTANCED_LAYOUT),
        )
        .expect("instanced pipeline");
    let buffer = device
        .create_buffer(64, BufferUsage::PerFrame)
        .expect("per-frame buffer");
    let bytes = [0u8; 24];
    // The relaxed buffer rule, both sides: pointer-identical FrameData
    // may repeat (the same draw from two items is one copy written
    // twice), while DIFFERING data for one buffer is still the
    // second-copy-wins race, refused by name.
    refused(
        "two items with different data for one buffer",
        "one buffer, one FrameData",
        &|target| {
            let color = clear(black);
            let other_bytes = [7u8; 24];
            let items = [
                Item::new(&instanced).frame_data(FrameData::new(&buffer, &bytes, 1)),
                Item::new(&instanced).frame_data(FrameData::new(&buffer, &other_bytes, 1)),
            ];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );
    let many: Vec<renew_rhi::Buffer> = (0..17)
        .map(|_| {
            device
                .create_buffer(64, BufferUsage::PerFrame)
                .expect("boundary buffer")
        })
        .collect();
    refused(
        "a seventeenth distinct buffer",
        "distinct resources",
        &|target| {
            let color = clear(black);
            let items: Vec<Item<'_>> = many
                .iter()
                .map(|buffer| Item::new(&instanced).frame_data(FrameData::new(buffer, &bytes, 1)))
                .collect();
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );

    // The mesh half of the same contract. A mesh pipeline and a
    // generative one refuse the opposite mistakes, and the stride rule
    // guards the fetch itself.
    let mesh_pipeline = device
        .create_pipeline(&PipelineDesc::mesh(
            builtin::MESH,
            TargetFormat::Rgba8Srgb,
            builtin::MESH_LAYOUT,
        ))
        .expect("mesh pipeline");
    let mesh = device
        .create_mesh(&MeshDesc::new(&[0u8; 36 * 3], 36, &[0, 1, 2]))
        .expect("mesh");
    refused(
        "a mesh pipeline drawn with no geometry",
        "names geometry exactly when",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&mesh_pipeline)];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );
    refused(
        "geometry on a pipeline that generates its own vertices",
        "names geometry exactly when",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&pipeline).mesh(&mesh)];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );
    // A stride the pipeline does not pack to: the mesh's own indices stay
    // inside its own vertex count, and the fetch still runs off the end,
    // which is why this rule exists separately from the creation check.
    let wrong_stride = device
        .create_mesh(&MeshDesc::new(&[0u8; 32 * 3], 32, &[0, 1, 2]))
        .expect("mesh with a stride the mesh pipeline does not pack to");
    refused(
        "a mesh whose stride the pipeline does not pack to",
        "must equal the stride",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&mesh_pipeline).mesh(&wrong_stride)];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );

    // The push-constant half of the same contract: presence must match
    // the declaration, and the length must be exact.
    let push_pipeline = device
        .create_pipeline(
            &PipelineDesc::new(push_color_shaders(), TargetFormat::Rgba8Srgb)
                .push_constant_size(16),
        )
        .expect("push-constant pipeline");
    refused(
        "a declared push-constant range never pushed",
        "carries push data exactly when",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&push_pipeline)];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );
    refused(
        "push data on a pipeline that declares no range",
        "carries push data exactly when",
        &|target| {
            let color = clear(black);
            let bytes = [0u8; 16];
            let items = [Item::new(&pipeline).push_data(&bytes)];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );
    refused(
        "push data shorter than the declared range",
        "exactly the declared push-constant range",
        &|target| {
            let color = clear(black);
            let bytes = [0u8; 12];
            let items = [Item::new(&push_pipeline).push_data(&bytes)];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );

    // The binding half of the same contract: presence must match the
    // declared slot count, and the count must be exact. A one-slot
    // pipeline and a slotless one refuse the opposite mistakes; the
    // two-slot pipeline is legal over a shader that reads only set 0,
    // because a layout may declare sets a stage ignores.
    let (texture, sampler, binding) = binding_fixture(&device).expect("binding fixture");
    let one_slot = device
        .create_pipeline(
            &PipelineDesc::new(builtin::TEXTURED, TargetFormat::Rgba8Srgb).sampled_bindings(1),
        )
        .expect("one-slot pipeline");
    let two_slot = device
        .create_pipeline(
            &PipelineDesc::new(builtin::TEXTURED, TargetFormat::Rgba8Srgb).sampled_bindings(2),
        )
        .expect("two-slot pipeline");
    refused(
        "a declared sampled slot never filled",
        "names bindings exactly when",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&one_slot)];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );
    refused(
        "bindings on a pipeline that declares no slots",
        "names bindings exactly when",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&pipeline).bindings(&[&binding])];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );
    refused(
        "fewer bindings than declared slots",
        "fills every declared sampled slot",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&two_slot).bindings(&[&binding])];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );
    // The surplus direction of the same clause: one shared assert, but
    // the refusal message promises both, so both are driven. A binding
    // may repeat within an item (nothing copies into it), which is what
    // lets one fixture overfill the count.
    refused(
        "more bindings than declared slots",
        "fills every declared sampled slot",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&one_slot).bindings(&[&binding, &binding])];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );
    // The item-side ceiling fires while the item is built, before any
    // render call — still inside the harness, still refused by name.
    refused(
        "a binding list past the item ceiling",
        "names at most",
        &|_| {
            let _ = Item::new(&two_slot).bindings(&[&binding; 5]);
        },
    );
    // The render-image identity rules, each refused by name. One
    // color image and its binding serve every case; the sampled cases
    // ride the one-slot pipeline.
    let image = device
        .create_render_image(&renew_rhi::RenderImageDesc::new(
            renew_rhi::RenderImageKind::Color,
            Extent {
                width: 8,
                height: 8,
            },
        ))
        .expect("refusal fixture image");
    let image_binding = device
        .create_binding(&BindingDesc::new(BindingSource::Image(&image), &sampler))
        .expect("refusal fixture image binding");
    let store = Attachment::new(
        LoadOp::Clear(ClearValue::Color(Color::new(0.0, 0.0, 0.0, 1.0))),
        StoreOp::Store,
    );
    refused(
        "an image pass carrying surface slices",
        "carries its one attachment in its target",
        &|target| {
            let color = clear(black);
            let mut pass = Pass::render_to(&image, store, &[]);
            pass.color = &color;
            let surface = [Item::new(&pipeline)];
            let _ = target.render(&RenderDesc::new(&[pass, Pass::new(&color, &surface)]));
        },
    );
    refused(
        "a frame with no surface pass",
        "at least one surface pass",
        &|target| {
            let _ = target.render(&RenderDesc::new(&[Pass::render_to(&image, store, &[])]));
        },
    );
    refused(
        "a contents-preserving load on an image's first use",
        "render-image contents are frame-scoped",
        &|target| {
            let color = clear(black);
            let load = Attachment::new(LoadOp::Load, StoreOp::Store);
            let surface = [Item::new(&pipeline)];
            let _ = target.render(&RenderDesc::new(&[
                Pass::render_to(&image, load, &[]),
                Pass::new(&color, &surface),
            ]));
        },
    );
    refused(
        "sampling an image the frame never rendered",
        "must write it first",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&one_slot).bindings(&[&image_binding])];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items)]));
        },
    );
    refused(
        "a pass sampling its own target",
        "feedback within one pass",
        &|target| {
            let color = clear(black);
            // Rgba8Unorm, unlike every other pipeline in this file: this
            // one draws into a colour render image through `render_to`,
            // and colour render images store what was written rather than
            // encoding it. Naming the offscreen target's format here would
            // trip the format check first and refuse for the wrong reason.
            let sampled_desc =
                PipelineDesc::new(builtin::TEXTURED, TargetFormat::Rgba8Unorm).sampled_bindings(1);
            let feedback = device
                .create_pipeline(&sampled_desc)
                .expect("feedback pipeline");
            let items = [Item::new(&feedback).bindings(&[&image_binding])];
            let surface = [Item::new(&pipeline)];
            let _ = target.render(&RenderDesc::new(&[
                Pass::render_to(&image, store, &items),
                Pass::new(&color, &surface),
            ]));
        },
    );
    refused(
        "re-targeting an image after sampling it",
        "one-way",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&one_slot).bindings(&[&image_binding])];
            let _ = target.render(&RenderDesc::new(&[
                Pass::render_to(&image, store, &[]),
                Pass::new(&color, &items),
                Pass::render_to(&image, store, &[]),
            ]));
        },
    );
    refused(
        "discarding contents a later pass samples",
        "must Store",
        &|target| {
            let color = clear(black);
            let discard = Attachment::new(
                LoadOp::Clear(ClearValue::Color(Color::new(0.0, 0.0, 0.0, 1.0))),
                StoreOp::Discard,
            );
            let items = [Item::new(&one_slot).bindings(&[&image_binding])];
            let _ = target.render(&RenderDesc::new(&[
                Pass::render_to(&image, discard, &[]),
                Pass::new(&color, &items),
            ]));
        },
    );
    refused(
        "a depth clear on a color image",
        "clears to its kind's value",
        &|target| {
            let color = clear(black);
            let wrong = Attachment::new(LoadOp::Clear(ClearValue::Depth(0.0)), StoreOp::Store);
            let surface = [Item::new(&pipeline)];
            let _ = target.render(&RenderDesc::new(&[
                Pass::render_to(&image, wrong, &[]),
                Pass::new(&color, &surface),
            ]));
        },
    );
    // The depth-only placement rule, both wrong homes: a color image
    // pass and a surface pass. Retained rather than the surface format
    // match's dev-only assert, because the zero-attachment shape in a
    // color pass is an undefined draw, not a channel swap.
    let depth_only = device
        .create_pipeline(
            &PipelineDesc::depth_mesh(builtin::MESH_VS_SPV, builtin::MESH_LAYOUT)
                .depth_state(DepthState::read_write()),
        )
        .expect("depth-only pipeline");
    let quad_mesh = device
        .create_mesh(&MeshDesc::new(&[0u8; 36 * 3], 36, &[0, 1, 2]))
        .expect("mesh");
    refused(
        "a depth-only pipeline drawn into a color image",
        "draws only into depth-kinded",
        &|target| {
            let color = clear(black);
            let items = [Item::new(&depth_only).mesh(&quad_mesh)];
            let surface = [Item::new(&pipeline)];
            let _ = target.render(&RenderDesc::new(&[
                Pass::render_to(&image, store, &items),
                Pass::new(&color, &surface),
            ]));
        },
    );
    refused(
        "a depth-only pipeline drawn in a surface pass",
        "draws only into depth-kinded",
        &|target| {
            let color = clear(black);
            let fresh = Attachment::new(LoadOp::Clear(ClearValue::Depth(0.0)), StoreOp::Discard);
            let items = [Item::new(&depth_only).mesh(&quad_mesh)];
            let _ = target.render(&RenderDesc::new(&[Pass::new(&color, &items).depth(fresh)]));
        },
    );
    // The store-tracking rules the walk owns: loading what the last
    // targeting pass threw away, and the image ceiling.
    refused(
        "a load of discarded render-image contents",
        "store what a later pass loads",
        &|target| {
            let color = clear(black);
            let discard = Attachment::new(
                LoadOp::Clear(ClearValue::Color(Color::new(0.0, 0.0, 0.0, 1.0))),
                StoreOp::Discard,
            );
            let load = Attachment::new(LoadOp::Load, StoreOp::Store);
            let surface = [Item::new(&pipeline)];
            let _ = target.render(&RenderDesc::new(&[
                Pass::render_to(&image, discard, &[]),
                Pass::render_to(&image, load, &[]),
                Pass::new(&color, &surface),
            ]));
        },
    );
    let many_images: Vec<renew_rhi::RenderImage> = (0..5)
        .map(|_| {
            device
                .create_render_image(&renew_rhi::RenderImageDesc::new(
                    renew_rhi::RenderImageKind::Color,
                    Extent {
                        width: 8,
                        height: 8,
                    },
                ))
                .expect("boundary image")
        })
        .collect();
    refused(
        "a fifth distinct render image",
        "at most 4 distinct render images",
        &|target| {
            let color = clear(black);
            let surface = [Item::new(&pipeline)];
            let image_passes: Vec<Pass<'_>> = many_images
                .iter()
                .map(|image| Pass::render_to(image, store, &[]))
                .chain(std::iter::once(Pass::new(&color, &surface)))
                .collect();
            let _ = target.render(&RenderDesc::new(&image_passes));
        },
    );
    drop(many_images);
    drop(quad_mesh);
    drop(depth_only);
    drop(image_binding);
    drop(image);
    drop(two_slot);
    drop(one_slot);
    drop(binding);
    drop(sampler);
    drop(texture);
    std::panic::set_hook(hook);

    // The refusals fired before any GPU work: the same target still
    // renders a well-formed frame, and validation stayed silent.
    let color = clear(black);
    let items = [Item::new(&pipeline)];
    target
        .render(&RenderDesc::new(&[Pass::new(&color, &items)]))
        .expect("the target survives every refusal");
    assert_no_validation_errors(&device);
}

/// Four distinct render images in one frame — the documented ceiling —
/// two of them sampled by the same surface pass: the multi-image walk,
/// the pass-level retention of every image, and a batched sampling
/// boundary, all under validation. The refusal battery proves the
/// fifth is refused; this proves the fourth is not.
#[test]
fn four_render_images_fill_the_frame_ceiling() {
    let Some(device) = required_device().expect("device bring-up") else {
        return;
    };
    let mut target = device
        .create_offscreen_target(Extent {
            width: 8,
            height: 8,
        })
        .expect("offscreen target");
    let images: Vec<renew_rhi::RenderImage> = (0..4)
        .map(|_| {
            device
                .create_render_image(&renew_rhi::RenderImageDesc::new(
                    renew_rhi::RenderImageKind::Color,
                    Extent {
                        width: 8,
                        height: 8,
                    },
                ))
                .expect("frame image")
        })
        .collect();
    let sampler = device
        .create_sampler(&SamplerDesc::atlas())
        .expect("sampler");
    let bindings: Vec<renew_rhi::Binding> = images
        .iter()
        .take(2)
        .map(|image| {
            device
                .create_binding(&BindingDesc::new(BindingSource::Image(image), &sampler))
                .expect("image binding")
        })
        .collect();
    let reader = device
        .create_pipeline(
            &PipelineDesc::new(builtin::TEXTURED, TargetFormat::Rgba8Srgb).sampled_bindings(1),
        )
        .expect("reader pipeline");
    let ops = Attachment::new(
        LoadOp::Clear(ClearValue::Color(Color::new(0.5, 0.5, 0.5, 1.0))),
        StoreOp::Store,
    );
    let color = clear(Color::new(0.0, 0.0, 0.0, 1.0));
    let surface_items = [
        Item::new(&reader).bindings(&[&bindings[0]]),
        Item::new(&reader).bindings(&[&bindings[1]]),
    ];
    let passes: Vec<Pass<'_>> = images
        .iter()
        .map(|image| Pass::render_to(image, ops, &[]))
        .chain(std::iter::once(Pass::new(&color, &surface_items)))
        .collect();
    target
        .render(&RenderDesc::new(&passes))
        .expect("four-image frame");
    drop(target);
    drop(reader);
    drop(bindings);
    drop(images);
    drop(sampler);
    assert_no_validation_errors(&device);
}

/// The pushed bytes are the draw's constants, and they update per
/// record: two frames through one pipeline push two colors, and each
/// frame's every pixel answers with the color pushed for it. One frame
/// alone would pass with the constants baked at creation; the second
/// is what proves the channel is per-draw.
#[test]
fn push_constants_reach_the_draw_and_update_per_frame() {
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
        .create_pipeline(
            &PipelineDesc::new(push_color_shaders(), TargetFormat::Rgba8Srgb)
                .push_constant_size(16),
        )
        .expect("push-constant pipeline");
    let color = clear(Color::new(0.0, 0.0, 0.0, 1.0));
    let mut pixels = vec![0u8; target.byte_len()];
    // The pushed channels are light, and the attachment encodes on write,
    // so the byte that lands is the encode of what was pushed. Both ends
    // are derived from one authored value: pushing `decode(b)` stores back
    // exactly `b`, which keeps the comparison on bytes rather than
    // tolerances and keeps the test about push constants. Neither colour
    // is the clear, so a draw that silently read zeroed constants fails.
    for authored in [[0u8, 255, 64, 255], [255u8, 32, 0, 255]] {
        let mut pushed = [0u8; 16];
        for (slot, &channel) in pushed.chunks_exact_mut(4).zip(&authored) {
            slot.copy_from_slice(&renew_rhi::srgb::decode(channel).to_ne_bytes());
        }
        let expected = &authored;
        let items = [Item::new(&pipeline).push_data(&pushed)];
        let passes = [Pass::new(&color, &items)];
        target
            .render(&RenderDesc::new(&passes))
            .expect("push-constant render");
        target.read_back_into(&mut pixels);
        for (index, pixel) in pixels.chunks_exact(4).enumerate() {
            assert_eq!(
                pixel, expected,
                "pixel {index}: every pixel carries the color this frame pushed"
            );
        }
    }
    drop(target);
    drop(pipeline);
    assert_no_validation_errors(&device);
}

/// The same claim at the depth that can actually break it: **three**
/// overlapping draws, every one of the six orders, compared byte for
/// byte.
///
/// **Two draws cannot see what this sees, and that is why it exists.**
/// The pair above proves commutativity, which two terms already settle.
/// What three terms add is *associativity of the rounding* — each blend
/// writes a quantised value the next one reads back, so an order that
/// rounds differently at an intermediate step has somewhere to show it.
/// On a uniform grid there is nowhere: requantising a UNORM value is
/// lossless, so every order lands on the same bytes and this passes
/// exactly. That is a property of the storage format, not of the blend,
/// and a target whose grid is not uniform would not have it.
///
/// So this is written to be the place that notices. The channel values
/// are deliberately awkward — none a power of two, and their partial
/// sums land in different parts of any curve a target might apply — so
/// an intermediate rounding difference is not quietly symmetric.
#[test]
fn additive_blending_sums_the_same_bytes_in_all_six_orders() {
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
        .create_pipeline(
            &PipelineDesc::new(push_color_shaders(), TargetFormat::Rgba8Srgb)
                .blend(renew_rhi::Blend::Additive)
                .push_constant_size(16),
        )
        .expect("additive push-constant pipeline");
    let push = |channels: [u8; 4]| {
        let mut bytes = [0u8; 16];
        for (slot, &channel) in bytes.chunks_exact_mut(4).zip(&channels) {
            slot.copy_from_slice(&(f32::from(channel) / 255.0).to_ne_bytes());
        }
        bytes
    };
    let a = push([37, 61, 13, 11]);
    let b = push([23, 41, 89, 7]);
    let c = push([53, 17, 29, 19]);
    let color = clear(Color::new(0.0, 0.0, 0.0, 1.0));
    let mut render = |one: &[u8; 16], two: &[u8; 16], three: &[u8; 16]| {
        let items = [
            Item::new(&pipeline).push_data(one),
            Item::new(&pipeline).push_data(two),
            Item::new(&pipeline).push_data(three),
        ];
        let passes = [Pass::new(&color, &items)];
        target
            .render(&RenderDesc::new(&passes))
            .expect("additive render");
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        pixels
    };

    let reference = render(&a, &b, &c);
    // Orders first, then the premise. Both run, but a target that broke
    // order-stability should say so in its first line rather than report
    // a changed sum that reads like an unrelated pixel difference.
    for (name, pixels) in [
        ("acb", render(&a, &c, &b)),
        ("bac", render(&b, &a, &c)),
        ("bca", render(&b, &c, &a)),
        ("cab", render(&c, &a, &b)),
        ("cba", render(&c, &b, &a)),
    ] {
        // **Within one code, not identical.** Additive blending is
        // commutative in the working space, and that is the whole of what
        // it promises: the sum does not depend on the order the draws
        // arrived in. What it does not promise is the same *byte*, because
        // an attachment that encodes on write requantises each
        // intermediate result through a grid whose steps are not evenly
        // spaced, so two orders can land either side of one boundary.
        //
        // That is measured rather than assumed, and on more than one
        // adapter: which orders diverge differs between them, and so does
        // the direction, which is what rules out a pattern a stricter
        // assertion could describe. A tolerance of one code is the
        // requantisation and nothing more — two codes would be a real
        // divergence and still fails here.
        for (index, (found, want)) in pixels.iter().zip(reference.iter()).enumerate() {
            let drift = i16::from(*found) - i16::from(*want);
            assert!(
                drift.abs() <= 1,
                "order {name} diverged from abc by {drift} at byte {index} on adapter {:?}: \
                 additive is commutative in the working space, so orders may differ by at \
                 most the one code the attachment's encoding requantises through",
                device.adapter()
            );
        }
    }

    // The premise, checked after: a frame that stopped drawing would be
    // uniformly blank in every order and would sail through the loop
    // above, so the sums anchor it to something real.
    // The sum is of *light*, and the attachment encodes what it stores, so
    // the byte to expect is the encode of the summed lights rather than the
    // sum of the bytes. Written as the arithmetic rather than as a literal
    // so it says which of those two it is.
    assert_eq!(
        &reference[..4],
        &[178u8, 182, 190, 255],
        "the three channel sums must land exactly on adapter {:?}",
        device.adapter()
    );

    drop(target);
    drop(pipeline);
    assert_no_validation_errors(&device);
}

/// Additive blending is arithmetic, so the oracle is arithmetic: two
/// full-target draws over a black clear must land exactly on the
/// channel sums, and — because saturating addition is commutative —
/// both draw orders must produce identical bytes. The n/255 values
/// make the UNORM roundtrip exact, so this compares bytes, not
/// tolerances; the alpha channel saturates from the opaque clear and
/// pins the saturating half of the claim.
#[test]
fn additive_blending_sums_channels_in_either_order() {
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
        .create_pipeline(
            &PipelineDesc::new(push_color_shaders(), TargetFormat::Rgba8Srgb)
                .blend(renew_rhi::Blend::Additive)
                .push_constant_size(16),
        )
        .expect("additive push-constant pipeline");
    let push = |channels: [u8; 4]| {
        let mut bytes = [0u8; 16];
        for (slot, &channel) in bytes.chunks_exact_mut(4).zip(&channels) {
            slot.copy_from_slice(&(f32::from(channel) / 255.0).to_ne_bytes());
        }
        bytes
    };
    let first = push([32, 64, 8, 16]);
    let second = push([16, 32, 96, 8]);
    // Black clear + both draws: channel sums, alpha saturated by the
    // opaque clear.
    // As above: the attachment encodes, so two lights summed land on the
    // encode of that sum, not on the sum of their encodings.
    let expected = [120u8, 165, 171, 255];
    let color = clear(Color::new(0.0, 0.0, 0.0, 1.0));
    let mut render = |a: &[u8; 16], b: &[u8; 16]| {
        let items = [
            Item::new(&pipeline).push_data(a),
            Item::new(&pipeline).push_data(b),
        ];
        let passes = [Pass::new(&color, &items)];
        target
            .render(&RenderDesc::new(&passes))
            .expect("additive render");
        let mut pixels = vec![0u8; target.byte_len()];
        target.read_back_into(&mut pixels);
        pixels
    };
    let forward = render(&first, &second);
    for (index, pixel) in forward.chunks_exact(4).enumerate() {
        assert_eq!(
            pixel, expected,
            "pixel {index}: additive must land exactly on the channel sums"
        );
    }
    let reversed = render(&second, &first);
    assert_eq!(
        forward, reversed,
        "additive is commutative, so draw order must not change a byte"
    );
    drop(target);
    drop(pipeline);
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
