# renew-sample-glide

A small complete game, headless-first: the glide world driven by
scripted traces or a recorded replay — and, behind the `window`
feature, playable in a window with the score in the title.

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

The library also exposes `world_at(trace, tick)` — the same replay
loop the runs use, promoted so image oracles need not copy it — and a
pure `scene` module that turns a world into draw-order rectangles with
no GPU crate in the normal graph. The frame-capture test consumes
both: committed traces replayed to a checkpoint, drawn through the
sprite renderer offscreen, compared against committed images. The GPU
crates enter the normal graph only behind the `window` feature (and
the dev graph for the oracle); the default build carries none of
them, and the build matrix proves it with a build-only probe.

`glide --window` opens the game (`--features window` builds it):
space or the primary mouse button flaps, the score lives in the
title, and the digest line prints at close marked `source=window` so
nothing ever compares a wall-clock run against a scripted one. Flap
edges ride a saturating counter consumed one per fixed step — a press
on a frame that plans no steps survives to the next, and two presses
with two due steps deliver two flaps.

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
