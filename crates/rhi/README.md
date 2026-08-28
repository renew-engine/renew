# renew-rhi

The engine's only doorway to the GPU: device bring-up, render targets,
and the v0 draw path, over Vulkan. A frame is described, not scripted:
the caller composes a `RenderDesc` — a list of `Pass`es, each rendering
into the target's surface or a render image, with `Item`s (a pipeline,
optionally the geometry it walks, optionally this frame's bytes,
optionally the bindings it samples) drawn in order — on its own stack, and
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
- `RenderDesc` / `Pass` / `Attachment` / `Item` / `ItemList` /
  `color_attachment` — the frame vocabulary.
  Attachments carry their load and store ops (`LoadOp::Clear` holds its
  `ClearValue`, so a clear value without a clearing load is
  unrepresentable); depth is a per-target internal image a pass opts
  into, sized and owned by the target. An `Item` may name geometry
  (`Item::mesh`), which makes its draw indexed — over the whole index list,
  or over a slice of it (`Item::indices`, an `IndexRange`) so that geometry
  meshed in contiguous pieces can be culled or sorted per piece without a
  second buffer — and may carry push data
  (`Item::push_data`) for a pipeline that declares a range. Malformed
  frames — no passes, no surface pass, a first-use `Load` on any
  attachment identity, a clear value of the wrong kind, a
  depth-testing pipeline in a depthless pass, two items carrying
  different data for one per-frame buffer, an item whose geometry and
  whose pipeline's per-vertex input disagree, a mesh whose stride the
  pipeline does not pack to, an index range without a mesh or reaching
  past the end of one, push data or bindings missing,
  mis-counted or of the wrong class against the declaration, uniform data
  missing, surplus or mis-sized, a block buffer a different size from the
  block its pipeline declares, a frame that reads a render
  image it never wrote or discarded — are refused by named assertions
  before any GPU call.
