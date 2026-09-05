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
- `particles()` — the live pool, particle by particle, in the order
  `write_instances` packs: a `Particle` (centre, velocity, size, colour,
  progress) per entry, exact-size so a caller can size a batch before
  pushing, allocating nothing. The same arrays read by the same progress
  and the same lerps as the packer, held slot by slot by test.

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
- **All allocation happens at construction** — `burst`, `step`,
  `write_instances` and the `particles()` walk, each gate-tested from
  its own first commit; a burst past capacity saturates.

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

## Drawing through a 2D sprite batch

A consumer with its own atlas and its own draw order — a sprite batch
rather than the billboard pipeline — reads the pool through
`particles()` and pushes one of its own sprites per entry. `position`
is the particle's centre, so a square sprite of side `size` sits at
`(x - size / 2, y - size / 2)` with `.size(size, size)`, where `x` and
`y` are the first two components of `position`; the third component
and the sign of the pool's axes are the effect's own — an effect drawn
on a canvas whose y grows downward is authored with its gravity along
`+y`, or the caller maps the axes. `color` is premultiplied and goes
straight into a premultiplied tint (`.tint(color)`); whether a tint can
add light without occluding is the sprite renderer's contract to state,
not this crate's. The source rectangle is the caller's, chosen from its
own atlas (by `progress`, if a flip-book by age is wanted), because
`EffectDesc::tile` is the billboard atlas's and is not carried here;
the painter's order is the caller's. Everything is premultiplied end to
end — a caller that lerps a straight colour of its own and then
multiplies alpha in gets fringes. `velocity` is exposed so a sprite can
be aligned or stretched along its motion; the view costs a walk over
the same arrays the packer walks and writes nothing.

## What this deliberately is not, yet

No continuous-rate emission, no per-particle rotation, no tile ranges:
each lands with the first consumer that needs it, because surface built
ahead of use is the pattern this repository's own register warns about.
Through the view a spin is the caller's to apply to its own sprite
until the pool carries one; a flip and the 2D placement stay the
caller's. No sorting: additive blending is order-independent by
arithmetic, and the alpha mode documents its unsorted artifact where
the choice is made.
