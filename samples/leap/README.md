# renew-sample-leap

The platformer: one character that runs, jumps, lands and slides
along a level of solid boxes, run headless from a named script and
answered with one digest line. `--show` draws the level and the
character as the box the simulation actually collides with.

## Running it

```
leap                                    # the stand script, 600 ticks
leap --script dash --ticks 120          # run right until the wall stops it
leap --script hop --ticks 900           # jump, land, jump again
leap --script dash --ticks 120 --show   # draw where it ended up
leap --script hop --ticks 900 --json    # the same answer, for a machine
```

From a fresh clone those are spelled
`cargo run --bin renew -- run leap -- <arguments>`, or
`cargo run -p renew-sample-leap --bin leap -- <arguments>` to skip
the wrapper. Everything after the bare `--` is this binary's own
command line, and `--help` lists every flag it accepts.

Every run prints one line last:

```console
$ leap --script dash --ticks 120
leap script=dash ticks=120 digest=0xc75411dac5b49142 grounded=true wall=true
```

Two runs of the same script and tick count print identical lines — in
one process and in separate ones. That is what the binary is for. The
world is a pure function and its own tests assert its digest, which
proves it reproduces on **one** machine; the obligation is across
machines, and comparing this line is how that gets checked. The line
above is also what a release build prints: every quantity in the
simulation is fixed-point, so there is no floating-point rounding left
to differ between profiles.

The digest covers the character's position, its velocity, its footing
and the jump latch — the latch because two characters standing in the
same place at the same speed still diverge on the next tick if one is
holding the button and the other is not. It does not cover the level,
which no script can change.

`--json` prints the same report as one document instead, carrying a
`schema_version` from its first release so a consumer can tell a shape
it understands from one it does not:

```console
$ leap --script hop --ticks 900 --json
{"schema_version":1,"sample":"leap","script":"hop","ticks":900,"digest":"0xd292bcd20d1b62a1","grounded":false,"against_wall":false}
```

## The flags

| Flag | What it does |
|---|---|
| `--script NAME` | Which built-in script drives the character: `stand`, `dash` or `hop`. Default `stand`. |
| `--ticks N` | How many ticks to run. Default 600. Zero is allowed, and answers without stepping the world at all. |
| `--show` | Draw the level around the character before printing the line. |
| `--json` | Answer with one line of JSON rather than a sentence. |
| `--help`, `-h` | Print the usage text and stop, successfully — nothing runs. |

Naming a flag twice is not an error; the last one wins, so
`--script stand --script hop` runs `hop`. `--ticks` takes a whole
number of ticks and nothing else, so `--ticks -5` is refused as not a
number rather than quietly clamped to zero.

## The picture

`--show` draws the level around the character — `#` solid, `@` the
character, `.` air — with the world's y down the left edge and the
cell the character is standing in named on the last line:

```console
$ leap --script dash --ticks 120 --show
  10 .............................................................
   9 .............................................................
   8 ...............................##............................
   7 ...............................##............................
   6 ...............................##............................
   5 ...............................##............................
   4 .....######....................##............................
   3 .....######..................@@##............................
   2 .............................@@##............................
   1 .............................@@##............................
   0 #############################################################
  -1 #############################################################
  -2 .............................................................
     x=8 y=2
leap script=dash ticks=120 digest=0xc75411dac5b49142 grounded=true wall=true
```

The character is drawn **as the box the simulation collides with** —
one unit wide and two tall, taken from the half-extents the world
crate exports for exactly this — rather than as a single mark. A
one-cell mark would be a picture of something that is not in the
simulation, and it would sit happily beside a wrong answer as well as
a right one. Drawn at its real size, the picture can be held against
the line beneath it: a character the line calls `grounded` has
something solid under its feet, and one it calls `wall=true` has
something solid beside it, which is what `@@##` above means.
`tests/view.rs` asserts both, and asserts them without recomputing the
overlap arithmetic the drawing uses — a test that re-derives the
implementation's own reasoning agrees with it wherever it is wrong.

The picture comes before the summary line, because a reader scanning a
terminal wants the summary nearest the prompt.

Four honest limits on the drawing.

