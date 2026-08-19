//! The allocation contract on the **window** path.
//!
//! `zero_alloc.rs` pins the offscreen path and stops there, so the path
//! that acquires, submits and presents — the one a real game runs every
//! frame — had no allocation gate at all. That is also the path the frame
//! signature and frames-in-flight work land on, so the oracle has to
//! exist before the work rather than after it.
//!
//! `harness = false` because winit's event loop owns the main thread, and
//! a global allocator is per-binary, so this cannot live inside the
//! offscreen suite.
//!
//! **The counter is `renew_memory::CountingAllocator`, not a local one.**
//! A test-local counting shim was written first and then deleted: it
//! would have been the sixth copy of a helper already recorded as
//! duplicated five times, and — because implementing a global allocator
//! needs `unsafe impl` — it would have enlarged the crate's recorded
//! unsafe surface for a counter the engine already ships. Reusing the
//! engine's costs no removability cell either: that crate is ratified
//! minimal core, so nothing removes it.
//!
//! Validation stays off: the debug messenger allocates when it speaks,
//! and it must not speak into the measurement. Driver-side host
//! allocations are invisible here by design — they route through the
//! instrumented Vulkan callbacks and their own ledger; this pins the
//! engine side of the line, exactly as the offscreen gate does.

use renew_memory::{CountingAllocator, counters};
use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowError, WindowEvent, WindowRef, run_window_app,
};
/// The uniform-block fixture's stages: a full-target triangle whose every
/// pixel answers from a 192-byte block. Shared with the device suite; the
/// compile record lives in the shaders README beside the builtins'.
static UNIFORM_TINT_VS_SPV: &[u8] = include_bytes!("../shaders/uniform_tint.vert.spv");
static UNIFORM_TINT_FS_SPV: &[u8] = include_bytes!("../shaders/uniform_tint.frag.spv");

use renew_rhi::{
    Attachment, Buffer, BufferUsage, ClearValue, Color, Device, DeviceDesc, DeviceError, Extent,
    FrameData, Item, LoadOp, Pass, PipelineDesc, PresentOutcome, RenderDesc, StoreOp, TargetError,
    Validation, WindowTarget, builtin,
};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Allocations recorded so far, process-wide.
fn allocations() -> u64 {
    counters::snapshot().allocations
}

/// Frames per measurement window.
const WINDOW_FRAMES: u32 = 16;
/// Warmup frames before measuring: the first present lazily initializes
/// driver state, and a swapchain may be rebuilt once on first show.
const WARMUP_FRAMES: u32 = 8;
/// Retries. The counter is process-wide, so a neighbour's one-shot
/// allocation must be allowed to ride out while a real render-path
/// allocation reproduces in every window — the same reasoning as
/// `renew_memory::counters::quiet_window`, which every other gate now
/// calls. This file deliberately does NOT: its window spans event-loop
/// callbacks with per-call bracketing (the event loop's own allocations
/// are the environment's, not the engine's) and restarts on resize, a
/// different measurement no call-frame-shaped helper can express. If
/// that ever stops being true, the helper is where this goes.
const ATTEMPTS: u32 = 5;
/// Poll-loop iterations before declaring the run wedged.
const UPDATE_BUDGET: u32 = 20_000;

