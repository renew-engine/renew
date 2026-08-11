# The glyph bake

The UI's text face is baked offline into generated sources; the
runtime ships glyph bitmaps and an integer advance table, never a font
file or a rasterizer. This directory is the recipe: the script, the
face's pinned identity, and the record of the run that produced the
committed outputs.

## The face

Arimo Regular, from the croscore release — the Apache-2.0 line of the
family. **The face's identity is pinned by hash and the script refuses
any other bytes**, because the family's later releases moved to a
different licence: any re-bake must use exactly this file, and any
version bump must re-verify the embedded licence before touching the
pin.

- File: `Arimo-Regular.ttf`, 583,364 bytes,
  SHA-256 `a5cb71302ce735698dfc756943c5bbcfbcc734e117ad452ccb3cb036e07dbd36`
- From: `croscorefonts-1.31.0.tar.bz2`,
  SHA-256 `672c3487883ec1ef83d9254240d4327b014212abc823d06d15816095867315e1`,
  fetched from
  `https://commondatastorage.googleapis.com/chromeos-localmirror/distfiles/croscorefonts-1.31.0.tar.bz2`
- Embedded notices: see `NOTICE` beside this file.

## The run that produced the committed outputs

- Tool: Python 3.9.13, Pillow 11.3.0 (bundled FreeType), Windows 10.
- Invocation, from this directory:
  `python bake_arimo.py <path-to-verified-Arimo-Regular.ttf>`
- Parameters (fixed in the script): 13 px, ASCII 32..=126, antialiased,
  one strip one line-height tall.
- Output: `../src/glyphs.rs` (strip 1034×15, 15,510 alpha bytes,
  strip SHA-256
  `7f2d15436dc024596c6c7cee4cd6aa87eac3e13014683d827b03d243e822a901`)
  and `../../ui/src/text.rs` (the advance table).

After a bake, run `cargo fmt --all` from the repository root: the
script emits compact rows and the committed form is the formatted one,
so a verifying re-bake diffs cleanly only after formatting.

A re-bake on a different rasterizer version may produce different
antialiasing bytes; that is a deliberate re-bake, reviewed as a visual
change, never an accident — the committed sources are the truth
between bakes, and the reproducibility test holds the atlas to them.
