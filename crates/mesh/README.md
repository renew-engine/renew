# renew-mesh

Readers for the mesh files other tools write — STL, PLY and OBJ so far, plus the
MTL material libraries an OBJ refers to, the binary glTF container and the glTF documents that travel on their
own, and a
canonical blob for keeping what was read without parsing the original again.
Bytes in, validated geometry out, and nothing else: this crate never opens a
file, never takes a path, and never reads a clock.

That last promise is why OBJ's `mtllib` is read as a name and not
followed: a material library is a second file, and naming one is as far
as a reader that cannot open anything is able to go. `obj::materials`
hands those names back, as an entry point of its own rather than a
second return value from `obj::read` — the two questions have different
callers, and one of them wants to know what a file depends on before
deciding whether to load it at all.

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

## Contract

- **No file is ever opened.** Readers take bytes and the writer returns
  them: no path, no handle, no clock. The caller that reads a file owns
  the file and owns the bound on reading it, which is what lets one
  reader serve a file on disk, a member of an archive and a slice out of
  the middle of something larger.
- **No input is trusted, and nothing is repaired.** Every refusal names
  its place, and no reader guesses at what a malformed file meant.
- **A ceiling on geometry, not on files.** A reader refuses a model past
  `MAX_GEOMETRY_BYTES` of positions rather than trying to hold it. That
  is a bound on what this crate will allocate from a header's say-so; it
  is not a bound on how large a file the caller should have read, which
  the caller sets and this crate cannot see.
- **`blob::write` and `blob::read` are inverses**, for every mesh the
  reader can produce, and the fuzz target asserts it on every accepted
  input. No other format here is written by this repository, so no other
  can make the claim.

The three subsections below say what that means format by format.

### What a reader promises

Every reader here answers, one way or the other, for every byte string
it can be handed. It does not panic, it does not read past what it was
given, and when it refuses it says which of the named ways in `MeshError`
the file was wrong, with the numbers and the place. That vocabulary is
[`REFUSALS.md`](../../REFUSALS.md), and a mesh file is where its first
two rules earn their keep: the caller is usually holding a file it did
not produce, exported by a tool it does not own, and "this STL is
malformed" sends that caller to a hex editor for what the reader already
knew.

### What a reader does not promise

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
`tests/obj.rs` does it for OBJ, where the whole input is a grammar: both
index spellings, the four ways a corner may name its streams, and the
line-by-line refusals a format with no header has instead of a size
check. `tests/mtl.rs` does it for the material libraries, where the
interesting cases are structural rather than numeric — a property before
the material it belongs to, two reciprocal spellings of one factor, a
map line whose options run past its file name. `tests/properties.rs` round-trips generated meshes through both
STL spellings and asserts that every byte string gets an answer.

**Each reader's census is a test, not a comment.** All five suites map
every `MeshError` variant to either a file in the suite that provokes it
or a sentence saying why this format cannot reach it, with no wildcard
arm —
so a new variant does not compile until someone says which it is, and a
refusal that stops being reachable cannot sit there unnoticed.

`fuzz/fuzz_targets/stl_read.rs`, `ply_read.rs`, `obj_read.rs`,
`mtl_read.rs` and `blob_read.rs` take bytes unfiltered — for STL the dispatch between the
two encodings is itself untrusted-input handling, for PLY the header is,
and for OBJ and MTL every line is — and each asserts the invariants its
own return type carries. `tests/corpus_replay.rs` replays all five
committed corpora on stable, on every merge, with a floor on the number
of distinct inputs and on the number of distinct answers they reach, plus
refusals named individually per format: four for each of the four
readers of somebody else's files, and six for the blob, which can be
held to more because every byte it accepts was written here. **The blob's replay checks
something the others cannot**: that every seed which reads writes itself
back to the same bytes, which is a claim only a format this crate also
writes can make. **MTL's outcome floor takes
one seed of slack where the others take two**: it can make only four
refusals, so the usual margin would be a hole rather than a margin.

**Every fixture is generated.** A model downloaded from a sample
repository would be a dependency with a licence, and a directory of them
would be a dependency nobody recorded — and a mesh, unlike a datagram, is
the kind of file that has an author. `examples/make_stl_corpus.rs` builds
the 25 STL seeds, `examples/make_ply_corpus.rs` the 24 PLY ones,
`examples/make_obj_corpus.rs` the 25 OBJ ones and
`examples/make_mtl_corpus.rs` the 21 MTL ones and
`examples/make_glb_corpus.rs` the 20 container ones and
`examples/make_accessor_corpus.rs` the 21 accessor ones and
`examples/make_gltf_corpus.rs` the 43 document-and-container ones; between them
every committed seed is built here rather than found. **The blob's 25 seeds
need no such argument at all**, because the format is this crate's own
and `examples/make_blob_corpus.rs` gets every byte from `blob::write`. **For OBJ that rule bites
hardest**: an OBJ is what a person exports out of a modelling tool, so
the obvious way to get one is to take somebody's model.

