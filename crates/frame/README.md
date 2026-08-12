# renew-frame

Fixed-timestep frame scheduling: the deterministic accumulator, the step
budget that bounds a stall, and the interpolation factor for rendering
between steps.

The loop is a **passive integer state machine**. It owns no loop, drives
no application, knows nothing of rendering, GPUs or windows, and never
reads a clock — it *cannot*, having no dependency that offers one. Its
whole job is one total function:

```rust
let plan = frame.begin_frame(now);          // how many steps, how many refused, how far between
for step in plan.steps() { world.step(step); }
let alpha = plan.alpha();                   // render between steps with this
stats.absorb(&plan);
```

The caller reads the one clock, executes the steps, and renders. Every
question about the operating-system loop, the RHI, removability and
headless operation resolves the same way: **the application owns those
things and the loop never learns their names.** That is why this crate
compiles identically with the GPU crate deleted, with windowing compiled
out, and with no GPU present — there is no edge to remove and not one
`#[cfg]` in the crate.

- `Nanos` / `Timestamp` — a duration and an instant, integer nanoseconds
  in separate newtypes, so passing one where the other belongs is a
  compile error rather than a simulation frozen forever.
- `Timestep` / `StepBudget` — the fixed step (`Timestep::HZ_60`) and the
  most steps one frame may run (`StepBudget::DEFAULT` = 5). Both non-zero
  by type: no division can trap and no constructor can fail.
- `FrameLoop` — the schedule. `begin_frame(now)`, plus `resync(now)` for
  pauses the caller knows about.
- `FramePlan` / `Steps` / `Step` / `Alpha` — what a frame must do. A
  `Copy` value that borrows nothing.
- `StateHash` — FNV-1a-64 by explicit ordered absorption; the answer to
  "did two runs produce the same state".
- `FrameStats` / `FrameTiming` — the deterministic tally and the measured
  timing, deliberately separate types, each with a JSON adapter.

## Contract

- **Deterministic.** For a fixed build and platform, `FrameLoop` is a
  pure function of `(timestep, budget, start, the sequence of timestamps
  passed to begin_frame)`. It reads no clock, allocates nothing, spawns
  nothing, and holds no iteration-order-dependent state. A headless run
  supplies that sequence synthetically and is reproducible; a realtime run
  supplies a measured one, which is a different *input trace*, not
  nondeterministic *code*.
- **Nothing can fail.** Non-zero types, a saturating bank and a saturating
  delta leave no error to report, so `begin_frame` returns no `Result` —
  an uninhabitable error variant would be a lie about the API. Nothing
  here panics and nothing unwinds.
- **The plan must be executed.** A caller that ignores its plan silently
  desynchronizes the simulation from the tick counter, and that is
  unobservable from inside the loop. `#[must_use]` and the iterator shape
  are the mitigation; the guarantee is the caller's.
- **Clamp and discard, always reported.** Steps beyond the budget are
  discarded, never banked — keeping the surplus *is* the spiral of death.
  Simulation time therefore falls permanently behind the wall clock, and
  `FramePlan::dropped()` is the exact, non-optional record of by how much.
  A frame with a nonzero drop count is a measurable budget violation.
- **`alpha` is never an input to simulation.** It is a render-side hint in
  `[0, 1)`, and it is deliberately excluded from the schedule digest —
  it is a pure function of the remainder and the timestep, both of which
  *are* digested. `remainder()` and `timestep()` stay public so an exact
  consumer never goes through the float.
- **Zero dependencies, and this crate never logs.** A dropped step is
  reported through the returned plan; whether that becomes a log line is
  the caller's decision.

## Binding to a windowing seam

Two properties of an inverted-control window loop that a reader will
otherwise get wrong:

- **The render lags the step phase by one iteration.** A redraw requested
  from the update callback arrives on the *next* iteration, so the frame
  that draws consumes the alpha stored by the previous update. Harmless —
  alpha is a hint, and an operating-system repaint with no intervening
  update correctly re-renders at the same alpha — but it must be stated or
  someone will "fix" it.
- **Close requests are latched, not acted on.** The event callback has no
  control handle, so a close request is recorded and acted on in the next
  update.

Anchor the schedule *after* bring-up. Device creation costs on the order
of 100 ms and must not be banked as frame one, or the loop opens with a
clamped burst and a drop count that means nothing.

A stall needs no loop knowledge and that is the payoff: a slow present, a
timeout, a driver hitch — the frame took 200 ms, the next `begin_frame`
sees a 200 ms delta, the budget clamps, and `dropped()` reports the
deficit (measured: due 11, run 5, dropped 6). After a stall the caller
*knows* was not real time — a breakpoint, a load — it calls `resync`. A
dormant window is per-application policy in one line at the call site:
keep stepping and skip the render, or return before `begin_frame` and
`resync` on resume.

