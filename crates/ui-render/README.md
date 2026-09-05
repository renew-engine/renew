# renew-ui-render

Presentation for the widget tree: retained snapshots blended at
display rate, clipped on the CPU, emitted as sprites through the 2D
renderer. The tree solves at the fixed timestep; frames arrive faster;
this crate is the seam between the two.

Machine-readable facts — maturity, dependencies, core status — live in
this crate's manifest metadata (`Cargo.toml`, `[package.metadata.renew]`).

## The snapshot pair

`UiPresenter::advance` captures the solved tree once per simulation
tick; every frame until the next capture blends the two most recent by
the ratified interpolation factor. The blend is keyed by
(slot, generation), so a node is only ever blended with *itself*: a
recycled slot's new tenant never inherits the old tenant's motion.
Nodes with one known tick draw unlerped at it — a newborn at the
current tick, a dying node once more at the previous, underneath the
living — so nothing vanishes mid-blend; what can change abruptly is
stacking, never existence.

## Frames as data

`UiPresenter::frame` answers one frame as an iterator of quads —
position, size, source rectangle, premultiplied tint — with clipping
applied and invisible quads dropped. Quads are in the solver's pixel
space, so the sprite renderer's canvas must match the viewport the
tree solves at, and one frame can hold up to `max_quads` of them —
twice the node limit, because a bulk replace draws every dying node
once more under every newborn. Every decision the presenter makes is
observable there without a device, which is where the unit and
property tests hold it; `emit` is a thin adapter pushing each quad as
a sprite. Clipping is rectangle intersection against the ancestor
chain, computed at capture and blended with the rest, and the quad's
source rectangle is cut by the same linear map, so every surviving
pixel samples the texel it would have sampled uncut. The one sampled
atlas region is still a uniform white texel, so no committed picture
moves yet.

## Text

The atlas carries a baked typeface: Arimo Regular (the Apache-2.0
croscore release) rasterized offline at one fixed size into a glyph
strip below the tile row — the runtime ships bitmaps and integers,
never a font file or a rasterizer. The recipe, the face's pinned
identity, and the licence notices live in `bake/`. `emit_text` draws
a line of glyph sprites advancing by the same integer table the
simulation measures with, so a label is exactly as wide as the tree
believed; bitmaps sit a fixed bearing to the pen's left, so ink may
reach that margin past either end of the measured box — bearings and
antialiasing live there, as type does. Characters outside the baked
range draw and measure as the question mark.

## Atlas and evidence

The atlas is generated — a white fill tile and a chrome border tile
reserved for later — and tested against its own regions. A computed
image oracle draws a solved tree offscreen and compares byte-exactly
against pixels the test derives from the solver's own answers,
inheriting the 2D renderer's exactness argument and its recorded
sunset (the move to a linear working space re-decides the test). After
construction the presenter allocates nothing: capture and frame churn
run inside a counting-allocator window on every push.
