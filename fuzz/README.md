# renew-fuzz

Fuzz harnesses for the eight parsers that read data the engine did not
write: the asset pack reader, the input-trace codec, the WAV reader, the
UI document reader, the UI text grammar, the datagram reader, the PNG
decoder, and the JSON reader. Three of them go
further than the rest: bytes that read as a UI document are also
instantiated as a tree, because validation claims instantiation never
needs to re-check; text that compiles is read back through the
runtime reader, because the compiler claims it only mints what that
reader accepts; and a JSON document that parses is walked, with every
typed question asked of every value in it, because validation claims an
accepted document has nothing deferred left in it. An input that breaks
any of those claims is a finding.

`json_parse` is also the only target given its bytes unfiltered.
`trace_parse` guards its input with `from_utf8`, because that parser
takes text and every crash found past a lossy conversion would be
unreachable from a real file; the JSON reader takes bytes on purpose, so
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

`corpus/<target>/` is the fuzzers' memory: most committed inputs are ones
the coverage-guided search found worth keeping, minimized by
`cargo fuzz cmin`. **Two corpora start differently** — `png_decode`'s
seeds are written by `cargo run -p renew-png --example make_corpus` and
`json_parse`'s by `cargo run -p renew-json --example make_corpus`, each
of which builds every one, so those directories carry no file this
repository did not author and no licence question with them. Each
generator is committed beside its seeds and they change together. Once a target's
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

Only that the parser answers. Whether the answer is *correct* belongs to
the round-trip properties beside each parser — a fuzz target that asserted
on content would fail on inputs that are legitimately malformed, and the
corpus would fill with noise.
