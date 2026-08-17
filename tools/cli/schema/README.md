# The `renew --json` schema registry

Every subcommand run with `--json` prints exactly one JSON document on stdout: the
**result envelope**. This directory is the contract for that surface — consumers validate
against these files rather than guessing from examples.

One carve-out: a command line the parser refuses (an unknown flag, a missing value, an
unknown sample name) exits `2` with usage text on stderr and **no envelope at all**. For
the parser's own refusals the invocation never reached a subcommand; an unknown sample
name is discovered inside `run`, `record`, or `replay` — which share one sample-resolution
path — but treated the same way, as an unreadable command line, and it is only reached in
a tree the tool accepted, because the earlier refusals win and those do emit envelopes.

## Versions

| `schema_version` | Status | Notes |
|---|---|---|
| 2 | current | added `target`, `coverage`, structured `failures` |
| 1 | retired | the shared leading fields were the same; none of the v2 fields existed |

This table is the **result envelope's** version, and nothing else's. `determinism --emit`
writes a separate leg document with its own `schema_version`, versioned independently of
this one; a leg file reading `"schema_version": 1` is current, not stale output.

Seven leading fields open every envelope, in a fixed order: `schema_version`, `command`,
`status`, `exit_code`, `duration_ms`, `stdout`, `stderr`. `exit_code` is the failing
child's own raw code (`-1` for a signal death) where a child delivered the outcome, and
this tool's own `0` or `1` where the verdict is its own — a refusal, an abort, or a
comparison it made itself. The `renew` process always exits `0`, `1`, or `2` whatever the
envelope reports. Per-command payload fields and the
v2 fields follow the leading seven; their relative order within that tail is not part of the
contract — read fields by name, never by position.

## The three v2 fields

**`target`** is an object — `{"kind", "root", "manifest"}` — saying what tree the invocation
ran in. `root` is the workspace directory the children ran from and `manifest` is that
directory's `Cargo.toml`; `kind` is `engine-workspace` (the engine repository,
identified by a `[workspace.metadata.renew]` table containing `engine = true` in its root
manifest — only that bracketed-table spelling counts, with whitespace and `#` comments
tolerated) or `project` (any other workspace with at least one dependency on a `renew-`
crate declared by a workspace member — classification reads `cargo metadata --no-deps`, so a
renew crate reached only transitively, through a wrapper the workspace depends on, does not
count). A standalone `[package]` manifest with no `[workspace]` table counts as its own
workspace — cargo treats it as a workspace of one, and the default `cargo new` game is
exactly that shape. A package nested under an enclosing `[workspace]` manifest resolves to
that ancestor unconditionally — including where cargo itself would treat the package as
standalone because the ancestor `exclude`s it; there is no override today. Table headers
are recognized in their bare-key spellings only
(whitespace inside the brackets and around dots is tolerated). A quoted spelling such as
`["workspace"]` — legal TOML that cargo reads as the workspace table — is not a table name
this scan can read, and a manifest it cannot read is **refused rather than walked past**: the
walk stops there, the invocation returns `classification-failed` naming the file, and no
`target` is emitted. The same is true of a manifest with a syntax error, or one whose bytes
are not UTF-8. Refusing is deliberate — walking on would anchor at some ancestor and report a
verdict about a tree the caller never asked about — but it does mean a manifest cargo itself
accepts can be one this tool declines to read. Bare keys are what it reads.

