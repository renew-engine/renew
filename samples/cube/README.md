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

## Meshing the world

`mesh::faces` turns the grid into the block faces a renderer would draw:
one quad per solid cell face whose neighbour is air. It is pure, it holds
no renderer, and it runs on a machine with no GPU -- the same position
the platformer's `scene` module occupies, for the same reason.

**The rule is "the neighbour is air *inside* the grid", not "the neighbour
is not solid".** Those sound identical and differ by more than half the
mesh.

`Grid::is_solid` answers `false` for cells outside the grid, deliberately:
a player who walks out of the world should fall, so the void is not
something to stand on. But a mesher written against that answer emits a
face wherever the neighbour is not solid -- including every cell of the
surrounding void -- so it also emits the box's entire **outer skin**. The
faces are all behind the walls, so the picture from inside looks perfectly
correct while the mesh is twice the size it should be, and the pipeline
culls nothing, so those backfaces rasterize rather than being free.

`Grid::get` answers with three cases where `is_solid` answers with two,
and the third is the one that matters: `None` means outside. Meshing
against `get` makes the distinction structural instead of something a
reader has to remember.

**The budget, computed before the mesher was written:**

| | |
|---|---|
| Solid cells | 41x12x41 shell minus its 39x10x39 interior, plus a 5x2x5 mound = **5012** |
| Cavity skin | 2(39x39) + 4(39x10) = **4602** |
| Mound | +65 exposed, -25 floor faces it covers |
| **Visible faces** | **4642** |
| Outer skin, if the rule is wrong | **5330** more, for **9972** total |

Both numbers are asserted. Swapping the emission rule to the naive one
makes the arena mesh to exactly 9972, which is how the test knows it is
testing something: the figures were derived from the arena's dimensions
first, so a mesher that agreed with itself rather than with the arithmetic
would fail.

Faces are shaded by direction -- brightest up, dimmest down, the two
horizontal axes distinguished. Nothing lights the scene in v0, so a world
drawn in one colour per block type reads as a single silhouette with an
outline; varying the colour by which way a face points is the cheapest
thing that makes an edge visible, and it is free at runtime because the
colour is baked into the vertex. A block type with no colour of its own
comes out magenta rather than a plausible grey.

Corners are wound counter-clockwise seen from outside, on every face, and
a test says so -- even though the pipeline culls nothing today and a
reversed quad draws identically. That is exactly why: the mistake is
invisible until culling is switched on, and then half the world disappears
with no recent change to blame.

## Writing the picture out

`png::encode` turns RGBA bytes into a PNG, in about a hundred lines and
with no dependency. Encoding one turns out not to need a compressor:
the data is a zlib stream, and a zlib stream may be made of deflate
**stored** blocks -- bytes copied verbatim behind a five-byte header. So
the encoder is four chunks, two checksums and some framing.

Stored blocks cost five bytes per 65535, about 0.008% over the raw
pixels. A real compressor would make the file much *smaller* than raw,
and for flat-coloured faces the difference would be large -- this trades
size for having no dependency and a byte layout a reader can check
against the specification in one sitting.

**How it was checked.** The tests assert the layout against the format,
including a hand-derived single-pixel file and the block split that only
appears past 65535 bytes. That catches a typo but would not catch a
specification consistently misread, since the same reading wrote the
encoder and the test -- so the output was also handed to an independent
decoder at five sizes, and each opened as RGBA at the right dimensions
with pixels round-tripping exactly.

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
