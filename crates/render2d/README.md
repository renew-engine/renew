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
- **Everything composites premultiplied.** Atlas bytes are authored,
  straight alpha: the hardware decodes them on sample and the fragment
  stage multiplies each texel's colour by its alpha. Tints are
  premultiplied by the caller. The pipeline composites
  `src + dst * (1 - src.a)`. Bytes that break either convention —
  already-premultiplied atlas bytes, a straight-alpha tint — composite
  wrong, visibly, not unsafely.
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
- **Zero rotation and unit scale are exact.** Such a sprite packs the
  corners an axis-aligned sprite always packed, and the vertex stage
  reproduces the previous arithmetic operation for operation, so a
  picture with no turned sprite in it is unchanged by this crate
  having rotation at all. Quarter turns, flips and mirrors are exact
  too, which is what the computed oracles rely on. The sine and cosine
  of a turn are this crate's own — adds, subtracts and multiplies only
  — so a turned sprite packs the same corners on every platform.

## What is here

- `Canvas`, `Region`, `Sprite` — the pure vocabulary: a logical pixel
  space (y down from the top-left), a rectangle of atlas texels, and
  one placed, sized, tinted sprite, mirrored on either axis when asked
  (`flip_x`/`flip_y` — a swap of the sampled edges, so the geometry and
  its winding never move), turned about a fractional pivot and scaled
  about it (`rotation` in turns — a quarter turn is `0.25`, clockwise on
  screen; `pivot`, the centre by default; `scale` per axis, where a
  negative factor is the geometric mirror). A uniform tint
  `[a, a, a, a]` is a fade to `a` of the sprite's opacity: the tint is
  premultiplied, so scaling all four channels is what "`a` as opaque"
  means, and scaling only the fourth would brighten the sprite as it
  faded.
- `AtlasDesc` — dimensions plus **authored, straight-alpha** RGBA8
  bytes: the hardware decodes them on sample and the fragment stage
  premultiplies afterwards, so handing this API already-premultiplied
  bytes double-multiplies them. This crate parses nothing: where the
  bytes come from (an asset pack, a test fixture) is the caller's
  business, and the untrusted-input surface here is zero.
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
carries its own four NDC corners, turned and scaled on the CPU in
canvas pixels before the per-axis ortho map, so a non-square canvas
needs no aspect term and no uniform, matrix, or push constant exists
anywhere in the crate. A consumer that stretches its canvas onto a
surface of another aspect ratio stretches a turned sprite with
everything else, and owns that choice.

## The instance record

Every sprite becomes one 64-byte record: seven attributes, sixteen
`f32`s, native-endian, in the order the vertex stage declares them.

| location | attribute | content |
|---|---|---|
| 0 | `Vec2` | corner a — the local top-left, NDC, after pivot, turn and scale |
| 1 | `Vec2` | corner b — the local top-right |
| 2 | `Vec2` | corner c — the local bottom-left |
| 3 | `Vec2` | corner d — the local bottom-right |
| 4 | `Vec2` | UV at corner a — the region's min, or its max on a flipped axis |
| 5 | `Vec2` | UV at corner d |
| 6 | `Vec4` | premultiplied tint |

The vertex stage selects a corner by a nested mix with weights of zero
and one — along the top edge, along the bottom edge, then between the
two — so for an axis-aligned sprite every position is the arithmetic
the two-corner record used to produce, operation for operation, under
either way a driver evaluates a mix.

`Sprite::instance(canvas, atlas)` packs one without a device, as an
opaque `Instance` whose `bytes()` are what `push` writes when no batch
offset or fade is set — `push` applies the batch state first, then
packs. Public so the packer can be timed and pinned; opaque so the
layout stays the pipeline's: the shader's locations, the layout slice
in the device half and the packer describe the same bytes and change
together.

## Testing

Unit tests pin the maps (all four canvas corners, exact) and the packed
bytes against hand-written records; a property test holds the ortho map
monotone, corner-exact, and invertible over random canvases, and two
more hold the batch fade to the premultiplied rule (every channel by
the same factor, composing to the product, never brightening) and the
batch offsets to adding; a flip swaps exactly the two UV lanes of its
axis and nothing else, pinned lane by lane. The turn is pinned three
ways: the untransformed sprite packs the rectangle bit for bit on a
fixture whose general path would round; quarter and half turns and the
negative-scale mirror permute integer corners exactly; and properties
hold a turn rigid about its pivot, compose two turns into one, and keep
edge lengths on a 640×360 canvas (the mutant that turns in NDC shears
by the aspect ratio). The crate's sine and cosine are exact at every
quarter turn, within two ulps of double precision over a fixed sweep,
and hashed to one constant asserted on every platform. A computed
image oracle proves placement, region selection, fill-order overwrite,
the batch offset, both mirrors, a quarter and a half turn, the
negative-scale mirror, and a diagonal turn's exact 264-pixel diamond
byte-exactly on every adapter; a committed golden proves the
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
