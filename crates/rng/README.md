# renew-rng

Seeded, reproducible pseudo-random numbers for simulation: PCG32 with
per-domain streams derived from one master seed, and bounded draws with no
modulo bias.

Randomness in a deterministic simulation is not a source of surprise — it
is a **function of the seed**, and the whole job of this crate is to keep
it that way. There is no ambient generator, no way to seed from the clock,
and no dependency that could offer either.

```rust
const LOOT: StreamId = StreamId::from_name("loot");     // streams are named where they are used
let mut loot = Rng::new(seed, LOOT);                    // a pure function of (seed, stream)
let mut enemy = Rng::new(seed, LOOT.child(entity_id));  // per-entity, order-independent

let roll = loot.below_u32(TABLE_SIZE);                  // uniform, no modulo bias
let crit = enemy.next_bool();
```

- `Seed` — the master seed for one run. The application supplies it: a
  command-line flag, a recorded trace, a lobby handshake.
- `StreamId` — names one independent sequence. `from_name` for systems
  (computed at compile time, so no central registry of stream numbers has
  to exist), `child(index)` for per-entity sequences.
- `Rng` — the generator. `next_u32` / `next_u64` / `next_bool` for raw
  draws, `below_u32` / `below_u64` for bounded ones, `parts` /
  `from_parts` to snapshot and resume.

Machine-readable facts about this crate — maturity, dependencies, core
status, whether it is simulation code — live in `Cargo.toml` under
`[package.metadata.renew]`, which is authoritative. This file does not
restate them.

## Contract

- **A run is a pure function of its seed.** For a fixed build and
  platform, every number produced is determined by the `(Seed, StreamId)`
  pair it came from and the number of draws taken before it. Nothing here
  reads a clock, allocates, spawns a thread, or holds
  iteration-order-dependent state.
- **Derivation is order-independent.** `Rng::new` is a pure function of
  its arguments — building a stream early, late, or twice gives the same
  generator. A replay can reconstruct one entity's sequence without
  replaying every other entity's.
- **Distinct streams under one seed cannot collide.** The derivation is a
  bijection, so two different `StreamId`s under one `Seed` always start
  from different internal states. What is *not* claimed is proven
  statistical independence between streams — see "What this crate does not
  guarantee" below.
- **Bounded draws are exactly uniform.** Not nearly uniform. Rejection
  sampling with a threshold, so the accepted range is an exact multiple of
  the bound.
- **Nothing can fail.** Non-zero bounds by type, a total constructor: no
  method returns a `Result`, nothing panics, nothing unwinds.
- **No floating point.** Not in the generator, not in the draws, not in
  the tests. The crate denies float arithmetic at its root, so it is the
  compiler that holds the line, not review.

## Why PCG32, and how we know it is PCG32

The generator is the XSH-RR 64/32 variant of PCG: a 64-bit linear
congruential step whose output is a 32-bit xorshift folded down and then
rotated by an amount taken from the state's own top bits. Integer
operations only — multiply, add, shift, xor, rotate — which is what lets
the sequence be described as bit-identical rather than as approximately
identical.

The reason it is this algorithm rather than one of the equally reasonable
alternatives is **evidence**. Its reference implementation ships a
demonstration program with published output, and `tests/known_answer.rs`
reproduces that output exactly: six raw words, sixty-five coin flips,
thirty-three dice rolls and a shuffled deck of fifty-two cards, all drawn
from one continuing stream. Because the stream continues across those
lines, the dice only come out right if the coins consumed exactly the
right number of words first, and the deck only comes out right if both
did.

That matters more than it sounds. A generator with a shift distance off by
one, or a rotation in the wrong direction, is still perfectly
deterministic and still passes every statistical and property test in this
crate — it is simply a *different* generator, one that nobody has
analysed, whose period nobody has proven. Only somebody else's numbers can
tell the two apart.

## Seeding: why the algorithm's own stream parameter is not exposed

PCG's native notion of a stream is the increment of its linear
congruential step. It is real, but it is not independence: two streams
that differ only in that parameter are related. Measured on the reference
seeding, the difference between two streams' internal states is a fixed
constant — the *same* constant for every master seed — and it stays fixed
as both advance.

Callers therefore never touch it. `Rng::new(seed, stream)` folds both
inputs through the SplitMix64 mixer and takes the generator's two words
out of a SplitMix64 walk. Three consequences, all tested:

- **Adjacent inputs decorrelate.** Seeds 1, 2, 3 and entity indices 0, 1,
  2 are what callers actually pass. After mixing, adjacent seeds differ in
  about sixteen of the first thirty-two output bits — half, which is what
  "unrelated" means.
- **Distinct streams cannot collide**, because every step of the
  derivation is invertible.
- **`from_parts` is not a seeder.** It restores a snapshot verbatim, which
  is exactly what a snapshot needs and exactly what a seed must never get:
  a generator whose state is set to a small number emits *zero* as its
  first draw for every value below 2^27. `tests/statistics.rs` pins that
  trap in place, as a reason and as a regression guard.

## What this crate does not guarantee

Stated plainly, because an unstated limit is how a reproducibility claim
quietly becomes false:

- **Statistical independence between streams.** PCG's designers do not
  claim it and neither does this crate. What the mixer buys is that the
  correlations known to exist between this algorithm's streams cannot be
  reached by adjacent or patterned identifiers.
- **Non-overlap between streams.** Two streams that happen to share an
  increment lie on one cycle at a random offset. With a 64-bit period the
  chance of overlap within any realistic run is negligible, but it is a
  probability, not a proof.
- **Cross-platform bit-identity, today.** The generator is integer-only
  and has no reason to differ between targets, and `tests/determinism.rs`
  freezes concrete values so CI checks the claim on every platform it
  runs. Until those runs exist for a platform, the claim is unverified
  there.
- **Cryptographic strength.** None whatsoever. PCG32 is trivially
  predictable from a few outputs. Nothing security-bearing — matchmaking
  tokens, anti-cheat nonces, save-file integrity — may use this crate.

## What v0 deliberately does not have

Each of these is a decision, not an oversight, and each names what would
bring it back:

- **Float draws.** A float in `[0, 1)` can be built exactly from integer
  bits, so the objection is not that it cannot be done — it is that no
  consumer exists yet to state which interval, which precision, and which
  rounding it needs. The recipe lives in the crate documentation until one
  does.
- **`shuffle`, `choose`, weighted picks, distributions.** Reductions with
  no call site. `tests/known_answer.rs` already contains a correct
  Fisher-Yates shuffle over this API, which is the pattern to copy when
  the first caller appears.
- **Signed and offset ranges.** `low + rng.below_u32(span)` is the whole
  implementation; the version that handles every overflow edge belongs
  with a caller who has one.
- **Jump-ahead / stream advance.** Derivation replaces the use case, and
  the jump polynomials would be one more thing to get right with no
  published vector to check them against.
- **Seeding from entropy.** Deliberately impossible: the crate has no
  dependency that could supply it, and its lint configuration bans the one
  self-seeding type the standard library offers.
