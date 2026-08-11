# renew-fuzz

Fuzz harnesses for the five parsers that read data the engine did not
write: the asset pack reader, the input-trace codec, the WAV reader, the
UI document reader, and the UI text grammar. The two UI targets go
further than the first three: bytes that read as a document are also
instantiated as a tree, because validation claims instantiation never
needs to re-check, and text that compiles is read back through the
runtime reader, because the compiler claims it only mints what that
reader accepts — an input that breaks either claim is a finding.

## Why this is a separate workspace

A libFuzzer target links a runtime needing the nightly toolchain and
sanitizer instrumentation. As a workspace member it would sit in the path
of `cargo build --workspace`, which runs on stable and gates every merge —
so the harness would break the build it exists to protect. The root
manifest excludes this directory by name.

The consequence worth knowing: the root licence check never resolves these
dependencies, so the scheduled job runs that check here instead.

## What these add over the existing suites

Both parsers already assert the property these targets test. The pack
reader's suite says it "answers, one way or the other, for every byte
string it can be handed"; the codec has the same over arbitrary text, plus
a mutation property shaped like a real file.

**Those run a few hundred cases from a fixed seed. These are coverage
guided** — inputs reaching new branches are kept and mutated, so the
fuzzer works past the magic number and the header into the entry table,
where uniform random bytes essentially never land. That is the difference:
not a first line of defence, a deeper one.

## The committed corpus

`corpus/<target>/` is the fuzzers' memory: every committed input is one
the coverage-guided search found worth keeping, minimized by
`cargo fuzz cmin`. Runs start from it (locally and on the schedule),
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

Only that the parser answers. Whether the answer is *correct* belongs to
the round-trip properties beside each parser — a fuzz target that asserted
on content would fail on inputs that are legitimately malformed, and the
corpus would fill with noise.
