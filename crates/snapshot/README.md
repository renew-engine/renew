# renew-snapshot

Two captures of one slot space, and the rule that decides which pairs may be blended.

**Status:** `bootstrap` · optional. Interface churn expected; breaking its API costs nothing yet.
The manifest in `Cargo.toml` is authoritative for maturity, dependencies and core status.

## What it is for

A producer steps at a fixed timestep; frames arrive faster. Drawing the current state on every
frame means the picture only moves when the producer does — positions repeat for a few frames and
then jump. The fix is to keep the last two states and draw between them.

The part that is easy to get wrong is identity. A dense slot that a producer frees and hands to
something else is the same *index* holding a different *thing*, and blending across that pairing
streaks the newcomer out of its predecessor's last position. So every value is keyed by
`(slot, generation)`, and a pair blends only when both halves agree:

```rust
use renew_snapshot::{Key, Snapshots};

let mut pipes = Snapshots::<f32>::new(16);

// Once per executed producer step:
let mut capture = pipes.capture();
capture.put(Key::new(3, 0), 320.0);
drop(capture);

// Once per frame, with the loop's interpolation factor:
for drawn in pipes.frame(alpha) {
    // drawn.value is already resolved — blended, or standing at its
    // one known tick. There is no call site here that could blend
    // across a recycled slot.
}
```

## Key decisions, and why

**The container blends; the consumer never does.** `frame` hands back resolved values rather than
a "here are the two endpoints, you decide" pair. A consumer holding both endpoints is a consumer
that can blend the wrong two, which is the one mistake this crate exists to prevent. Making it
unspellable is stronger than documenting it.

**The tick boundary is the container's guarantee, not the payload's.** At `Alpha::ZERO`, `frame`
returns the earlier capture bit for bit and never calls `Blend::blend`. Committed images and
computed-pixel oracles are rendered at tick boundaries, so this is the case that must not move,
and resting it on every payload implementing lerp exactly would be resting it on nothing.

**Keys are plain integers, not an entity handle.** The crate names no storage type, so a renderer
can depend on it without an entity-storage crate appearing in the renderer's build graph.
Widening a narrower generation counter at the fill site is lossless and costs the producer one
`u64::from`.

**The reset is by outgoing order, not a full clear.** Clearing every entry each step is
O(capacity) against a slot space that typically never shrinks. Instead each capture clears exactly
the slots the previous one wrote. The invariant is *every slot outside `order` has
`present == false`, and its stale value is unreachable* — inductive, because construction defaults
them all and each reset clears exactly the set the previous reset let through. It is load-bearing
rather than an optimisation: both passes cross-read the other buffer, so a slot live two steps ago
and absent since would otherwise still read as present and wrongly suppress a dying row.

**Capacity is fixed, with `resize` as the named escape.** The steady-state frame loop performs no
heap allocation, so growth-on-demand inside the loop is not available. The budget is the
consumer's obligation; a producer whose live set is bounded by its own rules asserts that bound in
a test, and `put` refuses by name rather than as a bare slice index if the rules ever outgrow it.

## Limits, stated rather than implied

- **Only what is captured through it interpolates.** A renderer packing instances straight from
  current state is unaffected by this crate existing.
- **A singleton does not want this.** A camera, or anything whose identity is the session, wants
  one `Option<T>` and one blend — `None` is the newborn rule already expressed in the type system.
  A key is needed exactly when a slot can be recycled.
- **Generations are assumed not to wrap.** A slot reused 2^64 times would alias its own past. No
  producer in this tree can reach that; it is stated because it is untestable.
- **`Fate::Dying` is reported, never acted on.** A painter's-order pass draws it once more
  underneath the living so nothing vanishes mid-blend; a depth-tested pass drops it with a
  one-line filter. This crate does not know which it is talking to.

## Testing

Unit tests pin the named cases; `tests/properties.rs` pins the rules those cases are instances of;
`tests/zero_alloc.rs` holds the allocation contract with a counting allocator, exercising the
recycle path *inside* the measured window.

The pair that matters most is `a_recycled_slot_never_inherits_the_dead_tenants_motion` and
`a_survivor_at_the_same_slot_and_generation_does_blend`. They ship adjacent and are named as a
pair on purpose: alone, the first passes against an implementation that never blends anything at
all.

No fuzz target: the testing table's fuzz row is for parsers of external data, and this crate
parses nothing.
