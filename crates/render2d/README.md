# renew-render2d

Batched 2D sprites over the rendering crate: one atlas, one pipeline,
one instanced draw per frame. Fill in canvas space, take the draw as an
item, compose the frame on your own stack, hand it to whichever target
you hold.

## Why it is a crate

Sprite batching is policy over the rendering crate's mechanisms — which
attributes an instance carries, what order sprites composite in, what
convention the bytes obey. Policy is removable; mechanism is not. An
engine build without 2D rendering drops this crate and loses nothing
else, which the build matrix proves by building and testing without it.

## Contract

- **Fill order is draw order.** Sprites composite in exactly the order
  pushed — painter's algorithm. **No sort keys, no batch splitting**; a
  caller that wants order sorts before pushing.
- **Everything is premultiplied.** Atlas texels and tints alike carry
  their alpha multiplied into their color channels, and the pipeline
  composites `src + dst * (1 - src.a)`. Bytes that break the convention
  composite wrong — visibly, not unsafely.
- **All allocations happen at creation.** `begin`, `push`, and `item`
  allocate nothing; a gate measures it over frames it first proves are
  alive and drawing — including the caller-side frame composition,
  which is stack arrays and allocates nothing either.
- **This crate never renders.** `item` returns the rendering crate's
  own draw item and the rendering crate's `color_attachment` the
  matching color attachment; the
  caller composes the pass and the frame itself (the borrows end at
  the render call), and targets belong to the caller. It never touches
  a window, a clock, or the filesystem — the lint file makes the ways
  that stops being true unwritable.
- **Capacity is refused by name.** Pushing past the size fixed at
  creation is a caller sizing bug and fails with a retained assertion
  saying so, never a truncated draw.

## What is here

- `Canvas`, `Region`, `Sprite` — the pure vocabulary: a logical pixel
  space (y down from the top-left), a rectangle of atlas texels, and
  one placed, sized, tinted sprite.
- `AtlasDesc` — dimensions plus premultiplied RGBA8 bytes. This crate
  parses nothing: where the bytes come from (an asset pack, a test
  fixture) is the caller's business, and the untrusted-input surface
  here is zero.
- `SpriteRenderer` — `new` uploads the atlas and builds the pipeline
  (premultiplied blending, nearest/clamped sampling) and the per-frame
  buffer; `begin`/`push` fill; `set_offset` and `set_alpha` move and
  fade every sprite pushed after them, so a whole group slides or
  dissolves without the code that builds each sprite knowing (the fade
  scales all four premultiplied channels, and `begin` resets both);
  `item` is the frame's draw, for a pass the caller composes with
  `renew_rhi::color_attachment(clear)`:

  ```rust
  let color = [renew_rhi::color_attachment(SKY)];
  let items = [renderer.item()];
  let passes = [Pass::new(&color, &items)];
  target.render(&RenderDesc::new(&passes))?;
  ```

The ortho and UV maps run on the CPU at push time — each instance
carries its own NDC rectangle, so no uniform, matrix, or push constant
exists anywhere in the crate.

## Testing

Unit tests pin the maps (all four canvas corners, exact) and the packed
bytes against hand-written records; a property test holds the ortho map
monotone, corner-exact, and invertible over random canvases, and two
more hold the batch fade to the premultiplied rule (every channel by
the same factor, composing to the product, never brightening) and the
batch offsets to adding. A computed image oracle proves placement,
region selection, fill-order overwrite and the batch offset byte-exactly
on every adapter; a committed golden proves the
premultiplied compositing convention on the pinned software-rasterizer
lane, with the same candidate/provenance ritual as the rendering
crate's goldens. Two scheduled facts about the oracles: the computed
image leans on alpha-1 blending degenerating to replacement, and the
first divergence report on any adapter scopes it to the software
rasterizer, no debate; and the planned move to a linear working space
re-decides it entirely — convert to a committed golden or retire. The
allocation gate measures fill-and-render windows it first proves are
drawing. Fuzzing: N/A — no parser; inputs are first-party structs and
trusted first-party bytes.

## Manifest

Machine-readable fields — maturity, dependencies, core status — live in
`Cargo.toml` under `[package.metadata.renew]`, which is authoritative.
