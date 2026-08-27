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
- `burst_along` is `burst` with the cone pointed somewhere other than
  the effect's own axis. An effect says *how* matter leaves a surface
  and is authored once; only the caller knows *which* surface, and for
  anything knocked off a face that changes with every burst. `burst` is
  `burst_along` on the effect's axis, held to that by test.

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
  is a per-step factor rather than a per-second power. Aiming a burst
  does not touch this: the jitter is drawn before the axis is used, so
  which axis arrives cannot change how many values the generator gives
  out.
- **All allocation happens at construction**, gate-tested from the
  first commit; a burst past capacity saturates.

## The renderer half, behind the `render` feature

`ParticleRenderer` draws what the pool packed: camera-facing quads
through a billboard pipeline, the camera and its right/up basis pushed
per draw, depth tested without writing so particles respect the world
and leave no footprint for each other. `CameraPush` packs the
ninety-six bytes; `ParticleBlend` chooses additive (light that
accumulates — order-independent, the recommended mode where sorting has
not been paid for) or premultiplied alpha (media that occlude, accepted
unsorted in v0). The atlas bytes are premultiplied RGBA8 — the same
caller obligation every blending path in this engine carries. One
renderer owns one per-frame buffer and yields one item per frame, which
is the rendering crate's contract for per-frame bytes. The feature
exists so a consumer that only steps pools never compiles a graphics
API — the pure half stays device-free.

## What this deliberately is not, yet

No continuous-rate emission, no per-particle rotation, no tile ranges:
each lands with the first consumer that needs it, because surface built
ahead of use is the pattern this repository's own register warns about.
No sorting: additive blending is order-independent by arithmetic, and
the alpha mode documents its unsorted artifact where the choice is
made.
