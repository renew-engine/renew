//! Present smoke: open a real window, render a handful of triangle
//! frames through the swapchain, and exit clean. `harness = false`
//! because the OS event loop must own the main thread.
//!
//! Skips (exit 0 with a SKIP line) when the environment cannot run it:
//! no display server, no Vulkan runtime, or no present support. CI
//! proves the offscreen path; this proves the glass path where glass
//! exists.

use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowError, WindowEvent, WindowRef, run_window_app,
};
use renew_rhi::{
    Attachment, BindingDesc, BindingSource, ClearValue, Color, DepthState, Device, DeviceDesc,
    DeviceError, Extent, Item, LoadOp, Pass, PipelineDesc, PresentOutcome, RenderDesc, SamplerDesc,
    StoreOp, TargetError, TextureDesc, Validation, WindowTarget, builtin,
};

/// The one color attachment these frames render into: cleared, stored.
fn clear(color: Color) -> [Attachment; 1] {
    [Attachment::new(
        LoadOp::Clear(ClearValue::Color(color)),
        StoreOp::Store,
    )]
}

/// The push-constant test fixture the device suite also embeds; its
/// compile record lives in the shaders README beside the builtins'.
static PUSH_COLOR_VS_SPV: &[u8] = include_bytes!("../shaders/push_color.vert.spv");
static PUSH_COLOR_FS_SPV: &[u8] = include_bytes!("../shaders/push_color.frag.spv");

const FRAMES_WANTED: u32 = 10;
/// Poll-loop iterations before declaring the run wedged.
const UPDATE_BUDGET: u32 = 20_000;

struct SmokeApp {
    device: Option<Device>,
    target: Option<WindowTarget>,
    pipeline: Option<renew_rhi::RenderPipeline>,
    /// The one binding the textured pipeline's item names — built with
    /// its pipeline, held for the run.
    binding: Option<renew_rhi::Binding>,
    /// A render image written by every frame's leading pass, sampled
    /// by a surface item — the render-then-read shape, its first-use
    /// barrier waiting on the previous frame's work against the ONE
    /// physical image, across frames in flight where only the
    /// validation layer can judge it.
    render_image: Option<renew_rhi::RenderImage>,
    /// The binding the sampling item reads the render image through.
    image_binding: Option<renew_rhi::Binding>,
    /// The pipeline the sampling item draws with, in the target's own
    /// format.
    sampled_pipeline: Option<renew_rhi::RenderPipeline>,
    /// A depth-kinded render image cleared by its own empty pass each
    /// frame — the depth-image target arms on the window path; `None`
    /// where the adapter has no depth format, mirroring the depth
    /// pipeline's own optionality.
    render_depth_image: Option<renew_rhi::RenderImage>,
    /// A depth-testing triangle for the frame's depth passes, `None`
    /// when the adapter offers no depth format — a reported skip of the
    /// depth passes off the strict lane, a failure on it (the exercise
    /// must not go silently vacuous where it is the point).
    depth_pipeline: Option<renew_rhi::RenderPipeline>,
    /// The push-constant path on the asynchronous target: constants
    /// recorded fresh into every frame's command buffer while several
    /// frames are in flight, under sync validation — the one exercise
    /// the synchronous offscreen suite cannot provide.
    push_pipeline: Option<renew_rhi::RenderPipeline>,
    /// The mesh path on the asynchronous target, and the only place its
    /// retention rule can be proved.
    mesh_pipeline: Option<renew_rhi::RenderPipeline>,
    /// Dropped mid-run, deliberately: see [`SmokeApp::MESH_DROP_FRAME`].
    mesh: Option<renew_rhi::Mesh>,
    size: Extent,
    frames: u32,
    updates: u32,
    cycled: bool,
    /// Bitmask of the frame slots observed across the run.
    slots_seen: u32,
    skip: Option<String>,
    failure: Option<String>,
}

