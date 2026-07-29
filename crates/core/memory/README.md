# renew-memory

The engine's allocation seam: explicit allocators passed as context, plus
an instrumented global allocator for counting.

- `LinearArena` — a fixed-capacity bump allocator for `Copy` data:
  allocate forward, hand out disjoint references, reclaim everything at
  once with `reset`. Tracks a high-water mark that survives resets.
- `Pool<T>` — a fixed-capacity object pool with generation-checked
  handles: stale handles miss instead of aliasing recycled slots.
- `CountingAllocator` — a wrapper over the system allocator that a
  *binary* installs with `#[global_allocator]`; every allocation in the
  process is then counted, readable through `counters::snapshot()`.

## Contract

- **Hot-path allocation is explicit.** Arenas and pools are passed to the
  code that uses them; ownership and lifetime are visible at every call
  site.
- **Backing storage is acquired up front from the process's global
  allocator** — never from platform APIs — and neither allocator grows.
- **Counters are diagnostics, never control flow.** Fields of a snapshot
  are independently read and only coherent enough for reporting.
- **No clock, no filesystem** (rejected at lint time), and `LinearArena`
  is deliberately not `Sync`.

## Status

Early-stage: surfaces grow when a consumer needs them. The
`[package.metadata.renew]` table in [Cargo.toml](Cargo.toml) is
authoritative for maturity and manifest metadata. `unsafe` is confined to
the allocator internals with the invariant stated at every block; the
arena is `Copy`-only until a consumer needs drop-aware allocation.

## Key decisions

- **`&self` allocation, `&mut self` reset.** Each allocation returns a
  disjoint region, so shared-borrow allocation is sound; reset demands
  exclusivity, so the borrow checker retires every outstanding reference
  before memory is reused.
- **Fixed capacity everywhere.** Frame-shaped memory is budgeted, not
  elastic; exhaustion is a visible `None`/`Err`, never a hidden growth.
- **The counting wrapper wraps the system allocator directly** — once
  installed, the global dispatch *is* the wrapper, so delegating to the
  dispatch would recurse.
