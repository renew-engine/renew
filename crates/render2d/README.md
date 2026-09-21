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
  `src + dst * (1 - src.a)` by default; a renderer built with
  `with_blend` composites that same premultiplied source by the mode
  it names, and every sprite it draws composites that way. Bytes that
  break either convention — already-premultiplied atlas bytes, a
  straight-alpha tint — composite wrong, visibly, not unsafely.
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
  having rotation at all. Quarter turns and the negative-scale mirror
  permute an integer-cornered sprite's corners bit for bit, and a flip
  is a lane swap — which is what the computed oracles rely on. The sine
  and cosine of a turn are this crate's own — adds, subtracts and
  multiplies only — so a turned sprite packs the same corners on every
  platform.
- **A region that is ever turned owes a gutter.** Sampling is nearest
  by default and clamped at the atlas's edge, not the region's, and a
  turned edge resolves to a texel inside the region only up to
  interpolation rounding; so the texels bordering such a region are
  kept transparent for one texel on every side. Axis-aligned sprites at
  texel-aligned sizes never reach a neighbour and need none.
- **Under a linear filter every region owes the gutter, in the art's
  own colour.** `AtlasDesc::filter(Filter::Linear)` blends the four
  texels around a sample, so a sample half a texel from a region's edge
  reaches the neighbour at every scale but exactly 1:1 — hence the
  gutter for all. And the hardware blends the *authored* colour before
  the fragment stage multiplies by alpha, so a gutter authored black
  darkens every edge on its way to transparent; one authored in the
  neighbouring art's colour at zero alpha only fades. There is one mip
  level — the rendering crate creates no others — so art drawn at less
  than half its authored size aliases under either filter.
  `AtlasDesc::address(AddressMode::Repeat)` tiles the atlas where a
  region reaches past it, for a background drawn from one tileable
  texture; a sprite inside the atlas is unchanged by it.
- **Additive light needs no second pipeline — and can have one.** A
  tint whose alpha is zero adds `A·(R·r, G·g, B·b)` to what is
  underneath and occludes nothing — the premultiplied blend is
  `src + dst·(1 − α_src)`, and at `α_src = 0` that is addition with the
  destination's own alpha kept. It is the same arithmetic an additive
  blend state performs, out of the one pipeline this crate builds, and
  it is pinned by `additive_light_is_a_tint_with_no_alpha` on fixed
  points of the transfer function, so the oracle is exact on every
  adapter rather than within a tolerance. A tint with an alpha between
  adds and occludes in proportion. A layer of glows, sparks or light
  shafts that should *all* add — and add their own alpha, which the tint
  trick cannot — is a second renderer over the same bytes,
  `SpriteRenderer::with_blend(…, Blend::Additive)`, composited
  `src + dst` colour and alpha alike; there a tint's colour scales the
  light added, its alpha scales only what the sprite adds to the
  target's alpha, and nothing occludes, so the identity tint is light
  and the batch fade dims it. Two quarter-alpha sprites overlapping
  over black read `2α` there and `2α − α²` under the default, which is
  what `additive_sprites_add_where_premultiplied_sprites_cover` pins. Both
  are order-independent by arithmetic, and exact only up to the
  target's eight-bit storage between fragments: stacked arbitrary light
  can differ by one code with draw order.
- **Greying is not darkening, and the two are different fields.**
  `saturation` moves a sprite toward grey *at its own luminance*, so a
  desaturated sprite keeps the brightness it had; a uniform tint
  `[l, l, l, 1.0]` scales every channel toward black and keeps the
  sprite fully occluding. Reaching for the tint to grey something gives
  a dark sprite, which is why the field exists.