impl SmokeApp {
    fn new() -> Self {
        Self {
            device: None,
            target: None,
            pipeline: None,
            binding: None,
            render_image: None,
            image_binding: None,
            sampled_pipeline: None,
            render_depth_image: None,
            depth_pipeline: None,
            push_pipeline: None,
            mesh_pipeline: None,
            mesh: None,
            size: Extent {
                width: 0,
                height: 0,
            },
            frames: 0,
            updates: 0,
            cycled: false,
            slots_seen: 0,
            skip: None,
            failure: None,
        }
    }

    /// The frame after which the caller's mesh handle is dropped.
    ///
    /// **This is the retention proof, and the number matters.** On this
    /// target `render` returns before the GPU has finished — the submit
    /// is still outstanding — so dropping the only caller handle here is
    /// exactly the moment a missing retention entry becomes a
    /// use-after-free. The frames after it keep rendering and presenting,
    /// so the destroyed buffer would be caught by the validation layer
    /// this suite runs under `Required`, and by the queue still reading
    /// it. Chosen below `FRAMES_WANTED` so several frames follow the
    /// drop, and above the ring depth so the drop lands while an earlier
    /// slot's work is genuinely in flight.
    ///
    /// The offscreen golden cannot make this claim: that target waits
    /// its fence inside `render`, so nothing outlives the call there.
    const MESH_DROP_FRAME: u32 = 6;

    fn done(&self) -> bool {
        self.skip.is_some() || self.failure.is_some() || self.frames >= FRAMES_WANTED
    }
}

