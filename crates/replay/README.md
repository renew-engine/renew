# renew-replay

Deterministic input replay: translation between the engine's event
vocabulary and the trace format (both directions), a tick-indexed loader
for stored traces, and the recorder that produces them.

## Why it is a crate

Any game shipping a replay, a demo mode, or a deterministic regression
harness needs exactly this. The code lived inside one sample until a
second consumer was committed; copying it would have meant maintaining a
correctness property in two places, and the property is load-bearing: an
unknown event **refuses** loudly, because silently dropping one makes a
replay diverge from its recording in a way no test would catch.

## Contract

**This crate does no I/O and reaches no OS capability, at any dependency
depth.** It declares the determinism marker, so the workspace structure
check refuses it a path to the platform crate; its lint file bans the
filesystem, the clock, threads and self-seeding collections by name.
Text in, events out; events in, text out. Where bytes come from and
where they go is the caller's business.

**Events are indexed by tick, counted from zero** — the trace format's
own meaning. Frame numbering is a driver convention; drivers apply their
own shift. The one driver that numbers frames from one does exactly
that, on its own side of the boundary.

## What is here

- `convert` — `to_trace` / `from_trace` between the event vocabulary and
  trace events, with `Unencodable` carrying the shape index a refusal
  names.
- `events(name, text)` — a stored trace's event lines as
  `(tick, WindowEvent)` pairs; the header is provenance and is not read
  here.
- `Recorder` — collects events against the tick they were delivered
  before, refusing once, at `finish`, so a recording failure does not
  abandon a run in progress.

## Testing

Unit tests from first commit. Determinism-test row: **N/A with
reasoning** — this crate is a stateless translator with no simulation to
run; the round-trip properties here and the committed-trace fixed-point
test in its consumer are the determinism evidence that exists. Fuzz row:
the text parser lives in `renew-trace`, which carries the fuzz target;
this crate consumes the parsed structure.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies
and extension points. Read it there rather than here.
