# renew-event

The engine's event vocabulary: what happened, as plain data. Three
enums — `WindowEvent`, `KeyCode`, `PointerButton` — plus `EVERY_EVENT_SHAPE`
and `shape_index` over it.

No dependencies. Nothing here can make anything happen.

## Contract

**This crate can never acquire a way to observe the outside world.**
That is what it is for, and it is checked rather than promised: a crate
declaring that its output depends only on build, seed and input may
depend on this one, and the ban it lives under is enforced over the
dependency graph. Adding a dependency here would defeat that guarantee
for every such crate at once.

The manifest declares no dependencies and must keep declaring none.

## Why it is a separate crate

The vocabulary used to be a module inside the platform crate, beside the
clock, the filesystem and thread spawning, marked *"deliberately not
behind the `window` feature"*.

That comment was describing the right instinct with the wrong mechanism.
A feature gate is a convention — whoever writes the next manifest can
forget it, and nothing that inspects the dependency graph can see it.
**A crate boundary is a fact that graph can read.** The isolation it
describes only became real, rather than intended, when the boundary
moved.

The types are also not a platform capability in the first place. A key
code describes something that happened; it cannot cause anything. It sat
next to three doorways to non-determinism because everything in that
crate was "platform-ish", and that shared address was the entire reason
the dependency edge could not simply be forbidden.

## Thread safety

Every type here is a plain `Copy` value with no interior mutability and
no shared state. They are `Send` and `Sync` by construction, and there is
nothing to synchronise.

## Relationship to `renew-platform`

`renew-platform` re-exports this crate as its `event` module, so every
path a consumer already uses keeps working. Code that *produces* these
values from the operating system lives there and does need a windowing
library; the vocabulary itself does not.

**The re-export makes `renew-platform` a downstream crate of enums it
used to define.** The enums are `#[non_exhaustive]`, which binds
downstream crates only — so the platform crate may still *construct*
these values, as its translation code does, but may no longer match on
them exhaustively.

## Testing

Unit tests cover the one invariant with a way to go wrong: that
`EVERY_EVENT_SHAPE` lists each shape exactly once and in the order
`shape_index` returns.

Not applicable, recorded rather than skipped silently:

- **Property tests** — not math, containers, or allocators.
- **Fuzz target** — nothing here parses external data. These values are
  built by translation from an OS event, never decoded from bytes, so
  the untrusted-input obligation does not reach this crate.
- **Determinism test** — no state and no clock; the crate declares
  `simulation = false` precisely because it has no simulation to run.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies
and extension points. Read it there rather than here.
