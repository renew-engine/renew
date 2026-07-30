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

## Ownership and lifetime

The lease model, spelled out: `LinearArena::alloc` returns `&mut T`
borrowing from the arena — a *lease*, alive until the arena is reset
or dropped. `reset(&mut self)` demands exclusivity, so the borrow
checker proves every lease has ended before memory is reused; there
is no runtime tracking because none is needed. `Pool::insert` moves
the value in and returns a generation-stamped `Handle` (plain data,
freely copyable); `remove` moves the value back out. A stale handle —
its slot since recycled — misses by generation instead of aliasing
the new occupant. A full pool returns `Err(value)`: ownership always
lands somewhere visible.

## Thread safety

`LinearArena` is deliberately `!Sync` (interior bump pointer; sharing
an arena across threads is a design error here — give each thread its
own), and `!Send` follows from its raw-pointer storage. `Pool<T>`
follows `T` like any container. `CountingAllocator` and the counter
snapshot are fully thread-safe: monotonic event counters on relaxed
atomics, read independently — coherent enough for diagnostics,
deliberately nothing more.

## Testing note

The allocator regime applies and is in place: property-based tests
over alignment and exact-exhaustion axes, plus the installed-for-real
counting test in its own process. Because this crate carries `unsafe`,
the scheduled checks workflow runs its unit and property suites under
Miri; the counting integration test stays outside the interpreter
(global-allocator installation trips a known std/Miri interaction —
the exclusion and its reason are recorded in that workflow); the
workspace benchmark suite times the arena and pool round trips and
asserts they never touch the heap after construction.

## Status

Settled enough to depend on: these surfaces are what other crates in
this workspace build against, and breaking them is avoided; they still
grow when a consumer needs them — examples and a performance narrative
are still to come. The `[package.metadata.renew]` table in
[Cargo.toml](Cargo.toml) is authoritative for maturity and manifest
metadata. `unsafe` is confined to the allocator internals with the
invariant stated at every block; the arena is `Copy`-only until a
consumer needs drop-aware allocation.

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