## The binary glTF container, and the rule that reads backwards

`glb::read` validates the framing of a `.glb` — a twelve-byte header and a
chain of chunks — and hands back the JSON chunk's bytes and the binary
chunk's bytes, borrowed, **parsing neither**. Everything that gives those
bytes meaning is a layer above, and the split is what lets the arithmetic
be fuzzed on its own, where every length in the file is hostile and none
of them has been read yet.

It has its own refusal type. `GlbError` is separate from `MeshError`
because a container fault and a geometry fault send a caller to different
places, and because folding eleven container-shaped variants into the
geometry enum would put an arm reading "not a container" in five readers'
refusal censuses.

**One rule here is the opposite of what every other reader in this crate
does, and it is the specification's requirement rather than leniency.**
The blob refuses a presence bit outside its vocabulary, on the argument
that guessing at an unknown construct means reading arrays at the wrong
offsets. The container specification says a client *must ignore* chunks
with unknown types, so that extensions can add their own. So this reader
**skips a chunk type it does not know and refuses a version it does
not**: a version is a claim about the whole file, and a chunk type is a
claim the format has promised is skippable. Skipping is safe only because
the chunk's own length is validated first — an unknown chunk is stepped
over, never read.

Two smaller consequences of reading the specification rather than
assuming: an **empty binary chunk is legal** (a container *should* omit
one, not *must*), so `Some(&[])` and `None` are different answers; and a
chunk's padding is counted **inside** its declared length, so a length
that is not a multiple of four is a writer that forgot to pad.

`Format::Glb` is detected but not yet read for geometry, and
`Format::read` says exactly that. Until it existed a `.glb` fell through
to the fallback and was refused as a truncated STL — a confident answer
about the wrong format.

## Accessors, and a bound that is not the obvious one

`accessor::Accessor` is six numbers and two flags claiming that a
sequence of typed values lives in a range of bytes: a component type, an
element shape, a count, an offset, an optional stride, and whether
integers are fractions of their own range. `view` borrows the range as
attributes and `indices` borrows it as element addresses.

**Nothing here knows where the numbers came from**, which is what makes
the layer worth having on its own. An accessor over a container's binary
chunk and one over a file loaded beside it are the same arithmetic, and
neither needs a document parsed first — so the arithmetic can be fuzzed
where every length is hostile and nothing has to be got past to reach it.

**The bound is `offset + (count - 1) * stride + size`.** The last element
needs its own size, not a whole stride. `count * stride` is larger
exactly when the stride exceeds the element size — that is, whenever the
data is interleaved — so the wrong expression **refuses files that are
correct**, and only the interleaved ones. A suite with no interleaved
fixture cannot tell the two apart at all.

**Two entry points rather than one function with two answers.** A
component type that cannot address anything, and one marked normalised
where a fraction addresses nothing, are refused when `indices` is called.
What is left of `None` means out of range and nothing else.

Matrix shapes are absent. Their columns are padded to four-byte
boundaries, so their element size is not the product of their parts, and
they carry inverse bind matrices — which is skinning, which this reads
nothing of. They are unrepresentable rather than accepted and mis-sized.

## The document, which is where the indices are

`gltf::read` takes the bytes of a binary glTF, or of a document on
its own, and returns geometry. It is
the only layer here that knows the format has a document at all: the
container hands back two byte strings and parses neither, the accessor
layer is arithmetic over a range, and this is where the numbers that
drive both come from.

**A document is a handful of parallel arrays and a great many indices
into them.** A primitive names an accessor by number, an accessor names a
buffer view by number, a view names a buffer by number. A number naming a
row that is not there is the commonest thing wrong with a hand-edited or
truncated document, so it has one refusal that carries the table, the
index, and how many rows there were.

**A document reads every buffer it names.** A buffer with no source is
the container's own chunk, and only the first may be: the specification
leaves any other sourceless buffer undefined, and undefined is refused
here rather than guessed at. A buffer may instead embed its payload as a
`data:` URI, which is decoded in place. Anything else names a second
file, which this crate will not open.

