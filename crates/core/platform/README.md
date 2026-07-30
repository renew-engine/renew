# renew-platform

The engine's only doorway to the operating system: a monotonic clock,
whole-file I/O, and named threads — each a thin, explicit seam.

- `Clock` — a value the caller owns, anchored at `start()`, reporting
  integer nanoseconds (`elapsed_nanos`, saturating at ~584 years). No
  floating-point time exists in the engine.
- `fs` — whole-file operations (`read`, `read_to_string`, `write`,
  `exists`), every error naming its path with a classified kind.
- `thread` — `spawn_named` and `ThreadHandle`: every engine thread
  carries a name, and a *joined* thread's panic surfaces as an error
  naming it. Dropping the handle detaches the thread — deliberate, and
  the handle is `#[must_use]` so detaching is always a visible choice.

## Contract

- **Doorway, not hallway.** The rest of the engine never touches
  `std::time`/`std::fs`/`std::thread` directly — thread creation belongs
  in this crate alone, and even the error-kind vocabulary is re-exported
  here so consumers never import `std::io`.
- **No ambient state.** No global clock, no environment reads; everything
  is a value or an explicit call.
- **Errors carry context** — paths and thread names, in crate-local
  enums.
- **Nothing here is simulation state.** The clock serves frame pacing and
  diagnostics; simulation time is fixed-step by construction.

## Status

Early-stage: surfaces grow when a consumer needs them (streaming I/O
arrives with the asset pipeline; dynamic-library loading is deliberately
deferred until something loads one). The `[package.metadata.renew]` table
in [Cargo.toml](Cargo.toml) is authoritative for maturity and manifest
metadata. This crate contains no `unsafe` code.

## Key decisions

- **Integer nanoseconds only** — matches the fixed-timestep vocabulary
  and keeps time bit-exact in every context that touches it.
- **Whole files only in v0** — the asset pipeline decides what streaming
  looks like; guessing now would speculate its API.
- **Named threads or no threads** — a nameless thread in a profiler or a
  panic message is a debugging tax nobody needs to pay.
