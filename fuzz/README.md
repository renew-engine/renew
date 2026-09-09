# renew-fuzz

Fuzz harnesses for the sixteen parsers that read data the engine did
not write: the asset pack reader, the input-trace codec, the WAV reader,
the UI document reader, the UI text grammar, the datagram reader, the
PNG decoder, the JSON reader, the three mesh readers STL, PLY and OBJ,
the MTL material libraries an OBJ refers to, the binary glTF container
— which parses nothing and validates only framing — the accessor layer
that turns a range of bytes into typed numbers, the binary glTF reader
whole, and the canonical mesh blob
— which is the one *mesh* format in this list the engine writes as
well as reads, and so the one whose target can assert that reading and
writing are inverses. Nine of them go further than the rest: bytes that read as a UI document are also
instantiated as a tree, because validation claims instantiation never
needs to re-check; text that compiles is read back through the
runtime reader, because the compiler claims it only mints what that
reader accepts; a JSON document that parses is walked, with every
typed question asked of every value in it, because validation claims an
accepted document has nothing deferred left in it; and a mesh that reads
is checked for whole triangles, finite coordinates and a normal array
matching what its format can carry, because those are what the returned
type claims of itself. An input that breaks any of those claims is a
finding.

**Fourteen of the sixteen take their bytes unfiltered; two do not, and
the dividing line is what the parser's own signature accepts.**
`trace_parse` and `ui_text` guard their input with `from_utf8` and return
early on failure, because both of those parsers take `&str` — a crash
found past a lossy conversion would be unreachable from any real file.
The other fourteen take `&[u8]`, so there is nothing for them to guard. The
JSON reader in particular takes bytes on purpose, so
that a caller holding one chunk of a larger file need not answer the "is
this even text" question itself — which means the fuzzer is the thing
that has to ask it.

## Why this is a separate workspace

A libFuzzer target links a runtime needing the nightly toolchain and
sanitizer instrumentation. As a workspace member it would sit in the path
of `cargo build --workspace`, which runs on stable and gates every merge —
so the harness would break the build it exists to protect. The root
manifest excludes this directory by name.

The consequence worth knowing: the root licence check never resolves these
dependencies, so the scheduled job runs that check here instead.

## What these add over the existing suites

**Two of these parsers already assert the property their own target
tests, in their stable-workspace suites.** The pack
reader's suite says it "answers, one way or the other, for every byte
string it can be handed"; the codec has the same over arbitrary text, plus
a mutation property shaped like a real file.

**Those run a few hundred cases from a fixed seed. These are coverage
guided** — inputs reaching new branches are kept and mutated, so the
fuzzer works past the magic number and the header into the entry table,
where uniform random bytes essentially never land. That is the difference:
not a first line of defence, a deeper one.

## The committed corpus

`corpus/<target>/` is the fuzzers' memory: most committed inputs are ones
the coverage-guided search found worth keeping, minimized by
`cargo fuzz cmin`. **Ten corpora start differently** — `png_decode`'s
seeds are written by `cargo run -p renew-png --example make_png_corpus`,
`json_parse`'s by `cargo run -p renew-json --example make_json_corpus`,
and the eight `renew-mesh` corpora by `cargo run -p renew-mesh --example
make_stl_corpus`, `--example make_ply_corpus`, `--example
make_obj_corpus`, `--example make_mtl_corpus`, `--example
make_blob_corpus`, `--example make_glb_corpus`, `--example
make_accessor_corpus` and `--example make_gltf_corpus`, each of
which builds every byte it writes rather than copying a file from
anywhere, so no licence question arrives with the starting seeds.

**One target refuses a seed it could have had, and the reason is a
stall.** `gltf_read` carries no document whose node hierarchy contains a
cycle. The reader refuses one by entering each node at most once, but if
that guard were removed a cycle seed would make the fuzzer **and the
merge-time replay gate** stop making progress rather than fail — and a
stall is the one outcome neither can report. Probing the mutation
locally did exactly that: one test failed, the next stopped, and a
timeout ended the run. The cycle is pinned by a deterministic test
beside the crate, where a wedged run is a failed test.