struct GateApp {
    device: Option<Device>,
    target: Option<WindowTarget>,
    pipeline: Option<renew_rhi::RenderPipeline>,
    /// The instanced pipeline and its per-frame buffer: the measured
    /// frame carries bytes, because a gate over a byte-free frame would
    /// pass vacuously the moment the data path allocated. This is also
    /// the one automated exercise of the window path's retention ring.
    instanced: Option<(renew_rhi::RenderPipeline, Buffer, Buffer)>,
    /// The mesh pipeline and its geometry. **The window path's mesh work
    /// is structurally different from the offscreen path's** — a per-slot
    /// retention table released while a submit may still be outstanding,
    /// rather than a single table behind a tail wait — so measuring the
    /// offscreen gate alone would leave the binds, the indexed draw and
    /// the dedupe scan on this path gated by nothing.
    mesh: Option<(renew_rhi::RenderPipeline, renew_rhi::Mesh)>,
    /// The camera-shaped push pipeline and an identity matrix's bytes:
    /// the camera's every-frame item shape — a mesh draw carrying
    /// sixty-four bytes of push data — measured on the window path so
    /// the claim that a push allocates nothing is gate-observed here
    /// too, not inherited from the offscreen gate by reading.
    push_camera: Option<(
        renew_rhi::RenderPipeline,
        [u8; 64],
        renew_rhi::Binding,
        [u8; 16],
    )>,
    /// A uniform-block pipeline, the per-frame buffer behind it, and the
    /// binding that reads it.
    ///
    /// **The one automated exercise of a nonzero frame slot.** The block's
    /// descriptor is written once and reaches its frame's bytes through a
    /// dynamic offset of `slot_stride * slot` — arithmetic that is the
    /// identity on the offscreen path, which is synchronous and always
    /// slot zero. Every other block test is offscreen, so without this the
    /// ring that `UNIFORM_BUFFER_DYNAMIC` was chosen *for* is never
    /// indexed past its first region, and a slot/offset disagreement is a
    /// window-only wrong picture no golden could see.
    ///
    /// It sits in this gate rather than the smoke test because a block
    /// frame is also the strictly stronger allocation shape: it binds a
    /// descriptor, copies into mapped memory, and passes a non-empty
    /// dynamic-offset array, all inside the measured window.
    block: Option<(renew_rhi::RenderPipeline, Buffer, renew_rhi::Binding)>,
    /// Which frame slots a uniform-block frame was recorded into, one bit
    /// each.
    ///
    /// **Asserted rather than inferred.** The whole reason this fixture is
    /// on the window path is that the offscreen path is synchronous and
    /// always slot zero, so the dynamic offset is never exercised there.
    /// Reasoning from the frame counter to the slot would be reasoning,
    /// not evidence — and if the cycle length and the ring depth ever
    /// share a factor, the block frame silently pins to one slot and the
    /// coverage this exists for quietly stops happening.
    block_slots: u32,
    size: Extent,
    presented: u32,
    updates: u32,
    /// Allocations charged to `render` calls inside the current window.
    /// Summed per call rather than measured across the window, so the
    /// event loop's own work between frames is never counted.
    window_spent: u64,
    /// Frames presented inside the current window.
    window_frames: u32,
    attempts: u32,
    last_delta: u64,
    observed_zero: bool,
    done: bool,
    skip: Option<String>,
    failure: Option<String>,
}

impl GateApp {
    fn new() -> Self {
        Self {
            device: None,
            target: None,
            pipeline: None,
            instanced: None,
            mesh: None,
            block: None,
            block_slots: 0,
            push_camera: None,
            size: Extent {
                width: 0,
                height: 0,
            },
            presented: 0,
            updates: 0,
            window_spent: 0,
            window_frames: 0,
            attempts: 0,
            last_delta: 0,
            observed_zero: false,
            done: false,
            skip: None,
            failure: None,
        }
    }
}

impl GateApp {
    /// Nothing left to do: measured, skipped, or broken.
    fn finished(&self) -> bool {
        self.done || self.skip.is_some() || self.failure.is_some()
    }
}