## Why integer nanoseconds

With `f32` seconds the banked time accumulates representation error and
the step count becomes a function of rounding history. With `u64` every
operation is exact, so the step count is a pure integer function of the
input sequence.

Where it strains, stated rather than hidden: 60 Hz is not representable —
`60 × 16_666_667 = 1_000_000_020`, so sixty ticks run 20 ns long against
the wall. That is closed by definition rather than by rounding.
`Step::sim_time` and `FrameLoop::simulated()` are `tick × dt`, so the
simulation's own clock is exact by construction and the 20 ns is a
property of the wall clock's relation to the simulation, never of the
simulation's own arithmetic.

The one float in the crate is `Alpha`, derived and clamped in one place.
The clamp is mandatory, not defensive: a naive `rem as f32 / dt as f32`
returns exactly `1.0` at 30 Hz, and even with an `f64` intermediate the
1 Hz case still rounds up to `1.0`. An alpha of `1.0` is a renderer
popping a full tick ahead of the state it interpolates from.

## Testing note

The simulation regime applies. Unit tests cover the accumulator, the
budget, resynchronization and saturation at both ends; property tests
cover the conservation law (every submitted nanosecond is executed,
dropped, or banked), totality over the whole 64-bit domain, and the alpha
bound over every `(timestep, remainder)` pair. `tests/determinism.rs`
carries the evidence: eight in-process runs of one hostile trace, a frozen
digest, a negative control that perturbs the anchor by one nanosecond, and
a fixed-point reference world stepped by the plans. Every one of those
asserts the trace was not vacuous — that it executed steps, engaged the
budget, and moved the digest — before it compares anything.
`tests/zero_alloc.rs` pins the allocation contract with a counting global
allocator.

Deliberately not applicable, each for one reason: fuzzing (no parser of
external data — the timestamp is an in-process `u64`), thread-sanitizer
and stress testing (the crate spawns no threads and is not shared across
them), and Miri (no `unsafe` anywhere in the crate).

## Status

Early-stage. The `[package.metadata.renew]` table in
[Cargo.toml](https://github.com/renew-engine/renew/blob/main/crates/frame/Cargo.toml) is authoritative for maturity and all manifest
metadata. The crate's contract lints live in [clippy.toml](https://github.com/renew-engine/renew/blob/main/crates/frame/clippy.toml):
clock reads, thread spawning, filesystem access and randomly seeded hash
containers are rejected at lint time, because this is the tree's first
crate designated as simulation code and one `Instant::now` inside
`begin_frame` would destroy determinism for every consumer with no test
failing anywhere until a replay diverged months later.

`extension_points = []` is honest: no trait, no `dyn`, no runtime
polymorphism. The growth point is named rather than pre-built — a trait
arrives when a second implementation exists.

## Key decisions

- **The loop plans, it does not drive.** A host trait with in-crate
  windowed and headless runners was considered and rejected: the windowed
  host must read a clock, which forfeits the simulation designation on the
  one crate that needs it, and its only justification is a small amount of
  sample glue.
- **Absolute timestamps, not deltas.** One subtraction in one place; a
  backwards clock becomes zero instead of 1.1 trillion phantom steps; the
  first-frame branch disappears into the constructor's `start`; and
  `resync` is trivially correct.
- **One guard, not two.** An elapsed-time clamp in front of the budget
  only changes how much time is discarded versus reported, destroying the
  information about how big the hitch was in exchange for a second knob, a
  second branch and a second coverage obligation.
- **Two report types.** A single type hashing all its fields would absorb
  measured wall time into the determinism digest — silently, since the
  gate would simply never go green and someone would "fix" it by loosening
  the comparison.
- **Hand-rolled FNV-1a-64.** `RandomState` is seeded per process and can
  never back a cross-run claim; `SipHasher13` has no cross-version
  stability guarantee; `#[derive(Hash)]` absorbs in declaration order
  implicitly, so reordering two fields would silently change every digest.

## Known gaps

- **Percentiles are absent** from `FrameTiming`: p50/p99 need a reservoir
  or a histogram, which is a real design. Count, minimum, maximum and sum
  are enough for a first baseline.
- **Step execution is unenforced.** A passive plan cannot prove the caller
  ran its steps; `#[must_use]` and the iterator shape are the mitigation.
- **Nothing here paces frames.** No sleeping, no vsync targeting, no
  render-rate limiter. A caller that polls without waiting busy-spins;
  fixing that needs a platform addition, not a loop change.
