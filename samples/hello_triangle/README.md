# hello_triangle

The fixed-timestep frame loop driving the renderer: a triangle over a
clear colour the simulation computes, in a window or into an offscreen
image.

```sh
cargo run -p renew-sample-hello-triangle --bin hello_triangle -- --frames 600
cargo run -p renew-sample-hello-triangle --bin hello_triangle -- --headless --frames 600
```

```console
$ cargo run -p renew-sample-hello-triangle --bin hello_triangle -- --headless --frames 600
renew-frame sample=hello_triangle seed=0 frames=600 ticks=600 dropped=0 schedule_hash=0xefc2181bbc0d588d state_hash=0x00f10d96fc295d99
```

That line is the same on every run, in every process, and in a build
with the whole windowing stack compiled out
(`--no-default-features`) — which is the point of the sample.

## What it does

`renew-frame` owns no loop. It answers one question — *given the
schedule so far and this instant, how many fixed steps are due, how many
did the budget refuse, and how far between steps is the renderer* — and
this sample is the other half of that arrangement. Both of its drivers
are the same three lines:

```rust
let plan = frame.begin_frame(now);
for step in plan.steps() { world.step(step); }
stats.absorb(&plan);
```

- **Windowed** (default): the OS owns the loop. The window seam calls
  `ready` once (device, swapchain target and pipeline are built there —
  it is the only place a live window exists), then `event` and `update`
  every iteration. The one clock read on the path is at the top of
  `update`; rendering happens on `RedrawRequested` and nowhere else.
- **Headless** (`--headless`): no window, no clock in the schedule. Time
  is synthetic — frame *k* happens at exactly `k × 16 666 667 ns` — so
  one step runs per frame and the whole run is a pure function of
  `(frames, seed)`.

The world is one integer value walked by a seeded stride. Its low three
bytes are the clear colour, converted with `k / 255`, which every
conformant adapter converts back to the byte `k` exactly. That is what
lets the headless test assert **every pixel** against a colour it
*computes* from the tick count, with no committed image and no refresh
ritual: run one step too many and the bytes change.

## Command line

| Flag | Meaning |
|---|---|
| `--headless` | No window: an offscreen image and a synthetic clock. |
| `--frames N` | Windowed, stop after N *presented* frames; headless, run N frames (default 600, ten seconds at 60 Hz). |
| `--seed N` | Selects the world's stride. The seed axis is a placeholder until there is a random-number service; it feeds the world so the shape of the flag survives. |
| `--dump-stats PATH` | Write the JSON report there, after the run. |

Two output channels, on purpose. Stdout carries exactly one line — the
digest line above, which the cross-process determinism gate
string-compares, needing no JSON parser and staying readable in a CI
log. `--dump-stats` carries the machine-readable document:

```json
{"schema_version":1,"sample":"hello_triangle","seed":0,
 "frame":{"frames":600,"ticks":600,"steps_dropped":0,"schedule_hash":"0x…"},
 "state_hash":"0x…",
 "timing":{"count":600,"min_ns":…,"max_ns":…,"sum_ns":…,"drawn":600,"skipped":0}}
```

Everything gated lives in `frame` and `state_hash`; everything measured
lives in `timing` and is recorded, never gated.

Exit codes: `0` for a completed run or a skip, `1` for a failure, `2`
for a command line this build cannot honour.

## Contract

- **`--headless` implies a synthetic time source.** Nothing measured
  reaches the schedule, the state hash or the digest line. The one clock
  the headless driver reads brackets each frame for the timing summary,
  which is the frame-time readout the allocation gate below exists to
  keep out of the frame path.
- **The simulation is integer-only.** Bit-determinism is scoped to one
  platform, and the transcendental functions differ between platform
  math libraries; a world holding an angle would quietly make the state
  hash a cross-platform promise the engine does not make. If the
  triangle ever spins, the angle is a tick count and the trig happens in
  the shader — render, not simulation.
- **Steady state is frames `[3, N)` of a headless run** (§10).
  Everything that allocates happens before frame zero: device, target,
  pipeline, and the readback buffer. Inside the boundary there is no
  file I/O, no logging, no formatting and no serialization —
  `--dump-stats` writes after the loop exits.
- **An environment that cannot host the run is a skip, not a failure.**
  No GPU runtime and no display server are ordinary answers on ordinary
  machines: the binary prints `SKIP:` and exits zero. Set
  `RENEW_FRAME_STRICT=1` on a lane that exists to run this, and a skip
  becomes a failure — a lane that passes by skipping proves nothing.
- **A dormant window is not an error.** A minimized window or a stale
  swapchain presents nothing; the frame is counted as skipped (so it
  cannot inflate the frame-time summary), the simulation keeps stepping,
  and the target is rebuilt at the size the app knows. A run that stops
  presenting altogether for five seconds ends by saying it is wedged,
  rather than spinning forever.

## Two seam properties, stated so nobody "fixes" them

- `update` runs *after* the event phase, so a redraw requested in
  iteration N arrives in N+1: the render lags the step phase by one
  iteration and draws with the alpha stored then. Harmless — alpha is a
  hint, and an OS repaint with no intervening update correctly re-renders
  at the same alpha.
- `WindowApp::event` receives no loop control, so a close request cannot
  exit the loop where it arrives. It is latched and acted on in `update`.

## Removability

`renew-frame` has no dependency on the renderer, and this sample is
where the two meet. Built with `--no-default-features` there is no
windowing library and no window-system integration anywhere in its
graph — `renew-frame`, `renew-platform` and `renew-rhi` only — and the
same binary produces the same digest line as the windowed build's
headless mode. Asking that build for a window exits non-zero and says
why.

## Tests

| File | What it proves |
|---|---|
| `tests/headless_frame.rs` | The readback holds the colour the world computed for the last tick; one more step is a different image; the triangle covers the middle and one tick drawn twice is the same bytes. |
| `tests/cli_determinism.rs` | Three separate processes print one digest line; a different frame count or seed prints a different one; the stats file agrees with it. |
| `tests/zero_alloc.rs` | The steady-state frame path performs no heap allocation. |

The unit tests beside the source drive the window callbacks directly —
`event`, `update`, the draw and stall verdicts — with no window at all.
Only `ready` needs one, because it borrows a live OS window.