impl WindowApp for GateApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let strict = std::env::var_os("RENEW_GOLDEN").is_some_and(|value| value == "1");
        let device = match Device::new(&DeviceDesc {
            app_name: "renew-present-zero-alloc",
            validation: Validation::Off,
        }) {
            Ok(device) => device,
            Err(DeviceError::LoaderUnavailable { message }) if !strict => {
                self.skip = Some(format!("no Vulkan runtime: {message}"));
                return;
            }
            Err(error) => {
                self.failure = Some(format!("device bring-up failed: {error}"));
                return;
            }
        };
        let (width, height) = window.physical_size();
        self.size = Extent { width, height };
        let target = match device.create_window_target(window.native(), self.size) {
            Ok(target) => target,
            Err(TargetError::PresentUnsupported { reason }) if !strict => {
                self.skip = Some(format!("cannot present to this surface: {reason}"));
                return;
            }
            Err(error) => {
                self.failure = Some(format!("window target failed: {error}"));
                return;
            }
        };
        let pipeline =
            match device.create_pipeline(&PipelineDesc::new(builtin::TRIANGLE, target.format())) {
                Ok(pipeline) => pipeline,
                Err(error) => {
                    self.failure = Some(format!("pipeline failed: {error}"));
                    return;
                }
            };
        let instanced = match device.create_pipeline(
            &PipelineDesc::new(builtin::INSTANCED, target.format())
                .instance_input(builtin::INSTANCED_LAYOUT),
        ) {
            // Two per-frame buffers, so the multi-pass frame below can
            // retain two distinct buffers in one slot — the fixed-width
            // retention claim, gate-observed.
            Ok(instanced) => match (
                device.create_buffer(64, BufferUsage::PerFrame),
                device.create_buffer(64, BufferUsage::PerFrame),
            ) {
                (Ok(buffer), Ok(buffer_two)) => (instanced, buffer, buffer_two),
                (Err(error), _) | (_, Err(error)) => {
                    self.failure = Some(format!("per-frame buffer failed: {error}"));
                    return;
                }
            },
            Err(error) => {
                self.failure = Some(format!("instanced pipeline failed: {error}"));
                return;
            }
        };
        let mesh = match mesh_fixture(&device, target.format()) {
            Ok(pair) => pair,
            Err(error) => {
                self.failure = Some(error);
                return;
            }
        };
        let push_camera = match push_camera_fixture(&device, target.format()) {
            Ok(pair) => pair,
            Err(error) => {
                self.failure = Some(error);
                return;
            }
        };
        let block = match block_fixture(&device, target.format()) {
            Ok(trio) => trio,
            Err(error) => {
                self.failure = Some(error);
                return;
            }
        };
        self.device = Some(device);
        self.target = Some(target);
        self.pipeline = Some(pipeline);
        self.instanced = Some(instanced);
        self.mesh = Some(mesh);
        self.push_camera = Some(push_camera);
        self.block = Some(block);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one measured redraw arm; the three frame shapes read top to bottom"
    )]
    fn event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::Resized { width, height } => {
                self.size = Extent { width, height };
                if let Some(target) = &mut self.target
                    && let Err(error) = target.resize(self.size)
                {
                    self.failure = Some(format!("resize failed: {error}"));
                }
            }
            WindowEvent::RedrawRequested => {
                let Some(target) = &mut self.target else {
                    return;
                };
                let clear = Color::new(0.1, 0.2, 0.3, 1.0);
                // Bracket `render` ITSELF, not the event-handling window
                // around it. Reading the counter at frame boundaries
                // instead put everything the OS event loop does between
                // redraws inside the measurement -- which is nothing on
                // Windows and two allocations per iteration on X11, so
                // the gate passed locally and failed in CI for a reason
                // that was never the engine's. The contract is about the
                // render path; measure the render path.
                // Cycle three frame shapes so the gate measures each:
                // byte-free, byte-carrying, and a two-pass frame whose
                // items retain two distinct buffers in one slot -- the
                // walk's loops and the retention table's width, with
                // retain and release running inside the measured
                // window.
                let instance_bytes = {
                    let mut bytes = [0u8; 24];
                    for (i, v) in [0.0f32, 0.0, 0.2, 0.6, 0.9, 1.0].iter().enumerate() {
                        bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
                    }
                    bytes
                };
                let before = allocations();
                let color = [Attachment::new(
                    LoadOp::Clear(ClearValue::Color(clear)),
                    StoreOp::Store,
                )];
                let load = [Attachment::new(LoadOp::Load, StoreOp::Store)];
                let items_storage;
                let pair_storage;
                let second_storage;
                let passes_one;
                let passes_two;
                let mesh_storage;
                let push_storage;
                let block_bytes = {
                    // Varied by frame, so consecutive slots hold different
                    // bytes and a slot read from the wrong region shows.
                    let mut bytes = [0u8; BLOCK_BYTES];
                    let level = f32::from(u8::try_from(self.presented % 8).unwrap_or(0)) / 8.0;
                    for chunk in bytes.chunks_exact_mut(4) {
                        chunk.copy_from_slice(&level.to_ne_bytes());
                    }
                    bytes
                };
                let block_storage;
                // Recorded inside the arm and folded in after the render,
                // because the slot belongs to the frame about to be
                // recorded rather than to the one that just finished.
                let mut block_slot = None;
                let passes: &[Pass<'_>] = match self.presented % 6 {
                    0 => {
                        if let Some(pipeline) = self.pipeline.as_ref() {
                            items_storage = [Item::new(pipeline)];
                            passes_one = [Pass::new(&color, &items_storage)];
                            &passes_one
                        } else {
                            passes_one = [Pass::new(&color, &[])];
                            &passes_one
                        }
                    }
                    1 => {
                        if let Some((instanced, buffer, _)) = self.instanced.as_ref() {
                            items_storage = [Item::new(instanced).frame_data(FrameData::new(
                                buffer,
                                &instance_bytes,
                                1,
                            ))];
                            passes_one = [Pass::new(&color, &items_storage)];
                            &passes_one
                        } else {
                            passes_one = [Pass::new(&color, &[])];
                            &passes_one
                        }
                    }
                    // The uniform-block frame: a descriptor bind with a
                    // dynamic offset, a copy into this slot's region, and
                    // a retention entry for the binding that holds the
                    // buffer. The slot is whatever the ring is on, which
                    // is the point.
                    5 => {
                        if let Some((pipeline, _, binding)) = self.block.as_ref() {
                            block_storage = [Item::new(pipeline)
                                .bindings(&[binding])
                                .uniform_data(&block_bytes)];
                            passes_one = [Pass::new(&color, &block_storage)];
                            block_slot = Some(target.frame_slot());
                            &passes_one
                        } else {
                            passes_one = [Pass::new(&color, &[])];
                            &passes_one
                        }
                    }
                    // The mesh frame: two buffer binds, an indexed draw,
                    // and a mesh retention entry taken and released on
                    // the per-slot table.
                    2 => {
                        if let Some((mesh_pipeline, mesh)) = self.mesh.as_ref() {
                            // **Two items naming one mesh**, which is the
                            // shape the per-frame buffers forbid and
                            // geometry allows. It costs one retention
                            // slot rather than two, and the scan that
                            // decides so runs on the frame path — so it
                            // belongs inside the measured window rather
                            // than being reasoned about outside it.
                            mesh_storage = [
                                Item::new(mesh_pipeline).mesh(mesh),
                                Item::new(mesh_pipeline).mesh(mesh),
                            ];
                            passes_one = [Pass::new(&color, &mesh_storage)];
                            &passes_one
                        } else {
                            passes_one = [Pass::new(&color, &[])];
                            &passes_one
                        }
                    }
                    // The push frame: the camera's every-frame shape on
                    // the asynchronous path — an indexed draw whose
                    // sixty-four matrix bytes are recorded as push
                    // constants into this slot's command buffer.
                    3 => {
                        if let (Some((push_pipeline, matrix, fade, horizon)), Some((_, mesh))) =
                            (self.push_camera.as_ref(), self.mesh.as_ref())
                        {
                            push_storage = [Item::new(push_pipeline)
                                .mesh(mesh)
                                .push_data(matrix)
                                .bindings(core::slice::from_ref(&fade))
                                .uniform_data(horizon)];
                            passes_one = [Pass::new(&color, &push_storage)];
                            &passes_one
                        } else {
                            passes_one = [Pass::new(&color, &[])];
                            &passes_one
                        }
                    }
                    _ => {
                        if let (Some(pipeline), Some((instanced, buffer, buffer_two))) =
                            (self.pipeline.as_ref(), self.instanced.as_ref())
                        {
                            pair_storage = [
                                Item::new(pipeline),
                                Item::new(instanced).frame_data(FrameData::new(
                                    buffer,
                                    &instance_bytes,
                                    1,
                                )),
                            ];
                            second_storage = [Item::new(instanced).frame_data(FrameData::new(
                                buffer_two,
                                &instance_bytes,
                                1,
                            ))];
                            passes_two = [
                                Pass::new(&color, &pair_storage),
                                Pass::new(&load, &second_storage),
                            ];
                            &passes_two
                        } else {
                            passes_one = [Pass::new(&color, &[])];
                            &passes_one
                        }
                    }
                };
                let outcome = target.render(&RenderDesc::new(passes));
                let spent = allocations() - before;
                match outcome {
                    Ok(PresentOutcome::Presented) => {
                        if let Some(slot) = block_slot {
                            self.block_slots |= 1u32 << slot;
                        }
                        self.presented += 1;
                        if self.presented > WARMUP_FRAMES {
                            self.window_frames += 1;
                            self.window_spent += spent;
                        }
                    }
                    // A rebuild is not steady state. Restart the window
                    // rather than charging its allocations to the frame
                    // path, which would make the gate report a defect
                    // where the environment merely resized us.
                    Ok(PresentOutcome::NeedsResize) => {
                        if let Err(error) = target.resize(self.size) {
                            self.failure = Some(format!("rebuild resize failed: {error}"));
                            return;
                        }
                        self.window_frames = 0;
                        self.window_spent = 0;
                    }
                    Err(error) => {
                        self.failure = Some(format!("render failed: {error}"));
                        return;
                    }
                }

                // Open the first measurement window exactly when warmup
                // ends, so the transition itself is never inside one.
                if self.presented == WARMUP_FRAMES {
                    self.window_frames = 0;
                    self.window_spent = 0;
                }
                if self.window_frames == WINDOW_FRAMES {
                    self.last_delta = self.window_spent;
                    self.attempts += 1;
                    if self.window_spent == 0 {
                        self.observed_zero = true;
                        self.done = true;
                    } else if self.attempts >= ATTEMPTS {
                        self.done = true;
                    } else {
                        self.window_frames = 0;
                        self.window_spent = 0;
                    }
                }
            }
            _ => {}
        }
    }

    fn update(&mut self, control: &mut LoopControl) {
        self.updates += 1;
        if self.updates > UPDATE_BUDGET && !self.finished() {
            self.failure = Some(format!(
                "wedged: {} updates, {} presented, {} in the current window",
                self.updates, self.presented, self.window_frames
            ));
        }
        if self.finished() {
            control.exit();
        } else {
            control.request_redraw();
        }
    }
}