**One target's input is not a file, and it is the only one that needed
an encoding invented for it.** `accessor_view` fuzzes a byte region plus
six parameters; a harness that could only vary the region would never
reach a single refusal about the parameters, so a nine-byte head carries
them. **That encoding is a fact three targets in two crates have to
agree on** — the fuzz target, the corpus generator and the merge-time
replay gate — so it lives in exactly one file,
`crates/mesh/tests/shared/accessor_seed.rs`, included by all three with
`#[path]`. A round-trip test in the replay gate holds its two halves
together, because `encode` has one caller and `decode` has three, and
nothing else in the tree would notice them drifting apart.

**Every one of those names carries its format, and that is load-bearing
rather than tidy.** Cargo writes an example to
`target/<profile>/examples/<name>` from the target's own name, ignoring
which package declared it, so three packages naming one example named one
file. Building the workspace then had two link steps writing one path;
on Windows the loser reported `LNK1104: cannot open file
'make_corpus.exe'` and the run went red on a tree that was fine. Cargo
warns about the collision and says it may become a hard error. A new
generator gets the format in its name. For
the mesh generators that is not only licensing: a mesh is the one format
here whose sample files are *art*, and art has an author.

**What is *not* gated is which of those seeds survive, and that is
deliberate.** None of the generators runs in CI, and no test compares the
committed directory against what a generator would produce today —
because `cargo fuzz cmin` renames what it keeps to a content hash and
drops what adds no coverage, so a check that demanded seed-for-seed
identity would forbid the minimisation the procedure requires. **The
property the merge gate holds instead is about reach, not identity:**
`corpus_replay.rs` beside each parser asserts a floor on the number of
*distinct* answers the surviving corpus provokes and names the individual
refusals that must stay reachable. A corpus can be minimised freely; it
cannot quietly stop exercising a guard. Running a generator after
changing it is a manual step, and nothing here will notice if it is
skipped — the reach floor is what would eventually catch the
consequence. Once a target's
corpus exists the rule below is the same for all of them: the fuzzer adds
its own finds, and nothing here deletes them. Runs start from it (locally
and on the schedule),
and the scheduled job uploads the grown corpus as an artifact —
**re-commits are manual and event-driven, and every one carries the
same two steps as the first commit**: `cargo fuzz cmin <target>
corpus/<target>` to minimize, then a vocabulary sweep over the bytes
before staging — mutation inserts arbitrary bytes, so grown inputs are
treated as untrusted text until swept, every time, not just once. The stable workspace replays every committed input in a
merge-gating test beside each parser, so "zero known crashes over the
recorded corpus" is a claim a gating run makes. A crash input, when
one ever exists, additionally becomes a permanent named regression
test beside the parser it broke — independent of the corpus.

## Running one

```
cargo fuzz run asset_pack
cargo fuzz run trace_parse -- -max_total_time=60
```

Nightly is required. `cargo fuzz list` names the targets.

## What a finding looks like

A crash writes the offending input to `artifacts/<target>/`. That file is
the bug report: it reproduces with

```
cargo fuzz run <target> artifacts/<target>/<file>
```

Keep it. A crash without its input is an anecdote, which is why the
scheduled job uploads the directory when a run fails.

## What these targets deliberately do not assert

**That an accepted input is *meaningful*.** Twelve of the sixteen do assert
something past "it answered" — the datagram re-encodes to its own bytes,
a UI document instantiates, compiled text reads back, a JSON document
answers every typed question, a mesh that reads has whole triangles,
finite coordinates and a normal count its format could have written, and
a glTF container that reads hands back slices *of the caller's own
buffer* whose lengths and headers fit inside what went in — but
each of those is a claim the parser itself makes about what it accepts,
checked only on inputs it accepted.

The container's is the odd one and worth the sentence. It asserts by
**address rather than by content**, because content equality would pass
for a copy and not copying is the whole promise: a container that read
but handed back a slice reaching past what it validated would look
exactly like one that read.
**None of them assert anything about a rejected input, and none assert
that a decoded value is the right value.** Whether the answer is correct
belongs to the round-trip properties beside each parser: a fuzz target
that asserted on content would fail on inputs that are legitimately
malformed, and the corpus would fill with noise.
