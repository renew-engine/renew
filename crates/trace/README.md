# renew-trace

The input-trace codec: a line-oriented text format for recorded input, a
reader that refuses everything it does not understand, and a writer that
is its exact inverse.

A run of this engine is reproducible from a build, a seed and its input.
The build is pinned and the seed is a flag; a trace is what makes the
third one a file. With one, a bug can be reproduced from the input that
caused it, a play session can become a regression test, one input can be
diffed across two builds, and a sample can be driven in automation by
input no programmer wrote.

```rust
let text = renew_trace::write(&trace);   // a String, never a file
let same = renew_trace::parse(&text)?;   // a Trace, or a refusal naming a line
```

- `TraceEvent` — nine variants for the nine things a trace can say
  happened, plus `TraceKey` and `TraceButton` for the two closed name
  sets. Plain data: nothing here interprets an event, because what an
  event *means* belongs to whatever is replaying it.
- `FiniteF64` / `FiniteF32` — a float as its IEEE-754 bit pattern, with
  infinities and NaNs refused at construction. This is what makes the
  writer total.
- `TraceHeader` — the four fields the codec owns (`sample`, `ticks`,
  `timestep_ns`, `budget`), then caller-owned keys kept verbatim and in
  order.
- `Trace` — a header and the events, each with the tick it is delivered
  before, in the order they were recorded.
- `parse` / `write` — text in, text out, and nothing in between touches a
  filesystem.
- `TraceError` / `TraceErrorKind` — which line, and what was expected
  there, as a value to match on rather than a string to search.

## Contract

- **No input is trusted.** Every rule rejects rather than repairs, and
  nothing is skipped: an unknown keyword is an error, because skipping is
  how a format silently forks. Every refusal names its line and what was
  expected. Where the file is at a version this reader accepts, the
  message for an unknown word says the reader's table may be the
  incomplete thing rather than blaming the file.
- **No file is ever opened.** The reader takes text and the writer returns
  it. That is what makes the codec testable and fuzzable with no
  filesystem, and it puts the bound on how much untrusted data is held
  where it can be enforced — at the seam that reads, which can refuse an
  oversized file before a byte reaches a parser. The crate's own
  `clippy.toml` bans the filesystem calls, the file handles and the path
  types, so nobody arrives at a file by accident and anyone who does has
  to delete a line with a reason written on it. It is a tripwire across
  the roads people actually take, not a proof: a ban on `File::open`
  alone was exactly that mistake, since `OpenOptions` plus
  `Read::read_to_string` is the same unbounded read spelled in two lines.
- **Two obligations that are the caller's, because this crate cannot
  check them.** Invalid UTF-8 never arrives here, so the caller has to
  read with something that refuses it rather than something that replaces
  bad bytes — lossy repair before the parser sees the file is still lossy
  repair. And `timestep_ns=0` or `budget=0` parse quite happily, because
  the codec does not interpret the schedule it stores; whoever turns a
  header into a real schedule has to refuse a zero, and will have to,
  because the types that carry those two numbers cannot hold one.
- **Writing and reading are inverses.** `parse(write(t))` is `t`, for
  every trace that can be built — no exceptions and nothing lossy. The
  reverse holds for every file the writer could have produced: a
  hand-written file may spell a number with leading zeros, and writing it
  back out spells it canonically.
- **The codec interprets nothing it does not own.** It knows four header
  fields. `sample`, every caller key, and what the events *mean* are the
  caller's: it preserves them verbatim and checks nothing but uniqueness.
  The caller checks that `sample` names the thing it is about to run and
  applies its own keys. A codec that guessed what a sample name implies
  would be wrong differently for every caller.
- **Order is never repaired.** Ticks must not decrease, but equal ticks
  are allowed and their recorded order is part of the trace: two keys
  going down on one tick were seen in one order, and sorting or
  deduplicating them would change the input while looking like tidying.
- **Zero dependencies, no clock, no threads, no logging.** A refusal is
  returned, never printed. Nothing here panics and nothing unwinds.

## What a trace reproduces

The simulation: the state a run reaches, and the exact interleaving of
events with steps. **Not** how many frames carried those steps, how many
were dropped, or which driver supplied the input — those are facts about
the schedule that carried the input rather than about the input.

Recording the frame timeline as well was tried and abandoned on a
measurement: a sample that presents nothing free-runs at about 19,000
frames per simulation tick, so ten seconds of it is roughly 11.5 million
frame entries — some 81 MB of text in a repository, to reproduce a hash of
polling noise. Where a frame timeline is affordable it is also redundant,
because a headless run executes exactly one step per frame by
construction. If schedule reproduction is ever wanted, the format grows an
optional timeline section for frame-bounded drivers; it is not needed now.

## Events are indexed by tick, not by frame

A frame may run no steps or several, so *the event on frame 40* is not a
point in simulation time. The definition is exact:

> Tick *k* means the event is delivered before the step whose tick is *k*.
> Ticks are 0-based. A tick equal to the header's `ticks` is legal and
> means **after the final step**.