impl WindowApp for SmokeApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        // `Required`, not `IfAvailable`: this is the only suite that
        // builds a `WindowTarget`, so it is the only place the present
        // path can be watched by the validation layer at all. Under
        // `IfAvailable` a lane with no layer installed still passed, and
        // every "no validation errors" claim about presenting was a claim
        // about nothing.
        //
        // A missing layer is a skip rather than a failure so a developer
        // without the SDK is not blocked -- and `RENEW_GOLDEN=1` turns
        // that skip back into a failure where the lane exists to run it,
        // which is what stops the skip being invisible. Same shape the
        // device suite already uses.
        let strict = std::env::var_os("RENEW_GOLDEN").is_some_and(|value| value == "1");
        let device = match Device::new(&DeviceDesc {
            app_name: "renew-present-smoke",
            validation: Validation::Required,
        }) {
            Ok(device) => device,
            Err(DeviceError::LoaderUnavailable { message }) => {
                self.skip = Some(format!("no Vulkan runtime: {message}"));
                return;
            }
            Err(DeviceError::ValidationUnavailable) if !strict => {
                self.skip = Some("validation layer not installed".to_string());
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
            Err(TargetError::PresentUnsupported { reason }) => {
                self.skip = Some(format!("cannot present to this surface: {reason}"));
                return;
            }
            Err(error) => {
                self.failure = Some(format!("window target failed: {error}"));
                return;
            }
        };
        // **Textured, so that the window record path actually binds a
        // descriptor set** — see `textured_fixture` for the reasoning,
        // which is the point of this suite drawing anything at all.
        let (pipeline, binding) = match textured_fixture(&device, target.format()) {
            Ok(pair) => pair,
            Err(error) => {
                self.failure = Some(error);
                return;
            }
        };
        // The depth exercise's pipeline: every frame's second pass
        // carries the target's depth image, so a run past the in-flight
        // count renders both slots' depth images — the per-slot
        // first-use transitions and between-frame behavior execute
        // where sync validation (Required, above) can judge them.
        let depth_pipeline = if device.depth_format_name().is_some() {
            match device.create_pipeline(
                &PipelineDesc::new(builtin::TRIANGLE, target.format())
                    .depth_state(DepthState::read_write()),
            ) {
                Ok(pipeline) => Some(pipeline),
                Err(error) => {
                    self.failure = Some(format!("depth pipeline failed: {error}"));
                    return;
                }
            }
        } else if strict {
            // On the lane that exists to run this, a depth-free run
            // would be the exercise silently going vacuous.
            self.failure =
                Some("no depth format: the depth exercise cannot run on this lane".to_string());
            return;
        } else {
            eprintln!("SKIP depth passes: adapter offers no chain depth format");
            None
        };
        // The push-constant fixture: sixteen bytes of color, pushed
        // fresh each frame so the window record path's push call runs
        // under sync validation with several frames in flight.
        let push_pipeline = match push_fixture(&device, target.format()) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                self.failure = Some(error);
                return;
            }
        };
        // The mesh path, on the one target where its lifetime rule can
        // be tested. Built here so the run can drop the handle later
        // while a submit that reads it is still outstanding.
        let (mesh_pipeline, mesh) = match mesh_fixture(&device, target.format()) {
            Ok(pair) => pair,
            Err(error) => {
                self.failure = Some(error);
                return;
            }
        };
        let (render_image, render_depth_image) = match render_image_fixture(&device) {
            Ok(pair) => pair,
            Err(error) => {
                self.failure = Some(error);
                return;
            }
        };
        let (image_binding, sampled_pipeline) =
            match image_consumer_fixture(&device, &render_image, target.format()) {
                Ok(pair) => pair,
                Err(error) => {
                    self.failure = Some(error);
                    return;
                }
            };
        self.device = Some(device);
        self.target = Some(target);
        self.pipeline = Some(pipeline);
        self.binding = Some(binding);
        self.render_image = Some(render_image);
        self.image_binding = Some(image_binding);
        self.sampled_pipeline = Some(sampled_pipeline);
        self.render_depth_image = render_depth_image;
        self.depth_pipeline = depth_pipeline;
        self.push_pipeline = Some(push_pipeline);
        self.mesh_pipeline = Some(mesh_pipeline);
        self.mesh = Some(mesh);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one event-dispatch narrative: resize, the dormancy cycle, frame assembly and the mid-flight mesh drop read top to bottom, and splitting it would separate the drop from the render that makes it a proof"
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
                let clear_color = Color::new(0.1, 0.2, 0.3, 1.0);
                if self.frames == 5 && !self.cycled {
                    // Dormant cycle mid-run: a zero-extent resize tears
                    // the swapchain down, a dormant render reports
                    // NeedsResize (and presents nothing), a real resize
                    // rebuilds — the minimize protocol, exercised on
                    // real glass.
                    self.cycled = true;
                    if let Err(error) = target.resize(Extent {
                        width: 0,
                        height: 0,
                    }) {
                        self.failure = Some(format!("dormant resize failed: {error}"));
                        return;
                    }
                    if target.extent()
                        != (Extent {
                            width: 0,
                            height: 0,
                        })
                    {
                        self.failure = Some("dormant target reports a size".to_string());
                        return;
                    }
                    let color = clear(clear_color);
                    match target.render(&RenderDesc::new(&[Pass::new(&color, &[])])) {
                        Ok(PresentOutcome::NeedsResize) => {}
                        Ok(PresentOutcome::Presented) => {
                            self.failure = Some("dormant target presented".to_string());
                            return;
                        }
                        Err(error) => {
                            self.failure = Some(format!("dormant render failed: {error}"));
                            return;
                        }
                    }
                    if let Err(error) = target.resize(self.size) {
                        self.failure = Some(format!("rebuild resize failed: {error}"));
                        return;
                    }
                }
                let color = clear(clear_color);
                let items_storage;
                let items_with_mesh;
                // The mesh rides the first pass while the caller still
                // holds it; once dropped, the frames that follow draw
                // without it and the retention table is the only thing
                // keeping the submit's memory alive.
                // `zip` rather than a nested match: the pipeline and the
                // mesh are built together in `ready` and only the mesh is
                // ever taken away, so "a mesh without its pipeline" is a
                // state this app cannot reach and does not need an arm.
                let geometry = self.mesh_pipeline.as_ref().zip(self.mesh.as_ref());
                // A color that changes per frame, drawn FIRST so the
                // textured quad covers it: what this proves is the
                // record path re-pushing constants into each in-flight
                // frame's command buffer, not a picture.
                let level = f32::from(u8::try_from(self.frames % 8).unwrap_or(0)) / 8.0;
                let mut pushed = [0u8; 16];
                for slot in pushed.as_chunks_mut::<4>().0 {
                    slot.copy_from_slice(&level.to_ne_bytes());
                }
                // Built together in `ready`, like the mesh pair: "one
                // without the other" is a state this app cannot reach.
                let drawn = self
                    .pipeline
                    .as_ref()
                    .zip(self.push_pipeline.as_ref())
                    .zip(self.binding.as_ref())
                    .zip(
                        self.sampled_pipeline
                            .as_ref()
                            .zip(self.image_binding.as_ref()),
                    );
                let items: &[Item<'_>] = match (drawn, geometry) {
                    (
                        Some((((pipeline, push_pipeline), binding), (sampled, image_binding))),
                        Some((mesh_pipeline, mesh)),
                    ) => {
                        items_with_mesh = [
                            Item::new(push_pipeline).push_data(&pushed),
                            Item::new(pipeline).bindings(&[binding]),
                            // The read half of the render image's frame:
                            // the sampling transition crosses at this
                            // pass's boundary, under the same oracle.
                            Item::new(sampled).bindings(&[image_binding]),
                            Item::new(mesh_pipeline).mesh(mesh),
                        ];
                        &items_with_mesh
                    }
                    (
                        Some((((pipeline, push_pipeline), binding), (sampled, image_binding))),
                        None,
                    ) => {
                        items_storage = [
                            Item::new(push_pipeline).push_data(&pushed),
                            Item::new(pipeline).bindings(&[binding]),
                            Item::new(sampled).bindings(&[image_binding]),
                        ];
                        &items_storage
                    }
                    (None, _) => &[],
                };
                // Three passes wherever a depth format exists: the
                // textured pass, then two passes that Load the color
                // result and draw the depth-tested triangle, each
                // clearing its own depth — multi-pass, color Load, the
                // per-slot depth image's first use AND its between-pass
                // barrier, all on the window path under Required
                // validation.
                let load = [Attachment::new(LoadOp::Load, StoreOp::Store)];
                // The leading image pass: cleared, stored, drawn by
                // nothing, sampled by nothing. What it exercises is
                // the asynchronous path's pass-target retention and
                // the render image's first-use barrier — the
                // write-after-write hazard against the previous
                // frame's pass over the same physical image, which
                // only this suite's validation can judge.
                let image_pass_storage;
                let image_pass: &[Pass<'_>] = match self.render_image.as_ref() {
                    Some(image) => {
                        let ops = Attachment::new(
                            LoadOp::Clear(ClearValue::Color(Color::new(0.0, 0.0, 0.0, 1.0))),
                            StoreOp::Store,
                        );
                        image_pass_storage = [Pass::render_to(image, ops, &[])];
                        &image_pass_storage
                    }
                    None => &[],
                };
                // The depth image's pass: cleared, stored, drawn by
                // nothing, sampled by nothing — the depth-kinded target
                // arms of the window path's walk, under the same
                // oracle. Its ops clear to the reversed-Z far plane.
                let depth_image_storage;
                let depth_image_pass: &[Pass<'_>] = match self.render_depth_image.as_ref() {
                    Some(image) => {
                        let ops =
                            Attachment::new(LoadOp::Clear(ClearValue::Depth(0.0)), StoreOp::Store);
                        depth_image_storage = [Pass::render_to(image, ops, &[])];
                        &depth_image_storage
                    }
                    None => &[],
                };
                let depth_items_storage;
                let passes_three;
                let passes_one;
                let passes: &[Pass<'_>] = if let Some(depth_pipeline) = self.depth_pipeline.as_ref()
                {
                    depth_items_storage = [Item::new(depth_pipeline)];
                    // The triangle writes z = 0.0, so against a 0.0
                    // clear it survives only on the compare's or-equal
                    // boundary — deterministic, and these passes exist
                    // for depth-image barrier coverage, not compare
                    // semantics. A future strictly-greater compare
                    // would quietly turn them into draws of nothing;
                    // whoever adds that builder gives this exercise an
                    // interior margin in the same change.
                    let fresh =
                        Attachment::new(LoadOp::Clear(ClearValue::Depth(0.0)), StoreOp::Discard);
                    passes_three = [
                        image_pass
                            .first()
                            .copied()
                            .unwrap_or(Pass::new(&color, items)),
                        depth_image_pass
                            .first()
                            .copied()
                            .unwrap_or(Pass::new(&color, items)),
                        Pass::new(&color, items),
                        Pass::new(&load, &depth_items_storage).depth(fresh),
                        Pass::new(&load, &depth_items_storage).depth(fresh),
                    ];
                    &passes_three
                } else {
                    passes_one = [
                        image_pass
                            .first()
                            .copied()
                            .unwrap_or(Pass::new(&color, items)),
                        Pass::new(&color, items),
                    ];
                    &passes_one
                };
                // The ring must actually cycle. A ring stuck on slot
                // zero is still *correct* -- every frame just waits its
                // own fence and the pipeline serialises -- so no other
                // assertion here can tell the difference. Record which
                // slots were used and check at the end that more than
                // one was.
                self.slots_seen |= 1u32 << target.frame_slot();
                match target.render(&RenderDesc::new(passes)) {
                    Ok(PresentOutcome::Presented) => {
                        self.frames += 1;
                        // **Dropped with the submit still outstanding.**
                        // `render` returned without waiting, so the queue
                        // is reading this mesh right now; only the
                        // target's retention clone stands between that
                        // read and a freed buffer. Every frame after this
                        // one keeps presenting, and the layer is on.
                        if self.frames == Self::MESH_DROP_FRAME {
                            self.mesh = None;
                        }
                    }
                    Ok(PresentOutcome::NeedsResize) => {
                        if let Err(error) = target.resize(self.size) {
                            self.failure = Some(format!("resize failed: {error}"));
                        }
                    }
                    Err(error) => {
                        self.failure = Some(format!("render failed: {error}"));
                    }
                }
            }
            _ => {}
        }
    }

    fn update(&mut self, control: &mut LoopControl) {
        self.updates += 1;
        if self.updates > UPDATE_BUDGET && !self.done() {
            self.failure = Some(format!(
                "wedged: {} frames after {} updates",
                self.frames, self.updates
            ));
        }
        if self.done() {
            control.exit();
        } else {
            control.request_redraw();
        }
    }
}

