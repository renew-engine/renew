<div align="center">

# renew

**A code-first, modular game engine written in Rust.**

[![CI](https://github.com/renew-engine/renew/actions/workflows/ci.yml/badge.svg)](https://github.com/renew-engine/renew/actions/workflows/ci.yml)
[![Nightly checks](https://github.com/renew-engine/renew/actions/workflows/nightly-checks.yml/badge.svg)](https://github.com/renew-engine/renew/actions/workflows/nightly-checks.yml)
[![Coverage](https://img.shields.io/badge/coverage-%E2%89%A595%25-brightgreen)](https://github.com/renew-engine/renew/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.97%2B-orange)](rust-toolchain.toml)
[![Edition](https://img.shields.io/badge/edition-2024-blueviolet)](Cargo.toml)
[![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey)](https://github.com/renew-engine/renew/actions/workflows/ci.yml)
[![Unsafe](https://img.shields.io/badge/unsafe-denied-success)](Cargo.toml)

</div>

---

**renew** is a game engine built for the long haul: every capability is a library with a
command-line face, every module is independently buildable, testable, and removable, and
the simulation core is deterministic by construction — same seed, same inputs, same
result, bit for bit.

> **Status: early development.** Pre-0.1, APIs are unstable and everything is subject
> to change. Not yet ready for use in projects.

## Design principles

- **Code-first.** Everything the engine does is scriptable and headless-capable;
  graphical tools are clients of the same public APIs, never privileged.
- **Deterministic simulation.** Fixed timestep, no hidden clocks, no unseeded
  randomness. Replays and lockstep behavior are a foundation, not an afterthought.
- **Modular to the core.** A small required core; everything else is optional,
  removable, and behind explicit interfaces.
- **Measured, never assumed.** Performance claims come with benchmarks; budgets are
  enforced, not wished for.

## Quality gates

Every commit on `main` passes, on Windows, Linux, and macOS:

| Gate | Enforced by |
|---|---|
| Format & lints (zero warnings) | `rustfmt`, `clippy` with a strict deny-set |
| Tests | `cargo test` on all three platforms |
| Test coverage ≥ 95% lines | `cargo llvm-cov` gate in CI |
| No `unsafe` code | `unsafe_code = "deny"` workspace-wide |
| No panicking shortcuts | `unwrap`/`expect` denied outside tests |
| Debug & release builds | build matrix |
| Sanitizers (ASan, TSan) | weekly scheduled runs |

## Getting started

Requires stable [Rust](https://rustup.rs) 1.97 or newer.

```sh
git clone https://github.com/renew-engine/renew
cd renew
cargo build --workspace
cargo test --workspace
cargo run --bin hello-engine
```

`hello-engine` is the current proof of life: a deterministic fixed-timestep loop that
produces bit-identical output on every run, on every platform.

## The `renew` tool

One binary drives the workspace's development tasks the same way for people, scripts,
and CI alike:

```sh
cargo run --bin renew -- help
```

| Command | What it does |
|---|---|
| `renew build` / `test` / `bench` / `lint` | the workspace tasks, the same commands CI runs (CI's bench stage uses `bench --smoke`: every bench once, no statistics) |
| `renew check` | verifies crate manifests and the dependency graph (also a CI gate) |
| `renew doctor` | checks your toolchain against the repository's pins |

Every command takes `--json` and emits a single schema-versioned document, so tooling
can build on stable output while the human output stays readable.

## Planned

Rendering (Vulkan-first), ECS, asset pipeline, audio, input mapping, and 2D samples —
in roughly that order, each landing behind the same quality gates.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
Contributions, when opened, are accepted under the same license — see
[CONTRIBUTING](CONTRIBUTING.md).
