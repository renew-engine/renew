# renew-diag

Diagnostics core: log records, severity levels, and the sink interface the
rest of the engine reports through. Code emits records via the level
macros; one `Sink`, installed at startup, receives them. Concrete sinks
live outside this crate and own their formatting, buffering, timestamps,
and output.

```rust
renew_diag::info!("frame {frame} took {nanos}ns");
renew_diag::error!(target: "renderer", "device lost: {cause}");
```

## Contract

- **The emit path performs no heap allocation.** Records borrow their
  message; formatting happens in the sink, into storage the sink owns.
  Enforced by a counting-allocator test, not by convention.
- **This crate never reads a clock and never touches the filesystem.**
  Records carry no timestamp; sinks stamp records on write from their own
  time source.
- **`install` is called at most once**, during process startup before
  other work begins. A second call is a contract violation — fatal in dev
  builds; in release the first installation stands.
- **Without an installed sink, emitting is a silent no-op.** Diagnostics
  must never become a crash source.

## Architecture and public API

Three small pieces and a slot: `Level` (five severities, ordered,
`as_str`), `Record<'a>` (level + static target + borrowed
`fmt::Arguments` message — no owned strings anywhere), and `Sink`
(the one extension point: `fn write(&self, record: &Record)`). `install`
writes a `&'static dyn Sink` into a process-wide write-once slot; the
free function `emit` reads the slot and forwards the record to it. The macros (`trace!` … `error!`, plus the `log!`
back end they share) construct a `Record` and call `emit` — nothing
else. There is deliberately no formatting, filtering, buffering, or
level-masking layer in this crate.

## Thread safety and ownership

`Sink` requires `Sync`: `emit` may be called from any thread, and one
sink instance serves them all — a sink handles its own interior
synchronization. The slot is write-once (first `install` wins;
enforced), so readers never observe a change. The sink is `'static`
and borrowed, never owned: the crate stores a reference, allocates
nothing, and drops nothing — whoever creates the sink (usually a
`static`) keeps it alive for the process lifetime by construction.
`Record` borrows its message for the duration of one `emit` call and
is neither stored nor sent across threads by this crate.

## Testing note

This crate is neither math, container, allocator, parser, threaded
system, nor simulation code — the specialized test regimes those
categories require don't apply. Its obligations are the unit suite,
the allocation-free proof (counting-allocator test), and the
process-isolated install/emit integration binaries, all present.

## Status

Settled enough to depend on: the record shape and macro surface are
what other crates in this workspace build against, and breaking them
is avoided; examples and a performance narrative are still to come.
The `[package.metadata.renew]` table in [Cargo.toml](Cargo.toml) is
authoritative for maturity and all manifest metadata. The crate's
contract lints live in [clippy.toml](clippy.toml): clock and
filesystem access are rejected at lint time, not by review.

## Key decisions

- **Sinks own time and I/O.** Keeping clocks and files out of this crate
  keeps it dependency-free at the root of the crate graph and makes its
  behavior identical in every build configuration.
- **`Sink` is the extension point.** One process-wide sink, registered
  once through a write-once slot (`&'static dyn Sink`; no allocation);
  fan-out, filtering, and buffering are sink concerns.
- **Assertions are not wrapped.** Contract violations use the standard
  assertion macros directly; this crate documents that policy rather than
  adding a layer over it.
- **No runtime level filtering here (yet).** The sink decides what to
  keep; compile-time level switches can arrive later without breaking the
  macro surface.