/// The mesh pipeline and one small indexed quad, built outside the
/// measured window. Extracted for the reason the offscreen gate's
/// fixtures are: fixture work, out of a `ready` at the length the lint
/// refuses. The texture coordinate is packed unread because the record
/// must be what the pipeline's layout says it is.
/// The fixture's uniform block: eight `vec4`s and a `mat4`, std140.
const BLOCK_BYTES: usize = 8 * 16 + 64;

/// A uniform-block pipeline, its per-frame buffer, and the binding that
/// reads it.
///
/// Extracted like its siblings: it is fixture work, and the bring-up it
/// would otherwise sit inside is already at the length the lint refuses.
fn block_fixture(
    device: &Device,
    format: renew_rhi::TargetFormat,
) -> Result<(renew_rhi::RenderPipeline, Buffer, renew_rhi::Binding), String> {
    let size = u32::try_from(BLOCK_BYTES).unwrap_or(u32::MAX);
    let pipeline = device
        .create_pipeline(
            &PipelineDesc::new(
                renew_rhi::Shaders::new(UNIFORM_TINT_VS_SPV, UNIFORM_TINT_FS_SPV, 3),
                format,
            )
            .uniform_block(size),
        )
        .map_err(|error| format!("uniform-block pipeline failed: {error}"))?;
    let buffer = device
        .create_buffer(BLOCK_BYTES, BufferUsage::PerFrame)
        .map_err(|error| format!("block buffer failed: {error}"))?;
    let binding = device
        .create_binding(&renew_rhi::BindingDesc::uniform(&buffer))
        .map_err(|error| format!("block binding failed: {error}"))?;
    Ok((pipeline, buffer, binding))
}

