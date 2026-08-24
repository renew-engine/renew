# renew-volume

A finite, chunked three-dimensional grid whose cells hold an opaque
sixteen-bit identifier.

Status: **bootstrap**. Interface churn expected; see the
[manifest](Cargo.toml) for maturity, dependencies and core status.

## What it is for

The storage and the queries a voxel world is actually asked:

- what is in this cell, and is it solid;
- **pick** — walk a ray from cell to cell until it meets something,
  answering which cell and which face;
- **sweep** — move a box until the lattice stops it, answering when, where
  and against what;
- **what changed** — asked two ways, because there are two questions. A
  version per chunk answers *how much* each one moved. A change feed
  answers *which* ones moved since a mark, in time proportional to the
  changes rather than to the world. Neither is consumed by reading, so a
  mesher, a stepper and a saver each keep their own place without agreeing
  with one another about when to forget.

## What it deliberately does not know

**What an identifier means.** Zero is absence; every other value is a
number this crate compares for equality and stores. It has no concept of
stone, of hardness, or of what fire does to anything — those belong to
whoever has the rules, and keeping them out is what lets one volume serve
consumers that disagree about everything else.

It does not step itself, and it does not draw itself. A volume that
stepped itself would have to know what its identifiers mean.

It is also **finite, and says so at its edges**: [`Volume::get`] answers
nothing outside rather than guessing. A volume that answered "empty"
would let a body walk off it and fall for ever with no way to tell that
from a hole; one that answered "solid" would trap it at the boundary with
no explanation.

**What lives below a cell.** A consumer whose world is finer than its
cells — destructible matter, a surface shaped inside the cell it sits in,
a level-of-detail scheme with several resolutions at once — keeps that
detail itself. `Volume::sweep_box_fine` is the seam: it sweeps a box
against a lattice `n` times finer than the cells and asks a predicate
which sub-cells are solid, so the detail never has to be stored here and
this crate never has to have an opinion about what shapes it. At `n = 1`
with the volume's own solidity it is exactly `Volume::sweep_box`, and a
test asserts that rather than the doc claiming it.

## The three decisions worth knowing before you use it

**Cells are centred on integers.** Cell zero spans −0.5 to +0.5, so every
cell's half-extent is exactly one half and the arithmetic stays exact. A
position exactly on a boundary resolves to the higher cell, for both
signs, so two bodies on the same seam agree about where they are.

**Per-chunk hashes are maintained on write, never walked.** Digesting a
volume by visiting every cell is correct and free at a few thousand cells
and impossible at millions. Exclusive-or is its own inverse, so a write
retires the old term and admits the new one, and undoing a write restores
the hash exactly. An empty chunk hashes to zero — **the converse does not
hold** and must not be relied on: the hash is an exclusive-or of 64-bit
terms, so populated chunks hashing to zero exist and can be constructed.
Ask [`Volume::solid_count`] whether anything is there.

**The change feed refuses rather than lies.** [`Volume::changed_since`]
answers from a ring holding one entry per chunk. A consumer that has been
away longer than the ring gets nothing back, meaning *treat every chunk as
changed* — never a partial answer, because a partial answer silently loses
chunks and the consumer has no way to tell. The bound scales the way it
should: a small volume overflows easily and costs nothing to rescan, while
a large one — where the scan is what hurt — gets a proportionally large
ring. Each chunk is named at most once however many times it was written,
so the feed can drive work directly without collecting into a set first.

## Determinism

Every iteration order this crate exposes is stated and stable —
[`Volume::solids`] walks ascending chunk index, then ascending cell index
within a chunk, which is x fastest. No floating point appears anywhere:
positions are `renew-fixed`, and the crate denies float arithmetic at its
root.

The determinism test pins a digest measured on one machine. That is a
**regression guard**, not evidence of cross-platform determinism — the
stronger claim needs the same input replayed on the other targets and the
digests compared against each other.

## Refusals

[`Volume::new`] returns nothing when a request cannot be addressed: more
chunks than the identifier space holds, or an extent that would carry the
highest cell past `i32::MAX`. **A refusal rather than a clamp**, because a
volume quietly smaller than asked for is a world with an invisible wall in
it, and the caller would find out by walking into one.

## Tests

Unit tests beside the code, property tests over arbitrary write sequences,
a determinism test with a pinned digest, and an allocation gate: after
construction, reading, writing, picking and sweeping allocate nothing.