**A buffer is its resource cut to the length it declares.** The
specification allows the resource to be longer and says only the first
`byteLength` bytes belong to the buffer — and that is not a
technicality, because the container pads its binary chunk to a four-byte
boundary, so the chunk is routinely longer than the buffer inside it.
Cutting is what stops a view reaching past the buffer into that padding.

**Both shapes of the format read through the same layers.** A binary
glTF wraps its document in a container beside a chunk of geometry; a
`.gltf` is that document on its own, carrying its geometry as embedded
payloads. `read` takes either, choosing on the four-byte magic, and
everything below the container layer is identical -- one shape has a
chunk to offer and the other has none.

**Its refusals name the layer that failed**, not just the fault: a
`GltfError` says whether the container, the document, a buffer, an
embedded payload, an accessor, a material, an image or the geometry objected, and the inner refusal's own numbers are one call away
through the value. A caller that only wants geometry can go
through `format::detect` and then `Format::read`, which wraps the same
answer in `MeshError`.

The scene walk is an explicit stack with a visited mark checked before
children are pushed, so a document whose nodes point at each other is a
refusal rather than a walk that never ends.

## Two material vocabularies, and why they are not one

`mtl::Material` holds the Phong vocabulary a material library carries —
ambient, diffuse, specular, a specular exponent. `pbr::Material` holds the
metallic-roughness vocabulary a glTF document carries — a base colour,
how metallic the surface is, how rough, and the maps that modulate those.

**They are not two spellings of one thing, and nothing here converts
between them.** Every mapping in circulation is a heuristic: there is no
specular exponent that *is* a roughness, only a formula somebody found
acceptable for their renderer. Applying one would hand back a material no
file contained, which is what this crate declines to do everywhere else.
A caller that wants one model out of both converts where the conversion
can be seen.

**A glTF material has no required members**, so an empty one is legal and
means every default — and the defaults are the format's, not this
crate's convenience. **A factor outside the range the schema states is
refused rather than clamped**, which is the opposite of what the material
library does with its specular exponent, and the two differ because the
formats do: that range is a convention files exceed, this one is stated
by the schema.

A primitive's material is **reported rather than stored**. Geometry and
the surface it wears are two facts, and the canonical form carries one of
them; `gltf::primitive_material` answers for the pairing without changing
what a mesh is.

## An image is bytes and one name for what they are

`gltf::images` reads the image table to the bytes a document carried and
the type it stated for them. **It decodes no image** — what those bytes
are is the caller's question, and answering it here would mean an image
decoder this layer has no need of.

**Exactly one source each.** The format's schema is a `oneOf` over `uri`
and `bufferView`, so an image names one or the other: both is a document
contradicting itself, neither describes nothing, and a reader that picked
between them would be answering a question the document did not settle.
A `uri` is a payload this reader decodes or a second file it will not
open — the same pair of answers a buffer's `uri` gets.

**One name for the bytes, not two.** An image may state its type in
`mimeType`, in its payload's URI, or in both; the format requires one
beside a view, because a view carries bytes and nothing about them. When
both are present and differ, the document is refused rather than the
contradiction being handed on — the same trade the payload decoder
makes when it refuses a resource two texts could spell. An absent type is
not a disagreement: a URI may omit one, and that is the document having
said nothing rather than having said something else.

## `data:` URIs, and why a decoder is strict about spelling

`data_uri::read` turns a URI whose payload *is* the resource back into
bytes — RFC 2397, base64 — and does nothing else. It does not know what
the bytes are for and never touches the filesystem, so a URI naming a
second file is not its to refuse, because it is not its to fetch.

**The rule it is built on is that a difference which cannot change an
output byte is forgiven, and one that can is refused.** The `;base64`
marker is read in any letter case. Whitespace, the URL-safe alphabet, a
payload that is not whole four-character groups, padding anywhere but the
end — all refused, and the three that are about one character name
its offset.

**The one that is easy to miss is the last group's unused bits.** `QQ==`
and `QR==` would both decode to the single byte `A`: the last four bits
of the second character reach no output byte. A decoder that ignores them
lets two different texts name one resource, which is the same trade the
container refuses when it insists its declared length is exactly the
file's length rather than at most it. Refusing the second spelling keeps
text and bytes one to one — and the fuzz target holds that line from the
other side, re-encoding everything the decoder accepts and demanding the
input back character for character.

The media type is reported and never judged. Which types are acceptable
is a fact about what the caller is reading, and belongs where that is
known.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies and
extension points. Contract lints live in `clippy.toml`: clock reads,
filesystem access, path types and thread spawning are rejected at lint
time.
