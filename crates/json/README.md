# renew-json

JSON with no dependencies, one direction: bytes in, a document that
borrows them out, and a named refusal for every way the bytes can be
wrong. It never touches the filesystem — opening the file is the
caller's business, which is what lets the same reader serve a whole file
and a chunk carved out of the middle of a larger one.

## What it is for

Asset metadata that arrives as text. A scene description, a material
table, a manifest somebody else's exporter wrote — the class of file
where the interesting question is never "can this be parsed" but "what
does it say, and what do I do when it says something impossible".

The reader was sized against glTF 2.0, which is the widest instance of
that class in ordinary use: sibling arrays with indices between them,
nested objects, arbitrary application data hanging off any node, and a
container format that hands the JSON over as one chunk of a bigger file
rather than as a file of its own. Nothing in the crate knows a single one
of that format's member names. A JSON reader that knew `POSITION` would
be a glTF reader wearing a general name, and would be no use to the next
consumer.

## The deliberate limits, and why

**Nesting stops at sixty-four, by name.** This is the single most
important line in the crate. A recursive-descent reader meets a document
of ten thousand open brackets with a stack overflow, and a stack overflow
is not a refusal: the process is gone, no error value is returned, and
nothing above can report what happened. So the parser is not recursive.
It holds the open containers in a vector and checks that vector's length,
which makes the limit a *policy* rather than a bet about how large a
stack frame turned out to be on whichever platform ran out first. Sixty
four is generous by roughly a factor of eight against anything a schema
produces on purpose — the deepest path through a glTF scene reaches six,
and its ratified extensions add two — and the slack is for the parts of
such a format that are explicitly arbitrary application data.

**A number stops at a hundred and twenty-eight characters, by name.** A
million-digit number is legal JSON, and every consumer of one pays for
it. The shortest decimal form that round-trips a double needs twenty-odd
characters.

**There is no writer, and there is no tree.** What comes back is a flat
table of fixed-width records, one per accepted token, and cursors into
it. The obvious alternative — a tree of enums holding owned strings and
nested vectors — is wrong here for three separate reasons. It allocates
once per member and once per string, all of it sized by an untrusted file
and all of it before any layer above has looked at a single field. It
drops *recursively*, so a deep document unwinds through as many frames on
the way out as the parser refused to use on the way in. And it has to
decide what a number is by how it was spelled, which is exactly the
decision that has to wait for a caller who knows the schema.

**Strings stay where they lie.** Every escape is validated during the
parse and none is decoded until somebody asks. Almost every string a
reader of asset metadata touches, it touches as a comparison — is this
member name `POSITION`, is this one `indices` — and a comparison needs no
buffer. Decoding at parse time would allocate once per member name in a
document to serve the rare caller who wanted an owned copy.

## Two rules that look like bugs and are not

**A duplicate member name is accepted, and the last one wins.** The habit
in this tree is to refuse rather than repair, and this is the one place
it is deliberately not followed. The formats this reader was sized for
say member names *should* be unique and that a reader *should* let a
later value override an earlier one; refusing would reject files those
formats bless. What keeps that from being a silent choice is that both
values stay reachable — `get` answers with the last, `get_all` hands over
every one — so a layer that wants to be stricter can be, with the
evidence in its hand.

**A whole number may be written `3.0` or `3e2`.** Those formats permit it
explicitly, so the whole-number readings accept any spelling whose value
is whole and refuse only a non-zero fraction. The conversion is exact
decimal arithmetic over the characters, never a trip through a float: the
one-line version — parse as a double, check the fraction is zero — is
inexact above 2^53, so a length or an offset near the top of a `u64` would
come back as the nearest double with nothing anywhere to say so.

## Where the grammar is checked by hand, and why it has to be

The standard library's float parser accepts `01`, `1.`, `.5` and `+1`.
JSON accepts none of them. A reader that scanned a plausible run of
characters and handed it to `from_str` would take documents no other
reader takes, which is the quiet kind of wrong: the file works here and
fails everywhere else, and the report comes back as "your exporter is
broken" against a reader that was. So the number grammar is written out
and checked, and `from_str` is used only for the fractions, where its
rounding is exactly what is wanted.

The same reasoning runs the other way for `NaN`, `Infinity`, `-Infinity`,
`undefined` and the capitalised `True`, `False` and `None`. Those are
values in the languages people write exporters in and values in no JSON
document, and they get a refusal that says so rather than "no value
begins with `N`" — because the fault is upstream in whatever wrote the
file, and a message about a stray character sends the reader looking in
the wrong place.