- `RenderPipeline` — two SPIR-V stages (or one, for the depth-only
  shape), optional per-vertex and
  per-instance input, an optional vertex-stage push-constant range (at
  most 128 bytes, the guaranteed device minimum; items then carry
  exactly that many bytes per draw), optional sampled-binding slots
  (items then name exactly that many `Binding`s per draw, which is how N
  textures share one pipeline), an optional `uniform_block` read at the set
  after them (items then name one more `Binding` and carry exactly that
  many bytes) — the two share a budget of 4 bound sets, the guaranteed
  device minimum — and optional
  `DepthState` (test/write, compare fixed
  `GREATER_OR_EQUAL` — depth is reversed: nearer is larger, the far
  plane is zero, depth clears to zero). Three pipeline shapes: `PipelineDesc::new` takes
  `Shaders`, whose stages write their own vertex list and carry the
  count they generate; `PipelineDesc::mesh` takes `MeshShaders` and a
  per-vertex layout, and has no count at all because the geometry
  supplies it; `PipelineDesc::depth_mesh` takes one vertex stage over a
  per-vertex layout — no fragment stage, no color attachment,
  `TargetFormat::DepthOnly` — for pipelines that draw only into
  depth-kinded render images. `builtin` carries the embedded shader bundles — a colored
  triangle, textured full-target quads over one and two sampled slots,
  instanced quads with and without per-instance depth, the particle
  billboard, and the mesh pairs (sources and compile record in
  [shaders/](https://github.com/renew-engine/renew/blob/main/crates/rhi/shaders/README.md)).
- `Binding` — one written descriptor set behind one of the device's two
  canonical layouts, and shared ownership of what it points at. Either a
  **sampled** binding (a combined image sampler at binding 0: a texture
  or render image, plus the sampler that reads it) or a **uniform
  block** (a dynamic uniform buffer at binding 0, over a per-frame
  buffer). Written once at creation, never rewritten — the
  write-while-outstanding rule a mutable set would need does not exist
  here, and a block's *contents* change without its descriptor moving.
  Items name bindings per draw in slot order (slot *i* is set *i*),
  sampled slots first and the block last; a mismatch against the
  pipeline's declared count **or class** is a named refusal before any
  GPU call, and a binding named by several items costs one retention
  slot, like a mesh.
- **Two per-draw data channels, and the second is the one with room.**
  `PipelineDesc::push_constant_size` declares a vertex-stage push range,
  capped at `MAX_PUSH_CONSTANT_BYTES` because that is what the device
  guarantees; `PipelineDesc::uniform_block` declares a std140 block up
  to `MAX_UNIFORM_BLOCK_BYTES` (16 KiB, the guaranteed
  `maxUniformBufferRange`), read at set `sampled_bindings`. An item
  supplies each with `push_data` and `uniform_data`; both are matched to
  their declaration for presence and length before any GPU call. Push
  bytes go into the command stream and retain nothing; block bytes are
  copied into the frame's slot region of the buffer its binding reads,
  under the same one-buffer-one-write rule instance data follows.
- `RenderImage` — what one pass renders into and a later pass samples:
  one physical image, kinded Color (`Rgba8Unorm`) or Depth (the
  device's chosen format) at creation, with the format pre-checked
  against the adapter's own sampled/attachment features. By default its
  **contents are frame-scoped** — every frame's first use starts
  undefined, enforced by the same walk that plans its barriers — while
  the image itself is retained by any frame that names it. An image
  created **kept** (`RenderImageDesc::kept`) persists its contents
  instead: a later frame may open it with `LoadOp::Load` or sample it
  without rendering it at all — the shape of any render-to-texture on
  its own cadence (a shadow map under a slow sun, a probe, a minimap).
  The first frame ever must still render before anything reads, and a
  discarding write takes the permission back; both refused by name. A pass
  renders into one via `Pass::render_to` (the image's kind decides the
  pass shape, so a mismatch is unrepresentable), and items sample it
  through an ordinary binding. The per-frame identity rules — write
  before read, no feedback within a pass, no re-targeting after
  sampling, Store before a later sample, at most 4 distinct images —
  are named refusals before any GPU call. Depth-kinded images draw
  through `PipelineDesc::depth_mesh` pipelines: no fragment stage, no
  color attachment, `TargetFormat::DepthOnly`.
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
wait-idle); `Texture`, `Sampler`, `Binding` and `RenderImage`
deliberately do not.
The binding holds shared ownership of its texture and sampler, so
their `Drop` cannot run while a set still points at them — and the
binding itself is held by the retention table of any frame that named
it, released only after that frame's work provably ended, so its own
`Drop` cannot run while a submit could still read the set. The
`WindowTarget` owns a keep-alive handle to its window: for as long as
the target lives, the OS window cannot be torn down under its surface,
by construction. On a platform that revokes windows, the platform
layer's surface-epoch contract obliges the target's owner to drop the
whole target when the epoch closes — the keep-alive covers the
target's own lifetime, and the epoch decides how long that may be.

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
pass vocabulary + three pipeline shapes + per-draw sampled bindings and
one uniform block + render images + geometry — per-vertex and
per-instance input, vertex-stage push constants, indexed draws,
target-owned depth, and render-to-texture with one shared barrier walk
exist; no MSAA, no multiple render targets, no storage buffers and no
compute, two fixed descriptor-set layouts (a combined image sampler at
binding 0, repeated per declared slot; and a dynamic uniform buffer at
binding 0, at most one) — grown only when a consumer demands it. Mesh memory is
host-visible rather than device-local, which is a recorded decision with
a written reopening trigger (a real-GPU frame-time measurement showing
vertex fetch matters) and not an oversight. The `[package.metadata.renew]` table
in [Cargo.toml](https://github.com/renew-engine/renew/blob/main/crates/rhi/Cargo.toml) is authoritative for maturity and manifest
metadata. Contract lints live in [clippy.toml](https://github.com/renew-engine/renew/blob/main/crates/rhi/clippy.toml): thread
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