/// The frame's render images: the color one every run writes and
/// samples, and the depth one whose empty pass exists wherever the
/// adapter has a depth format — mirroring the depth pipeline's own
/// optionality.
fn render_image_fixture(
    device: &Device,
) -> Result<(renew_rhi::RenderImage, Option<renew_rhi::RenderImage>), String> {
    let size = Extent {
        width: 16,
        height: 16,
    };
    let color = device
        .create_render_image(&renew_rhi::RenderImageDesc::new(
            renew_rhi::RenderImageKind::Color,
            size,
        ))
        .map_err(|error| format!("render image failed: {error}"))?;
    let depth = if device.depth_format_name().is_some() {
        Some(
            device
                .create_render_image(&renew_rhi::RenderImageDesc::new(
                    renew_rhi::RenderImageKind::Depth,
                    size,
                ))
                .map_err(|error| format!("depth render image failed: {error}"))?,
        )
    } else {
        None
    };
    Ok((color, depth))
}

/// The render image's sampling consumer: an atlas sampler over the
/// image, bound once, drawn by a full-target quad in the target's own
/// format. Extracted for the reason every fixture here is: it is
/// fixture work, and `ready` is at the length the lint refuses.
fn image_consumer_fixture(
    device: &Device,
    render_image: &renew_rhi::RenderImage,
    format: renew_rhi::TargetFormat,
) -> Result<(renew_rhi::Binding, renew_rhi::RenderPipeline), String> {
    let sampler = device
        .create_sampler(&SamplerDesc::atlas())
        .map_err(|error| format!("image sampler failed: {error}"))?;
    let binding = device
        .create_binding(&BindingDesc::new(
            BindingSource::Image(render_image),
            &sampler,
        ))
        .map_err(|error| format!("image binding failed: {error}"))?;
    let pipeline = device
        .create_pipeline(&PipelineDesc::new(builtin::TEXTURED, format).sampled_bindings(1))
        .map_err(|error| format!("sampled pipeline failed: {error}"))?;
    Ok((binding, pipeline))
}

