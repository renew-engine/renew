# renew-sample-glide-world

The glide game's rules: a bird, gravity, pipes, a score. A pure
fixed-step function of (seed, per-tick input), integer-only, with a
digest folded over every store and scalar each tick.

## Why the game is two crates

This crate declares the determinism marker, so the workspace structure
check refuses it a dependency path to the platform crate — no clock, no
filesystem, no window, at any depth. A game needs all three of those
things, and the driver crate beside it owns them: it maps keys and
scripted traces to the one action, calls `step`, and does every piece
of I/O. The split is what turns "the replay was identical" from a hope
into a machine-checked property.

## Contract

**Output is a function of build, seed and input, and nothing else.**
Two runs with the same seed and the same flap schedule produce
bit-identical digests; a different seed must move the digest, and a
test asserts each direction — the second one exists because a digest
that omits state passes the first check while hashing nothing that
matters.

**The steady-state tick allocates nothing**, held by a
counting-allocator gate. The world owns one scratch allocation for its
whole life.

## Testing

Determinism and discrimination over 3,000 ticks; an entity-slot leak
regression over 60,000 (the feasibility version's slot count climbs
into the hundreds; this holds at or under sixteen); scoring and death
reachability; ticking past death; the zero-allocation gate. Property
tests: N/A — not math, containers or allocators. Fuzz: N/A — no
external data enters this crate; the trace bytes are parsed elsewhere
and arrive as resolved booleans.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies
and extension points.
