# renew-cli

`renew` is the workspace's command-line entry point: one binary wrapping the
canonical developer tasks so that scripts, CI, and people all drive the same
commands the same way.

```
usage: renew <command> [options]

commands:
  configure  verify the toolchain and cargo are present and sane
  build      build the workspace
  test       run the workspace test suite
  bench      run the workspace benchmarks
  lint       check formatting, then run clippy with warnings denied
  check      verify workspace crate manifests and dependencies
  coverage   hold a coverage report against the exemption manifest
  doctor     check the development environment

options:
  --json            emit one machine-readable JSON document on stdout
  --report <path>   (coverage only, required) the llvm-cov JSON export to read
  --smoke           (bench only) run each benchmark once, without statistics
```

`bench --smoke` is a second fixed entry in the command table (every bench
executes once — the fast run-proof mode CI's benchmark stage uses), not a
pass-through: the flag is rejected on every other subcommand. The JSON
envelope does not distinguish smoke from a full bench run — the caller
knows which mode it invoked, and the envelope shape stays uniform.

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
  runs.
- **The coverage gate reads a report; it does not produce one.** The
  collection is CI's job and takes minutes; comparing an export against the
  manifest is pure, instant, and unit-testable down to each failure
  message. It also keeps the tool from owning a second copy of the
  collection's filters.
- **Environment checks read the workspace's own pins.** `doctor` compares
  the active toolchain against `rust-toolchain.toml` and takes its version
  floor from the workspace manifest's `rust-version`, rather than
  hardcoding either.
