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

## Status

Early-stage: the record shape and macro surface may still change without
a deprecation cycle. The `[package.metadata.renew]` table in
[Cargo.toml](Cargo.toml) is authoritative for maturity and all manifest
metadata. The crate's contract lints live in [clippy.toml](clippy.toml):
clock and filesystem access are rejected at lint time, not by review.

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