- **A smear never reads a neighbour.** `smear` draws a sprite as the
  average of itself over a displacement — the time-average of a moving
  object across an exposure, which is what motion blur is. The footprint
  grows to hold it, and the eight taps that fall outside the sprite's
  own source rectangle count as **transparent rather than clamping**, so
  the band fades out instead of dragging in whatever the atlas holds
  next door. It is an average, not eight copies at an eighth opacity:
  stacking eight layers at `a/8` never gets back to `a` — an opaque
  sprite composited that way lands on about 0.66 — while the mean of
  eight premultiplied samples keeps the sprite's own opacity where the
  motion covered it. A pixel whose eight taps all land in one
  solid region is byte-exact on every adapter, because the taps are
  summed as a tree and eight identical samples average back to
  themselves without rounding. **Zero smear is exact**: it takes a
  single-sample path and packs two lanes of positive zero, so a picture
  with no smeared sprite in it is unchanged by this crate having smear
  at all. Its ceiling is nearest sampling: a smear spanning `k` texels
  yields at most `k + 1` distinct samples, so at 1:1 art the band steps
  rather than gradates, and over a solid region it is a silhouette
  ghost.

## What is here

- `cut_to` — cut a rectangle to a clip box and cut its source by the
  same linear map, so every surviving pixel samples the texel it would
  have sampled uncut. It takes a rectangle and a source rather than a
  `Sprite`, deliberately: the arithmetic is about a rectangle, and an
  earlier shape that took a sprite made its one caller build one to ask
  a question about a rectangle, which cost 24% of the frame that
  exercises it. A turned or scaled quad is out of scope here — an
  axis-aligned cut of a turned quad takes the wrong pixels and cuts the
  source to match, so a caller that turns its geometry owes the clip to
  that geometry.

- `Canvas`, `Region`, `SubRegion`, `Sprite` — the pure vocabulary: a
  logical pixel space (y down from the top-left), a rectangle of atlas
  texels on texel boundaries, the same rectangle with sub-texel edges
  for a cut or nine-sliced sprite, and one placed, sized, tinted
  sprite, mirrored on either axis when asked (`flip_x`/`flip_y` — a swap
  of the sampled edges, so the geometry and its winding never move),
  turned about a fractional pivot and scaled about it (`rotation` in
  turns — a quarter turn is `0.25`, clockwise on screen; `pivot`, the
  centre by default; `scale` per axis, where a negative factor is the
  geometric mirror). A sprite built from a whole `Region` packs the same
  bytes it packed when the field was integral — both paths take the same
  `as f32` cast, so they agree even for coordinates past 2^24 where that
  cast rounds. ("Converts exactly" would be the stronger and false
  claim: `Region { x: 16_777_217, .. }` widens to `16_777_216.0`.) A
  uniform tint `[a, a, a, a]` is a fade to `a` of the sprite's opacity:
  the tint is premultiplied, so scaling all four channels is what "`a`
  as opaque" means, and scaling only the fourth would brighten the sprite
  as it faded. It can also be greyed toward its own luminance (`saturation`)
  and flashed toward a silhouette of its own alpha (`flash`), each an
  identity by default and each one lane of the record, and smeared along
  a direction in canvas pixels (`smear` — projected onto the sprite's own
  drawn axes, so turning a sprite and its smear together smears along the
  same edge of the art), two lanes and an identity of `[0.0, 0.0]`.
- `AtlasDesc` — dimensions plus **authored, straight-alpha** RGBA8
  bytes: the hardware decodes them on sample and the fragment stage
  premultiplies afterwards, so handing this API already-premultiplied
  bytes double-multiplies them. This crate parses nothing: where the
  bytes come from (an asset pack, a test fixture) is the caller's
  business, and the untrusted-input surface here is zero. How the
  texels are read is the atlas's choice — nearest and clamped from
  `new`, or `.filter(Filter::Linear)` for painted art at non-integer
  scales and `.address(AddressMode::Repeat)` for a tileable one — the
  rendering crate's own `Filter` and `AddressMode`, taken as they are,
  the way `Extent` and `TargetFormat` are.
- `SpriteRenderer` — `new` uploads the atlas and builds the pipeline
  (premultiplied blending; the sampling the atlas asked for) and the
  per-frame buffer; `with_blend` is the same with the compositing mode
  named — the rendering crate's `Blend`, so an additive renderer for
  glows is one more constructor argument, and it is a constructor
  variant rather than an atlas field because the mode is the
  renderer's: two renderers over the same bytes may composite
  differently, and that is how a scene keeps its glows beside its
  sprites; `blend` reads it back. `begin`/`push` fill; `set_offset`
  and `set_alpha` move and fade every sprite pushed after them, so a
  whole group slides or dissolves without the code that builds each
  sprite knowing (the fade scales all four premultiplied channels, and
  `begin` resets both); `item` is the frame's draw, for a pass the
  caller composes with `renew_rhi::color_attachment(clear)`:

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

