# renew-rhi

The engine's only doorway to the GPU: device bring-up, render targets,
and the v0 draw path, over Vulkan. A frame is described, not scripted:
the caller composes a `RenderDesc` — a list of `Pass`es, each with one
color attachment, an optional depth attachment, and `Item`s (a pipeline,
optionally the geometry it walks, optionally this frame's bytes) drawn in
order — on its own stack, and
hands it to a target's `render`. Correctness is provable headless — the
offscreen target renders and reads back pixels without a window or a
display server, and the golden-image tests attest the bytes.

- `Device` — the GPU context, built from a plain-data `DeviceDesc`
  (validation policy: `Off` / `IfAvailable` / `Required`). Deterministic
  adapter choice: discrete > integrated > virtual > software, then
  lowest device id; requires Vulkan 1.3 dynamic rendering and
  synchronization2 plus a graphics queue.
- `OffscreenTarget` — a fixed-size RGBA8 image with a synchronous
  `render(&RenderDesc)` and CPU readback (`read_back_into`). The
  correctness spine.
- `WindowTarget` (feature `present`, default on) — surface + swapchain
  over an opaque window handle; `render(&RenderDesc)` returns
  `Presented` or `NeedsResize` (resizes and minimized windows are
  protocol outcomes, never errors). Two frames in flight, FIFO
  presentation.
- `RenderDesc` / `Pass` / `Attachment` / `Item` — the frame vocabulary.
  Attachments carry their load and store ops (`LoadOp::Clear` holds its
  `ClearValue`, so a clear value without a clearing load is
  unrepresentable); depth is a per-target internal image a pass opts
  into, sized and owned by the target. An `Item` may name geometry
  (`Item::mesh`), which makes its draw indexed, and may carry push data
  (`Item::push_data`) for a pipeline that declares a range. Malformed
  frames — no passes, a first-pass `Load`, a clear value of the wrong
  kind, a depth-testing pipeline in a depthless pass, two items naming
  one per-frame buffer, an item whose geometry and whose pipeline's
  per-vertex input disagree, a mesh whose stride the pipeline does not
  pack to, push data missing or mis-sized against the declared range —
  are refused by named assertions before any GPU call.
- `RenderPipeline` — two SPIR-V stages, optional per-vertex and
  per-instance input, an optional vertex-stage push-constant range (at
  most 128 bytes, the guaranteed device minimum; items then carry
  exactly that many bytes per draw), an optional sampled texture bound
  at creation, and optional `DepthState` (test/write, compare fixed
  `GREATER_OR_EQUAL` — depth is reversed: nearer is larger, the far
  plane is zero, depth clears to zero). Two pipeline shapes: `PipelineDesc::new` takes
  `Shaders`, whose stages write their own vertex list and carry the
  count they generate; `PipelineDesc::mesh` takes `MeshShaders` and a
  per-vertex layout, and has no count at all because the geometry
  supplies it. `builtin` carries the embedded shader bundles — a colored
  triangle, a textured full-target quad, instanced quads with and
  without per-instance depth, and the mesh pairs (sources and compile
  record in [shaders/](shaders/README.md)).
- `Mesh` — vertex and index bytes written once at creation and read-only
  to the GPU thereafter, in one allocation. Indices are `&[u32]`, and
  **every index is checked against the vertex count at creation**: an
  out-of-range index is data no validation layer here reads, so creation
  is the only place it can be caught.
- `Texture` — a sampled RGBA8 image, filled once from host bytes during
  creation and immutable thereafter.
- `Sampler` — filter and address mode; `SamplerDesc::atlas()` is
  nearest and clamped.

## Contract

- **The GPU API never leaks.** No Vulkan or windowing type appears in
  any public signature; the one shared vocabulary with the platform
  window is the standard window-handle traits.
- **Single-threaded by construction.** Everything hangs off an
  `Rc`-shared spine, so the whole crate is structurally `!Send + !Sync`
  — Vulkan's external-synchronization rules are unrepresentable to
  violate. Lifting this is a future, deliberate change.
- **Errors are the environment's; assertions are the caller's.** A
  missing Vulkan runtime (`LoaderUnavailable`), a lost device, a stale
  swapchain — recoverable results. Mixing objects across devices or a
  wrong-sized readback buffer — contract violations (the readback
  length check is retained in release builds; it guards memory safety).
- **Validation is evidence, and the lanes are uneven.** The device
  suite brings devices up with `Validation::Required` and mechanically
  asserts zero validation errors at teardown; the present smoke suite
  runs its window target under `Validation::Required` too, so both
  target kinds sit under the strict oracle. The fault suite runs
  `IfAvailable` with one `Required` scenario, and the golden and
  present-allocation suites run `IfAvailable`. Golden tests pin exact
  bytes on the CI-pinned software rasterizer.
- **Steady-state frames allocate nothing** on the engine side — pinned
  by allocation-counting integration tests on **both** targets (the
  offscreen gate and the window-path gate). Driver host allocations
  are instrumented separately through `VkAllocationCallbacks` into a
  per-device ledger (`host_allocation_stats`), diagnostics only.

## Ownership and teardown

Every resource holds the device spine alive (`Rc`), so drop order is
free for consumers, and each `Drop` destroys in exact reverse creation
order. The targets and the pipeline quiesce the GPU first (best-effort
wait-idle); `Texture` and `Sampler` deliberately do not, because a
pipeline that references either holds shared ownership of it, so their
`Drop` cannot run while a submit could still name them. The
`WindowTarget` owns a keep-alive handle to its window: the OS window
cannot be torn down under a live surface, by construction.

## Testing note

Host-pure units (SPIR-V structural checks, allocation-callback
round-trips with seeded property tests, error display) run everywhere
and under the scheduled Miri job. Device, golden, and zero-allocation
suites run wherever a Vulkan runtime exists and skip loudly where none
does — except in the CI rendering lane (`RENEW_GOLDEN=1`), where a
skip is a failure. The present smoke test opens a real window and
presents frames where a display exists.

## Status

Early-stage: the surface is exactly device + two target kinds + the
pass vocabulary + two pipeline shapes + one sampled texture + geometry
— per-vertex and per-instance input, vertex-stage push constants,
indexed draws and target-owned depth exist; no MSAA, no image identity
on attachments, one fixed descriptor layout (a combined image sampler at
set 0, binding 0) — grown only when a consumer demands it. Mesh memory is
host-visible rather than device-local, which is a recorded decision with
a written reopening trigger (a real-GPU frame-time measurement showing
vertex fetch matters) and not an oversight. The `[package.metadata.renew]` table
in [Cargo.toml](Cargo.toml) is authoritative for maturity and manifest
metadata. Contract lints live in [clippy.toml](clippy.toml): thread
spawning, clock reads, and filesystem access are rejected at lint time.
`unsafe` is confined to `src/vk/`; the safe modules deny it, and every
site carries a `// SAFETY:` comment (lint-enforced).

## Key decisions

- **Vulkan through thin bindings, no framework.** The RHI owns its
  abstraction boundary; a middleware layer would own it instead and
  leak its vocabulary upward.
- **`Rc` spine over lifetimes or handles.** Resources outliving their
  device is unrepresentable, drop order stays free, and the crate
  becomes structurally single-threaded — three contracts for one
  mechanism.
- **Offscreen-first.** The headless target is the primary product;
  presentation is a feature-gated passenger. Correctness never needs
  glass.
- **Embedded pre-compiled shaders.** Offline compilation with a pinned
  toolchain keeps the runtime free of a compiler dependency; the asset
  pipeline will own shader delivery later.