fn mesh_fixture(
    device: &Device,
    format: renew_rhi::TargetFormat,
) -> Result<(renew_rhi::RenderPipeline, renew_rhi::Mesh), String> {
    let pipeline = device
        .create_pipeline(&PipelineDesc::mesh(
            builtin::MESH,
            format,
            builtin::MESH_LAYOUT,
        ))
        .map_err(|error| format!("mesh pipeline failed: {error}"))?;
    let mut vertices = Vec::new();
    for corner in [
        [-0.6f32, -0.6, 0.0],
        [0.6, -0.6, 0.0],
        [0.6, 0.6, 0.0],
        [-0.6, 0.6, 0.0],
    ] {
        for value in corner {
            vertices.extend_from_slice(&value.to_ne_bytes());
        }
        for value in [0.2f32, 0.8, 0.4, 1.0] {
            vertices.extend_from_slice(&value.to_ne_bytes());
        }
        for value in [0.0f32, 0.0] {
            vertices.extend_from_slice(&value.to_ne_bytes());
        }
    }
    let mesh = device
        .create_mesh(&renew_rhi::MeshDesc::new(
            &vertices,
            12 + 16 + 8,
            &[0, 1, 2, 0, 2, 3],
        ))
        .map_err(|error| format!("mesh failed: {error}"))?;
    Ok((pipeline, mesh))
}

