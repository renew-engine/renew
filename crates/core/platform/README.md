# renew-platform

The engine's only doorway to the operating system: a monotonic clock,
whole-file I/O, named threads, the window, audio output, and UDP
datagrams — each a thin, explicit seam.

- `Clock` — a value the caller owns, anchored at `start()`, reporting
  integer nanoseconds (`elapsed_nanos`, saturating at ~584 years). No
  floating-point time exists in the engine.
- `fs` — whole-file operations (`read`, `read_to_string`,
  `read_to_string_bounded`, `write`, `exists`), every error naming its
  path with a classified kind. The bounded read exists because a parser
  can only validate a hostile file once it is already in memory, so a
  size limit has to be enforced here, before the allocation — and it is
  applied to the bytes actually read, never to the size the filesystem
  claims, which can be stale or a lie about a growing file.
- `thread` — `spawn_named` and `ThreadHandle`: every engine thread
  carries a name, and a *joined* thread's panic surfaces as an error
  naming it. Dropping the handle detaches the thread — deliberate, and
  the handle is `#[must_use]` so detaching is always a visible choice.
- `event` — a re-export of **`renew-event`**, which owns the vocabulary:
  `WindowEvent`, `KeyCode`, `PointerButton`, and the shape table that
  forces a new variant to be handled. Naming a key is not the same as
  opening a window, and a consumer that speaks only the vocabulary — an
  input layer, a replay harness, a headless server — should take
  `renew-event` directly and not this crate at all.
  **It used to be a module here, kept out from behind the `window`
  feature deliberately.** That expressed the right intent through a
  mechanism the dependency graph cannot see: the consumer still took on
  a crate owning a clock, a filesystem and thread spawning. A crate
  boundary is the version of that promise a graph can read.
  `window` re-exports it too, so every existing path works.
- `window` (default-on feature) — one OS window, its event loop, and
  keyboard/mouse input behind an engine-only vocabulary: the OS owns
  the loop, a `WindowApp` receives translated events and drives exit
  and redraws (`WindowApp` is the manifest's `window-app` extension
  point), and no windowing-library type crosses the boundary. The
  window's title can be changed after creation, which is how a sample
  puts a live number where a person can see it without a text renderer.
  Headless builds disable default features and compile the entire
  windowing stack out — the vocabulary in `event` survives that, which is
  the point of the split; headless environments at runtime get a
  recoverable `LoopUnavailable`. The loop runs on the main thread only
  (every desktop platform requires it).
- `audio` (default-**off** `audio-out` feature) — the default output
  device, a negotiated stream shape, and a fill callback the OS audio
  thread drives. Bring-up is two phases because the shape has to be
  known before anything can be built to produce it: `open` reports
  channels and sample rate, `start` takes the callback. `f32` only, no
  device enumeration, and no audio-library type in any signature. A
  machine with no sound card gets a recoverable `Unavailable`, and
  `healthy()` reports whether the stream is still playing — a route
  change or an underrun leaves it true, because those are survivable
  and reporting a muted run over audible sound would be a lie. The
  feature is off by default: a build that plays nothing compiles no
  audio stack at all.

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

## Thread safety and ownership

`Clock` is `Copy + Send + Sync`; copies share the anchor, monotonicity
holds per program-ordered call sequence, and no cross-thread ordering
is claimed beyond the caller's own synchronization. `ThreadHandle<T>`
is `Send` and structurally `Sync` — ownership of the join may move
threads, and shared references are harmless because `&self` offers
only the name while `join` consumes the handle, so exactly one owner
observes the result. Dropping a handle detaches the
thread: deliberate, documented at the type, and `#[must_use]` so it is
always a visible choice; a detached thread's panic never reaches the
engine's error model. The `fs` functions are stateless; concurrent
callers get whatever ordering the operating system gives them.

## Testing note

This crate is a set of thin, safe OS wrappers — not a job system, so
the stress-test regime for threaded systems belongs to the future jobs
crate, not here; not a parser of external data, so fuzzing does not
apply. Its obligations are the unit and integration suites (error
classification injected per kind, real-filesystem round trips with
drop-guarded scratch files, spawn/join/panic surfacing) plus the
scheduled sanitizer workflow, all present.

## Status

Settled enough to depend on: these seams are what other crates in this
workspace build against, and breaking them is avoided; surfaces still
grow when a consumer needs them (streaming I/O arrives with the asset
pipeline; dynamic-library loading stays deferred until something loads
one) — examples and a performance narrative are still to come. The
`[package.metadata.renew]` table in [Cargo.toml](https://github.com/renew-engine/renew/blob/main/crates/core/platform/Cargo.toml) is
authoritative for maturity and manifest metadata. This crate contains
no `unsafe` code.

## Key decisions

- **Integer nanoseconds only** — matches the fixed-timestep vocabulary
  and keeps time bit-exact in every context that touches it.
- **Whole files only in v0** — the asset pipeline decides what streaming
  looks like; guessing now would speculate its API.
- **Named threads or no threads** — a nameless thread in a profiler or a
  panic message is a debugging tax nobody needs to pay.
