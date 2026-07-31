# renew-ecs

Entity handles and component storage. A sparse set, chosen by measurement,
with an iteration order that is part of the contract rather than a
consequence of the layout.

## Contract

- **Every query iterates in ascending entity-slot order.** A system's
  result cannot depend on the order components happened to be inserted or
  removed in. That is the point of the whole design: it makes the
  determinism invariant structural, rather than a rule every future
  contributor has to remember and every reviewer has to catch.
- **A stale handle is dead, not dangerous.** An entity is a slot plus a
  generation. Reusing a slot bumps its generation, so a handle to a
  despawned entity reports as not alive instead of quietly naming whoever
  took its place.
- **Ordered iteration costs the gaps.** It walks slots, so it is
  proportional to the highest occupied slot rather than to the number of
  components. The allocator reuses low slots first to keep that range
  tight, and `Store::iter_unordered` exists for systems that genuinely do
  not care about order — its name is the warning.
- **Nothing here reads a clock, opens a file, or spawns a thread**, and
  the crate's [clippy.toml](clippy.toml) rejects all three at lint time.
  `HashMap` is banned for the same reason: its hasher is seeded per
  process, so iterating one would make the order differ every run.

## Architecture

Three arrays per component type. `sparse` maps an entity slot to a dense
position, `dense` maps back, and `values` sits alongside `dense`. Insert
and remove are constant time; remove swaps the last element into the hole,
which keeps `values` contiguous.

That swap is why order is not free. After churn, `dense` is in no useful
order, so a query walking it would visit entities in an order decided by
their removal history. `Store::iter` walks `sparse` instead, which is
ascending by construction.

## Public API

`Entities` for spawn, despawn, `is_alive` and an ordered walk of the live
set. `Store<T>` for insert, remove, get, `get_mut`, `contains`, and the two
iterators. `join` for the ordered intersection of two stores — it walks one
side and probes the other, which is what a sparse set is good at.

A caller holds one store per component type explicitly. **There is no type
map**, and that absence is deliberate: asking a world for `Store<Position>`
by type needs a design for how systems declare what they touch, and there
is no system yet to design against.

## Thread safety and ownership

No shared state, no interior mutability, no globals. `Store<T>` and
`Entities` are `Send` and `Sync` exactly when their contents are; nothing
here synchronises, because nothing here is shared. Components are owned by
their store and borrowed out; despawning an entity does **not** remove its
components — nothing walks every store on despawn, and pretending
otherwise would be hidden behaviour. A caller filters against `Entities`.

## Testing

Unit tests for each operation, including the swap-remove back-pointer fix
that only misbehaves when the removed element is not the last one — the
classic bug in this structure.

Beyond that, model-based property tests: a `Vec<Option<T>>` is the
obvious, slow, obviously-correct version of the same thing, and both run
against the same random operation sequence with every slot compared after
**every step**. The failures in a sparse set are all about history, so a
divergence caught at step 3 names the operation; one caught at the end
names only the sequence.

The determinism property is tested directly: two stores reaching the same
contents by different histories iterate identically, and iteration is
sorted whatever the churn.

## Status

`bootstrap`. Storage and handles are settled; everything above them —
systems, scheduling, a type map — is not. The `[package.metadata.renew]`
table in [Cargo.toml](Cargo.toml) is authoritative for maturity.

## Key decisions

- **Sparse set over archetype, decided by measurement** rather than
  preference, with an ordered-iteration cost of ~1.7% of a 16.7 ms frame
  at 100,000 entities against ~16% for the alternative. The archetype
  layout wins dense iteration by about 0.3% of a frame; that is the price
  of this choice and it is measured, not estimated.
- **A defined order, rather than forbidding order-dependence.** The
  cheaper option was to declare order-dependent queries a mistake and rely
  on review. Defining the order costs runtime and buys a property nobody
  has to remember.
- **`u32::MAX` as the absent sentinel**, not `Option<u32>`: four bytes per
  slot rather than eight at this alignment, on an array that is as long as
  the highest slot ever used.
- **Despawn does not cascade.** Removing an entity's components would mean
  walking every store, which the crate cannot do — it does not know what
  stores exist. A caller filters, and the alternative is a type map that
  has not been designed.
