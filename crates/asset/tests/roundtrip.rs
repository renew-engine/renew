//! Properties over generated packs, and the hostile-input claim.
//!
//! Two claims, different in kind. The first is that writing and reading
//! are inverses over packs nobody thought to write by hand. On its own
//! that is weak — a writer and a reader making the *same* mistake are
//! still inverses — so the golden test beside it anchors the format with
//! bytes asserted by hand.
//!
//! The second is about hostile input: the reader answers, one way or the
//! other, for every byte string it can be handed. It never panics and
//! never hangs. That is a small fuzzer with a fixed budget, standing in
//! until real fuzzing infrastructure exists — which this module needs
//! before it can be called stable, and not before now.

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use renew_asset::{Pack, PackBuilder, fnv1a64};

/// Names that exercise the sort: shared prefixes, path separators, and
/// characters either side of `/` in byte order.
fn name() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        proptest::sample::select(vec!["a", "b", "z", "-", ".", "0", "A", "/"]),
        1..6,
    )
    .prop_map(|parts| parts.concat())
}

fn blob() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..64)
}

/// A set of uniquely-named entries, in arbitrary order.
fn entries() -> impl Strategy<Value = Vec<(String, Vec<u8>)>> {
    proptest::collection::vec((name(), blob()), 0..12).prop_map(|mut items| {
        items.sort_by(|left, right| left.0.cmp(&right.0));
        items.dedup_by(|left, right| left.0 == right.0);
        items
    })
}

/// Returns `Result` rather than unwrapping: the lint that forbids
/// `expect` outside tests reaches helpers in a test file too, because the
/// exemption follows `#[test]` rather than the file.
fn pack_of(items: &[(String, Vec<u8>)]) -> Result<Vec<u8>, renew_asset::BuildError> {
    let mut builder = PackBuilder::new();
    for (name, bytes) in items {
        builder.insert(name, bytes)?;
    }
    builder.finish()
}

proptest! {
    // Seeded: a property suite that picks a fresh seed each run reports a
    // different question every time and cannot be re-run against a
    // failure.
    #![proptest_config(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x8ea1_2b47_5cd3_9061),
        cases: 256,
        ..ProptestConfig::default()
    })]

    /// Writing then reading gives back what went in.
    #[test]
    fn writing_then_reading_returns_every_entry(items in entries()) {
        let bytes = pack_of(&items).expect("unique names, and it fits");
        let pack = Pack::read(&bytes).expect("our own writer's output must read");
        prop_assert_eq!(pack.len(), items.len());
        for (name, blob) in &items {
            let found = pack.get(name).expect("every inserted name is present");
            prop_assert_eq!(found.bytes, &blob[..]);
            prop_assert_eq!(found.hash, fnv1a64(blob));
        }
        prop_assert!(pack.mismatched().is_empty());
    }

    /// Entries come back in name order however they went in.
    #[test]
    fn entries_are_always_in_name_order(items in entries()) {
        let mut shuffled = items.clone();
        shuffled.reverse();
        let bytes = pack_of(&shuffled).expect("packs");
        let pack = Pack::read(&bytes).expect("reads");
        let names: Vec<&str> = pack.entries().map(|entry| entry.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        prop_assert_eq!(names, sorted);
    }

    /// Insertion order never reaches the bytes.
    #[test]
    fn the_bytes_do_not_depend_on_insertion_order(items in entries()) {
        let forward = pack_of(&items).expect("packs");
        let mut backward_items = items.clone();
        backward_items.reverse();
        let backward = pack_of(&backward_items).expect("packs");
        prop_assert_eq!(forward, backward);
    }

    /// **Any** byte string gets an answer rather than a panic.
    #[test]
    fn arbitrary_bytes_get_an_answer(raw in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Pack::read(&raw);
    }

    /// And so does anything shaped like a pack, which reaches far deeper
    /// into the reader than random bytes ever do: random input dies at
    /// the magic, so without this the bounds checks below it would be
    /// exercised by nothing.
    #[test]
    fn bytes_shaped_like_a_pack_get_an_answer(
        items in entries(),
        cut in 0usize..256,
        noise in proptest::collection::vec(any::<u8>(), 0..24),
    ) {
        let mut bytes = pack_of(&items).expect("packs");
        // Truncate somewhere, then append something. Both are refusals
        // the format promises to make.
        bytes.truncate(cut.min(bytes.len()));
        bytes.extend_from_slice(&noise);
        let _ = Pack::read(&bytes);
    }

    /// Corrupting a payload is caught by verification and not by reading,
    /// which is the split the API makes on purpose.
    #[test]
    fn a_flipped_payload_byte_is_found_only_by_verifying(
        items in entries().prop_filter("needs a payload", |i| i.iter().any(|(_, b)| !b.is_empty())),
    ) {
        let mut bytes = pack_of(&items).expect("packs");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let pack = Pack::read(&bytes).expect("structure is untouched, so it still reads");
        prop_assert!(!pack.mismatched().is_empty(), "verification missed a flipped byte");
    }
}

