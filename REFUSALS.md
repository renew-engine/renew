# Refusing input the engine did not write

A reader of untrusted data is judged by its refusals, not by its
successes. This is the catalogue of them: every distinct way input can be
wrong, what each one means, what provokes it, and why refusing beats
coping. It is written to be implemented *against* — you go down the list
and answer it — because a parser tested only on the files it can already
read is exactly the failure that fuzzing exists to catch, and it passes
its own suite the whole time.

Thirteen readers in this tree already take bytes nobody here wrote. Their
refusals are the worked examples throughout, so what follows describes the
house pattern rather than inventing one.

**The second half was written before any reader here took geometry, and
four of the thirteen now do** — the STL, PLY, OBJ and blob readers in `crates/mesh`, built
against this list rather than against the handful of files that happened
to be on somebody's disk, which is what the list was for. Where an entry
below says there is no local precedent, check `crates/mesh` first: its
eleven `MeshError` variants answer "not this format at all", "too short
to hold its own header", "a declared count the bytes present cannot
supply", "a value outside the format's own domain", "zero where zero
has no meaning" (entry 12, which OBJ's one-based indices are exactly),
"legal in the format, not implemented here", and the product ceiling of
entry 31. The rest of part
two is still ahead of the code, and still says so.

## How to use this

Before you write the reader, go down the numbered list and give every
entry one of two answers:

- **the name of the variant you will return**, or
- **why that category cannot arise in your format**, written down.

Both are answers. "I did not think about it" is the one that ships. A
category you rule out is worth a line in the error type's documentation
saying so, because the next person to extend the format needs to know
whether the silence was a decision.