/// The textured pipeline, so that the window record path actually
/// binds a descriptor set.
///
/// The bind is one method shared by both targets, and until this
/// fixture existed only the offscreen target ever reached its body —
/// on this path it always took the early return, so a bind recorded
/// outside the render pass, or after the draw, would have passed every
/// check. Validation is `Required` here and the run asserts zero
/// errors, which is what makes that reachable at all.
///
/// The pipeline takes shared ownership of the texture and sampler, so
/// neither needs a field on the app: the keep-alive is the point of
/// the design and letting them drop here exercises it. Extracted for
/// the reason `mesh_fixture` was: fixture work, out of a `ready` at
/// the length the lint refuses.
fn textured_fixture(
    device: &Device,
    format: renew_rhi::TargetFormat,
) -> Result<(renew_rhi::RenderPipeline, renew_rhi::Binding), String> {
    let texels: [u8; 16] = [
        10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
    ];
    let texture = device
        .create_texture(&TextureDesc::new(
            Extent {
                width: 2,
                height: 2,
            },
            &texels,
        ))
        .map_err(|error| format!("atlas upload failed: {error}"))?;
    let sampler = device
        .create_sampler(&SamplerDesc::atlas())
        .map_err(|error| format!("sampler failed: {error}"))?;
    let binding = device
        .create_binding(&BindingDesc::new(
            BindingSource::Texture(&texture),
            &sampler,
        ))
        .map_err(|error| format!("binding failed: {error}"))?;
    let pipeline = device
        .create_pipeline(&PipelineDesc::new(builtin::TEXTURED, format).sampled_bindings(1))
        .map_err(|error| format!("pipeline failed: {error}"))?;
    Ok((pipeline, binding))
}

