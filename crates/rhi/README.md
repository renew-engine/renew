# renew-rhi

The engine's only doorway to the GPU: device bring-up, render targets,
and the v0 clear-and-triangle draw path, over Vulkan. Correctness is
provable headless — the offscreen target renders and reads back pixels
without a window or a display server, and the golden-image tests attest
the bytes.

- `Device` — the GPU context, built from a plain-data `DeviceDesc`
  (validation policy: `Off` / `IfAvailable` / `Required`). Deterministic
  adapter choice: discrete > integrated > virtual > software, then
  lowest device id; requires Vulkan 1.3 dynamic rendering and
  synchronization2 plus a graphics queue.
- `OffscreenTarget` — a fixed-size RGBA8 image with synchronous
  `render(clear, Option<&RenderPipeline>)` and CPU readback
  (`read_back_into`). The correctness spine.
- `WindowTarget` (feature `present`, default on) — surface + swapchain
  over an opaque window handle; `render` returns `Presented` or
  `NeedsResize` (resizes and minimized windows are protocol outcomes,
  never errors). One frame in flight, FIFO presentation.
- `RenderPipeline` — two SPIR-V stages and no vertex buffers, with an
  optional sampled texture bound at creation. `builtin` carries the
  embedded shader bundles — a colored triangle and a textured
  full-target quad — each pairing its stages with the vertex count they
  generate, so the two cannot be mismatched (sources and compile record
  in [shaders/](shaders/README.md)).
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
  asserts zero validation errors at teardown; the fault suite runs
  `IfAvailable` with one `Required` scenario, and the golden and
  present suites run `IfAvailable`. **No `Required` lane constructs a
  window target**, so the strict validation oracle covers the offscreen
  path only. Golden tests pin exact bytes on the CI-pinned software
  rasterizer.
- **Steady-state frames allocate nothing** on the engine side — pinned
  by an allocation-counting integration test **that builds an offscreen
  target only**. The window render path has no allocation gate; the
  contract covers it, the test does not. Driver host allocations
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

Early-stage: the surface is exactly device + two target kinds + one
pipeline shape + one sampled texture — no vertex buffers, no depth, no
MSAA, one fixed descriptor layout (a combined image sampler at set 0,
binding 0) — grown only when a consumer demands it. The `[package.metadata.renew]` table
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