/// The format, asserted by hand.
///
/// The property tests above prove the writer and reader agree with each
/// other. This one proves they agree with the format as documented — the
/// one thing a round-trip can never show, because two halves of the same
/// mistake round-trip perfectly.
#[test]
fn the_bytes_are_exactly_what_the_format_says() {
    let mut builder = PackBuilder::new();
    builder.insert("b", b"\x01\x02").expect("insert");
    builder.insert("a", b"\xff").expect("insert");
    let bytes = builder.finish().expect("pack");

    // header(24) + 2 entries(64) + names(2) + data(3)
    assert_eq!(bytes.len(), 24 + 64 + 2 + 3);
    assert_eq!(&bytes[0..8], b"RENEWPK\0");
    assert_eq!(&bytes[8..12], &1u32.to_le_bytes(), "format");
    assert_eq!(&bytes[12..16], &2u32.to_le_bytes(), "count");
    assert_eq!(&bytes[16..20], &2u32.to_le_bytes(), "names_len");
    assert_eq!(&bytes[20..24], &3u32.to_le_bytes(), "data_len");

    // Entry 0 is "a", because entries are sorted and "a" was added second.
    assert_eq!(&bytes[24..32], &fnv1a64(b"\xff").to_le_bytes(), "hash of a");
    assert_eq!(&bytes[32..36], &0u32.to_le_bytes(), "a name_off");
    assert_eq!(&bytes[36..40], &1u32.to_le_bytes(), "a name_len");
    assert_eq!(&bytes[40..48], &0u64.to_le_bytes(), "a data_off");
    assert_eq!(&bytes[48..56], &1u64.to_le_bytes(), "a data_len");

    // Names then data, each contiguous and in entry order.
    assert_eq!(&bytes[88..90], b"ab");
    assert_eq!(&bytes[90..93], b"\xff\x01\x02");
}

/// A pack whose entries are out of order is refused, even though every
/// offset in it is valid. Built by hand, because our writer cannot
/// produce one.
#[test]
fn an_unsorted_table_is_refused() {
    let mut builder = PackBuilder::new();
    builder.insert("a", b"1").expect("insert");
    builder.insert("b", b"2").expect("insert");
    let mut bytes = builder.finish().expect("pack");

    // Swap the two 32-byte entries, leaving the names blob alone: the
    // table now reads "b" then "a".
    let (left, right) = (24usize, 56usize);
    for offset in 0..32 {
        bytes.swap(left + offset, right + offset);
    }
    let error = Pack::read(&bytes).expect_err("an unsorted table must be refused");
    assert!(error.to_string().contains("ordered"), "{error}");
}

/// Truncation is refused rather than partially read.
#[test]
fn a_truncated_pack_is_refused() {
    let mut builder = PackBuilder::new();
    builder.insert("only", b"payload").expect("insert");
    let bytes = builder.finish().expect("pack");
    for cut in 0..bytes.len() {
        let error = Pack::read(&bytes[..cut]).expect_err("a short pack must be refused");
        // Every refusal names something; none of them panics, which is
        // the claim that matters on this path.
        assert!(!error.to_string().is_empty());
    }
    assert!(Pack::read(&bytes).is_ok(), "the whole thing still reads");
}

/// Appending to a valid pack is refused as firmly as cutting it short.
#[test]
fn trailing_bytes_are_refused() {
    let mut builder = PackBuilder::new();
    builder.insert("only", b"payload").expect("insert");
    let mut bytes = builder.finish().expect("pack");
    bytes.push(0);
    let error = Pack::read(&bytes).expect_err("trailing bytes must be refused");
    assert!(error.to_string().contains("holds"), "{error}");
}

/// An empty pack round-trips, because "no assets" is a real answer.
#[test]
fn an_empty_pack_round_trips() {
    let bytes = PackBuilder::new().finish().expect("pack");
    let pack = Pack::read(&bytes).expect("an empty pack reads");
    assert!(pack.is_empty());
    assert_eq!(pack.len(), 0);
    assert!(pack.get("anything").is_none());
    assert!(pack.mismatched().is_empty());
}

/// A file that is not a pack is refused at the magic, before any length
/// in it is believed.
#[test]
fn a_file_that_is_not_a_pack_is_refused_at_the_magic() {
    let error = Pack::read(b"\x89PNG\r\n\x1a\n and then some more bytes").expect_err("not a pack");
    assert!(error.to_string().contains("magic"), "{error}");
}

/// A two-entry pack, and the offsets of the fields a test wants to bend.
///
/// Built by our own writer and then corrupted in place: every refusal
/// below is reachable only through a file this writer cannot produce,
/// which is exactly the input the reader exists to survive.
fn two_entry_pack() -> Result<Vec<u8>, renew_asset::BuildError> {
    let mut builder = PackBuilder::new();
    builder.insert("a", b"1")?;
    builder.insert("b", b"22")?;
    builder.finish()
}

