# Contributing to renew

Thank you for your interest in renew.

The project is young and moving quickly, and much of it is explicitly not
settled yet. Before building on a module, check what it claims to be:

```sh
cargo run --bin renew -- modules
```

Nothing at `bootstrap` should be treated as stable, and today that is most
of the tree. That is stated up front because it decides what kind of
contribution is likely to land.

## Issues

**Issues are welcome and are the most useful thing you can send.** Bug
reports, questions about why something is the way it is, and cases where
the documentation and the code disagree are all valuable — the last of
those especially, since a document that misdescribes the code is treated
here as a defect rather than as tidying owed later.

A report that lets someone reproduce the problem is worth several that
describe it. Where you can, include:

- what you ran, exactly — the command line, not a paraphrase;
- what happened, and what you expected instead;
- your platform and `rustc --version`;
- for anything involving the GPU, your adapter and driver version.

If a run printed a state digest, include it verbatim. Simulation runs are
reproducible from their inputs, so a digest that differs from the expected
one is often the whole diagnosis.

## Pull requests

Small, focused changes are welcome. **For anything larger than a bug fix,
please open an issue first** — not as a formality, but because the shape
of a module is usually already decided, and a pull request that cuts
across that decision is work nobody enjoys discarding.

Every pull request also needs its author to have agreed to the
[Contributor License Agreement](CLA.md) — once, not per change. It is
worth reading before you write code rather than after, because it says
what the project may do with your contribution. The short version is in
[License and the CLA](#license-and-the-cla) below.

Expect review to be slow. The project has one maintainer.

## The bar a change is held to

This is the bar every change already in the tree was held to:

- **Every change arrives with its tests.** A bug fix arrives with a test
  that fails without it; new behaviour arrives with tests for that
  behaviour.
- **A test must be able to fail.** If you cannot say what would break it,
  it is not testing anything. Breaking the code deliberately and watching
  the test fail is a cheap way to be sure — and it catches the tests that
  pass for the wrong reason.
- **Documentation changes in the same commit as the behaviour it
  describes.** Not in a follow-up.
- **Claims carry evidence.** "This is faster" needs numbers and the
  configuration that produced them. "This passes" means it was run and the
  output seen. Nothing is asserted from expectation.
- **No new dependencies without discussing it first.** Every third-party
  crate is a long-term commitment, and the runtime accepts only
  permissively licensed ones. Open an issue.
- **Simulation code reads no clock and no unseeded randomness.** Anything
  that must produce the same result from the same inputs takes its time
  and its seed as arguments. Some crates enforce this with lints; the rule
  holds whether or not a lint catches you.
- **`unsafe` is denied by default.** Where a crate needs it, every block
  carries a comment justifying it, and the question asked in review is
  what makes the block sound — not whether it appears to work.

## Running the gates before you push

Build and test instructions are in the [README](README.md). Beyond those,
run what CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --bin renew -- check
```

CI then runs thirteen required checks on every pull request: formatting,
lint, structure, tests on Linux, macOS and Windows, dev and release
builds, benchmark build, licence and advisory scanning, line coverage,
software-Vulkan rendering, and a configuration matrix proving every
optional module can be removed. **All thirteen must pass before a change
is merged.**

Coverage is worth a note because it surprises people: the threshold is
100% of measured lines, against a short list of named exemptions that each
carry an individual reason. A line that genuinely cannot be covered needs
an entry and a justification — not a lowered threshold.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/): `feat(scope):
…`, `fix(scope): …`, `docs: …`, `test: …`. The subject says what changed;
the body says **why**, which is the part still useful a year later. A
commit explaining the reasoning behind a trade-off is worth more than one
listing the files it touched.

## What a module's maturity means for your change

- **`bootstrap`** — interface churn is expected. Break it freely.
- **`internal`** — other modules depend on it. Breaking changes need a
  reason and get more scrutiny.
- **`stable`** — the public API is a promise, and breaking it needs a
  deprecation path.

`cargo run --bin renew -- modules` prints the current level for every
crate, read from that crate's own manifest. That manifest is the only
place maturity is recorded, so it cannot disagree with anything else.

## License and the CLA

renew is licensed under the [Apache License, Version 2.0](LICENSE), and
every contribution is distributed under those terms.

Beyond that, contributors agree to a [Contributor License Agreement](CLA.md)
before their first pull request is merged. Three things are worth saying
plainly, because a CLA that surprises someone has failed at its job:

- **You keep your copyright.** It is a license, not a transfer. Your work
  stays yours to use anywhere else, including in a competing project.
- **The project may one day be offered under other terms**, including
  commercial ones, with your contribution part of that. This is what
  allows the project to fund its own maintenance.
- **The open source version cannot be withdrawn.** Section 9 of the
  agreement commits renew to remaining available under an OSI-approved
  license, at no charge. A commercial edition could exist beside it,
  never instead of it — and anything already released stays released,
  since an Apache-2.0 grant cannot be revoked.

Agreeing takes one filled-in block in your pull request description; the
template puts it there for you. You do it once, and it covers everything
you contribute afterwards.

If you are contributing as part of your job, read section 4 of the
agreement first — your employer may need to agree rather than you.