That last case is the common one, not an edge case. With thousands of
frames per tick, a terminating event almost always arrives during a frame
that runs no step at all, so its recorded tick is the run's own tick
count. A rule of *tick below ticks* would make a recorder emit files its
own reader refuses, in the normal case.

This rests on a property the simulation must have: its state may depend
only on the tick index and the events delivered before that tick — never
on frame boundaries, on the accumulator's remainder, or on whether a step
was dropped. A simulation that reacted to a stall would not be
reproducible from a tick index by anything.

## The grammar

```text
renew-trace <version> sample=<name> ticks=<u64> timestep_ns=<u64> budget=<u32> [key=value…]
e <tick> key <name> <down|up> [repeat]
e <tick> pointer <hex-f64> <hex-f64>
e <tick> button <name|other:<u16>> <down|up>
e <tick> wheel <hex-f32> <hex-f32>
e <tick> focus <in|out>
e <tick> resize <u32> <u32>
e <tick> scale <hex-f64>
e <tick> redraw
e <tick> close
```

The version is positional and first, because a reader has to know how to
read the rest of a line before it reads it. A reader accepts its own
version and every older one.

A new **caller-owned key** does not move the version: those keys were
never the codec's to interpret, so a reader that has never seen one keeps
it verbatim and reads the file. Everything else in the vocabulary does
move it — a new event kind, a new key name, a new button name. Each of
those is a word an existing reader does not know, and this format refuses
words it does not know rather than skipping them, so adding one makes
every reader already in the world reject the whole file. That is a new
format however small the addition looked.

Fields are separated by exactly one space. A header field is split at its
**first** `=`, so a value may carry as many more as it likes
(`extent=640=480` is the value `640=480`) while a *key* may carry none —
a key with one would be read back shorter than it was written, which is
silent corruption, and a key of `ticks=9` would walk a reserved name past
the uniqueness check. The positional `sample` is a value in this sense
and may contain `=` freely. Numbers are ASCII digits with
no sign and no underscores: *whatever the standard parser accepts* is not
a specification for a byte-exact format, and what it accepts is free to
grow. Bit patterns are `0x` and exactly the width of their type, in
lowercase. A trailing carriage return is stripped, because these are text
files in a repository whose builds run on Windows and a line ending is not
a change to what a line says. A byte order mark is an error that names
itself, because it is invisible on screen and every other message would
send the reader looking at the wrong thing.

`unidentified` is a first-class key name, not a refusal: it is what the
windowing seam produces for every physical key outside the mapped set, so
treating it as unencodable would abort a recording the first time someone
pressed Shift — during exactly the session a recording exists to capture.
What is unrecoverable is *which* physical key it was; the event itself
records and replays fine.

## Why text, and why floats are integers

Diffable in version control, hand-writable in a test, readable in a build
log, parseable with no dependency. Binary wins only on compactness, which
— with no frame timeline in the file — is the difference between a small
file and a slightly smaller one.

Every field is an integer or a keyword. A float value has two zeros and no
equality for `NaN`, and decimal text does not survive a round trip through
a parser without care, so floats are written as bit patterns: the parser
never parses a decimal float, values round-trip exactly, and the two zeros
stay distinguishable. Non-finite patterns cannot be constructed and are
refused on read.

## Testing note

The recorded fuzz corpus is replayed as a merge gate: every committed
input under `fuzz/corpus/trace_parse/` that parses as UTF-8 feeds
`parse` in a stable test that fails loudly if the corpus is missing or
below its committed floor. A crash input, when one ever exists,
additionally becomes a permanent named regression test beside this
suite, independent of the corpus.

Unit tests cover the vocabulary, the header, the tick rules and every line
shape in both directions. `tests/rejects.rs` gives every distinct
malformed input its own test, asserting one refusal and one line number
each — a single test claiming *all of these are rejected* would still pass
if all but one of them started being rejected for the wrong reason.
`tests/golden.rs` holds one trace as a literal string that no code
produced, asserted byte for byte in both directions; it is the anchor
under the round-trip property in `tests/properties.rs`, which on its own
proves less than it looks like it proves, because a writer and a reader
that made the *same* mistake are still exact inverses of each other. The
property suite also hands the reader generated garbage and generated
near-traces and requires an answer rather than a crash, with a fixed
generator seed so the same inputs are explored on every machine.

One caution is worth writing down as a rule, not an anecdote, because it
is the failure mode this kind of suite has:

> **An input set that contains no counterexample proves nothing, however
> much of the code it runs.** Coverage says every line executed. It does
> not say any input could have told correct behaviour from incorrect.

It has already been true here twice, in two different rules. The
generator once drew header text from an alphabet with no `=` in it, and a
reader deliberately broken to split a header field at its *last* `=`
rather than its first — inverting the one thing the format states about
that character — passed every test. Separately, every negative word in
the refusal tests was a stranger to the vocabulary (`gamepad`, `thumb`,
`meta`), so a reader that accepted any word merely *beginning* with a
legal one passed too: `event 0 close` read as a close, and `renew-tracex`
read as this format's own identity line.