/// Overwrite a little-endian `u32` at `offset`.
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Overwrite a little-endian `u64` at `offset`.
fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// A name that points outside the names region is refused.
#[test]
fn a_name_reaching_past_its_region_is_refused() {
    let mut bytes = two_entry_pack().expect("the fixture packs");
    // Entry 0's name_off sits at header(24) + hash(8).
    put_u32(&mut bytes, 24 + 8, 9999);
    let error = Pack::read(&bytes).expect_err("must refuse");
    assert!(error.to_string().contains("names region"), "{error}");
}

/// So is a name whose offset plus length overflows rather than merely
/// exceeding the region.
#[test]
fn a_name_whose_length_overflows_is_refused() {
    let mut bytes = two_entry_pack().expect("the fixture packs");
    put_u32(&mut bytes, 24 + 8, u32::MAX);
    put_u32(&mut bytes, 24 + 12, 2);
    let error = Pack::read(&bytes).expect_err("must refuse");
    assert!(error.to_string().contains("names region"), "{error}");
}

/// A zero-length name is refused: no lookup could ever match it.
#[test]
fn a_zero_length_name_is_refused() {
    let mut bytes = two_entry_pack().expect("the fixture packs");
    put_u32(&mut bytes, 24 + 12, 0);
    let error = Pack::read(&bytes).expect_err("must refuse");
    assert!(error.to_string().contains("empty name"), "{error}");
}

/// A name longer than the format admits is refused before it is sliced.
#[test]
fn an_over_long_name_is_refused_by_the_reader() {
    let mut bytes = two_entry_pack().expect("the fixture packs");
    put_u32(&mut bytes, 24 + 12, 5000);
    let error = Pack::read(&bytes).expect_err("must refuse");
    assert!(error.to_string().contains("5000"), "{error}");
}

/// A payload reaching past the data region is refused.
#[test]
fn a_payload_reaching_past_its_region_is_refused() {
    let mut bytes = two_entry_pack().expect("the fixture packs");
    // Entry 0's data_len sits at header(24) + hash(8) + name(8) + off(8).
    put_u64(&mut bytes, 24 + 24, 9999);
    let error = Pack::read(&bytes).expect_err("must refuse");
    assert!(error.to_string().contains("data region"), "{error}");
}

/// And a payload whose offset plus length overflows.
#[test]
fn a_payload_whose_length_overflows_is_refused() {
    let mut bytes = two_entry_pack().expect("the fixture packs");
    put_u64(&mut bytes, 24 + 16, u64::MAX);
    put_u64(&mut bytes, 24 + 24, 2);
    let error = Pack::read(&bytes).expect_err("must refuse");
    assert!(error.to_string().contains("data region"), "{error}");
}

/// A name that is not UTF-8 is refused.
#[test]
fn a_name_that_is_not_utf8_is_refused() {
    let mut builder = PackBuilder::new();
    builder.insert("ab", b"1").expect("insert");
    let mut bytes = builder.finish().expect("pack");
    // The names blob is the last two bytes before the data.
    let names_at = bytes.len() - 2 - 1;
    bytes[names_at] = 0xFF;
    let error = Pack::read(&bytes).expect_err("must refuse");
    assert!(error.to_string().contains("UTF-8"), "{error}");
}

/// Two entries with one name have no defined lookup, so the reader
/// refuses even though every offset is valid.
#[test]
fn a_duplicated_name_is_refused_by_the_reader() {
    let mut bytes = two_entry_pack().expect("the fixture packs");
    // Point entry 1's name at entry 0's, so the table reads "a" twice.
    put_u32(&mut bytes, 24 + 32 + 8, 0);
    put_u32(&mut bytes, 24 + 32 + 12, 1);
    let error = Pack::read(&bytes).expect_err("must refuse");
    assert!(error.to_string().contains("two of one name"), "{error}");
}

/// A pack claiming a version this build does not implement is refused
/// rather than read on the assumption it is close enough.
#[test]
fn an_unknown_format_version_is_refused() {
    let mut bytes = two_entry_pack().expect("the fixture packs");
    put_u32(&mut bytes, 8, 9);
    let error = Pack::read(&bytes).expect_err("must refuse");
    assert!(error.to_string().contains('9'), "{error}");
}

/// A count so large the table could not fit is refused on the size check
/// rather than by attempting the allocation.
#[test]
fn an_impossible_entry_count_is_refused() {
    let mut bytes = two_entry_pack().expect("the fixture packs");
    put_u32(&mut bytes, 12, u32::MAX);
    let error = Pack::read(&bytes).expect_err("must refuse");
    assert!(!error.to_string().is_empty());
}

/// Lookup misses answer `None` rather than the neighbouring entry, which
/// is the way a binary search goes wrong when its comparator is off.
#[test]
fn a_lookup_miss_returns_nothing() {
    let bytes = two_entry_pack().expect("the fixture packs");
    let pack = Pack::read(&bytes).expect("reads");
    assert!(pack.get("a").is_some());
    assert!(pack.get("b").is_some());
    for miss in ["", "aa", "c", "A", "ab"] {
        assert!(pack.get(miss).is_none(), "`{miss}` matched something");
    }
}
