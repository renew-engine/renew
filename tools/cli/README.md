# renew-cli

`renew` is the workspace's command-line entry point: one binary wrapping the
canonical developer tasks so that scripts, CI, and people all drive the same
commands the same way.

```
usage: renew <command> [options]
       renew [options] run <sample> [--] [sample arguments...]
       renew record --output <path> <sample> [--] [sample arguments...]
       renew replay --input <path> <sample> [--] [sample arguments...]

commands:
  configure  verify the toolchain and cargo are present and sane
  build      build the workspace
  test       run the workspace test suite
  bench      run the workspace benchmarks
  run        build and run a workspace sample
  record     run a sample, writing the input it saw to a file
  replay     run a sample from a recorded input file
  lint       check formatting, then run clippy with warnings denied
  check      verify workspace crate manifests and dependencies
  coverage   hold a coverage report against the exemption manifest
  modules    list every module with its maturity, from the manifests
  asset-pack  build an asset pack from a directory of files
  asset-inspect  list an asset pack's entries, optionally verifying them
  ui-compile  compile a text document into the binary form the engine loads
  determinism  emit this target's simulation digests, or compare several targets'
  doctor     check the development environment

options:
  --json            emit one machine-readable JSON document on stdout
  --report <path>   (coverage only, required) the llvm-cov JSON export to read
  --smoke           (bench only) run each benchmark once, without statistics
  --output <path>   (record only, required) the trace file to write
  --input <path>    (replay only, required) the trace file to read
  --pack <path>     (asset-pack, asset-inspect; required) the pack file
  --from <path>     (asset-pack, ui-compile; required) the directory to
                    pack, or the text document to compile
  --out <path>      (ui-compile only, required) where the compiled document
                    is written
  --verify          (asset-inspect only) check each entry against its digest
  --emit <path>     (determinism only) write this target's digests here
  --compare <path>  (determinism only, repeatable) a target report to compare
  --target <triple> (determinism --emit only) build and run the pinned
                    simulations for this triple, through cargo's runner
                    mechanism where one is configured
  --features <list> (run, record, replay; repeatable) cargo features to build
                    the sample with, e.g. `--features window` for a window
  --help, -h        print this text; `renew help` does the same

Everything after `run <sample>` goes to the sample untouched, including
flags renew itself knows: `renew run hello_triangle --json` gives the sample
`--json`, while `renew --json run hello_triangle` gives it to renew. One `--`
after the sample name is an optional separator and is not passed on.

`record` and `replay` are `run` with a trace file: their flag goes before
the sample name for the same reason, and reaches the sample as
`--record-trace <path>` or `--replay-trace <path>` at the front of its line.
Recording and replaying are headless: a windowed replay is a live run
wearing a replay's name. How a sample spells headless is the sample's own
business — some take `--headless`, others are headless unless asked for a
window — so its usage says which, and this tool assumes nothing.

`--features` reaches cargo, not the sample. It builds the sample with those
features on, which is how a sample's optional capabilities are named:
`renew --features window run glide --window` builds the window in, then
asks for it.
```