## The charter, so this does not become a junk drawer

**Reading JSON, and nothing else.**

Explicitly out of scope:

- **Writing JSON.** A writer is a second body of knowledge with a second
  set of decisions — how to escape, how to round a float so it reads back
  — and it belongs beside whatever needs it.
- **A mapping to a caller's own types.** No derive, no schema, no
  reflection. That is a second crate if anything ever wants it.
- **Any particular format's member names.** The layer that knows a schema
  owns index validation, required fields and defaults. It has the offset
  of every value from `Value::at`, so its own refusals can point at a
  line as precisely as these can.
- **File I/O.** The caller owns the file, and owns the bound on reading
  it, for the same reason the pack reader gives for owning its own.

## Errors

`JsonErrorKind` names twenty-nine ways a byte string can fail to be the
document a caller asked for. There is deliberately no `Other` and no
string-typed catch-all: a reader that can say "malformed" without saying
how has stopped being able to tell a truncated download from an attack.

The enum is **closed** rather than open, so a caller that handles every
refusal today stops compiling the day a new one is added, instead of
silently routing it to a catch-all arm. That also lets the corpus replay
beside this crate name its outcomes without a wildcard, which the image
decoder's equivalent cannot do.

Every refusal carries a byte offset. A line and a column are computed on
demand from it, because a caller that forwards a refusal to another layer
wants the offset and a caller that prints one wants the line, and only
one of those is on a hot path.

## How this is checked

**Every refusal has a document that provokes it, and the list cannot
rot.** A test beside the enum builds one byte string for each of the
twenty-nine and asserts that the answers they reach number exactly
twenty-nine — so a validation check that becomes dead code is caught by
its document falling through onto some other refusal, which no
per-refusal test can notice. Under that test is a function matching the
error enum exhaustively with no wildcard, so a refusal added later stops
the file compiling until somebody writes the bytes that provoke it. The
same test formats every refusal it reaches, because a variant whose words
nobody has ever read is a variant whose words are wrong.

**Five properties run on generated documents at every merge.** A tree is
generated, written out as text, and the reader has to reproduce it
through its own accessors; every byte string gets an answer and never a
panic; no proper prefix of a document is a document; nesting is accepted
exactly while it is within the limit; and a value's own text parses back
to the same value. Each was probed by deleting the code it guards, and
where a probe taught something it is recorded in that property's own
documentation — including the one that came back green until the input
mixture was widened, because uniform random bytes essentially never parse
and the walk over an accepted document was never being reached.

**The reader is fuzzed, and the corpus is generated rather than
collected.** `cargo run -p renew-json --example make_json_corpus` builds every
seed, so the corpus carries no licence question. Between them the
thirty-eight seeds reach twenty-six different answers — every refusal the
parse can make, plus `Ok`. `tests/corpus_replay.rs` replays them on the
stable toolchain at every merge and asserts two things a file count
cannot: that the seeds still reach at least twenty-two distinct answers,
and that five specific refusals stay reachable by name — the depth bound,
the number-length bound, the lone surrogate, the text check and the
leading zero. Each has exactly one seed, so losing one means a real
defence stops being exercised while the total barely moves.

**Refusing a large document costs nothing proportional to it.**
`tests/no_reservation.rs` measures it, because a comment saying "we do
not reserve" is not a gate: reserving the node table from the input
length, or copying the bytes into a string before scanning them, both
compile cleanly and pass every other test in the crate.

## A note on the tool's own reader

The command-line tool carries a small JSON reader of its own, in
`tools/cli/src/json.rs`, for reading the build system's output and
emitting the tool's `--json`. It refuses with strings and has no fuzz
target.

**An earlier version of this section argued the tool could not adopt this
crate because doing so would invert the layering. That argument was
backwards.** The tool crate already depends on `renew-asset` and
`renew-ui`; depending on `renew-json` is the same direction, and this
crate depends on nothing at all, so there is no cycle to create. The
layering is not what stands in the way.

What actually stands in the way is that it is a behaviour change to a
shipped tool — the two readers do not refuse the same inputs with the same
messages — and that is a decision with an owner. The honest statement of
the position is therefore the reverse of what was written: **the tool
could adopt this crate, this crate currently has no production consumer,
and those two facts are the same fact.** Whoever owns the tool should
decide it on those terms rather than on a dependency argument that does
not hold.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies and
extension points. Contract lints live in `clippy.toml`: clock reads,
filesystem access, path types and thread spawning are rejected at lint
time.
