# renew-ui

The retained widget tree: an arena of generationally addressed nodes,
capacities fixed at construction. This crate is the simulation-side
ground the rest of the UI stands on — layout solving, input handling,
and state digestion arrive as their own steps and all walk the tree
built here.

Machine-readable facts — maturity, dependencies, core status — live in
this crate's manifest metadata (`Cargo.toml`, `[package.metadata.renew]`).

## Shape

Nodes live in one arena, linked intrusively: parent, first and last
child, previous and next sibling. Children keep insertion order —
document order — which is the order every later consumer (layout,
drawing, hit-testing) walks them in. No node owns a collection, so the
steady state allocates nothing: insertion pops a free slot, removal
pushes one back, and the arena never grows. A test holds the tree to
that promise with a counting allocator: after construction, insert,
remove, and walk touch the heap exactly zero times.

## Addressing

A `NodeId` is a slot index plus the generation the slot carried when
the node was created. Removing a node bumps its slot's generation, so
every id that named it goes stale at once and stays stale forever —
the generation is 64 bits, and no physical run recycles one slot often
enough to see it repeat. A stale id is data, not a fault: every
operation given one misses — `None`, `false`, an empty iterator, or
`UiRefused::MissingParent` — rather than panicking or touching the
slot's new tenant.

That promise is scoped to the tree that issued the id. A `NodeId`
carries no memory of its tree: two trees issue the same id sequence,
so an id used on the wrong tree is not detected — it may miss, or it
may name an unrelated node. Holding ids across trees is a logic
error.

## Bounds

`UiLimits` fixes the arena's size at construction (zero clamps to one:
a tree exists to hold at least its root). A full tree refuses insertion
with `UiRefused::Full` and stands unchanged. Nothing here grows, and
nothing here panics on data — the two refusals are the API's whole
error vocabulary, and property tests drive random operation sequences
against the tree's invariants: reachability matches the live count,
children and parents agree, capacity is a wall, and stale ids miss
everywhere at once.
