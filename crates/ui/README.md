# renew-ui

The retained widget tree and its fixed-point layout solver: an arena
of generationally addressed nodes, capacities fixed at construction,
solved into pixel rectangles by a trimmed flexbox subset over Q47.16.
This crate is the simulation-side half of the UI — input handling and
state digestion arrive as their own steps and walk what is built here.

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

## Layout

`solve` turns styles into absolute rectangles, the root filling the
viewport. The v0 surface is deliberately small — row and column
containers; pixel or content-driven sizes; start, centre, and end
placement on both axes; margin, padding, gap; and integer `grow` —
with the rest of flexbox landing when a real document needs it.

Everything is `Fixed` (Q47.16), so a solve is integer arithmetic
under the hood, the same on every target by construction. A test
builds the same tree twice and compares every rectangle to the bit;
the cross-target lane that turns that claim into evidence arrives
with state digestion. Leftover space
among growers is shared by largest remainder over raw fixed-point
units — shares sum to the leftover exactly, property-tested, with
ties breaking toward the earlier sibling. Both passes are iterative
(the tree is data, and data must not choose the stack depth), run in
scratch buffers sized at construction, and re-solving allocates
nothing — the same counting-allocator gate holds both promises.
Solving is retained behind one dirty flag: a clean tree with an
unchanged viewport returns without walking. Exact per-node damage
arrives with the compiled style tables.