- **It is quantised to whole cells**, and a body stopped by a slide
  rests a skin distance short of what stopped it rather than exactly
  on it. So a two-unit-tall character standing on the floor covers
  three rows, as above, and two positions inside the same cell draw
  the same picture.
- **The view follows the character** — sixty-one cells by seventeen —
  rather than framing the level, because a level may be larger than a
  terminal and a character that walks off the drawing tells the reader
  nothing. The consequence is that two pictures from different ticks
  do not line up by eye; the coordinates on the left edge and the last
  line are what makes them comparable, so they are printed.
- **`--show` draws nothing under `--json`.** The machine-readable
  output is one document and a picture is not part of it; the flag is
  accepted there rather than refused.
- **It is text.** There is no renderer in this sample's dependency
  graph at all.

## The level

Three solid boxes, the same for every script:

- a floor centred on the origin, spanning x from −40 to 40, with its
  top surface at y = 1;
- a wall from x = 9 to x = 11, rising from the floor to y = 9;
- a ledge from x = −17 to x = −11, between y = 3 and y = 5.

The character starts in mid-air at x = 0, y = 6, and falls onto the
floor. The floor is finite and there is nothing beneath it, so a
character that walked off its end would fall with nothing to land on
— none of the three scripts gets near it, because each is stopped by
the wall or the ledge long before x = ±40.

## The scripts

They are built in and named rather than read from a file, because the
point of the binary is to be run identically on three platforms and
compared, and a file is one more thing that can differ between them.

**`stand`** asks for nothing at all. The character falls from the
start, lands, and stays: `grounded=true wall=false`. It is the run
where anything moving is a defect.

**`dash`** runs right for 120 ticks, then left for 120, and repeats —
**and this is the shape the sample exists to test.** It reaches the
wall on tick 43 and spends the remaining 77 ticks of that leg pressed
against it, still asking to move right the whole time. That is the
case a collision routine gets wrong in interesting ways: keep the
horizontal speed and the character tunnels into the wall or jitters
along it; spend the vertical speed along with it and the character
sticks to the wall instead of standing on the floor. For all 77 ticks
the line says `grounded=true wall=true`, and the picture above shows
why — the character is beside the wall, not inside it.

Running back the other way it does not pass under the ledge: the
ledge's underside is at y = 3 and a character standing on the floor
reaches exactly that high, so it stops with its left edge against the
ledge's right face, prints `######@@` in the picture, and presses
there — `wall=true` again — for the rest of that leg.

**`hop`** runs right for ten ticks, jumps, runs left for ten, jumps
again, once every twenty-four ticks. A jump is worth about seven units
of height and takes roughly fifty ticks to come back down, so most of
those presses arrive in mid-air and do nothing at all: a jump fires on
the tick the button goes down, and only while the character is
grounded or inside its coyote window. That is why most `hop` runs end
`grounded=false` — a jump in progress rather than a fall. Ten ticks
each way also cancel out, so `hop` stays within a couple of units of
where it started and never touches the wall.

## What the line does not print

The world knows more about the character's footing than the report
carries. How many ticks it has been airborne — the thing coyote time
is made of — reaches the digest but not the printed line. Whether a
move ran out of slide iterations with displacement still unspent
reaches neither, and a caller who wants that distinction calls the
library rather than the binary. [`world/README.md`](world/README.md)
explains what those two facts are and why they are kept apart.

## Shape

Two crates. [`world/`](world/README.md) is the simulation: a pure
fixed-step function of per-tick intent, standing on the
two-dimensional collision crate, with no clock, no file and no
randomness at any dependency depth. This crate is the driver — it
owns the level, the scripts, the loop, the drawing, and everything
printed.

Everything the binary does — parsing, running, drawing, choosing the
exit code — lives in the library, so a test can drive it without
spawning a process; the parts most worth testing are the refusals, and
a spawned process is what makes those hardest to inspect.
`tests/binary.rs` spawns one anyway, because the process shell, the
argument plumbing and the exit codes are only exercised as a caller
meets them.

Exit codes: `0` for a completed run and for `--help`, `1` for a
command line this sample cannot honour. A refusal goes to the error
stream, prints nothing on the output stream, and names the thing at
fault — an unknown script also lists the real ones:

```console
$ leap --script fly
leap: no script called `fly`; try stand, dash, hop
```

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies
and extension points.
