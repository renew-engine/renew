# renew-cli

`renew` is the workspace's command-line entry point: one binary wrapping the
canonical developer tasks so that scripts, CI, and people all drive the same
commands the same way.

```
usage: renew <command> [--json]

commands:
  configure  verify the toolchain and cargo are present and sane
  build      build the workspace
  test       run the workspace test suite
  bench      run the workspace benchmarks
  lint       check formatting, then run clippy with warnings denied
  doctor     check the development environment
```

## Status

Early-stage (`bootstrap` maturity — see the `[package.metadata.renew]` table
in [Cargo.toml](Cargo.toml), which is authoritative for maturity and
manifest metadata): the flag surface and JSON schema may still change
without a deprecation cycle. The small config parsers here (toolchain pin,
manifest fields) are covered by unit tests today; fuzz coverage is planned
as the tool matures toward a stable interface.

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
  the same envelope.
- `schema_version` increments on breaking changes to this shape.

## Key decisions

- **Zero dependencies.** Six fixed subcommands need a `match`, not an
  argument-parsing library, and the flat JSON above needs a small tested
  writer, not a serialization framework.
- **Thin shell over a testable core.** The binary (`main.rs`) only does
  process I/O; parsing, the command table, JSON emission, and the doctor
  rules live in library modules with unit tests.
- **The command table is the single source of truth.** Each subcommand maps
  to fixed argument vectors in `src/plan.rs`; nothing else decides what
  runs.
- **Environment checks read the workspace's own pins.** `doctor` compares
  the active toolchain against `rust-toolchain.toml` and takes its version
  floor from the workspace manifest's `rust-version`, rather than
  hardcoding either.
