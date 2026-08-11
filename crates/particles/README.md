# renew-particles

Presentation-side particles: a fixed-capacity pool stepped at the
simulation's cadence, seeded so a replayed trace reproduces the same
picture bit for bit. The manifest in `Cargo.toml` is authoritative for
maturity, dependencies and core status.

- `EffectDesc` / `VelocityCone` — what an effect is, as pure data a
  generic pool interprets. Authored in code in v0; a file format is a
  deliberate later step carrying its own validation obligations.
- `ParticleSystem` — the pool: `burst` spawns (saturating at capacity),
  `step` advances at the fixed cadence, `write_instances` packs live
  particles into 48-byte records with the count and the bytes derived
  from one walk so they cannot disagree.

## Contract

- **Nothing here is simulation state.** No particle value is digested
  and no simulation system reads one back; the flow is one-way, from
  digested observables into the seed and bursts, out to pixels. Floats
  are the medium because nothing flows back.
- **Reproducible anyway.** Same effect, seed, bursts and step count →
  same bytes: a repeated-run test proves it on one machine, and a
  committed hash asserted by the ordinary suite proves it on every
  platform the engine builds for. The update restricts itself to IEEE
  correctly-rounded operations — add, subtract, multiply, divide, min,
  max, square root; no transcendental function runs per particle, which
  is why the cone takes a jitter radius rather than an angle and drag
  is a per-step factor rather than a per-second power.
- **All allocation happens at construction**, gate-tested from the
  first commit; a burst past capacity saturates.

## What this deliberately is not, yet

No renderer — the GPU-facing half (billboard pipeline, atlas, blend
choice) arrives as its own module with its own dependency, and until
then this crate touches no device. No continuous-rate emission, no
per-particle rotation, no tile ranges: each lands with the first
consumer that needs it, because surface built ahead of use is the
pattern this repository's own register warns about. No sorting: the
recommended blend for unsorted batches is additive, which is
order-independent by arithmetic.
