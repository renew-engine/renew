# renew-physics2d

Collision detection and kinematic movement in two dimensions, in fixed point.

**Status: bootstrap.** The public surface is expected to change. There is no dynamics here — no
forces, no mass, no impulses. What exists is the part a character controller needs: a set of
colliders you can query, sweep against, and move through.

## What it does

| | |
|---|---|
| **Bodies and shapes** | A body carries a transform and any number of shapes, each with its own local placement and filter. Boxes and circles are measurable; capsules are accepted and **not implemented** — see below. |
| **Broadphase** | A rebuildable pair list over bounding boxes, inflated by the contact tolerance so a near miss still reaches the narrowphase. |
| **Narrowphase** | Separating-axis tests with incident-face clipping, producing a manifold: one normal, up to two points, each with a depth. |
| **Queries** | Point, ray and overlap, each with a mask and an exclusion list. |
| **Sweeps** | Conservative advancement: a moving shape against a static one, answering when it first touches. It cannot tunnel — that is the property the technique is chosen for. |
| **Slide** | Move a body along a displacement, sliding along whatever stops it, reporting what it hit and which way each surface faced. |
| **Clearing** | Push a body out until nothing is closer than the skin distance. |

## Why it is fixed point

Every number here is `renew-fixed`'s `Fixed` — Q47.16 in an `i64`. Floating-point arithmetic is
denied at the crate root, not by convention but by the compiler.

The reason is reproduction. A simulation that must produce identical results on different machines
cannot afford an addition whose rounding depends on the compiler's instruction selection. Fixed
point makes the arithmetic exact and the overflow behaviour identical everywhere, and pays for it
in range — which is why the bounds are measured and written down rather than assumed.

## Key decisions, and why

**The slide reports ground state; the contact array does not.** A body stopped by a slide rests a
skin distance from what stopped it — far enough that a contact test will not report it. So the hits
the slide writes are the only record that it landed, what it landed on, and which way that surface
faced. A character controller reads its footing from there.

**Touching is not the same as being obstructed.** A body resting against a wall and sliding along
it is in contact for the whole move; reporting that as a blocking hit would stop it dead. A hit
blocks only when the body is not already moving away from the surface. Penetration is the
exception — a body genuinely inside something reports whichever way it is moving.

**The clearing step runs twice per slide, and has its own iteration budget.** Once before the
sweep, because a sweep asks what is ahead and that is the wrong question when the answer is already
touching. Once after, because the slide alone lands slightly inside the skin distance by an amount
that grows with the distance travelled. Re-establishing the clearance directly **removes** that
dependence rather than bounding it; every attempt to bound it failed, because cutting a move into
pieces leaves the proportional part untouched and makes the constant part worse.

The budget is separate from the slide's on purpose. A caller asking for one slide iteration is
asking about the slide, not about how many pushes it takes to re-establish a clearance — and
sharing the two made the shortfall scale with distance while looking like a property of the slide.

**Ties break on the collider, everywhere.** Two surfaces met at the same instant — which is what a
corner is — must resolve the same way on every machine, so every "closest" and "earliest" search
here breaks ties on a stable identifier rather than on iteration order.

**Clearing moves the body and nothing else.** Choosing which of two bodies yields is a question
about mass and kind this crate has no answer for, and splitting the movement between them would
make the result depend on the order they were created in.

## What is not here

- **Capsules.** The shape exists and nothing can measure it. The narrowphase declines such pairs
  rather than reporting that they do not touch, which would be a lie, and the clearing step
  inherits that exactly: a capsule is neither pushed out of anything nor pushes anything out.
  Tests pin this in both directions, so implementing capsules will fail them.
- **Rotation in sweeps.** Shapes rotate; sweeps treat the rotation as fixed for the duration.
- **Dynamics.** No forces, no restitution, no joints.
- **A spatial index that persists between frames.** The broadphase is rebuilt.

## Example

```rust
use renew_ecs::Entities;
use renew_fixed::{Fixed, Vec2};
use renew_physics2d::{BodyKind, Filter, Shape, Transform, World};

let mut entities = Entities::new();
let mut world = World::new();

let floor = entities.spawn();
world.create_body(
    floor,
    BodyKind::Static,
    Transform::at(Vec2::new(Fixed::ZERO, Fixed::from_int(-1))),
);
world.add_shape(
    floor,
    Shape::Box { half_extents: Vec2::new(Fixed::from_int(40), Fixed::ONE) },
    Transform::IDENTITY,
    Filter::new(1, 1),
);

let player = entities.spawn();
world.create_body(
    player,
    BodyKind::Kinematic,
    Transform::at(Vec2::new(Fixed::ZERO, Fixed::from_int(4))),
);
world.add_shape(
    player,
    Shape::Box { half_extents: Vec2::new(Fixed::ONE, Fixed::ONE) },
    Transform::IDENTITY,
    Filter::new(2, 1),
);
```

Then call `move_and_slide` with the displacement, a mask, a skin distance, an iteration limit, and
a slice for it to write hits into. **`SlideHit` has no `Default`**, so the slice is built
explicitly — `samples/leap/world/src/lib.rs` shows a complete call, and is the shortest honest
example of the whole loop.

`samples/leap` is a working consumer: a platformer with running, jumping, walls and ledges,
operable headless and drawable from the command line.
