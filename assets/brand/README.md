# renew — brand assets

The mark is a square spiral: a loop that closes one turn and steps inward to begin the
next. It is drawn as a single unbroken stroke, because that is what the engine is — one
loop, run again, each pass starting from the last one's result.

The wordmark is a geometric monoline lowercase built on the same grid: one stroke weight,
one radius family, no typeface dependency (every letter is a path, so it renders
identically everywhere).

The identity is monochrome. No accent color, no gradient on any form — the only tonal
values in the system are the ones in the table below.

## Files

| File | Use |
|---|---|
| `renew-mark.svg` | Mark, `currentColor` — inherits the surrounding text color |
| `renew-mark-dark.svg` | Mark for dark backgrounds |
| `renew-mark-light.svg` | Mark for light backgrounds |
| `renew-wordmark-dark.svg` / `-light.svg` | Wordmark alone |
| `renew-lockup-dark.svg` / `-light.svg` | Mark + wordmark, horizontal |
| `renew-banner.svg` / `.png` | 1280×640 — repository social preview |
| `renew-banner-wide.svg` | 1200×300 — README header |
| `renew-icon.svg` / `renew-icon-512.png` | App icon and favicon (self-contained tile) |

The PNGs are rasterized from the SVGs of the same name; regenerate them rather than
editing them, and rasterize with grayscale antialiasing (in Chrome, `--disable-lcd-text`)
so subpixel rendering doesn't leave colored fringes in a monochrome asset.

## Values

| Token | Value | Use |
|---|---|---|
| Ink | `#0A0A0A` | Primary background |
| Tile | `#1B1B1B` → `#0A0A0A` | Icon tile, vertical |
| Paper | `#FAFAFA` | Light background |
| Foreground | `#F5F5F5` | Wordmark and mark on ink |
| Muted | `#8C8C8C` | Taglines, secondary type |

Every value is neutral gray by construction — no hue bias. Depth on the banners comes from
a white radial glow at 8% and a black vignette, never from color.

## Geometry

Both forms are drawn on integer grids so they can be redrawn from scratch.

- **Mark** — 64×64 box, ink from 6.5 to 57.5. Stroke 7, butt caps, round joins. Outer
  corner radius 12, inner 8. Track spacing 14, i.e. exactly one stroke of clearance
  between the outer ring and the inner return.
- **Wordmark** — x-height 48 (top 47.5, baseline 104.5, outer edges), stroke 9, butt caps,
  miter joins. Round letters: radius 24. Arches: radius 22. The `r`'s shoulder is radius 24
  too, springing from the stem at y 76 — the bowls' own radius and center height, so the
  whole wordmark is built from two radii. The `w`'s vertices are placed so its miter tips
  land on the x-height and baseline, not past them.
- **The `e`** — the crossbar overshoots to x = 100 and is then clipped to its own bowl
  (`circle r=28.5` at the bowl center, i.e. the bowl's outer edge). Ending the bar at the
  bowl's centerline leaves a step; ending it at the outer edge leaves a flat rectangular
  tip on a circular silhouette. Clipping it to the bowl is what makes the outer contour a
  true, uninterrupted circle, with the aperture cutting in horizontally beneath the bar.
- **Lockup** — the mark scaled to 0.68, the wordmark to 0.5, so both carry a stroke of
  ~4.8; the gap between them is 15 units at that scale. Tracking after the `r` is 10, not
  14 — its arm overhangs an open counter, which already supplies the air.

## Copy

The approved lines, exactly as set on the banners:

| Line | Where |
|---|---|
| `AN AI-FIRST GAME ENGINE IN RUST` | 1280×640 social preview |
| `AI-FIRST · DETERMINISTIC · MODULAR` | 1200×300 README header |

Set in uppercase, letter-spaced, in Muted — never in the drawn wordmark's letterforms. The
wordmark carries the name and nothing else.

## Usage

- Keep clear space of at least half the mark's height on every side.
- Minimum sizes: mark 16 px, lockup 110 px wide, icon 16 px.
- Do not recolor the wordmark per-letter, add effects, outline it, or set the name in
  another typeface next to the mark — the wordmark *is* the typography.
- On photography or busy surfaces, use the solid `renew-icon.svg` tile rather than the
  bare mark.
