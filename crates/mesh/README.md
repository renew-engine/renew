# renew-mesh

Readers for the mesh files other tools write — STL and PLY so far. Bytes in, validated
geometry out, and nothing else: this crate never opens a file, never
takes a path, and never reads a clock.

```rust
use renew_mesh::stl;

let mesh = stl::read(&bytes)?;
println!("{} triangles", mesh.triangles());
```

## Why a crate that will not read a file

A caller that reads a file owns the file and owns the bound on reading
it. What arrives here is a byte string, which is what lets one reader
serve a file on disk, a member of an archive, and a chunk carved out of
the middle of something larger — and what keeps the whole untrusted-input
surface to one function per format, fuzzable with no filesystem at all.
The lints in `clippy.toml` are the tripwire: the path types and the
whole-file reads are refused at lint time, not by convention.

## What a reader promises

Every reader here answers, one way or the other, for every byte string
it can be handed. It does not panic, it does not read past what it was
given, and when it refuses it says which of the named ways in `MeshError`
the file was wrong, with the numbers and the place. That vocabulary is
[`REFUSALS.md`](../../REFUSALS.md), and a mesh file is where its first
two rules earn their keep: the caller is usually holding a file it did
not produce, exported by a tool it does not own, and "this STL is
malformed" sends that caller to a hex editor for what the reader already
knew.

## What a reader does not promise

**That the geometry is good, only that it is geometry.** Degenerate
triangles, inconsistent winding, a surface that is not closed — all of
those read successfully, because they are facts about a model rather
than about a file, and refusing them would refuse a great deal of real
art. What will not come back is a coordinate that is not a finite
number, because nothing downstream can bound one.

`Mesh::winding_disagreements` counts the triangles whose stored normal
points away from the face their corners wind. That is the most common
thing wrong with a real STL and it is reported rather than refused: a
few in a large model is an exporter's rounding, and *all* of them is a
file with its winding convention inverted, which is worth knowing before
it is drawn inside out.

## STL, and the one hard thing about it

The format has two encodings and a file does not label which it is. A
binary file's eighty-byte header is arbitrary, **so it can begin with the
word `solid`** — and exporters do exactly that, because the header is
often a comment. A reader that dispatches on that word reads binary files
as text and dies on the first byte of the first float. This is the
classic way to get STL wrong.

`stl::read` decides by arithmetic instead: a binary file is exactly
`84 + 50n` bytes for the `n` written at offset eighty, and a text file
has no reason to satisfy that. What is neither goes to the binary
reader, because the commonest way a real STL is wrong is a binary file
cut short, and that deserves an answer naming the count declared and the
bytes that arrived rather than "expected `solid`".

**The format has no magic number**, so "these are not STL bytes" and
"these are STL bytes that were cut short" are the same observation. This
reader does not pretend to the distinction; it reports the more useful of
the two.

## What it deliberately does not do

- **Weld vertices.** STL stores three full corners per triangle with no
  index buffer, so a cube arrives as thirty-six positions rather than
  eight. How near is the same point, and what becomes of the normals of
  the faces meeting there, are decisions for whoever knows the model's
  scale.
- **Correct winding.** Counted, not fixed — see above.
- **Compute a missing normal.** A derived normal in the array is a value
  the file did not contain, and a caller cannot then tell what the
  exporter said from what this crate guessed. Deriving belongs where a
  caller opts into it.
- **Read the attribute word** at the end of a binary record. It has no
  agreed meaning: some tools write zero, some a colour in one of two
  incompatible packings, some uninitialised memory. Reading it would
  mean choosing one of those.

## PLY, and what a self-describing format costs

A PLY opens with an ASCII header that is a **schema**: elements, how
many rows each has, what columns those rows carry and how wide each
column is. The body is rows against it, in ASCII or in binary of either
byte order. That is what makes the format worth reading — a file
describes its own layout, so a reader that follows the header reads
files nobody anticipated — and it is what makes it worth being careful
about, because every one of those numbers is written by whoever wrote
the file and they multiply into an offset.

So the ceilings are on the header rather than on the body: a bound on
how many elements and properties a schema may declare, and on how many
corners a face may name. Without them a twenty-byte header asks for four
billion of either. The binary cursor bounds-checks every read as it
takes it and never computes an offset ahead.

Coordinates are found **by name**, not by position, and everything else
in the file costs its own width in bytes and nothing more. Colours,
confidence values, per-vertex normals and elements this reader has never
heard of are skipped rather than parsed. Files from scanners look like
that; files from modellers do not; a reader that assumed column order
would work on half the PLY files in the world.

`IndexOutOfRange` is **the refusal this format adds over STL**, and it
is the one that matters: STL repeats every corner, so it has no index to
be wrong, while PLY numbers its vertices and a face pointing one past
the end is the difference between a mesh and a read past a buffer.

PLY does have a magic word, so `ply::looks_like` can say "not mine"
where the STL reader cannot.

Faces with more than three corners are fanned from the first, which is
correct for a convex polygon and wrong for a concave one. Real files are
overwhelmingly triangles and quads, both convex; anything more general
needs an ear-clipping pass and a decision about self-intersecting faces,
which is more than a reader should make alone.

## Testing

`tests/stl.rs` provokes every refusal the STL reader can make, checks
both dialects against the shapes exporters actually emit, and sweeps
random noise, single-byte corruptions and every prefix of a good file.
`tests/ply.rs` does the same for PLY, where the header is a schema rather
than a fixed layout, so it also reads every scalar type at its own width,
sign and byte order, and bounds the time a header is allowed to take.
`tests/properties.rs` round-trips generated meshes through both STL
spellings and asserts that every byte string gets an answer.

**Each reader's census is a test, not a comment.** Both suites map every
`MeshError` variant to either a file in the suite that provokes it or a
sentence saying why this format cannot reach it, with no wildcard arm —
so a new variant does not compile until someone says which it is, and a
refusal that stops being reachable cannot sit there unnoticed.

`fuzz/fuzz_targets/stl_read.rs` and `ply_read.rs` take bytes unfiltered —
for STL the dispatch between the two encodings is itself untrusted-input
handling, and for PLY the header is — and each asserts the invariants a
returned mesh carries. `tests/corpus_replay.rs` replays both committed
corpora on stable, on every merge, with a floor on the number of distinct
inputs and on the number of distinct answers they reach, plus four
refusals named individually per format.

**Every fixture is generated.** A model downloaded from a sample
repository would be a dependency with a licence, and a directory of them
would be a dependency nobody recorded — and a mesh, unlike a datagram, is
the kind of file that has an author. `examples/make_corpus.rs` builds the
25 STL seeds and `examples/make_ply_corpus.rs` the 23 PLY ones; between
them every committed seed is built here rather than found.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies and
extension points. Contract lints live in `clippy.toml`: clock reads,
filesystem access, path types and thread spawning are rejected at lint
time.
