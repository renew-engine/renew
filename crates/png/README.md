# renew-png

PNG encoding with no dependencies: RGBA pixels in, the bytes of a file
out. It never touches the filesystem — writing the file is the caller's
business, which is what keeps the encoder a pure function and testable
without one.

## What it is for

A picture a person can look at. Samples draw their world and commit the
result to their README; a debug capture wants the same thing. Encoding a
PNG turns out not to need a compressor: the format's data is a zlib
stream, and deflate's *fixed* Huffman tables are published constants, so
the whole encoder is four chunks, two checksums and a small
back-reference search.

## The charter, so this does not become a junk drawer

**The PNG format, in memory, and nothing else.**

The one direction it may grow is a **decoder for the same format** —
which `renew-asset` names as a missing piece — because encoding and
decoding one format are one body of knowledge, and splitting them puts
the same specification in two crates.

Explicitly out of scope:

- **A second image format.** That is a second crate.
- **Anything that manipulates pixels** rather than framing them: no
  resizing, no filtering, no colour conversion. That belongs to whoever
  owns the pixels.
- **File I/O.** The caller owns the file, for the same reason
  `renew-asset` gives for owning its own.

## What it does

Fixed Huffman codes over a three-candidate back-reference search: the
pixel to the left, the pixel above, and the byte before. Those are what a
rendered picture is made of, and they take a 256×256 flat image from
256 KiB to about **two kilobytes**.

Data without that structure comes out slightly *larger* than raw, because
fixed Huffman spends nine bits on half the byte values. That is the
honest trade: this is an encoder for pictures of geometry, not for
photographs.

No dynamic Huffman, no filtering (every scanline carries filter byte 0),
no palettes, no interlacing, 8-bit RGBA only.

Output is a pure function of the pixels, so the same image encodes to the
same bytes on every platform and every run — which is what lets an
encoded file be compared rather than merely looked at.

## How the format is checked

The tests assert the byte layout against the specification: a
hand-derived single-pixel file, the published check values for CRC-32 and
Adler-32, the block split that only appears past 65535 bytes, the zlib
header's multiple-of-31 rule, and the length and distance symbol tables
against the published ones.

**That is not enough on its own, and the tests say so.** One reading of a
specification wrote both the encoder and the tests, so they agree with
each other by construction. The output is therefore also handed to an
independent decoder — flat, banded, striped and incompressible images,
each checked for exact pixels.

**It caught a real defect.** The length-symbol arithmetic was off by one,
so every match of eleven bytes or more encoded as the wrong symbol. The
file was still small, still structurally a PNG, still had a valid header,
and every test passed. Only a decoder refused it. The symbol tables are
pinned to the published ones now, so the next mistake fails here.

## Errors

`encode` returns `Result`, and the error names which of three caller
mistakes it was: a shape with no pixels, a buffer that does not match the
shape it claims, or an image too large for the format's lengths. It
returned `Option` once, which told a caller that something was wrong and
left them to work out which — and the three call for different fixes.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies and
extension points. Contract lints live in `clippy.toml`: clock reads,
filesystem access and thread spawning are rejected at lint time.