/// The push-constant pipeline, built outside the frame loop.
///
/// Extracted for the reason `mesh_fixture` was: it is fixture work,
/// and `ready` is already at the length the lint refuses.
fn push_fixture(
    device: &Device,
    format: renew_rhi::TargetFormat,
) -> Result<renew_rhi::RenderPipeline, String> {
    device
        .create_pipeline(
            &PipelineDesc::new(
                renew_rhi::Shaders::new(PUSH_COLOR_VS_SPV, PUSH_COLOR_FS_SPV, 3),
                format,
            )
            .push_constant_size(16),
        )
        .map_err(|error| format!("push-constant pipeline failed: {error}"))
}

/// A mesh pipeline and one indexed quad, built outside the frame loop.
///
/// Extracted from  rather than inlined: it is fixture work, and
/// the setup it would otherwise sit inside is already at the length the
/// lint refuses.
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
        // The texture coordinate the layout declares. This draw goes
        // through the untextured mesh shaders, which do not consume it,
        // but the record must be what the pipeline says it is.
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

fn main() {
    let mut app = SmokeApp::new();
    let config = WindowConfig {
        title: "renew present smoke".to_string(),
        logical_width: 640.0,
        logical_height: 360.0,
        resizable: true,
        ..WindowConfig::default()
    };
    let run = run_window_app(&config, &mut app);
    // Tear down GPU objects BEFORE reading the report so validation
    // findings from destruction are counted in the verdict.
    drop(app.target.take());
    drop(app.pipeline.take());
    drop(app.depth_pipeline.take());
    let report = app.device.as_ref().map(Device::validation_report);

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
    if let Some(report) = report
        && report.errors > 0
    {
        eprintln!(
            "FAIL: {} validation errors; first: {:?}",
            report.errors, report.first_messages
        );
        std::process::exit(1);
    }
    assert!(app.frames >= FRAMES_WANTED);
    assert!(
        app.slots_seen.count_ones() > 1,
        "the frame ring never advanced past one slot ({:#b}); frames pipeline correctly but          serially, which no other assertion here can detect",
        app.slots_seen
    );
    // State the oracle in the output: "validation on" means the frames
    // above ran under the layer (with synchronization checking) and the
    // zero-error verdict is meaningful, not vacuous.
    let validation = app.device.as_ref().map(Device::validation_active);
    println!(
        "OK: presented {} frames (validation {})",
        app.frames,
        match validation {
            Some(true) => "on",
            _ => "OFF — zero-error verdict vacuous",
        }
    );
}