**`coverage`** appears on the subcommands that take `--features` — `build`, `test`, `bench`
and `lint` (clippy compiles what it lints), and `run`, `record` and `replay` once the sample
is resolved — and states what the cargo invocation actually enabled: the feature list passed
through, whether `--all-features` was on, the package scope (`workspace`, or the sample's own
package), and the profile cargo compiles for the verb (`dev` for build, lint and the sample
runners, `test` for test, `bench` for bench — cargo's own profile names). It is descriptive,
never aspirational: a green verdict's reader can see which feature flags that green was built
with, because a default-feature run can leave feature-gated code entirely uncompiled while
reporting success. On the four workspace verbs it describes the invocation the compiling child
was given, whether or not that child was reached: on a refusal it states what the refused run
would have covered, and on a multi-step verb whose earlier step failed (`lint` runs
`cargo fmt` before `cargo clippy`) it states what the clippy step would have compiled. The
three sample runners are the exception their rollout row records: they have no package to name
until the sample resolves, so their refusals carry no statement at all. Which step a red came
from is named in `failures[].summary`, and `status` separates a refusal (`error`) from a
delivered red (`failed`).

`determinism --emit` compiles and runs the pinned simulations and carries no coverage
statement: it takes no `--features`, so its runs are always default-featured, and the leg
document it writes records the target and toolchain rather than a feature set.

What it does **not** state is which *targets* cargo compiled. `cargo build --workspace`
builds lib and bin targets only, so a green `build` says nothing about a broken test or
example target; `test` and `lint` compile those too. Default features are always on — there
is no `--no-default-features` — so the feature list names what was added to them, never the
whole set.

**`failures`** is an array of `{code, summary}` entries; on the subcommands the rollout
table below marks as carrying it, a successful envelope carries it empty.

The two gate subcommands are the exception worth stating plainly: `check` and `coverage`
deliver their reds in their **own payload arrays** — `findings` for check, `uncovered` and
`stale` for coverage — with `status` `failed`, `failures` empty, and `stderr` empty. For
those two, a consumer reads `status` plus the payload, because their verdict lives there.
On every other subcommand that carries `failures` at all — the rollout table below marks
which; `doctor`, `asset-inspect`, `asset-pack`, `ui-compile` and `help` never do, and of those
only `doctor` and `asset-inspect` ever report `status` `failed` (the other three are `ok` or
`error`) — the array carries both kinds: the reasons a run could not deliver a verdict
(refusals, aborts) *and* delivered reds (`step-failed`, the two determinism verdicts). It is
`status` that tells those apart — `error` for the former, `failed` for the latter — never the
array's emptiness. The code set is open — new codes may join within a schema version — and the known
codes are listed in `envelope.v2.json` under `x-known-codes`:

- `not-a-renew-project` — the workspace's metadata was read cleanly and no member depends on
  a renew crate; the tool refuses to report on it.
- `classification-failed` — the tool could not establish what the tree is (no manifest of
  either shape above the working directory, an unreadable manifest, broken metadata, a
  toolchain that would not answer). Deliberately distinct from the code above: "could not
  tell" is not "told and found nothing".
- `engine-only-subcommand` — the subcommand reads surfaces only the engine has (its samples,
  manifests, structure rules, pinned simulations, coverage ledger) and the tree is a project.
- `step-failed` — a step the subcommand exists to run (a build, test, bench, lint, or
  configure step; a sample; a pinned determinism run) exited nonzero; its output is split across the
  envelope's `stdout` and `stderr` fields.
- `aborted` — the invocation ended before a verdict could be delivered: anything that
  stopped it short. A spawn failure; an unreadable input, including a determinism leg file
  that could not be read or parsed; an unwritable output; a probe that would not answer
  (the compiler declining to name its own version); auxiliary machinery such as the
  `cargo metadata` call behind the sample listing failing before the run it served ever
  started; or an inconsistency in the engine's own pinned list. The reason is in `stderr`
  and the summary.
- `determinism-diverged` — the determinism comparison delivered its verdict and the targets
  disagree: the one red this tool exists to produce, never to be mistaken for an abort.
- `determinism-inconclusive` — every leg file was read, and the comparison judged them
  insufficient or mismatched: fewer targets than the engine claims, a leg that carries no
  digests at all, **a leg that ran only part of the pinned list — some bound digest names
  absent**, a set of (os, arch) targets that does not match the bound rows, the same target
  reported twice, or mismatched toolchains. A failure, not a pass, and not an abort either — a leg file that
  could not be read at all is `aborted`, because nothing was judged.

Errors past classification inside a subcommand's own machinery carry `aborted` too — the
invocation ended before a verdict, whichever line raised it — so a consumer that dispatches
on the code is never handed a red with nothing to dispatch on. Finer-grained codes for
compiler and test failures may join the set in a later version.

## Rollout, per subcommand — exhaustive

`target` appears once classification has succeeded, on every outcome from that point on —
success, `step-failed`, `aborted`, delivered red verdicts, engine-only refusals, and errors
past classification in a subcommand's own machinery (the kind was established, so the
envelope keeps it). This is one rule for every tree-anchored subcommand; no row below
overrides it. A tree the tool could not classify, or refused as no one's project, carries
the failure code in place of `target`.

| Subcommands | `target` | `coverage` | `failures` |
|---|---|---|---|
| `build`, `test`, `bench`, `lint` | once classified | always — even on a refusal, including the rootless one, it states what the run would have covered | yes |
| `configure` | once classified | no | yes |
| `run`, `record`, `replay` | once classified | once the sample is resolved — `packages` names the sample's own package | yes |
| `determinism` (both modes) | once classified | no | yes |
| `check`, `modules` | once classified | no | yes |
| `coverage` | once classified | no | yes |
| `doctor` | no — it diagnoses environments that may have no workspace at all | no | no |
| `asset-pack`, `asset-inspect`, `ui-compile` | no — they operate on files, not trees | no | no |
| `help` | no | no | no |

`doctor` is the one subcommand that reads a tree without naming one: where a workspace root
exists above the working directory, its `toolchain-pin` check is read out of that root's
`rust-toolchain.toml` and its `cargo` check compares the installed cargo against the floor
that root's `Cargo.toml` declares. Where no root exists, `toolchain-pin` reports the absence
while the `cargo` check falls back to this tool's own built-in minimum, so its detail names a
floor no manifest supplied. `doctor` classifies nothing and carries no `target`, so a
consumer collecting doctor envelopes beside build envelopes correlates them by working
directory rather than by the envelope.

A workspace that is neither the engine nor a project is refused by every tree-anchored
subcommand above before any child runs, with the refusal's code in `failures`. Note that
`configure` and `lint` are tree-anchored too: they classify, and they refuse a stranger's
tree like the rest.
