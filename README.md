<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/brand/renew-banner-wide.svg">
  <source media="(prefers-color-scheme: light)" srcset="assets/brand/renew-banner-wide-light.svg">
  <img src="assets/brand/renew-banner-wide.svg" alt="renew — an AI-first game engine in Rust" width="820">
</picture>

<br>
<br>

[![CI](https://github.com/renew-engine/renew/actions/workflows/ci.yml/badge.svg)](https://github.com/renew-engine/renew/actions/workflows/ci.yml)
[![Nightly checks](https://github.com/renew-engine/renew/actions/workflows/nightly-checks.yml/badge.svg)](https://github.com/renew-engine/renew/actions/workflows/nightly-checks.yml)
[![Coverage](https://img.shields.io/badge/coverage-100%25%20minus%20named%20exemptions-brightgreen)](https://github.com/renew-engine/renew/actions/workflows/ci.yml)

[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.97%2B-orange)](rust-toolchain.toml)
[![Edition](https://img.shields.io/badge/edition-2024-blueviolet)](Cargo.toml)
[![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey)](https://github.com/renew-engine/renew/actions/workflows/ci.yml)
[![Unsafe](https://img.shields.io/badge/unsafe-denied%20by%20default-success)](#quality-gates)

**[Quick start](#quick-start)**&nbsp; · &nbsp;**[Modules](#modules)**&nbsp; · &nbsp;**[The `renew` CLI](#the-renew-cli)**&nbsp; · &nbsp;**[Quality gates](#quality-gates)**&nbsp; · &nbsp;**[Contributing](CONTRIBUTING.md)**

</div>

---

**renew** is a game engine built for the long haul — and built to be *operated*, not just
used. Every capability is a library with a command-line face and machine-readable output,
so a person, a script, or an agent drives the engine through exactly the same surface. The
simulation core is deterministic by construction: same build, same seed, same inputs, same
result, bit for bit.

> [!NOTE]
> **What "AI-first" means here.** Every capability is headless, scriptable, and emits
> schema-versioned JSON beside its human-readable output, so nothing about the engine
> requires a person in front of a screen. It does *not* mean the engine ships AI features —
> it means the machine-operable surface is the primary one, and the GUI, when it arrives,
> will be a client of it like everything else.

> [!IMPORTANT]
> **Status: early development.** Pre-0.1. APIs are unstable, modules are still moving
> through the maturity ladder, and this is not yet ready to build a game on. What is here
> is real, tested, and honest about what it is.

## Quick start

Requires stable [Rust](https://rustup.rs) 1.97 or newer.

```sh
git clone https://github.com/renew-engine/renew
cd renew
cargo build --workspace
cargo test --workspace
cargo run --bin hello-engine
```

`hello-engine` is the current proof of life — a fixed-timestep accumulator driven through
60 frames of deliberately uneven frame times, reading no clocks at all:

```console
$ cargo run --bin hello-engine
hello-engine 0.1.1
fixed timestep: 16666667 ns
frames simulated: 60
time submitted: 1245000015 ns
ticks executed: 74
time pending: 11666657 ns
```

> [!TIP]
> Run it twice. Run it on another machine, or another OS. Every byte is identical — that
> is the property everything else in the engine is built to preserve.

## Modules

Every module is independently buildable, testable, and — outside the minimal core —
removable. CI proves the last part on every commit, and proves it *one crate at a time*: twenty-one
configurations, each with one optional crate and everything that depends on it excluded, every one
built **and** tested. A twenty-second builds the minimal core alone and asserts that no optional crate
reached its graph. The platform crate is built again with its windowing feature compiled away, and
the game is built with no graphics crate in its dependency graph at all — which is the removability
claim from the other side, and the reason the window is a feature rather than a default.

| Module | What it does | Maturity |
|---|---|---|
| **`renew-diag`** | Log records, severity levels, and the sink interface the engine reports through | `internal` · core |
| **`renew-event`** | The event vocabulary — key codes, pointer buttons, event shapes — as plain data with no dependencies | `internal` · core |
| **`renew-math`** | `Vec2/3/4`, `Mat4`, `Quat`, `Aabb3` — plain data, documented layout, branchless kernels | `internal` · core |
| **`renew-memory`** | `LinearArena`, a generation-checked `Pool<T>`, and a counting global allocator | `internal` · core |
| **`renew-platform`** | The engine's only doorway to the OS: clock, files, named threads, window | `internal` · core |
| **`renew-fixed`** | Q47.16 fixed-point arithmetic — the number type the simulation is written in, so a result cannot depend on a floating-point mode | `bootstrap` · optional |
| **`renew-frame`** | The fixed-timestep loop: an accumulator over integer nanoseconds, with the clock passed in | `bootstrap` · optional |
| **`renew-ecs`** | Sparse-set storage with a defined iteration order, because an undefined one is a determinism bug waiting for a rehash | `bootstrap` · optional |
| **`renew-jobs`** | A fixed-size worker pool with a deterministic-chunk `parallel_for` | `bootstrap` · optional |
| **`renew-rng`** | Seeded, reproducible random numbers — no thread-local state, no entropy the caller did not ask for | `bootstrap` · optional |
| **`renew-input`** | Input state and mapping, over the event vocabulary | `bootstrap` · optional |
| **`renew-replay`** | Input traces as files: record a run, replay it, compare the digests | `bootstrap` · optional |
| **`renew-trace`** | Simulation digests — the hash the cross-platform determinism lane compares | `bootstrap` · optional |
| **`renew-physics2d`** | Bodies, shapes, broadphase, SAT narrowphase, raycasts, sweeps, and slide resolution, in fixed point | `bootstrap` · optional |
| **`renew-physics3d`** | The same surface in three dimensions, axis-aligned only — rotation waits on a fixed-point orientation type, and the crate says so rather than pretending | `bootstrap` · optional |
| **`renew-rhi`** | The GPU doorway: Vulkan through `ash`, behind an interface that names no Vulkan type in its public API | `bootstrap` · optional |
| **`renew-render2d`** | Sprites from an atlas, one instanced draw | `bootstrap` · optional |
| **`renew-render3d`** | Indexed geometry, depth-tested, in submission order | `bootstrap` · optional |
| **`renew-camera`** | Presentation-side viewpoints: a look-at view, a reversed-depth perspective, and a blend between ticks for display-rate smoothness | `bootstrap` · optional |
| **`renew-snapshot`** | Two captures of one slot space blended by the interpolation factor, keyed so a recycled slot never inherits the previous tenant's motion | `bootstrap` · optional |
| **`renew-particles`** | A fixed-capacity particle pool stepped at the simulation's cadence, seeded so replays reproduce the picture | `bootstrap` · optional |
| **`renew-ui`** | A widget tree solved in fixed point inside the simulation, so a layout is part of what a replay reproduces | `bootstrap` · optional |
| **`renew-ui-render`** | Presentation for the widget tree: retained snapshots blended at display rate, clipped on the CPU, emitted as sprites | `bootstrap` · optional |
| **`renew-audio`** | Mixing and playback, behind the platform's device seam | `bootstrap` · optional |
| **`renew-asset`** | Content-addressed asset packs, with every entry verifiable against its digest | `bootstrap` · optional |
| **`renew-png`** | PNG encoding with no dependencies — pixels in, the bytes of a file out, so a sample can commit a picture of itself | `bootstrap` · optional |

Twenty-six engine crates, five of them core. Six samples and two tools sit beside them; `renew modules`
prints the live list with each crate's declared maturity, read from its manifest rather than from
this table.

Maturity runs `bootstrap` → `internal` → `stable`. A module never claims a level it has not
earned: `internal` means other modules may depend on it, `stable` means the public API is
under change control. Nothing here is `stable` yet.

## How it's built

- **Code-first.** Every capability is a library with a CLI face — build, test, benchmark,
  asset work, running samples. Graphical tools are clients of the same public APIs, never
  privileged.
- **Deterministic simulation.** Fixed timestep, integer nanoseconds, no wall-clock reads, no
  unseeded randomness, no iteration-order-dependent state. Replay and lockstep are a
  foundation, not a retrofit.
- **Modular to the core.** A small required core; everything else is optional, removable,
  and behind an explicit interface — enforced by CI, not by good intentions.
- **Measured, never assumed.** Performance claims arrive with numbers and the configuration
  that produced them. The steady-state frame loop is held to zero heap allocations through
  the engine's allocators, counted in dev builds.
- **Explicit over implicit.** No global mutable state in engine modules; state lives in
  context objects that callers own and pass. Ownership and lifetime are visible at every API
  boundary.

## The `renew` CLI

One binary drives the workspace the same way for people, scripts, and CI:

```sh
cargo run --bin renew -- help
```

| Command | What it does |
|---|---|
| `configure` | verify the toolchain and cargo are present and sane |
| `build` | build the workspace |
| `test` | run the workspace test suite |
| `bench` | run the workspace benchmarks (`--smoke` runs each once, without statistics — CI's mode) |
| `run` | build and start a sample; everything after its name goes to the sample verbatim |
| `record` | run a sample, writing the input it saw to a trace file |
| `replay` | run a sample from a recorded trace, and compare the digest |
| `lint` | check formatting, then run clippy with warnings denied |
| `check` | verify workspace crate manifests and dependencies |
| `coverage` | hold a coverage report against the exemption manifest |
| `modules` | list every module with its maturity, read from the manifests |
| `asset-pack` | build an asset pack from a directory of files |
| `asset-inspect` | list a pack's entries, optionally re-hashing every payload |
| `determinism` | emit this target's simulation digests, or compare several targets' |
| `doctor` | check the development environment |

Every command takes `--json` and emits a single schema-versioned document, so tooling can
build against stable output while the human-readable output stays free to change.

## Quality gates

Every commit on `main` clears all of these, on Windows, Linux, and macOS:

| Gate | Enforced by |
|---|---|
| Format and lints, zero warnings | `rustfmt` and `clippy` with a strict deny-set |
| Tests, debug and release | `cargo test` across the three-platform matrix |
| Line coverage: 100% of every line not individually exempted | `renew coverage --report`, against `coverage-exemptions.toml`. Each exemption names its lines and its reason, and the gate fails in **both** directions — an uncovered line with no entry, and an entry whose line is covered again. `--fail-under-lines 95` also runs, as a loose backstop against a collection that collapsed wholesale |
| No panicking shortcuts | `unwrap`, `expect`, `panic`, `todo`, `dbg!` denied outside tests |
| `unsafe` denied by default | workspace-wide `unsafe_code = "deny"`; the crates that need it opt in per crate, and `undocumented_unsafe_blocks = "deny"` makes every block state the invariant that keeps it sound |
| Module graph is a DAG | crate manifest and dependency-graph check (`renew check`) |
| Optional modules stay removable | twenty-one configurations, each excluding one optional crate and its dependents, all built and tested; plus the minimal core alone, asserted to contain no optional crate |
| Licenses and advisories | `cargo-deny`, over the full dependency tree including dev-dependencies |
| Sanitizers and Miri | scheduled nightly runs (ASan, TSan, Miri) |

## What's next

The list this section used to carry — rendering, an ECS, the asset pipeline, audio, input
mapping, 2D samples — has shipped. What is actually next:

- **The 3D renderer, deepened.** The voxel sample meshes its world, plays in a window through a
  perspective camera — now an engine crate of its own, with display-rate smoothing between
  simulation ticks — and draws from a texture atlas generated in code so golden images stay
  byte-comparable. Depth is reversed engine-wide, and per-draw constants ride push constants.
  Still to come: the particle pool's renderer half, and instancing for chunked geometry.
- **Modules climbing the maturity ladder.** Nothing is `stable` yet, and nothing will claim it
  before its API is under change control and its parsers have been fuzzed.
- **An editor, eventually**, as a client of the same public APIs every other tool uses — never a
  privileged one.

Each lands behind the same gates as everything above.

## Contributing

Issues and pull requests are welcome — see [CONTRIBUTING](CONTRIBUTING.md) for the workflow
and the bar a change is held to. The short version: a change arrives with its tests, its
documentation, and evidence that it works.

## License

Licensed under the [Apache License, Version 2.0](LICENSE). Contributions are accepted under
the same license.

<div align="center">
<br>
<sub>Brand assets live in <a href="assets/brand/"><code>assets/brand/</code></a>.</sub>
</div>