/// The camera-shaped push pipeline and an identity matrix's sixty-four
/// bytes, built outside the measured window. Extracted for the reason
/// the offscreen gate's fixtures are: fixture work, out of a `ready`
/// at the length the lint refuses.
/// **The fade block is fixture, not measurement.** `mesh_camera.frag`
/// reads its horizon from a uniform block, so a pipeline built from that
/// shader has to declare one or the fragment stage reads a descriptor
/// nothing bound. Its buffer and binding are made here, before the
/// A camera pipeline, its matrix bytes, and the fade block its
/// fragment stage reads.
type CameraFixture = (
    renew_rhi::RenderPipeline,
    [u8; 64],
    renew_rhi::Binding,
    [u8; 16],
);

/// window opens.
fn push_camera_fixture(
    device: &Device,
    format: renew_rhi::TargetFormat,
) -> Result<CameraFixture, String> {
    let pipeline = device
        .create_pipeline(
            &PipelineDesc::mesh(builtin::MESH_CAMERA, format, builtin::MESH_LAYOUT)
                .push_constant_size(64)
                .uniform_block(16),
        )
        .map_err(|error| format!("push pipeline failed: {error}"))?;
    let mut matrix = [0u8; 64];
    for (index, value) in [
        1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
    .iter()
    .enumerate()
    {
        matrix[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    let fade_buffer = device
        .create_buffer(16, renew_rhi::BufferUsage::PerFrame)
        .map_err(|error| format!("fade buffer failed: {error}"))?;
    let fade = device
        .create_binding(&renew_rhi::BindingDesc::uniform(&fade_buffer))
        .map_err(|error| format!("fade binding failed: {error}"))?;
    let mut horizon = [0u8; 16];
    for (slot, value) in horizon
        .chunks_exact_mut(4)
        .zip(builtin::HORIZON.into_iter().chain([1.0]))
    {
        slot.copy_from_slice(&value.to_ne_bytes());
    }
    Ok((pipeline, matrix, fade, horizon))
}

fn main() {
    let mut app = GateApp::new();
    let config = WindowConfig {
        title: "renew present zero-alloc gate".to_string(),
        logical_width: 320.0,
        logical_height: 240.0,
        resizable: true,
    };
    let run = run_window_app(&config, &mut app);
    // Drop GPU objects before the verdict, matching the sibling suite:
    // teardown is part of the frame path's lifetime, not after it.
    drop(app.target.take());
    drop(app.pipeline.take());

    match run {
        Ok(()) => {}
        Err(WindowError::LoopUnavailable { message }) => {
            println!("SKIP: window loop unavailable: {message}");
            return;
        }
        Err(error) => {
            eprintln!("FAIL: window loop: {error}");
            std::process::exit(1);
        }
    }
    if let Some(message) = app.skip {
        println!("SKIP: {message}");
        return;
    }
    if let Some(message) = app.failure {
        eprintln!("FAIL: {message}");
        std::process::exit(1);
    }
    // The driver-side ledger is printed for the record, never gated:
    // driver host-allocation behaviour is the driver's, not ours.
    if let Some(device) = &app.device {
        let stats = device.host_allocation_stats();
        println!("driver host allocations: {stats:?}");
    }
    assert!(
        app.observed_zero,
        "the window render path allocated on the frame path: {} allocations across {WINDOW_FRAMES} \
         steady frames, reproduced in all {ATTEMPTS} measurement windows. The offscreen gate has \
         covered this contract for the offscreen path only; this is the window path.",
        app.last_delta
    );
    // **The nonzero slot, stated rather than assumed.** A uniform block
    // reaches its frame's bytes through a dynamic offset of
    // `slot_stride * slot`, which is the identity on the offscreen path —
    // synchronous, always slot zero — so this gate is the only place the
    // arithmetic does anything. If the frame cycle and the ring depth ever
    // share a factor, the block pins to one slot and this coverage stops
    // happening silently; that is what the assertion is for.
    assert!(
        app.block_slots & !1 != 0,
        "no uniform-block frame was recorded into a nonzero frame slot (slots seen: {:#b}), so          the dynamic offset was never exercised past its identity case",
        app.block_slots
    );
    println!(
        "OK: {WINDOW_FRAMES} steady window frames allocated nothing (after {WARMUP_FRAMES} warmup, \
         {} presented total)",
        app.presented
    );
}