Both are fixed the same way — by choosing inputs a wrong implementation
would get wrong. Values are generated with `=` in them, every keyword
table is probed with a near miss in both directions as well as with a
stranger, and the rules that a generator would have to be lucky to hit
have fixed hand-written cases of their own. When adding a rule here, the
question to ask is not "is this line covered" but "what would still be
green if this rule did nothing".

Measured with `cargo llvm-cov`, line, region and function coverage are all
100% — which for a reader means every refusal edge is *taken* by a test,
not merely stepped over on the way to a success.

Deliberately not applicable, each for one reason: thread-sanitizer and
stress testing (nothing here spawns a thread or is shared across one),
and Miri (no `unsafe` anywhere in the crate).

## Status

Early-stage. The `[package.metadata.renew]` table in
[Cargo.toml](Cargo.toml) is authoritative for maturity and all manifest
metadata. `extension_points = []` is honest: no trait, no `dyn`, no
runtime polymorphism.

The contract lints live in [clippy.toml](clippy.toml): filesystem calls,
file handles, path types, clock reads, thread spawning, environment reads
and randomly seeded hash containers are all rejected at lint time,
because *the codec does no I/O* is a property the whole design rests on
and one `fs::read` added for convenience would move an untrusted-input
bound into a crate with no way to enforce it. Every entry was checked by
writing the call and watching the lint fire.

## Key decisions

- **The event types carry a `Trace` prefix on purpose, against the usual
  advice.** House style says an item should not repeat its crate's name:
  `renew_trace::Event`, not `renew_trace::TraceEvent`. The exception is
  taken deliberately and for one concrete reason — **the only code that
  imports these types imports the engine's event vocabulary beside
  them**, to translate between the two:

  ```rust
  use renew_event::{KeyCode, PointerButton, WindowEvent};
  use renew_trace::{TraceButton, TraceEvent, TraceKey};
  ```

  **`renew-event`, not the platform crate.** Three paths resolve to the
  same items — the vocabulary crate directly, the platform crate's
  `event` re-export, or its `window` module — and the first is the one to
  use. It is a crate with no dependencies, so a consumer takes the
  vocabulary and nothing else; the other two hand it a crate that owns a
  clock, a filesystem and thread spawning as well.
  That is not a hypothetical concern: a replay harness reading traces has
  no reason to link a windowing stack *or* to acquire the ability to read
  the wall clock, and this crate depends on nothing precisely so it never
  forces either.

  `WindowEvent` and `TraceEvent` name two vocabularies at a seam whose
  whole job is converting one into the other. `WindowEvent` and a bare
  `Event` would read as though one were the general case of the other,
  which is exactly the confusion this crate's independence exists to
  avoid. `Trace` itself keeps the short name, since a type sharing its
  module's name reads cleanly; the prefix is only on its companions.
  `TraceError` is a separate matter — error types are named for their
  domain everywhere in this workspace.

- **The vocabulary is this crate's own, not the windowing layer's.** That
  is what lets the crate depend on nothing. A codec naming the windowing
  crate's event enum would pull an entire windowing stack into every build
  that merely reads a file, headless ones included — and it would make the
  meaning of an already-written file hostage to that enum's growth, since
  a variant added upstream would change what an existing trace means. The
  conversion between the two vocabularies lives in the application that
  owns both.
- **The name sets are closed to extension.** A downstream conversion
  should stop compiling the day a key is added, because that is the only
  moment anyone can still decide what the new key is called in a file.
- **Finiteness is a type, not a check.** `FiniteF64` and `FiniteF32`
  refuse infinities and NaNs at construction, which is what leaves the
  writer with no failure case to report and makes the inverse property
  total rather than conditional.
- **One error type, one line-numbering scheme.** A trace is numbered by
  its text lines in both directions: the header is line 1, and because
  every line after it is exactly one event, the line a constructor
  computes for an out-of-order tick is the line the file actually has.
- **The tick rules live in `Trace::new`, not in the parser.** A trace
  built in memory and a trace read from a file are held to one
  implementation of them, so a recorder cannot produce something its own
  reader would refuse.
- **A header carries no version field.** There is exactly one format
  version, so storing a number that can only hold one value would be
  machinery pretending to be a decision. When a reader accepts an older
  version as well as its own, whether rewriting preserves the older claim
  becomes a real question, and it should be answered by a visible addition
  rather than by a field that quietly upgraded every file it touched.

## Known gaps

- **No fuzz target.** The property suite hands the reader generated
  garbage with a fixed budget, which is a small fuzzer and not a
  substitute for one. A real target needs tooling this tree does not have
  yet, and one is required before this crate could be considered stable —
  which is among the reasons it has not been promoted.
- **Nothing here records or replays.** This crate is the codec: the
  recorder that produces a trace from a live session, the driver that
  feeds one back into a run, and the command-line face of both are
  separate work that depends on this.
- **No compression and no motion decimation.** Files are tens of lines.
  A pointer moving every tick for a long run would change that, and the
  answer then is a format decision, not a smarter writer.
