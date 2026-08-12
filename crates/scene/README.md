# renew-scene

Local placements, parenting, and a deterministic pass that composes them into world placements.

**Status:** `bootstrap` · optional. Interface churn expected; breaking its API costs nothing yet.
The manifest in `Cargo.toml` is authoritative for maturity, dependencies and core status.

## What it is for

Things in a world are usually placed relative to other things: a turret on a tank, a lamp on a
post, a platform on the arm that swings it. Writing down where each one *is* means rewriting all
of them whenever the thing underneath moves, and getting one of them wrong looks exactly like a
physics bug.

So a node stores where it sits relative to its parent, and the world placement is derived:

```rust
use renew_ecs::{Entities, Store};
use renew_fixed::{Angle, Fixed, Vec2};
use renew_scene::{Global, Local, Parent, Scratch, propagate};

let mut entities = Entities::new();
let (mut parents, mut locals, mut globals) = (Store::default(), Store::default(), Store::default());

// A hub, turned a quarter turn, with a child one unit along its x axis.
let hub = entities.spawn();
locals.insert(hub.index(), Local::new(Vec2::ZERO, Angle::QUARTER));

let arm = entities.spawn();
locals.insert(arm.index(), Local::new(Vec2::new(Fixed::ONE, Fixed::ZERO), Angle::ZERO));
parents.insert(arm.index(), Parent(hub));

let mut scratch = Scratch::new();
let counts = propagate(&mut scratch, &entities, &parents, &locals, &mut globals);
assert_eq!(counts.nodes, 2);

// The child swung round with its parent rather than staying on the x axis.
let placed: Global = *globals.get(arm.index()).expect("placed");
assert_eq!(placed.translation(), Vec2::new(Fixed::ZERO, Fixed::ONE));
assert_eq!(placed.rotation(), Angle::QUARTER);
```

That last assertion is the whole reason the crate exists. A child that only ever *translated* with
its parent could be computed by adding two vectors at the call site; rotating the offset into the
parent's frame is the part that has to live somewhere and be tested.

## What it promises

**Total.** Every *live* entity with a `Local` gets a `Global` — including nodes whose parent was
despawned, nodes whose parent is not a scene node, and nodes inside a loop. There is no live input
for which a node is silently skipped. A despawned entity is not walked at all, and keeps whatever
`Global` it last had; see *Not here* below. `Propagated` counts each category so a caller can assert
`orphaned + cyclic == 0` and hear about the mistake at the point it was made.

**Exact rotation.** Angles are binary-angle integers and compose by wrapping addition, so a chain
of a hundred rotations lands on exactly the angle one multiplication would give. Translation
composes in fixed point and saturates rather than wrapping, and `renew_fixed::saturations()`
counts it when it does.

**Derived, never authored.** `Global` has no public fields, and no public way to build one *from a
placement* — no constructor taking a translation and a rotation, and deliberately no `Default`,
since a derived one is a public constructor reachable through every `unwrap_or_default` in every
consumer. `Global::IDENTITY` names one value, the world origin. Everything else exists because
`propagate` wrote it, which makes "a world placement is a function of the hierarchy" a property of
the type rather than a convention anyone can forget.

**No steady-state allocation.** `Scratch` is caller-owned capacity. Once it and the output store
have grown to fit the world, propagating allocates nothing, and a counting-allocator gate holds
that against a window that moves and re-parents the hierarchy every tick.

## Two mechanics that look like details and are not

**A parent is a whole handle, not a slot.** `Parent` stores an `Entity` including its generation,
and the pass checks it. Slots are recycled; a bare index would silently re-attach an orphaned
child to whatever entity moved in next. The check is against the `Entities` passed to `propagate`,
and generations are unique only within one allocator — a handle minted by a *different* `Entities`
whose slot and generation both happen to match will be obeyed. Nothing in the engine hands entities
from two allocators into one world, and this crate does not defend against it.

**Resolution order is not slot order.** The entity allocator hands out recycled slots
newest-first, so a child can hold a *lower* slot than its parent. A single ascending pass would
then compose that child against its parent's *previous-tick* placement — one frame of lag, on some
entities, depending on spawn history, and perfectly reproducible, so no determinism test would
ever see it. The pass climbs each node's ancestry first and composes on the way back down.

## Determinism, and the one place it stops

For a hierarchy without loops, the output depends on the shape of that hierarchy and on nothing
else. The same shape laid into a different set of slots produces the same placements bit for bit,
which is held by a property test that permutes the slot assignment and compares.

**A loop is the exception, and it is stated rather than hedged.** Every node still gets a
placement and `Propagated::cyclic` still reports the loop. The cut falls on the member the climb
reaches **last** — the one whose own parent is the member the climb entered the loop by — and which
member that is follows from which node the pass seeded from, which is entity order. Relabel the
world and a different member may be cut. No traversal order fixes this — a loop has no member that
a hierarchy can call first — so a caller who needs reproducible placements must not build one, and
the count is how they find out they did.

## Where it is used

`renew-sample-leap-world` drives its moving platforms this way: a hub turns, a deck hangs off it
at a fixed arm, and the deck's collision transform is whatever composing those two gives. Nothing
in that sample writes a deck position down, which is what makes it evidence — and its tests hold
the collider against the composed placement every tick, so the two cannot drift apart unnoticed.

That evidence is in the world crate's tests rather than on screen. The playable level contains no
moving platform, because the sample draws with a grid of characters and has no rule for what a
rotated box looks like in one.

## Testing

Property tests rather than examples, because the pass is a relation over arbitrary trees: an
independent top-down reference the traversal shares no code with, a relabelling test that permutes
the slot assignment and compares bit for bit, and the tree-count relation. A counting-allocator gate
holds the steady-state claim, over a window that moves and re-parents the hierarchy every tick and
whose chain is laid out so the climb runs its full depth.

No fuzz target: that obligation is for parsers of external data, and this crate parses nothing —
its inputs are components another crate's storage already validated.

No state-hash test: the determinism obligation is discharged by the relabelling property above,
since this crate holds no state of its own between calls. `Scratch` is capacity, not state, and a
test asserts that a buffer which has just served a different world answers identically.

## Not here

**No transform matrices, no 3D, no scale.** Two dimensions, a translation and a rotation, because
that is what the one consumer needs and every unused axis is a thing to keep correct for nothing.

**No change tracking.** The pass composes the whole hierarchy each call. A dirty-flag scheme would
be measured against this one before it replaced it, not instead of it.

**No despawn cleanup.** Despawning an entity does not remove its components — true of every store
in this engine, and scene is not special. A despawned node is not walked, so its `Global` simply
stops being updated; it is never read as anybody's parent, because the handle check rejects it
first. The same is true of a live node whose `Local` was removed.
