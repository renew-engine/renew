# renew-physics3d

Collision detection and kinematic movement in three dimensions, in fixed point.

**Status: bootstrap.** The public surface is expected to change. No dynamics — no forces, no mass,
no impulses.

**Axis-aligned only, deliberately.** `Transform` here carries a translation and nothing else. That
is not an oversight and not a stub: a rotation in fixed point needs an orientation type that
composes without drifting, and that type does not exist yet. A `Transform` with a rotation field
nothing respected would be worse than one that does not offer it, because callers would set it and
be quietly ignored.

## What it does

The same surface as `renew-physics2d`, one dimension up: bodies with shapes and filters, a
broadphase over bounding boxes, separating-axis narrowphase, point and ray and overlap queries,
conservative-advancement sweeps that cannot tunnel, `move_and_slide`, and `clear_of_geometry`.

Shapes are **boxes and spheres**. Every pairing of them is measurable, so nothing is silently
skipped — unlike the two-dimensional crate, which accepts a capsule it cannot measure and says so.

## Why it is fixed point

Every number is `renew-fixed`'s `Fixed`, and float arithmetic is denied at the crate root by the
compiler rather than by convention. A simulation that must reproduce on different machines cannot
afford an addition whose rounding depends on instruction selection.

Three dimensions costs range: the coordinate bounds are tighter than the two-dimensional ones
because a squared distance sums three terms instead of two. Those bounds are measured and written
down rather than reasoned about, in the fixed-point crate's own bounds harness.

## Key decisions, and why

Most of them are the two-dimensional crate's, for the same reasons — read that README for the full
argument. The ones specific to three dimensions:

**The clearing step has its own iteration budget, and finding out why is worth recording.** A slide
hands `clear_of_geometry` a fixed budget rather than its own iteration limit. Sharing them meant a
caller asking for a single slide iteration got a single push, and one push lands short by whatever
the separating direction's rounding costs — with a remainder that grows with how deep the body was.
Depth after a one-iteration slide grows with distance travelled, so the shortfall correlated with
distance and looked exactly like accumulated drift. It was not: it was a budget of one.

The two-dimensional crate had the same latent case and was passing only because its arithmetic
converges in a single push where three dimensions does not.

**A three-way corner needs more than one push.** Coming out of one face can put the body inside
another, which is why clearing iterates and reports how many pushes it took and whether it
finished. Where geometry genuinely has no room — a body wider than the slot it is in — it says it
ran out rather than claiming success.

**The clearance is verified against arithmetic outside the engine.** The tests compute box
separation from centres and half-extents themselves, so a slide that stops in the wrong place
cannot also be the thing that rules the place was fine. That oracle has its own test, being the
one piece of reasoning nothing else checks.

## What is not here

- **Rotation.** See above. Sweeps, queries and manifolds all assume axis-aligned boxes.
- **Capsules or convex hulls.** Boxes and spheres only.
- **Dynamics**, and **a persistent spatial index** — the broadphase is rebuilt.

## Consumer

`samples/cube` is a working consumer: a voxel world with a walking, jumping, block-breaking player
in a closed arena, operable headless and drawable as two slices from the command line.