## Recipes

Seven things consumers ask for, each one field or one choice and each
stated as its arithmetic rather than as a preset:

| effect | how | what it does |
|---|---|---|
| hit flash | `.flash(f)` | toward a silhouette of the sprite's own alpha, so a full flash is white on an opaque texel and stays transparent where the sprite is |
| grey it | `.saturation(0.0)` | toward the sprite's own luminance, keeping its brightness |
| fade it | `.tint([d; 4])` | scales all four premultiplied channels, which is what `d` of its opacity means |
| light | `.tint([r, g, b, 0.0])` | adds and never occludes |
| a layer of light | a second renderer, `with_blend(…, Blend::Additive)` | every sprite in it adds, colour and alpha; overlapping glows brighten instead of covering |
| painted art at any scale | `AtlasDesc::new(…).filter(Filter::Linear)` | blends the four texels around each sample; every region then owes a gutter in its own colour |
| dim the world | one canvas-sized sprite of a solid region, `.tint([0.0, 0.0, 0.0, a])` | darkens everything under a panel in one draw |

The fade multiplies all four channels together because the tint is
premultiplied: scaling only the fourth would brighten the sprite as it
faded. The flash happens before the tint, so a flashing sprite that is
also fading flashes less as it goes — which is what a dying thing
should look like, and is pinned by `a_flash_fades_with_the_sprite`.

## The instance record

Every sprite becomes one 80-byte record: nine attributes, twenty
`f32`s, native-endian, in the order the vertex stage declares them.

| location | attribute | content |
|---|---|---|
| 0 | `Vec2` | corner a — the local top-left, NDC, after pivot, turn and scale |
| 1 | `Vec2` | corner b — the local top-right |
| 2 | `Vec2` | corner c — the local bottom-left |
| 3 | `Vec2` | corner d — the local bottom-right |
| 4 | `Vec2` | UV at corner a — the source's min, or its max on a flipped axis, grown outward by half the smear |
| 5 | `Vec2` | UV at corner d — the other end of the same rectangle, grown the same way |
| 6 | `Vec4` | premultiplied tint |
| 7 | `Vec2` | effect — how much colour survives, then how far toward a silhouette |
| 8 | `Vec2` | smear — the displacement to average over, in UV units |

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
monotone, corner-exact, and invertible over random canvases, two more
hold the batch fade to the premultiplied rule (every channel by the
same factor, composing to the product, never brightening) and the
batch offsets to adding, and two more hold a whole `Region` widened
into a `SubRegion` packing the bytes the integer field packed over the
entire `u32` domain and a fractional source reaching the UVs unrounded;
a flip swaps exactly the two UV lanes of its axis and nothing else,
pinned lane by lane. The turn is pinned three
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
byte-exactly on every adapter. Six more device oracles pin the effect
lanes: additive light exactly (its fixture is built from fixed points of
the transfer function, so no rounding can move a byte), light stacking
without touching alpha, a full desaturation landing on the texel's own
luminance, a full flash on an opaque texel landing exactly on white,
a faded flash letting the background through in proportion, and a full
flash on a half-transparent texel staying half transparent — the last
pinning that the flash targets the sprite's own alpha rather than white,
which every other oracle is blind to because they all flash opaque
texels. Three more pin the two choices a renderer makes at creation,
each against its own two-texel atlas: two quarter-alpha sprites overlap
to `2α` under an additive renderer and `2α − α²` under the default,
eleven codes apart and each within one of the transfer function; a
linear filter reads `34/64` and `30/64` of white at the two pixels
either side of a checker's texel boundary where nearest reads `255`
and `0` exactly, and the picture `new` draws equals the one an
explicitly nearest, clamped, premultiplied renderer draws byte for
byte; and a repeating atlas shows a checker again past its edge where
the clamped one holds the edge texel, on fixed points. A committed
golden proves the
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
