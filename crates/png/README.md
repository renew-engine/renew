# renew-png

PNG with no dependencies, both ways: RGBA pixels in and the bytes of a
file out, or a file in and RGBA pixels out. It never touches the
filesystem — reading and writing the file are the caller's business, which
is what keeps both halves pure functions and testable without one.

## What it is for

A picture a person can look at. Samples draw their world and commit the
result to their README; a debug capture wants the same thing. Encoding a
PNG turns out not to need a compressor: the format's data is a zlib
stream, and deflate's *fixed* Huffman tables are published constants, so
the whole encoder is four chunks, two checksums and a small
back-reference search.

## Reading is the wider half

The encoder writes one shape of file — 8-bit RGBA, fixed Huffman, no
filtering — because that is all a picture of geometry needs. The decoder
has no such freedom: it reads what other tools wrote. So it carries
dynamic Huffman, all five scanline filters, palettes with `tRNS`,
greyscale, and 16-bit samples reduced to eight.

It refuses two things by name rather than mis-reading them: **interlaced**
images, which are a different image layout rather than a different pixel
format, and **bit depths below eight**, which want a bit-unpacker nothing
has asked for. Both are additions here when something needs them, not a
second decoder.

Every chunk's CRC is checked, every declared length is validated against
the bytes actually present, and the decompressor takes a ceiling — a
sixty-byte header can otherwise ask for sixty-four gigabytes.

## The charter, so this does not become a junk drawer

**The PNG format, in memory, and nothing else.**

The decoder this once named as its one permitted direction of growth —
because encoding and decoding one format are one body of knowledge, and
splitting them puts the same specification in two crates — has landed.
That direction is now closed: the charter below is the whole of it.

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

**Every refusal has a file that provokes it, and the list cannot rot.**
`decode` returns one of seventeen named refusals. A test beside it builds
one file for each and asserts that the answers those files reach number
exactly seventeen -- so a validation check that becomes dead code is
caught by its file falling through onto some other refusal, which no
per-refusal test can notice. The guard under that test is a function
matching the error enum exhaustively with no wildcard: it lives inside
this crate, where the enum's openness does not apply, so a refusal added
later stops the file compiling until somebody writes the bytes that
provoke it. That is the check a test outside the crate cannot make --
from outside the wildcard is mandatory, and a new refusal lands in it in
silence.

**Four properties run on generated inputs at every merge.**
`tests/properties.rs` states what must hold for every file in a shape
rather than for files somebody sat down and wrote: what the encoder
writes the decoder returns exactly; every byte string gets an answer and
never a panic; no proper prefix of a file decodes; and one flipped bit
anywhere in a file is refused. The last of those is what makes the chunk
checksums load-bearing rather than decorative -- every byte after the
signature lies in some chunk's length, type, payload or checksum, so a
single bit has nowhere to hide. Each property was probed by deleting the
code it guards, and the one probe that came back green is recorded in
that property's own documentation rather than left to be discovered: the
round trip cannot reach filters one to four, because this crate never
writes them.

**The decoder is fuzzed, and the corpus is generated rather than
collected.** Everything above is about the encoder — the half this crate
writes. The half that reads a file somebody else wrote is checked by
`png_decode`, whose seeds are built by `cargo run -p renew-png --example
make_png_corpus`: every input is first-party, so the corpus carries no
licence question. All but one are written by that program, and the
exception is the subject of the next paragraph. `tests/corpus_replay.rs`
replays them on the stable toolchain at every merge, and asserts two
things a file count cannot. The seeds must still reach at least ten
distinct answers between them, and four specific refusals must stay
reachable by name — the allocation ceiling, the chunk checksum, the zlib
header and the missing terminator. Each of those has exactly one seed, so
losing it means a real defence stops being exercised while the total
barely moves.

**The corpus reaches past what this crate can write.** An encoder-only
corpus would be a monoculture: everything this crate emits is fixed
Huffman, filter zero, colour type 6, depth 8, no palette, so a decoder
seeded only from it is never asked about the dynamic-Huffman header —
the largest parser here — or about filters one to four. One seed is
therefore a file written by a design tool rather than by this program:
the project icon, already in the tree and already a decoder fixture,
which carries dynamic Huffman and all four filter types. First-party, so
the no-borrowed-fixtures rule is untouched.

**What is still unseeded, said plainly.** Palette parsing and the `tRNS`
chunk have no valid seed — the header-only indexed seed refuses before
reaching them, and several refusals have no seed in the corpus -- though
every one of them now has a file in the suite above. The fuzzer can find
its own way there; the corpus does not put it there.

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
