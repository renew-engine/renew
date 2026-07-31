# input_echo

Window input feeding a fixed-timestep world — with no renderer anywhere
in its dependency graph.

```sh
cargo run -p renew-sample-input-echo --bin input_echo -- --frames 600
cargo run -p renew-sample-input-echo --bin input_echo -- --headless --input-trace walk
```

```console
$ cargo run -p renew-sample-input-echo --bin input_echo -- --headless --input-trace walk
renew-frame sample=input_echo seed=0 source=walk frames=20 ticks=20 dropped=0 schedule_hash=0xcaa0947bf9fa1305 state_hash=0x833e49c9dd637b92
```

## What it does

Hold a direction key (WASD or the arrows) and a position moves — one
speed per *tick*, never per event and never per frame. That is the whole
point of a fixed timestep, and it is why key repeats are counted and
then ignored: acting on them would make movement depend on the
keyboard's repeat rate. Escape and the close button both end the run.
Everything the window seam delivers is echoed as it arrives.

Events arrive whenever the OS says so; the world advances only in fixed
steps. The two drivers differ only in where the events come from:

- **Windowed** (default): real input, the real clock, one clock read per
  iteration at the top of `update`.
- **Headless** (`--headless --input-trace NAME`): a built-in table of
  `(frame index, event)` pairs replayed against the same state machine,
  on a synthetic clock that reads no clock at all. No CI runner supplies
  keystrokes, so without this the binary could never be executed by a
  test — and an unexecuted binary is an untested one.

Traces: `walk` (keys, pointer, wheel, focus and resize, ending in a
close request at frame twenty) and `idle` (no input at all — the shape a
dedicated server or a determinism harness has).

## Command line

| Flag | Meaning |
|---|---|
| `--headless` | No window: replay a scripted trace on a synthetic clock. |
| `--input-trace NAME` | Which trace to replay (default `walk`). Refused outside `--headless`: a window is driven by the person at the keyboard. |
| `--frames N` | Frames of simulation to run (default 600, ten seconds at 60 Hz). Headless, one frame is exactly one step; windowed, the run ends once the simulation has advanced this many steps. A close request ends it sooner. |
| `--seed N` | Selects the movement speed. The seed axis is a placeholder until there is a random-number service; it feeds the world so the shape of the flag survives. |
| `--dump-stats PATH` | Write the JSON report there, after the run. |

The last line on stdout is always the digest line — the string the
cross-process determinism test compares. `--dump-stats` writes the
machine-readable document: the frame schedule, the state digest,
and what the input added up to.

There is **no `timing` section**, deliberately: this sample presents
nothing, and a drawn-versus-skipped split for a loop that never draws
would be a measurement of nothing.

Exit codes: `0` for a completed run or a skip, `1` for a failure, `2`
for a command line this sample cannot honour.

## Contract

- **`--headless` implies a synthetic time source.** The scripted driver
  reads no clock at all, so its digest line is a pure function of
  `(trace, frames, seed)` and is identical across runs, processes and
  machines.
- **Events change what the next step does, never the state.** The world
  is advanced by `step` and by nothing else.
- **The state hash is a fingerprint of behaviour, not of configuration.**
  The seed is deliberately not folded into it. A digest that absorbs an
  input cannot show that input had an effect — every seed would produce
  its own number even if the seed never reached the simulation. Leaving
  it out means two digests differ only when two worlds differ, which is
  what lets the seed matrix assert something real. The configuration is
  still printed: the seed appears beside the hash on the same line.
- **The simulation is integer-only.** Positions are whole units and
  pointer coordinates are truncated on arrival: a state hash that
  absorbed float arithmetic would quietly become a cross-platform
  promise the engine does not make.
- **No display server is a skip, not a failure.** The windowed run
  prints `SKIP:` and exits zero where no window can be opened; set
  `RENEW_FRAME_STRICT=1` where a skip would be a lie.
- **Windowing is not a feature flag here.** This *is* the windowing
  sample, so the seam is always compiled in — a feature that can never
  be turned off would be a lie in the manifest. Headless mode compiles
  the same event vocabulary and simply opens no window.

## Why it exists twice over

It is the complementary proof to `hello_triangle`: a *running*
frame-loop sample in a workspace with the GPU crate removed. Its
dependencies are `renew-frame` and `renew-platform`, and nothing else.

## Tests

`tests/cli_determinism.rs` runs the real binary three times and compares
the digest lines, checks that a different trace or seed moves them, and
reads back the stats file. It also runs a seed matrix: four seeds, three
processes each, where every seed must reproduce itself exactly and no two
seeds may hash alike. The second half is the one that catches a seed
which is read and then ignored — a failure identity alone cannot see.
`RENEW_DETERMINISM_RUNS` deepens the matrix without editing the file.

It needs no adapter and no compositor, so unlike its sibling it never
skips: on every machine and every lane, those assertions actually run.

The unit tests beside the source drive the window callbacks directly —
`event` and `update` with no window at all. Only `ready` needs one,
because it borrows a live OS window.