Then write one test per variant, and make the list of variants
mechanically checkable — see [Keeping the catalogue
reachable](#keeping-the-catalogue-reachable). A refusal nothing provokes
is dead code that reads like safety.

## Where the refusals named below live

| Reader | Error type | Declared in | Refusals | Fuzz target | Corpus floor |
|---|---|---|---|---|---|
| Asset pack | `PackError` | `crates/asset/src/error.rs` | 12 | `asset_pack` | 10 |
| Input trace | `TraceErrorKind` | `crates/trace/src/error.rs` | 28 | `trace_parse` | 100 |
| WAV | `WavError` | `crates/audio/src/wav.rs` | 21 | `wav` | 5 |
| UI document | `DocumentError` | `crates/ui/src/document.rs` | 16 | `ui_document` | 10 |
| UI text | `Diagnostic` | `tools/cli/src/ui_compile.rs` | a message | `ui_text` | 10 |
| Datagram | `WireError` | `crates/net/src/wire.rs` | 20 | `net_datagram` | 20 |
| PNG | `DecodeError` | `crates/png/src/decode.rs` | 17 | `png_decode` | 16 |
| JSON | `JsonErrorKind` | `crates/json/src/error.rs` | 29 | `json_parse` | 30 |
| Deflate, inside PNG | `InflateError` | `crates/png/src/inflate.rs` | 7 | `png_decode` | — |
| Mesh descriptor | `TargetError::Creation` | `crates/rhi/src/vk/mesh.rs` | 8, as strings | none | none |
| STL | `MeshError` | `crates/mesh/src/error.rs` | 11, shared | `stl_read` | 18 |
| PLY | `MeshError` | `crates/mesh/src/error.rs` | 11, shared | `ply_read` | 16 |
| OBJ | `MeshError` | `crates/mesh/src/error.rs` | 11, shared | `obj_read` | 16 |
| MTL | `MeshError` | `crates/mesh/src/error.rs` | 4 of the 11 | `mtl_read` | 13 |
| Mesh blob | `MeshError` | `crates/mesh/src/error.rs` | 8 of the 11 | `blob_read` | 16 |

**The MTL row is the shortest in this table, and that is the honest
number rather than a gap.** A material library indexes nothing, declares
no counts and multiplies nothing, so four refusals is the whole of what
can go wrong in it, and its census writes a sentence for each of the
seven it cannot reach. **A reader with few answers needs a tighter
corpus floor, not the same one**, which is why its gate takes one seed of
slack where the others take two.

**The five mesh readers share one error type, and each names in a test
which variants it cannot reach** — STL has no index to be out of range,
PLY numbers its vertices from zero so no index of its own can be, OBJ
has no header of a fixed length to be too short for
— so the shared type costs no reader the ability to say its own list is
complete. **That census is what forced `IndexZero` into existence and
what deleted a refusal from the STL reader that no input could produce**:
it demands a sentence per variant per reader, and a sentence that cannot
be written truthfully is a finding.

The mesh-descriptor row is the one to read before writing an importer
that touches the GPU. Its refusals are formatted strings rather than
variants — the shape the rest of this document argues against — and it
sees a vertex stream as opaque bytes: it can prove the stream divides by
its stride and that no index escapes it, and nothing whatever about what
the numbers mean.

## Six rules the refusals here are held to

**"Held to" rather than "follow":** two readers do not hold rule one, and
this document said rule five more strongly than any reader holds it. Both
are recorded below rather than quietly softened, because a rule with a
named exception is a design and a rule with a hidden one is a lie.

**One.** *One variant per way the input can be wrong.* No `Other`, no
string-typed catch-all. `crates/asset/src/error.rs` states the reason: a
parser that can say "malformed" without saying how has stopped being able
to distinguish a truncated download from an attack. The two have the same
bytes and completely different responses.

**Two.** *Every variant carries the numbers.* `declared` and `actual`,
`saw` and `ceiling`, `expected` and `found`, `ours` and `theirs`. The
person reading the message is usually holding a file they did not build
and cannot inspect by eye, so a refusal that omits the numbers sends them
to a hex editor to recover what the reader already knew.

**Three.** *Every refusal names its place.* A byte offset (`at`), a record
index (`index`), or a line — `TraceError` wraps every kind as
`{ line, kind }` for exactly this reason, and `Diagnostic` carries line
and column. "Something in this file is wrong" is not a diagnosis of a file
with forty thousand vertices in it.

**Four.** *Refuse; never repair, never skip.* The trace reader's message
for an unknown line says it best: the line is refused rather than skipped,
because if the file really is a version this reader accepts, then this
reader's table is the thing that is incomplete. Skipping converts somebody
else's bug into your silent wrong answer, and it surfaces years later as
an asset that did not change when it was changed.

**Five.** *A refused input never allocates more than its own length buys.*
Validate the arithmetic before you reserve. A reader that reserves on a
declared number turns a refusal into an out-of-memory kill, which is not
an answer — sixty bytes of header can ask for sixty-four gigabytes.

**An earlier version of this rule said "a refused input costs no
allocation", and that was false of both readers it named.** `Pack::read`
reserves its entry vector before parsing a single entry, so a pack that
refuses on entry four hundred has already reserved for all of them; the
PNG decoder accumulates `PLTE`, `tRNS` and every `IDAT` body before it can
reach the error that rejects the file. What both actually hold is the
weaker and sufficient property: **the reservation is bounded by a linear
function of the bytes the caller already had in memory.** `Pack` checks
that header, table, names and data account for the file *exactly* before
reserving, which caps the vector at one entry per thirty-two bytes of
input; the PNG decoder's accumulations are slices of the file, and the one
number that is not — the expanded image — is computed in full and
compared against `CEILING` before a byte of it is reserved.

That is the distinction worth keeping: the danger is never allocation, it
is *amplification*. A hostile file that costs its own size to refuse is
fine. One that costs a million times its size is the attack.

**Six.** *The writer's mistakes are a different enum from the reader's.*
`PackError` and `BuildError` are split, and `WireError` and `WriteError`
with them, because the audiences differ: a build failure is a mistake by
whoever is packing, in their own inputs, and it can name them; a read
failure is a statement about a file of unknown origin. An importer will
want the same split the moment it can also write.

### Two places in this tree do not hold rule one

`tools/cli/src/ui_compile.rs` returns `Diagnostic { line, column, message }`
— a formatted string. It is fuzzed and corpus-gated like the others, but a
caller cannot match on *why* it refused. It is a compiler for text a person
is editing, where prose is what the reader wants, so the trade was
deliberate.

`tools/cli/src/json.rs` is the second, and it was missed when this section
claimed there was one. It is a hand-rolled JSON reader serving the CLI's
own `--json` output and its reading of `cargo metadata`; it refuses with
strings and has no fuzz target of its own. Its input is a program this
repository invoked rather than a file a stranger supplied — a real
distinction, and not the same as being safe.

**The correction is worth more than the entry.** The heading said "the one
place", which is a claim about every reader in the tree, and it was checked
by confirming that `ui_compile.rs` is such a place. That is a different
proposition. **A universal is not verified by an example**, and this
document is made almost entirely of universals.

Copy either only if your consumer is a person at a terminal. An importer's
consumer is a build step deciding whether to retry, substitute or fail, and
it cannot decide that from a sentence.

## Keeping the catalogue reachable

Two mechanisms in the tree keep a refusal list from rotting, and an
importer wants both.

**An exhaustive match inside the crate, with no wildcard.**
`crates/png/src/decode.rs` carries a test module whose `name` function
maps every `DecodeError` variant to its own name and has no `_` arm.
Inside the defining crate `#[non_exhaustive]` does not apply, so adding a
variant stops that file compiling until somebody decides which bytes
provoke it. The test builds one input per refusal, asserts each reaches
the refusal it names, and asserts that the number of *distinct* outcomes
equals the number of inputs — so two entries that collapse onto one
refusal are caught, and one of them was not testing what it claimed.

**A replay gate over the committed corpus.** Every input under
`fuzz/corpus/<target>/` is fed back through the parser on the stable
toolchain at every merge, so a recorded input that starts panicking fails a
merge rather than a nightly job nobody is watching.

**What each gate asserts is NOT uniform across the tree, and an earlier
version of this section said it was.** The strongest form — a low-water
mark on *distinct* inputs counted by content, a floor on how many distinct
outcomes the seeds reach between them, and a list of refusals that must
stay reachable **by name** — is carried by `crates/png` and `crates/json`.
The rest replay their corpus and assert that every input still answers,
without the distinctness and by-name floors. **The three-part form is what
an importer should copy; it is not what it will find if it opens whichever
gate is nearest.**

That last one exists because a count floor alone is not enough, and the
evidence is recorded rather than assumed: deleting the PNG decoder's
allocation ceiling did not remove an outcome from the corpus, because the
seed that had reached `TooLarge` simply fell through to `MissingEnd`. The
total dropped by one and cleared any floor loose enough to let a minimiser
run. Name the refusals that matter.

---

# Part one: every reader of bytes

Ordered the way a reader is written, top to bottom. Each entry names what
provokes it, why refusing beats coping, and what already implements it.

## Before you believe a single field

### 1. Not this format at all

**What.** The magic number, signature, or leading keyword does not match.

**Provoked by.** A JPEG handed to the PNG path; a text file renamed; a
truncated download whose first bytes are an error page from a proxy.

**Why refuse separately.** This is a different fault from "damaged", and
the two want different responses. A file that is not a PNG is usually a
wiring mistake — the wrong path, the wrong entry, a mislabelled asset —
and saying "this is not a PNG at all" points at the wiring. Folding it
into a generic parse failure sends somebody looking for corruption in a
perfectly good JPEG.

**In the tree.** `DecodeError::NotAPng` (whose documentation says exactly
this), `PackError::NotAPack`, `WavError::NotRiff` and `NotWave` — two
variants, because the container and the payload kind are separate claims —
`DocumentError::NotADocument`, `WireError::BadMagic`,
`TraceErrorKind::NotATrace`, `MeshError::NotThisFormat { expected }`.

**And the mesh one is the entry's own argument, learned the hard way.**
For three rounds of work the mesh readers had no such variant. PLY answered
a file of another format with `ExpectedKeyword` naming `end_header` on line
1 — because the terminator was searched for before anything checked the
magic — which is *the identical refusal a genuinely truncated PLY gets*.
Two faults, one message, and the message described the fault the file did
not have. That is this entry's "somebody looking for corruption in a
perfectly good JPEG", reproduced exactly, in the reader written last.

**STL still cannot reach it, and that is not an omission.** The format has
no magic: its binary encoding opens with eighty bytes of anything at all,
so "these are not STL bytes" and "these are STL bytes that were cut short"
are the same observation. OBJ and MTL have no signature either. A reader
whose format gives it no way to tell should say the more useful thing —
the count declared and the bytes that arrived — and its refusal census
should say why it cannot do this one, which all three do.

### 2. Too short to hold its own header

**What.** The input is shorter than the fixed-size prologue every field
below is read from.

**Provoked by.** An empty file; a zero-length network read; a four-byte
file where the header is thirty-two.

**Why refuse first.** Because everything after this point indexes into the
header, and a bounds check per field is a bounds check you will eventually
forget. One length comparison up front makes every subsequent field read
total. It also gives the honest message: the file is not corrupt in some
interesting way, there is nothing there.

**In the tree.** `PackError::NoHeader { len }`,
`WavError::TooShort { len }`, `DocumentError::NoHeader { len }`,
`WireError::TooShort { len }`, `TraceErrorKind::Empty`,
`DecodeError::BadHeader` (no `IHDR`, or one that is not thirteen bytes),
`MeshError::TooShortForHeader { needs, len }` — carrying both numbers, so
the message is the requirement and the shortfall rather than "too short".

The mesh blob shows the "one comparison up front" argument paying off
literally: its header offsets are a running sum of field widths and every
field after the check is read with a total accessor, because the length
was established once.

### 3. A version this build does not read

**What.** A well-formed file declaring a format revision this code was not
written against.

**Provoked by.** An asset produced by a newer toolchain; a file from a
branch where the layout changed.

**Why refuse rather than guess.** Reading an unknown layout means treating
attacker-chosen bytes as offsets — that is the asset pack's own phrasing,
and it is the whole argument. A reader that tries anyway because the
header looks close enough has converted a version check into an arbitrary
offset read. Accept your own version and older ones you have actually
implemented; never a newer one.

**In the tree.** `PackError::UnknownFormat { found }`,
`DocumentError::UnknownVersion { found }`,
`WireError::BadVersion { saw }`,
`TraceErrorKind::UnsupportedVersion { found, supported }`,
`MeshError::Unsupported { wanted: "a blob of version 1" }`.

The mesh blob also has this entry's subtler member, in its own shape: a
presence bitmask with a bit set outside this version's vocabulary is
refused as `Unsupported { wanted: "a blob using only the arrays this
version defines" }` rather than masked off. A file whose header uses a
construct the reader does not know is a file a later build wrote, and
quietly ignoring the bit would read its arrays at the wrong offsets.

There is a subtler member of this family worth copying:
`TraceErrorKind::EventFromANewerFormat { kind, introduced, declared }`
fires when a file uses a construct the reader *does* know but the file's
own header disclaims. A header that lies about its own vocabulary sends
every older reader into the wrong refusal, so the inconsistency is named
directly.

## Before you slice

### 4. A declared length that runs past the end

**What.** A count or size field that, followed, would read beyond the
buffer.

**Provoked by.** A chunk header claiming four gigabytes inside a
two-hundred-byte file. This is the single most common malformed-input
shape there is, and the first thing a fuzzer finds.

**Why refuse.** In a language without bounds checks this is the classic
read past the end of an allocation; here it is a panic, which is a denial
of service rather than a disclosure — better, and still a defect. Every
length arrives from the file and none of it is trustworthy. Compare
against what is actually present, use saturating or checked arithmetic so
the comparison itself cannot wrap, and remember any trailer: the PNG
decoder checks `declared + 4` because the checksum bytes must be there
too.

**In the tree.**
`DecodeError::ChunkOverruns { id, at, declared, available }`,
`WavError::ChunkOverruns`, `RiffSizeOverruns` and `ChunkHeaderTruncated`,
`PackError::NameOutOfRange` and `DataOutOfRange`,
`InflateError::Truncated { at }`,
`TraceErrorKind::LineEndsEarly { expected }`.

### 5. A declared size that is not exactly the size present

**What.** The regions the header describes do not account for the bytes in
hand — checked as equality, not as "at least".

**Provoked by.** A truncated transfer, and equally by a file with
something appended to it.

**Why equality.** Something appended to a pack is as much a sign of
trouble as something cut off it. A reader that accepts trailing bytes
accepts a file with a second payload hidden after the first, and accepts
two different files as the same file. `WireError::SizeMismatch` puts it as
equality, never a lower bound.

**In the tree.** `PackError::SizeMismatch { declared, actual }`,
`WavError::TrailingBytes`,
`WireError::SizeMismatch { kind, declared, actual }`,
`DocumentError::SizeMismatch`, `TraceErrorKind::TrailingText`,
`MeshError::CountMismatch { declared, actual, count }`.

The mesh blob is equality (`wanted != bytes.len()`), and it buys more than
this entry claims. Because the byte total the header implies must *equal*
the length in hand, the bytes the reader allocates are the bytes it was
handed: the amplification factor is exactly one, and a header-only file
can ask for nothing. An "at least" comparison would have given that up
along with everything else in this entry.

### 6. A checksum that does not match its bytes

**What.** A stored digest disagrees with the content it covers.

**Provoked by.** A single flipped bit anywhere in a covered region — the
cheapest property test there is, and one worth writing.

**Why refuse, and what it does not buy.** It catches accidental damage:
bad storage, a bad cable, a partial write. It is not integrity against
somebody choosing the bytes, unless the function is collision-resistant
and the digest is carried out of band. The asset pack says this plainly
about its own FNV-1a-64 digests: a pack from an untrusted source is safe
because the reader validates its structure, not because its hashes could
not be forged. Say which of the two yours is, in the enum's documentation,
or somebody will lean on it for the wrong one.

**In the tree.** `DecodeError::BadChecksum { id, declared, found }`.

## Before you allocate

### 7. A ceiling, and arithmetic that cannot wrap past it

**What.** A hard limit on how much memory the input may cause you to
reserve, checked before reserving it, computed with checked arithmetic.

**Provoked by.** A sixty-byte file. The PNG decoder does the arithmetic in
its own documentation: a header is four bytes of width and four of height,
so a sixty-byte file can ask for sixty-four gigabytes. Without a limit,
the refusal is an allocation failure rather than an answer.

**Why a ceiling rather than trust.** Because the alternative is not a
graceful failure — it is the process dying, or the machine swapping, on
input a stranger chose. Pick a number far past anything real and far short
of anything that hurts, name it as a constant, and say in its
documentation why the number is where it is. The PNG decoder uses
`const CEILING: usize = 256 << 20;` and argues both halves.

The arithmetic matters as much as the limit. Compute the total with
`checked_mul` and `checked_add` and refuse on overflow, rather than
computing a wrapped total and comparing that. The asset pack widens its
region arithmetic to 64 bits so a hostile count cannot wrap the sum on a
32-bit target and land back inside the file.

**In the tree.** `DecodeError::TooLarge { width, height }`,
`InflateError::TooLarge { limit }`, `PackError::TooLarge { field, value }`,
`WireError::TooLong` and `TickOverflow`, `MeshError::TooLarge { field,
value }`, and — for the seam that reads a path rather than bytes —
`FsError::TooLarge { path, limit }`.

`MeshError::TooLarge` is worth reading for the distinction it draws in its
own documentation: **a policy ceiling, not a representation limit**. A
representation limit is reached only after the allocation has been
attempted; a policy ceiling is a refusal that costs nothing. It bounds a
schema's element and property counts, the corners one face may name, and
— separately — the total geometry a file may build, because the first
three bound factors and none of them bounds the product. That last
ceiling is this entry's `checked_mul` argument arriving as a fourth
constant rather than as arithmetic.

Note where that last one lives. The parser crates take bytes and never a
path; the bound on reading an untrusted *file* belongs at the seam that
can refuse an oversized one, which is the caller. An importer should take
bytes for the same reason.

### 8. A payload that is not a whole number of records

**What.** A region whose length does not divide by the size of the thing
it holds.

**Provoked by.** A vertex buffer of 1001 bytes at a stride of 12; a WAV
`data` body one byte short of a whole frame.

**Why refuse.** The remainder is either a partial record you would read as
a whole one, or evidence that the stride you believe is not the stride the
writer used. Both produce a plausible wrong answer rather than an obvious
one. Refusing here is the cheapest possible place to catch a disagreement
about the record layout.

**In the tree.** `DecodeError::BadImageLength { expected, found }`,
`WavError::DataNotWholeFrames { len, block_align }`,
`IconError::WrongLength { expected, found }`, and the mesh descriptor's
`create_mesh(vertex length is not a whole number of records)`.

## Field by field

### 9. A value outside the format's own domain

**What.** An enumerated byte the specification does not define.

**Provoked by.** A colour type of 9 where the format defines 0, 2, 3, 4
and 6; a compression method other than the single legal one.

**Why refuse rather than default.** A reader that maps unknown values onto
a default has invented a meaning the writer never gave, and it will keep
inventing it consistently enough that nobody notices for a year. The
number in the file is evidence about what the writer believed; if it is
not a value the format defines, then the writer and the reader do not
agree about the rest of the file either.

**In the tree.** `DecodeError::BadColourType`, `BadMethod` and
`BadFilter`, `InflateError::BadBlockKind`,
`DocumentError::BadStyleByte { index, field, value }` and `BadPatch`,
`WireError::UnknownKind`, and the trace reader's whole row of keyword
refusals — `UnknownKeyword`, `UnknownEventKind`, `UnknownKey`,
`UnknownButton`, `NotAPressedState`, `NotAFocusState`, `NotATouchPhase`.

### 10. Legal in the format, not implemented here

**What.** Input the specification allows and this code does not read.

**Provoked by.** An interlaced PNG; a one-bit-per-channel PNG; a
floating-point WAV.

**Why it is its own category.** Because it is not the file's fault, and
the message should say so. "This decoder does not read interlaced images"
sends somebody to re-export; "malformed PNG" sends them to file a bug
against their own asset pipeline. It is also the honest record of what you
did not build: a named refusal is a scope boundary somebody can find,
where a wrong-looking image is a scope boundary nobody can.

The trap this category exists to close is reading it *anyway*.
`DecodeError::Interlaced` makes the point — an interlaced image is a
different image layout rather than a different pixel format, so a decoder
that ignores the flag produces a scrambled picture instead of an error.

**In the tree.** `DecodeError::UnsupportedDepth { depth, colour }` and
`Interlaced`, `WavError::NotPcm { format }`, `BitDepth`, `ChannelCount`,
`SampleRate`, `ExtensionSize` and `FmtSize`.

### 11. Outside this build's range

**What.** A value the format permits but this implementation bounds: a
floor, a ceiling, or both.

**Provoked by.** A document with a million nodes; a name longer than the
format's field; a peer count of zero, or of nine hundred.

**Why bound it at all.** Every one of these is a number that sizes a
buffer, an index type, or a loop. Bounding it makes the arithmetic
downstream provably in range, which is the difference between a check per
use and a check once. State the bound as a named constant and carry both
the value seen and the bound in the variant, so the message can say how
far past it is.

**In the tree.** `WireError::PeerCountOutOfRange { saw, floor, ceiling }`,
`InputBytesPastCeiling`, `ChatTooLong` and `FrameCountPastRedundancy`,
`PackError::NameTooLong { index, len }`, `DocumentError::TooManyNodes` and
`TooManyPatches`, and the UI text compiler's `MAX_DEPTH` of 64 — whose
comment gives the general rule for recursion in a parser: a hostile
document must not choose the compiler's stack depth.

### 12. Zero where zero has no meaning

**What.** A count, length, extent, or identifier of zero in a position
where zero cannot describe anything.

**Provoked by.** A zero-width image; an entry with an empty name; a frame
count of zero.

**Why a separate variant from "out of range".** Because it is usually a
separate bug. An out-of-range value is a writer that computed the wrong
number; a zero is very often a field nobody filled in, a default that
escaped, or a structure that was never initialised. Telling them apart
points at different code. Zero also tends to be the value that makes
downstream arithmetic degenerate — a division, or an empty loop a later
step assumes ran at least once.

**In the tree.** `DecodeError::ZeroExtent { width, height }` and
`NoImageData`, `PackError::EmptyName { index }` (an empty name no lookup
could ever match), `WireError::FrameCountZero`, `InputBytesZero`,
`ChatEmpty`, `SessionZero` and `DigestPeriodZero`, `DocumentError::Empty`,
`IconError::Empty`, `MeshError::IndexZero { line }` and
`MeshError::NoGeometry`, and the mesh descriptor's
`create_mesh(no vertices)`, `create_mesh(no indices)` and
`create_mesh(zero vertex stride)`.

`IndexZero` is this entry's "separate variant from out of range" written
out: OBJ numbers vertices from one, so a face naming vertex `0` is a
default that escaped rather than a number computed wrongly, and the two
send you to different code. `NoGeometry` is the other half — a file that
is well-formed and declares nothing — refused rather than returned as an
empty mesh, because every way a file ends up with zero triangles is a
mistake upstream and a caller that wanted nothing did not need a file.

### 13. Redundant fields that disagree with what they are derived from

**What.** A header carries a value that can be computed from other values
in the same header, and the two do not match.

**Provoked by.** A WAV whose declared block alignment is not channels
times bytes per sample; a byte rate that is not the sample rate times the
block alignment.

**Why refuse instead of preferring one.** A file whose redundant fields
disagree is a file whose author and this reader do not mean the same thing
by the rest of it. There is no way to tell which of the two is the
mistake, so choosing one is a coin flip that silently reinterprets every
byte after it. Refuse, and carry both numbers so the message can show the
disagreement.

The strongest version of this rule in the tree is
`DocumentError::WrongPatchFlag`, which gives the principle: the runtime
trusts the flag in the frame loop, so the reader proves it at the door.
Anything a hot path takes on faith is something the reader owes a proof
of.

**In the tree.** `WavError::BlockAlign { declared, derived }` and
`ByteRate { declared, derived }`, `DocumentError::WrongPatchFlag`,
`Refusal::SenderNotSource { claimed, source }`.

### 14. Text that is not text

**What.** A byte range the format says is a string, which is not a valid
string; or a numeric field whose text does not parse.

**Provoked by.** An invalid UTF-8 sequence in a name; a decimal field
containing letters; an integer that does not fit its type.

**Why refuse rather than substitute.** Lossy conversion invents
characters, and the replacement character then travels into a filename, a
lookup key, or a log line. A name that was refused is a bug report; a name
that was silently repaired is an asset that cannot be found by the name it
appears to have.

**In the tree.** `PackError::NameNotUtf8 { index }`,
`TraceErrorKind::NotADecimalInteger`, `IntegerTooLarge`, `NotTypedText`,
`NotAHexPattern` and `UnwritableText`, `FsError::InvalidUtf8 { path }`,
`MeshError::NotANumber { found, line }` — the numeric half, for a column
in an ASCII PLY or an OBJ that is not the number the grammar requires,
carrying the text truncated to something printable so the message cannot
itself become an injection of the file's bytes into a log.

The fuzz targets show the boundary version of the same rule. The trace
reader's contract starts at `&str`, so its target skips non-UTF-8 input
rather than lossily converting it — feeding the reader replacement
characters would fuzz the conversion instead of the parser, and every
crash found that way would be unreachable from a real file. Where the
conversion is your reader's own job, that same input is a refusal
instead; the rule in both cases is that nothing invents a character.

## Once the whole file is in hand

### 15. An index that points outside what it addresses

**What.** A reference — into a table, a pool, a palette, a vertex stream —
that names something not there.

**Provoked by.** A palette index of 200 into a 16-entry palette. A
back-reference in a compressed stream pointing before the start of the
output. A triangle index equal to the vertex count.

**Why this is the category to be most careful about.** Two reasons. First,
`>=` and not `>`: an index equal to the count addresses the record one
past the end, and that off-by-one is the entire bug. Second, and this is
the argument that should decide an importer's design, there is often no
oracle for it anywhere else in the system. The mesh module says it
directly: an index past the end of the vertex stream is *data*, the
validation layer does not read index-buffer contents, so an out-of-range
index draws a plausible wrong picture or reads memory the draw was never
given. Nothing downstream will ever tell you.

**Where the cost goes.** Checking every index is a full scan of the index
stream. The mesh module can afford it because the bytes arrive exactly
once and never change again — immutability is what buys the proof. An
importer is in the same position: it reads each file once. Do the scan.

**In the tree.** `DecodeError::BadPalette { entries, index }`,
`DocumentError::PatchOutOfPool { index, entry }` and
`BadParent { index, parent }`,
`InflateError::BadDistance { distance, produced }`,
`WireError::SeatNotInRoster`,
`MeshError::IndexOutOfRange { index, count, face }`, and
`create_mesh(index past the last vertex)`.

The mesh one settles a question this entry does not raise: **its `index`
is signed.** OBJ lets a face count backwards from what has been declared
so far, so `-5` in a file with three vertices is a real thing a writer
emits, and reporting its magnitude would send somebody looking for a fifth
vertex that was never the subject. An index refusal should spell the index
the way the file spelled it. It carries the face as well, because one bad
face and an index base that is off by one everywhere are different
problems and the count alone cannot tell them apart.

### 16. A required section that is absent

**What.** A file that parsed to its end without containing something the
format requires.

**Provoked by.** A WAV with no `fmt ` chunk. A PNG with no `IEND`.

**Why the terminator is the one that matters.** `DecodeError::MissingEnd`
carries the argument, and it generalises to every format: image data is
written before the terminator, so a file cut short after its last data
chunk holds a complete-looking image and would otherwise decode without
complaint — a half-downloaded sprite that looks like a whole one. If your
format has an explicit end marker, requiring it is how truncation is
caught. If it does not, this is the strongest argument for adding one.

**In the tree.** `DecodeError::NoImageData` and `MissingEnd`,
`WavError::MissingChunk { id }` and `MissingPadByte`,
`MeshError::ExpectedKeyword { expected: "end_header", .. }`.

PLY is this entry's argument in a text format: the header is variable
length and the body follows it immediately, so a file cut short inside the
header has no terminator, and requiring `end_header` is the only thing
that catches it. Note what it took to make that refusal *mean* truncation
— until the magic was checked first, every file of every other format got
this same answer, and a refusal that fires for two unrelated reasons
carries no information about either.

### 17. A second legal spelling of one fact

**What.** Input that is structurally valid and semantically identical to
other input the format also admits: unordered entries, duplicate keys,
non-zero padding, a pool holding things nothing references.

**Provoked by.** Two pack entries with the same name. A reserved byte set
to 1. A patch pool in an order no table produced.

**Why refuse a file that means the right thing.** Three reasons, and all
three are load-bearing here.

*Determinism.* If two byte strings mean the same thing, then the same
inputs do not produce the same bytes, and byte-identical output is how
this project checks that anything is reproducible at all. The asset pack
sorts entries by name so the order a directory happened to be walked in
never reaches the bytes, and then re-checks the order on read, because a
reader must not assume the file in front of it came from this writer.

*No correct answer.* A pack with two `mesh/hero` entries has no correct
reading, and silently keeping one is discovered years later by somebody
whose asset did not change when they changed it.

*Pinned padding closes a channel.* Every reserved region in the datagram
format is required to be zero, which is what stops a second spelling of
the same datagram existing — and closes the covert-channel road as a side
effect.

**In the tree.** `PackError::Unsorted { index }` and
`DuplicateName { index }`, `WavError::DuplicateChunk` and `DataBeforeFmt`,
`WireError::PadNotZero { offset, saw }`, `DocumentError::NotPreorder`,
`PoolNotCanonical { expected, found }`, `UnsetSizeBits` and
`UnsetPatchBits`, `TraceErrorKind::DuplicateHeaderKey`,
`HeaderFieldOutOfOrder`, `HeaderAfterEvents` and `ByteOrderMark`.

`PoolNotCanonical` is the most complete instance and worth reading in
full: scanning every table entry in document order, each pooled index must
first appear exactly when the count of already-seen entries equals it, and
the scan must end having seen them all. That single rule refuses both a
permuted pool and an unreferenced entry.

---

# Part two: what an importer of geometry meets

**This half was written when nothing in this tree refused any of the
following, and that is no longer true of all of it.** `crates/mesh`
answers the entries an untextured triangle-soup reader meets: a count
that cannot be supplied by the bytes present, a header that never ends, a
coordinate that is not finite, an index naming a vertex that is not
there, a face that is not a surface, and a ceiling on the product rather
than on its factors (entry 31). Those entries now have local precedent,
and it is named in each.

The rest is still ahead of the code — not because those cases are exotic,
they are the ordinary content of a model file, but because no reader here
has taken a format that carries them. Materials, skins, joint hierarchies
and texture coordinates are all in that group.

So this half is written the other way round from part one. Each entry
says what to refuse and why, and says whether there is a local precedent
to copy or none yet.

Two facts about this engine shape the whole part, and both are worth
holding while reading it:

- **Positions reach the renderer already packed**, as bytes plus a
  stride. There is no layer between an importer and the GPU that will
  look at a number and object to it.
- **Simulation arithmetic is fixed point** — Q47.16 in a 64-bit integer,
  with no float in any signature — precisely so that the same inputs give
  the same results everywhere. Anything an importer produces that reaches
  simulation crosses that boundary, and the crossing is where several of
  these refusals belong.

## Numbers that are not numbers

### 18. A non-finite float

**What.** A NaN or an infinity in a position, a normal, a texture
coordinate, a weight, or a matrix element.

**Provoked by.** A division by zero in whatever produced the file — a
normal computed for a degenerate triangle, a bone with no length, a
scale of zero inverted. Also by four bytes of garbage read as a float,
because a large fraction of all bit patterns are NaN.

**Why this is the highest-value refusal in part two.** A NaN is not a
wrong number, it is a number that destroys every number it touches, and
it propagates silently and permanently. One NaN position makes a bounding
box that contains nothing and rejects every query against it; it makes a
broadphase that never reports a hit, or always does; it poisons any
digest computed over the state it reaches, so a run stops being
comparable against another run for a reason nobody can see. Comparisons
against NaN are all false, so the usual defensive test — a range check —
passes it through. And it is invisible on screen: the triangle simply
does not draw.

Every one of those failures appears far away from the file that caused
it, hours later, as something else's bug.

**How.** `f32::is_finite` on every float as it is read, before it is
stored. Carry the field name and the record index. Do not "clamp" it: a
NaN clamped to zero is a vertex at the origin, which is a wrong model
that renders, and that is worse than a refusal.

**In the tree.** `MeshError::NotFinite { field, index }` — **refused by all
five mesh readers**, STL, PLY, OBJ, MTL and the blob, on every coordinate,
normal and texture coordinate as it is read. `TraceErrorKind::NonFinite`
refuses the same thing for a float written as hexadecimal in a text field.

*This entry read "the reasoning transfers exactly; the code does not" until
the readers existed. They do, and it does.*

### 19. Negative zero, and the second spelling of a number

**What.** Two distinct encodings that mean one value: negative zero
against positive zero, and — if your format admits them — non-canonical
NaN payloads or denormals where the consumer flushes them.

**Provoked by.** Any exporter that negates a coordinate. It is
completely ordinary and completely invisible.

**Why it matters here more than elsewhere.** This is entry 17 in float
form, and the argument is the same one: if two byte strings mean the
same model, then a model does not have one representation, and any check
that compares two imports byte for byte is comparing something other
than meaning. The trace format sidesteps the whole problem by writing
floats as bit patterns and never parsing a decimal one, and it states
why: a float value has two zeros and no equality for NaN.

An importer cannot sidestep it, because it does not choose its input
format. So it has to choose a *canonical form on the way in* — normalise
negative zero to zero as the value is read, and say in the reader's
documentation that it does — or refuse the second spelling outright.
Either is defensible. Silently keeping both is not, because it makes
every downstream equality check quietly unreliable.

**Nearest thing here.** `WireError::PadNotZero` for the principle, and
the trace format's float encoding for the specific hazard.

### 20. A value the engine's own arithmetic cannot hold

**What.** A number that is a perfectly good `f32` and is not
representable, or not usefully representable, once converted into the
fixed-point type simulation runs on.

**Provoked by.** A coordinate of `1e30` — a common sentinel in exported
scenes, and about **16** orders of magnitude past what Q47.16 represents
(±1.4 × 10¹⁴). Also by values so small they convert to zero, which turns a
thin triangle into a degenerate one somewhere the importer is no longer
looking.

An earlier version of this entry said 24, which is the distance to the
*squarable* bound rather than the representable one, and the mistake is
worth keeping visible because the two numbers are both real and eight
orders apart. `crates/fixed` publishes both: ±1.4 × 10¹⁴ representable,
±1.2 × 10⁷ squarable — because physics squares things and a squared
value has to fit the type it lands in. **For a coordinate that will be
fed to physics, the squarable bound is the one that binds**, so the
distance that matters here is nearer 23 orders than 16. A refusal at the
boundary should say which bound it checked.

**Why refuse at the boundary.** Because the conversion is where the
information is lost, and it is the last place anything knows both
numbers. A saturating conversion produces a vertex at an enormous
distance that still draws and still collides; a truncating one produces a
model that is subtly the wrong size. Refusing names the file and the
field.

This is entry 11 — outside this build's range — but the range comes from
the engine's number type rather than from the file format, which is why
it is easy to forget: nothing in the file is wrong.

**Nearest thing here.** `PackError::TooLarge { field, value }`, for a
value that cannot be represented on this target. **Nothing in this tree
refuses a coordinate for being outside the engine's number range**, and
`crates/mesh` cannot: nothing it depends on publishes a fixed-point
type, so it never converts anything to `Fixed` and has no bound to check
against. The refusal this entry asks for belongs to whatever converts an
imported model into simulation state, which does not exist yet.

*This paragraph read "it has no dependencies at all" until that stopped
being true. The conclusion survived the reason, which is the good case;
the sentence is corrected in the change that made it false rather than
left for somebody to find and have to work out which half had moved.*

*This entry briefly claimed `MeshError::TooLarge` implemented it. That
variant is a **policy ceiling**, refusing counts a reader will not hold —
entry 7 — and its own documentation records that an earlier version of
*it* made the same conflation in the other direction. Two documents have
now confused a limit chosen by a reader with a limit imposed by a number
type, which is a good reason to keep the sentence that tells them
apart: a representation limit is reached after the work is attempted, a
policy ceiling before.*

## Streams that have to agree

### 21. Parallel streams of different lengths

**What.** Positions, normals, texture coordinates, colours, joint indices
and weights are separate arrays that describe the same vertices, and one
of them is a different length from the others.

**Provoked by.** An export interrupted partway; a tool that wrote
normals for a mesh it had already re-indexed; a hand-edited file.

**Why this category is genuinely new.** Every reader in part one has *one*
payload whose length must divide (entry 8). A mesh has many, and the
constraint between them is not divisibility but equality. Nothing in this
tree refuses "stream A holds 512 records and stream B holds 511", and
nothing downstream can: the renderer takes one packed byte stream, so
the importer is what interleaves these arrays, and reading past the end
of the short one is the importer's own bug rather than the file's.

**How.** Establish the vertex count once, from whichever stream the
format makes authoritative, and check every other stream against it
before interleaving anything. Carry the attribute name, the count you
expected, and the count it had.

### 22. A required attribute that is absent

**What.** No positions at all; or no normals, tangents, or texture
coordinates when the material that will consume the mesh needs them.

**Provoked by.** A point cloud exported to a mesh format. A model whose
author never unwrapped it, fed to a pipeline that samples a texture.

**Why refuse rather than synthesise.** Synthesising is defensible for
some attributes and a trap for others, and the difference is whether the
synthesised value can be wrong in a way that looks right. Missing
positions cannot be synthesised at all. Missing normals can be computed
from the faces, and the result is a flat-shaded model that looks
deliberate — so if you do that, it must be a stated behaviour of the
importer and not a fallback nobody documented. Missing texture
coordinates cannot be invented usefully at all.

The rule that keeps this honest: an importer may substitute only where
the substitution is announced and where the wrong outcome is visibly
wrong. Everything else is a refusal.

**In the tree.** `MeshError::Unsupported { wanted }` — PLY refuses a file
whose vertex element declares no `x`, and one carrying no `face` element at
all, **naming which of the two is missing** rather than reporting the file as
empty. That is this entry for a single stream. `WavError::MissingChunk { id }`
is the shape for a container. **Neither yet answers about a set of streams that
have to be present together**, which is what glTF attributes will need.

### 23. A component type or stride that contradicts its own accessor

**What.** The declared element type, component count, normalisation flag
and stride do not agree with each other, or with the length of the buffer
region they address.

**Provoked by.** An accessor declaring 512 elements of three 32-bit
floats — 6144 bytes — inside a 4096-byte region. An accessor declaring
16-bit integers and marking them normalised where the consumer requires
unnormalised indices.

**Why refuse.** This is entries 8 and 13 meeting: it is both a
divisibility failure and a redundancy disagreement, and either half alone
misses cases. Byte length divided by stride is one claim about the count;
the declared count is another; the region the accessor names is a third.
All three must agree. A reader that trusts the declared count and
computes offsets from the stride will read outside the region without
ever failing a bounds check on the region itself, because it never
compared the two.

**In the tree.** `AccessorError`, whose whole subject is this entry:
`OutOfRange { needs, available }` compares the count, the stride and the
region against each other; `StrideSmallerThanElement { stride, element }`
catches a stride that would overlap the elements it separates;
`StrideNotAligned`, `StrideOutOfRange` and `OffsetNotAligned` catch the
values the format constrains directly; and `NormalizedIsMeaningless` and
`NormalizedIndices` are this entry's second example — integers marked
normalised where the consumer needs them unnormalised — split into the
two different faults it turns out to be. `WavError::BlockAlign
{ declared, derived }` is the same rule one level simpler: a declared
value checked against the one its own siblings imply.

**The bound is not the obvious expression, and getting it wrong fails in
the direction nobody notices.** The region must hold `offset + (count -
1) * stride + size`, because the last element needs its own size and not
a whole stride. Writing `count * stride` is larger whenever the stride
exceeds the element size — that is, whenever the data is interleaved —
so the wrong expression **refuses correct files**, and only the
interleaved ones. A suite with no interleaved case cannot tell the two
apart at all; probing the mutation reddens six tests here, and would
have reddened none before an interleaved fixture existed.

**What is still open is the third claim.** This entry names three: the
declared count, the stride, and the region the accessor addresses. The
refusals above compare the first two against a region *handed to them*.
Checking that the region is itself inside the buffer it names — byte
length divided by stride as a fourth opinion on the count — belongs to
whatever resolves a buffer view against a buffer, which does not exist
yet.

## Geometry that is not geometry

### 24. A degenerate triangle

**What.** A face whose three indices are not distinct, or whose vertices
are collinear, or whose area is zero.

**Provoked by.** A collapsed edge from an automatic simplifier. A quad
triangulated after two of its corners were welded. Repeated indices
appear constantly in real files.

**Why refuse, and why it is not obvious.** A degenerate triangle
rasterises to nothing, so on screen it costs a little and does no harm.
It is what *else* consumes the mesh that breaks: a face normal is a cross
product of two edge vectors, and for a degenerate face that cross product
is the zero vector, which normalises to a NaN — entry 18, arriving from a
file that contained no NaN at all. Collision meshes, lightmap
parameterisation and adjacency structures all do the same division.

**Where the judgement is.** The index-equality case is exact, free, and
should always be refused. The zero-area case needs a tolerance and
belongs with the entries discussed under [The tolerance
problem](#the-tolerance-problem) below. Refusing exact degeneracy and
counting near-degeneracy is a defensible split, as long as the reader
says which it did.

**In the tree, and the answer is "neither, deliberately".** No mesh reader
refuses a degenerate triangle. `crates/mesh/README.md` states the rule and the
reason: degenerate triangles, inconsistent winding and unclosed surfaces "read
successfully, because they are facts about a model rather than about a file,
and refusing them would refuse a great deal of real art."

**The consequence this entry predicts is handled where it happens rather than
at the reader.** `render3d::scene::face_normal` is
`unit(cross(sub(b, a), sub(c, a))).unwrap_or([0.0, 0.0, 0.0])` — so a
degenerate face yields a zero normal, **not the NaN this entry warns arrives
"from a file that contained no NaN at all"**. That is the split this entry asks
for, made explicit: the file is accepted, and the one computation that would
have manufactured a NaN from it does not.

**What is still open** is everything downstream of the renderer — collision,
lightmap parameterisation, adjacency — which this tree does not have yet and
which each do the same division. The entry stands for them.

### 25. A normal that is not a unit vector

**What.** A stored normal whose length is not 1, outside a tolerance.

**Provoked by.** An exporter that applied a non-uniform scale to
positions and to normals alike, which is the standard mistake — normals
transform by the inverse transpose, and a tool that uses the same matrix
for both produces normals that are wrong in direction as well as in
length. Also by 8-bit quantised normals decoded with the wrong mapping.

**Why refuse rather than normalise on load.** Because normalising hides
the exporter bug, and the length is the only evidence of it that will
ever reach anybody. A normal of length 1.4 is not a normal that needs
scaling — it is a normal pointing somewhere other than where the surface
faces, and normalising it produces a confidently wrong direction that
shades plausibly and lights the model incorrectly forever.

Length near zero is a separate and worse case: it carries no direction at
all, and normalising it gives a NaN.

**Nearest thing here.** None. This is entry 13 — a value disagreeing with
what it should be derived from — but it is the first one in the tree that
would be decided by a floating-point comparison rather than an exact
integer one.

### 26. A tangent basis that is not a basis

**What.** A tangent that is not unit length; a tangent not perpendicular
to its normal, outside a tolerance; a handedness component that is
neither +1 nor -1.

**Provoked by.** Tangents generated against one set of texture
coordinates and shipped beside another. A handedness field left at zero
because the exporter did not fill it in.

**Why refuse.** The handedness case is exact and cheap: the value has two
legal spellings and anything else is a field nobody wrote — entry 12 and
entry 9 together. A handedness of zero produces a bitangent of zero,
which produces a normal-mapped surface lit from a direction that does not
exist, and it is a class of visual bug that gets misdiagnosed as a
lighting problem or an art problem for a long time.

Non-perpendicularity is the tolerance-bearing half, and it is worth
checking because it detects the mismatched-texture-coordinate case that
nothing else will.

**Nearest thing here.** None.

### 27. A bounding volume that does not bound

**What.** The file declares minimum and maximum corners, or a sphere, and
the vertices it claims to cover fall outside it — or the minimum exceeds
the maximum on some axis.

**Provoked by.** A file edited after its bounds were computed. An
exporter that computed bounds before applying a transform.

**Why refuse rather than recompute.** Because a declared bound is used to
skip work — culling, broadphase rejection, hierarchy descent — and every
one of those uses is a place where a bound that is too small deletes
geometry that should have been there. The failure is a model that
disappears at certain camera angles, which reads as a rendering bug
rather than an asset bug and gets investigated in entirely the wrong
crate.

Recomputing is defensible, but only if it is announced. Silently
recomputing means the importer accepts a file whose own fields
contradict each other, which is entry 13, and it means nobody ever
learns the exporter is broken.

The inverted case — minimum greater than maximum — is exact, needs no
tolerance, and should always be refused: it is a bound that contains
nothing at all.

**Nearest thing here.** None, though the argument is the same as
`DocumentError::WrongPatchFlag`: the frame loop trusts it, so the reader
proves it.

### 28. Winding that is not consistent

**What.** If the contract is that front faces wind one way, a mesh whose
faces do not all agree.

**Provoked by.** A mirrored model whose transform has a negative
determinant, which reverses winding without changing a single index.

**Why it is a refusal and not a fix.** Because it cannot be fixed
locally. Deciding which winding is right requires knowing which side of
the surface is outside, and for an open mesh there is no answer. A
renderer that culls back faces will drop the disagreeing faces; one that
does not will light them inside out.

**Whether to check it at all** is a real decision, because the check
costs a full traversal and the contract may not require a single winding.
State the answer either way; the point of this entry is that it should be
a decision rather than an omission.

**In the tree, and it is a decision: counted, not refused.**
`Mesh::winding_disagreements` returns how many triangles have a stored normal
pointing away from the face their corners wind. `crates/mesh/README.md` gives
the reasoning — "a few in a large model is an exporter's rounding, and *all* of
them is a file with its winding convention inverted, which is worth knowing
before it is drawn inside out."

**A count says more than a refusal here**, which is why this entry is answered
by a number rather than by an error: the two failures it distinguishes want
different responses, and a boolean would collapse them.

## Topology

### 29. A primitive mode this reader does not draw

**What.** Triangle strips, fans, lines, or points in a reader that
implements indexed triangle lists.

**Provoked by.** Any exporter configured for a different target.

**Why refuse by name.** This is entry 10, and it needs saying separately
only because the consequence of getting it wrong is unusually quiet:
reading a triangle strip's indices as a triangle list produces a mesh
with the right vertex count and the right index count and completely
wrong faces. Every structural check in this document passes. It renders.
It is simply a different shape.

**Nearest thing here.** `DecodeError::Interlaced` is the exact analogue —
legal input, a different layout rather than a different format, refused
by name so that it cannot be misread as the layout this code implements.
The mesh module's own documentation already notes that a primitive
topology parameter would arrive as a later change, which is where this
refusal will need to exist.

### 30. An index count that is not a whole number of primitives

**What.** For a triangle list, an index count that is not a multiple of
three.

**Provoked by.** A truncated index stream. A file that was a strip.

**Why it is not the same check as entry 8.** Entry 8 divides a byte
length by a record size — a question about the buffer. This divides an
element count by the arity of a primitive — a question about what the
elements mean. A stream can pass the first and fail the second, and the
leftover indices are a partial triangle that a draw call either drops
silently or reads past.

**Nearest thing here.** `WavError::DataNotWholeFrames { len, block_align }`
is the same arithmetic against a frame rather than a face.

### 31. Two ceilings and a product

**What.** A limit on vertex count, a limit on index count, and a limit on
the total bytes the two imply — all three, checked before allocating.

**Provoked by.** A header declaring four billion vertices in a file of
two hundred bytes. This is entry 7, and it is listed again because the
mesh case has a shape the single-payload parsers do not: two independent
counts whose *product* with a stride is what actually gets allocated.

**What already exists, and what it does not do.** The mesh descriptor's
`layout_for` refuses a vertex count past `u32`, an index count past
`u32`, and a total size that overflows a 64-bit integer. Those are
representation limits — the largest values the types can carry — and they
are the right checks at that layer, which receives bytes that are already
in memory.

An importer needs something different: a *policy* ceiling, well below the
representable maximum, in the spirit of the PNG decoder's 256 megabytes.
The reason is the same one the decoder gives. The representation limit is
reached only after the allocation has been attempted; a policy ceiling is
a refusal that costs nothing, and it is the difference between an answer
and an out-of-memory kill on a file somebody sent you.

## Structure above the mesh

### 32. A node graph with a cycle

**What.** A scene hierarchy in which some node is its own ancestor.

**Provoked by.** A hand-edited file; a tool that wrote parent links
without checking them.

**Why this one has no precedent to copy and needs real code.** The UI
document format solves cycles by making them unrepresentable: parents are
always earlier records, and `DocumentError::NotPreorder` enforces
depth-first order, so a cycle cannot be spelled. That is the better
design and it is worth stealing wherever the format is yours to choose.

An importer does not get to choose. A format with arbitrary parent
indices admits cycles, so the reader has to detect them — a traversal
marking visited nodes, refusing when it re-enters one still on the open
path. Skipping the check means the first traversal of the hierarchy never
terminates, and the symptom is a hang rather than an error, which is the
one failure the fuzz harness explicitly cannot catch.

**Nearest thing here.** `DocumentError::BadParent` and `NotPreorder`, for
the design rather than for the algorithm.

### 33. Skin weights that do not sum to one

**What.** Per-vertex bone weights whose total is not 1, outside a
tolerance; or a joint index past the end of the joint list.

**Provoked by.** An exporter that dropped the smallest influences to fit
a four-bone limit and did not renormalise afterwards. This is extremely
common.

**Why they are two refusals, not one.** The joint index is entry 15 —
exact, and it must be refused, because an out-of-range joint index reads
a matrix that is not there and deforms the mesh into noise. The weight
sum is entry 13 with a tolerance, and it is softer: weights summing to
0.98 produce a limb that shrinks slightly when it bends, which is a
subtle animation artefact rather than a corruption. Renormalising is
defensible here in a way it is not for normals, because unlike a normal's
direction, the ratios carry the whole meaning and the sum carries none.
As always: announce it, or refuse.

A sum of zero is the case that must be refused either way — there is
nothing to renormalise, and the vertex belongs to no bone.

**Nearest thing here.** `DocumentError::PatchOutOfPool` for the index
half. Nothing for the weights.

### 34. A transform that cannot be inverted

**What.** A node matrix that is singular, or — where the pipeline
requires it — one that does not decompose into translation, rotation and
scale.

**Provoked by.** A scale of zero on one axis, which is how modelling
tools flatten something. A matrix containing a shear, in a pipeline whose
node transforms are stored as translation, rotation and scale.

**Why refuse.** A singular transform cannot produce the inverse transpose
that normals need, so it manufactures entry 18 out of a file that
contained no NaN. A sheared matrix silently loses the shear when it is
decomposed, and the model is imported at the wrong shape with no
indication anywhere that anything was dropped.

**Nearest thing here.** None. This is the entry furthest from anything
the tree has had to reason about.

### 35. A reference to something the pack does not hold

**What.** A mesh naming a material, a material naming a texture, an
animation naming a node — where the thing named is not present.

**Provoked by.** Partial exports; assets renamed on one side of a
reference; a pack assembled from two sources.

**Why refuse at import rather than at load.** Because this is the last
moment the reference is cheap to check and the file that carries it is
still known. A dangling reference discovered at load time is a missing
texture in a running game, reported by whoever is playing it, with
nothing left to say which asset named it.

**Why nothing here does it.** `renew-asset` refuses duplicate and
unsorted names within one pack, but it has no concept of one entry
referring to another — entries are opaque blobs to it, by design, and it
states plainly that it is not an importer and holds no codecs.
Cross-entry reference integrity is a job for whatever writes the pack,
and an importer is the first thing in this tree that will have to do it.

## Two refusals that are policy, not correctness

These two are worth separating, because treating a policy as a
correctness rule produces an importer that refuses perfectly good files,
and that is its own kind of broken.

**Texture coordinates outside the unit square.** Entirely legal:
coordinates outside `[0, 1]` are how tiling works, and a wall with a
coordinate of 8 is a wall that repeats its texture eight times. Refusing
is only correct if your material system does not implement wrapping, in
which case it is entry 10 — legal in the format, not implemented here —
and should say so in those words. What is worth refusing unconditionally
is a non-finite coordinate (entry 18), because that is not a coordinate.

**A mesh with no triangles.** Whether an empty mesh is a refusal or an
ordinary value is a real decision, and this tree has already made the
analogous one in the other direction: `Render3dError::EmptyScene` exists
because an all-air world and a fully culled mesh are ordinary data, not
mistakes, so they get an ordinary refusal rather than an assertion. Note
what that means — it is refused, but as a value the caller is expected to
handle, not as evidence of a bad file. An importer reading a file that
declares a mesh and gives it no faces is in a different position: the
file claims something it does not contain, which is closer to entry 12.
Decide, and write down which.

## The tolerance problem

Every refusal in part one is decided by exact integer arithmetic. That
has a property nobody has had to think about here, because it has never
been absent: **the answer is the same everywhere.** A file refused on one
machine is refused on every machine, in every build, forever.

Entries 24 through 27 and 33 break that, because they are decided by
comparing a floating-point quantity against a tolerance. Two things
follow, and both need handling.

**The refusal must be reproducible.** If the comparison is done in
floating point over values the compiler may contract or reorder, the
answer can differ between targets — and a file that imports on one
machine and is refused on another is worse than either answer taken
alone, because the disagreement is invisible until two people compare
notes. Two mitigations, and they compose: compare squared quantities
rather than lengths, so no square root enters the decision; and put the
check on the far side of the conversion into the engine's own fixed-point
representation, where the arithmetic is exact and the same everywhere,
rather than on the raw float. The second is the stronger of the two, and
it has a pleasant side effect — it makes the check answer the question
you actually care about, which is whether the *imported* mesh is sound,
not whether the file's floats were.

**The tolerance is a policy number and will be wrong for somebody.** A
normal-length tolerance tight enough to catch a broken exporter will
reject 8-bit quantised normals, whose length is legitimately off by up to
about half a percent. So: name it as a constant, document the units and
the reasoning the way `CEILING` does, and carry both the measured value
and the tolerance in the variant. A refusal that says "normal at vertex
1841 has length 1.41, tolerance 0.01" can be argued with. One that says
"bad normal" cannot, and the argument is how the number gets fixed.

**And prefer the exact half where there is one.** Several entries above
split into an exact case and a tolerance case: distinct indices against
zero area (24), handedness against perpendicularity (26), inverted bounds
against unbounded vertices (27), joint range against weight sum (33). The
exact half is free, reproducible, and catches most real files. Implement
it first, and treat the tolerance half as a separate decision with its
own justification.

## Before you call the reader finished

- Every entry above has an answer: a variant, or a written reason it
  cannot arise.
- Every variant has a test that provokes it, and the tests reach as many
  distinct refusals as there are tests — two that collapse onto one
  refusal mean one of them tests something other than its name.
- The variant list is guarded by an exhaustive match with no wildcard,
  inside the crate that declares the enum.
- A fuzz target exists, its seed corpus is generated by a committed
  program rather than collected, and a replay gate feeds every committed
  input back through the reader on the stable toolchain with a low-water
  mark and a list of refusals that must stay reachable by name.
- No input, however malformed, causes an allocation proportional to a
  number the input chose.
- Every fixture the tests use is built in code or generated by a
  committed program. A borrowed file is a dependency with a licence.
