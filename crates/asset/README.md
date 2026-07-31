# renew-asset

The runtime asset pack: a single file holding many named blobs, each with
a digest of its own contents. A writer that is deterministic, and a
reader that trusts nothing the file tells it.

## Contract

- **The same inputs produce byte-identical output.** Entries are sorted by
  name before writing, so the order they were added in — and therefore the
  order a directory happened to be walked in — never reaches the bytes.
  Nothing here reads a clock, a path, or the environment, and the crate's
  [clippy.toml](clippy.toml) rejects all three at lint time rather than by
  review.
- **Names are unique and sorted.** Enforced when writing and re-checked
  when reading, because a reader must not assume the file in front of it
  came from this writer.
- **A pack is read, never trusted.** Every length is validated against the
  bytes actually present before it is used to slice anything; all
  arithmetic is checked; and the four regions must account for the file
  *exactly*, so a truncated download and an appended payload are refused
  alike. A malformed pack costs no allocation — everything the reader
  returns borrows the caller's buffer.
- **The crate never touches the filesystem.** It takes bytes and returns
  bytes. The size bound on reading an untrusted file belongs at the seam
  that can refuse an oversized one, which is the caller.

## The format

Little-endian, four regions:

```
header    magic "RENEWPK\0" (8) | format u32 | count u32 | names_len u32 | data_len u32
entries   count × { hash u64 | name_off u32 | name_len u32 | data_off u64 | data_len u64 }
names     names_len bytes of UTF-8
data      data_len bytes
```

Entries are fixed width, so the table can be bounds-checked as a whole
before any of it is read and an entry can be found by index without
parsing the ones before it. Names live in one blob rather than inline,
which is what keeps the table fixed width. `format` is a version, and a
reader refuses one it does not know rather than guessing — reading an
unknown layout means treating attacker-chosen bytes as offsets.

## Public API

`PackBuilder::insert` and `finish` to write; `Pack::read` to validate and
borrow; `entries`, `get` (a binary search, which the sort order exists to
allow), and `mismatched` to verify payloads against their digests.

Verification is separate from reading on purpose: reading is what every
consumer does and stays proportional to the table, while verifying touches
every byte. `renew asset-inspect --verify` asks for it; a runtime load
would not.

## Thread safety and ownership

No shared state, no interior mutability, no globals. `Pack<'a>` borrows
the buffer it was read from and cannot outlive it, which is what makes a
malformed file free of allocation. Both types are `Send` and `Sync` when
their contents are; nothing here synchronises because nothing here is
shared.

## Testing

Round-trip and ordering properties over generated packs (seeded), a
hostile-input property that hands the reader arbitrary bytes and
pack-shaped bytes and requires an answer rather than a panic, and a golden
test asserting the exact byte layout by hand.

That last one earns its place: the properties prove the writer and reader
agree with *each other*, which two halves of the same mistake also do. The
golden test is the only one that proves they agree with the format as
documented.

**Real fuzzing is required before this module is called stable, and is
not here yet.** The hostile-input property is a small fuzzer with a fixed
budget standing in for it, and the gap is tracked rather than left
implicit.

## Status

`bootstrap`. The container is settled enough to build on; what is stored
in it is not. The `[package.metadata.renew]` table in
[Cargo.toml](Cargo.toml) is authoritative for maturity and manifest
metadata.

## Key decisions

- **No importer, and no codecs.** A real importer needs an image or audio
  decoder, which is a dependency the owner has not approved — and a
  subcommand that only copied bytes would be worse than its absence. v0 is
  the container, which is the part that has to be right before anything is
  stored in it.
- **FNV-1a-64 for the digest.** No dependency, a dozen lines, and **not
  collision-resistant**. This is content *addressing* — telling whether
  two blobs are the same one, for change detection and deduplication — not
  integrity against someone choosing the bytes. A pack from an untrusted
  source is safe because the reader validates its structure, not because
  its hashes could not be forged.
- **Sorting is the determinism mechanism**, rather than recording an
  explicit order. It is cheap, it is directly testable, and it makes
  lookup a binary search instead of a scan.
- **Duplicates are refused, not resolved.** A pack with two `mesh/hero`
  entries has no correct reading, and silently keeping one is discovered
  years later by someone whose asset did not change when they changed it.
