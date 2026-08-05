# renew-sample-cube

The three-dimensional sample: a voxel world with a walking, jumping,
block-breaking player in a closed arena, run headless from a named
script and answered with one digest line. `--show` draws the world
it left behind as two slices of text.

## Running it

```
cube                                    # the stand script, 600 ticks
cube --script patrol --ticks 2000       # walk a loop around the arena
cube --script build --ticks 900         # walk, jump, dig and place
cube --script stand --ticks 100 --show  # draw what it left behind
cube --script build --ticks 900 --json  # the same answer, for a machine
```

From a fresh clone the invocations above are spelled
`cargo run --bin renew -- run cube -- <arguments>`, or
`cargo run -p renew-sample-cube --bin cube -- <arguments>` to skip
the wrapper. `--help` lists every flag the parser accepts, and a test
holds it to that: a flag a reader cannot discover is a flag that does
not exist as far as anyone but its author is concerned.

Every run prints one line last:

```console
$ cube --script build --ticks 900
cube script=build ticks=900 digest=0xc2b4534b2679a6a1 solids=5020 broken=2 placed=10 grounded=false
```

Two runs of the same script and tick count print identical lines —
in one process and in separate ones. That is what the binary is for.
The world is a pure function and its own tests assert its digest,
which proves it reproduces on **one** machine; the obligation is
across machines, and comparing this line is how that gets checked.
The digest covers everything that can change future behaviour, the
terrain included: a world whose blocks differ plays differently from
the next tick on.

`--json` prints the same report as one document instead, carrying a
`schema_version` from its first release so a consumer can tell a shape
it understands from one it does not:

```console
$ cube --script build --ticks 900 --json
{"schema_version":1,"sample":"cube","script":"build","ticks":900,"digest":"0xc2b4534b2679a6a1","solids":5020,"broken":2,"placed":10,"grounded":false}
```

`--show` draws a plan and an elevation, each sliced through the cell
the player is standing in and labelled with where it cut:

```console
$ cube --script stand --ticks 100 --show
elevation, looking along z, at z=0
  11 #########################################
  10 #.......................................#
   9 #.......................................#
   8 #.......................................#
   7 #.......................................#
   6 #.......................................#
   5 #.......................................#
   4 #.......................................#
   3 #.......................................#
   2 #.....................#####.............#
   1 #...................@.#####.............#
   0 #########################################
     x from -20, y up the side
cube script=stand ticks=100 digest=0x1d07e6e9840ed836 solids=5012 broken=0 placed=0 grounded=true
```

Both views are the whole grid rather than a window onto it, so two
runs can be read column for column. `@` is the player, `#` a solid
block, `.` air; height increases upward in the elevation, because a
picture drawn the other way is still a correct slice and an
unreadable one. The pictures come before the summary line: a reader
scanning a terminal wants the summary nearest the prompt.

Two limits worth stating. `--show` draws nothing under `--json` — the
machine-readable output is one document and a picture is not part of
it — and the flag is accepted rather than refused there. And the
drawing is text: there is no renderer in this sample's dependency
graph at all.

## The arena

Forty-one cells across, twelve high and forty-one deep — x and z from
−20 to 20, y from 0 to 11 — and it is a **closed box, solid on every
face**.

That is not decoration. Outside the grid is neither solid nor air:
the grid answers "I do not know" past its edge rather than guessing,
so a player who leaves it falls forever with nothing to land on. The
shell refuses to be turned back into air, and the boundary it refuses
is the boundary in all three axes, so there is no direction left to
leave by. Weaker enclosures were tried first and are recorded in
[`world/README.md`](world/README.md) beside the rule itself.

Drawing the world is what exposed the problem. The first picture of it
showed the player twenty-five blocks below a floor at y = 0 after a
four-hundred-tick run — the building script had dug through the floor
and spent most of its run outside the world, and its digest described
a fall. Every test passed, because they all asked whether the run
reproduced rather than whether it made sense. `tests/view.rs` now runs
every script at six lengths and asserts the player is still inside the
grid, which is the assertion that was missing.

Closing the box also made the floor unbreakable, which quietly turned
the digging script into a script that reaches for the shell and is
refused. So the arena has a mound — x 2 to 6, y 1 to 2, z −2 to 2 —
sitting in front of the start along the direction the player looks,
and a test keeps it there.

The player's look direction is set once, down and forward, and never
changes: what a script digs at is whatever its walking puts in front
of it. That is why `build` edits less than its button presses suggest.
Over nine hundred ticks it breaks two blocks and places ten, all
within the first four hundred and fifty; after that it has walked into
a corner where the only thing in reach is shell. It also ends most
runs with `grounded=false`, which is a jump in progress rather than a
fall — a jump clears about five blocks, and the script presses jump
every seventeenth tick.

## Shape

Two crates. `world/` is the simulation: a fixed-step pure function of
terrain and per-tick intent, with no clock, no file and no randomness
— see [`world/README.md`](world/README.md). This crate is the driver.
It builds the arena, owns the scripts, runs the loop, and owns
everything printed.

The scripts are built in and named rather than read from a file,
because the point of the binary is to be run identically on three
platforms and compared, and a file is one more thing that can differ
between them.

Everything the binary does — parsing, running, drawing, choosing the
exit code — lives in the library, so it can be driven by a test
without spawning a process; the parts most worth testing are the
refusals, and a spawned process is what makes those hardest to
inspect. `tests/binary.rs` spawns one anyway, because the process
shell, the argument plumbing and the exit codes are only exercised as
a caller meets them.

Exit codes: `0` for a completed run and for `--help`, `1` for a
command line this sample cannot honour. A refusal goes to the error
stream, prints nothing on the output stream, and names the thing at
fault — an unknown script also lists the real ones.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies
and extension points.
