# renew-sample-glide

A small complete game, headless-first: the glide world driven by
scripted traces or a recorded replay, with a windowed mode arriving
later behind a feature.

## Running it

```
glide                                  # seed 7, the soar trace, 2000 frames
glide --seed 3 --frames 600            # shorter, different world
glide --input-trace sink               # no input; gravity wins
glide --record-trace out.trace         # record the run's input
glide --replay-trace out.trace         # the recording owns the run
```

Every run prints one digest line last; two runs with the same seed,
trace and length print bit-identical lines, cross-process, and the
test suite holds that.

`--replay-trace` owns the whole run — the header carries the seed and
the length — so the flags it would contradict are refused by name
rather than silently ignored.

## Shape

Two crates. `world/` is the simulation — a pure fixed-step function of
seed and per-tick input, forbidden by the workspace structure rules
from reaching a clock, a file or a window at any dependency depth.
This crate is the driver: it maps input to the world's one action
through the same bindings a windowed mode will use, runs the loop on a
synthetic clock, and owns every file that is read or written.

Traces index events by tick from zero, exactly as the loader returns
them; there is no frame-numbering shift anywhere in this sample.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies
and extension points.