`bench --smoke` is a second fixed entry in the command table (every bench
executes once — the fast run-proof mode CI's benchmark stage uses), not a
pass-through: the flag is rejected on every other subcommand. The JSON
envelope does not distinguish smoke from a full bench run — the caller
knows which mode it invoked, and the envelope shape stays uniform.

## Running a sample

```
renew run hello_triangle --headless --frames 600 --dump-stats stats.json
renew run hello_triangle -- --headless --frames 600
renew run input_echo -- --headless --input-trace walk
```

**Everything after the sample name belongs to the sample**, taken
verbatim — including flags `renew` itself understands. `renew run
hello_triangle --json` hands the sample `--json`; `renew --json run
hello_triangle` keeps it for `renew`. The rule is positional rather than
a list of exceptions, because a list would mean a sample could never own
a flag whose name this tool also uses, and the day the two disagreed the
failure would be silent.

A single `--` may stand between the two halves for a human reader. It is
the marker, not an argument, so it is dropped and the two spellings above
are indistinguishable to the sample. Only the first one is dropped: a
sample wanting a literal `--` writes two, exactly as it would through
`cargo run`.

**Which samples exist is discovered, not listed here.** Every invocation
reads `cargo metadata` and takes the binary targets of every package
under `samples/`; a sample added, renamed, or deleted needs no edit to
this tool. The name you type is the *binary's* name (`hello_triangle`),
not the package's (`renew-sample-hello-triangle`) — the same name the
sample prints about itself. A name matching nothing is a usage error: it
lists the samples that do exist and exits `2`, like any other unreadable
command line. A sample list that cannot be *read* is a different answer
and gets a different one — exit `1`, saying so, never "unknown sample".

The child is `cargo run --package … --bin … -- …`, run from the
workspace root, so a sample is always built before it runs and always
against the same tree. In the default (non-`--json`) mode the sample
inherits this process's stdout and stderr and its output arrives as it is
written, unbuffered and in its own order — which is what lets CI grep a
sample's digest line straight out of the log. `run --json` captures
instead, exactly as `build --json` does, because that mode promises
exactly one document on stdout; the sample's output is then in the
envelope's `stdout` field rather than beside it. The envelope carries no
`sample` field, for the same reason `bench --smoke` adds nothing: the
caller knows what it invoked, and the envelope shape stays uniform.

A failing sample follows the same contract as any other failing child:
the `renew` process exits `1`, and the sample's raw exit code survives in
the envelope's `exit_code`.

## Asset packs

A pack is one file holding many named blobs, each with a digest of its own
contents. `asset-pack` builds one from a directory; `asset-inspect` reads
one back.

```
renew asset-pack --from assets/ --pack game.rpk
renew asset-inspect --pack game.rpk --verify
```

Entries are named by their forward-slashed path relative to `--from`, on
every platform, and the pack is sorted by name before it is written. Those
two together are what make the output **byte-identical for the same
inputs**: neither the order a directory happened to be walked in nor the
separator a filesystem happens to use reaches the bytes.

`--verify` re-hashes every payload and exits non-zero if any disagrees with
its recorded digest. It is off by default because listing reads only the
table while verifying reads every byte — a distinction that matters once a
pack is large.

There is no `import` subcommand. A real importer needs an image or audio
decoder, and one that only copied bytes would be worse than its absence.

## The module inventory

`modules` prints every workspace crate with the maturity it declares, read
from that crate's own manifest:

```
renew modules
renew modules --json
```

Each crate states its maturity in `[package.metadata.renew]`, and that is
the only place it is written down. Anything that needs the list — a
release note recording what a version promises, a document naming the
optional crates — reads it from here rather than restating it, because a
retyped table is a second copy of a fact that goes stale without saying
so. Rows are ordered by maturity rather than alphabetically, and the
summary line counts how many crates are `stable`, since that is the set a
version's compatibility promise can cover.

A crate whose metadata does not parse still gets a row, carrying the
reason in place of its fields. Dropping it would make the inventory
quietly shorter than the workspace, and an inventory that silently omits
what it could not read is the kind that gets believed.

It reports; it does not gate. `check` is what fails on a malformed
manifest.

## The coverage gate

`coverage` reads an `llvm-cov` JSON export and holds it against
`coverage-exemptions.toml` at the repository root, which names — per line,
with a reason — the handful of lines that cannot be covered. Everything
else must be: the threshold for the rest of the tree is 100%.

```
cargo llvm-cov report --json --output-path target/coverage.json
renew coverage --report target/coverage.json
```

The ratchet runs both ways, and both fail:

- an uncovered line with no entry in the manifest is a **new gap**;
- an entry whose line the report says is covered — or whose file the
  report no longer measures — is a **stale exemption**, and leaving it is
  a hole in the gate on exactly the line someone once proved could not be
  closed.

An entry left behind by code that moved shows up as the second of those,
because the old number now points at whatever took its place. Deleting it
would be wrong — the exemption is still earned, at a new line — so a
covered-now finding also names any lines of the same file that are
uncovered and unexempted. That is where the code most likely went.

The subcommand does not run the collection: CI produces the export and
hands it over. That keeps the command pure and fast, keeps the rule itself
under unit test, and keeps the ignore filter in one place (the collection),
so the table in the log and the gate measure the same tree.

`--ignore-filename-regex` filters the export's `files` list but not its
`functions` records, so the gate takes its measured set from `files` and
ignores regions naming anything else. A line counts as uncovered when some
region with a zero execution count spans it and no region with a positive
count does — the rule `cargo llvm-cov --show-missing-lines` reports
against, verified to reproduce its output exactly on this workspace, and
deliberately *not* the segment table, which paints closing braces and
never-taken `else` arms that the report itself counts as covered.

## Status

Early-stage (`bootstrap` maturity — see the `[package.metadata.renew]` table
in [Cargo.toml](Cargo.toml), which is authoritative for maturity and
manifest metadata): the flag surface and JSON schema may still change
without a deprecation cycle. The parsers here — the toolchain-pin and
manifest-field readers, the JSON parser behind `check` and `coverage`, and
the exemption-manifest reader — are covered by unit tests today (the JSON
parser also bounds nesting depth so hostile input errors instead of
exhausting the stack); fuzz coverage is planned as the tool matures toward
a stable interface.

## Machine-readable output

Every subcommand accepts `--json` and then emits exactly one JSON document
on stdout:

```json
{"schema_version":1,"command":"test","status":"ok","exit_code":0,
 "duration_ms":8421,"stdout":"…","stderr":"…"}
```

- `status` is `ok`, `failed` (the underlying command ran and failed), or
  `error` (it could not run).
- `exit_code` is the child's raw exit code (`-1` for signal deaths); the
  `renew` process itself always exits `0` (ok), `1` (failed/error), or `2`
  (usage error).
- `doctor --json` adds a `checks` array of `{name, ok, detail}` objects to
  the same envelope; `check --json` adds a `findings` array of
  `{rule, message}` objects (empty when the workspace is healthy).
- `coverage --json` adds `measured_files` and `exempt_lines` counts, an
  `uncovered` array of `{file, line}` (new gaps) and a `stale` array of
  `{file, line, state, reason}`, where `state` is `now-covered` or
  `file-absent`. All four keys are unconditional, including on the
  `error` path, so consumers never see a conditional key.
- `run --json` adds nothing: the sample's own stdout and stderr are what
  the `stdout` and `stderr` fields carry, and `exit_code` is the sample's.
- A **usage error emits no document at all**, `--json` or not, because
  nothing ran to report on. That includes `run` with a sample name that
  matches nothing.
- `schema_version` increments on breaking changes to this shape.

## Key decisions

- **Zero dependencies.** A fixed handful of subcommands needs a `match`,
  not an argument-parsing library, and the JSON in and out of this tool
  needs a small tested writer and parser, not a serialization framework.
- **Thin shell over a testable core.** The binary (`main.rs`) only does
  process I/O; parsing, the command table, JSON emission, and the doctor
  rules live in library modules with unit tests.
- **The command table is the single source of truth.** Each subcommand maps
  to fixed argument vectors in `src/plan.rs`; nothing else decides what
  runs. `run` is the one subcommand whose arguments cannot be fixed — it
  builds them in the same module, from the sample the command line named.
- **The sample list is discovered, never written down.** A table of
  samples in this tool would be a second place to edit whenever one is
  added or renamed, and the copy nobody runs is the copy that goes stale.
  `src/samples.rs` reads them out of `cargo metadata` on every
  invocation, by location (`samples/`) and by binary target — the same
  way `src/structure.rs` decides which crates are engine crates.
- **The coverage gate reads a report; it does not produce one.** The
  collection is CI's job and takes minutes; comparing an export against the
  manifest is pure, instant, and unit-testable down to each failure
  message. It also keeps the tool from owning a second copy of the
  collection's filters.
- **Environment checks read the workspace's own pins.** `doctor` compares
  the active toolchain against `rust-toolchain.toml` and takes its version
  floor from the workspace manifest's `rust-version`, rather than
  hardcoding either.

## The cross-platform determinism gate

Every other determinism check in this repository compares a run to itself,
or to a constant this repository minted. Both prove an unseeded generator
absent; neither can prove the *target* did not matter, because both halves
of those comparisons ran on one target.

`determinism` is the only place that claim is tested, and it is two modes
because the claim needs two machines.

```
renew determinism --emit leg.json
```

runs the pinned simulations — eleven of them, contributing fifteen digests,
because a run reports whichever digests its own report carries and the four
glide configurations each carry two (the frame schedule's and the world's)
— and writes what this target saw, together with its architecture and the exact `rustc --version`
that built it. Digests are hex **strings**, not JSON numbers: a `u64`
exceeds what a JSON number is guaranteed to carry exactly, and a reader
that silently rounded one would report two different states as identical,
which is the single failure this gate exists to prevent.

Adding `--target <triple>` builds and runs those same simulations for
another target instead of this one:

```
renew determinism --emit leg.json --target x86_64-linux-android
```

**This needs a runner configured, and says so rather than assuming it.**
Cargo executes a cross-built binary through whatever
`CARGO_TARGET_<TRIPLE>_RUNNER` names — for the triple above, that is
`CARGO_TARGET_X86_64_LINUX_ANDROID_RUNNER`. With no runner set, cargo
tries to execute the binary here, which fails rather than quietly
measuring the wrong machine.

This repository ships two:

| runner | for | what it does |
|---|---|---|
| `tools/android-runner.sh` | `*-linux-android` | pushes the binary to a connected device with `adb`, runs it there, and carries its exit code back through a file, because `adb shell` reports its own shell's status rather than the program's |
| `tools/ios-sim-runner.sh` | `aarch64-apple-ios-sim` | runs the binary on a booted simulator with `xcrun simctl spawn`, which needs no push (the simulator shares this filesystem) and reports the child's exit code directly |

Android needs a linker for the target as well; the CI lanes set what
each one needs.

The triple also decides what the leg calls itself, so only triples this
tool has been taught are accepted — anything else is refused by name
before a build starts, because a leg labelled by a guess would be
compared against rows it does not belong to.

```
renew determinism --compare linux.json --compare windows.json --compare macos.json
```

holds them against each other. It exits 0 only when every target agrees
over a non-empty digest set. Everything else is exit 1, and the reasons
are deliberately separated:

- **Diverged** — the targets ran the same inputs and reached different
  state. This is the finding the gate exists to produce, and the message
  names the digest, both values, and both architectures.
- **Inconclusive** — the comparison could not be made, and *this is a
  failure, not a pass*. A leg is missing, a leg carries no digests, the
  reported architecture set does not match the one the tool binds, or two
  legs were built by different compilers. The toolchain check outranks the
  digest comparison on purpose: two compilers producing two digests is not
  evidence of a portability bug, and reporting it as one sends somebody
  hunting something that is not there.

The architecture set is matched row for row rather than counted. Three
legs on one instruction set satisfy a count of three and prove strictly
less than the tool claims, so a runner fleet that quietly changes
architecture fails here rather than passing while measuring less.
