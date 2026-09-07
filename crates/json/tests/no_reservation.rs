//! The refusal path holds nothing proportional to the document it
//! refuses.
//!
//! **The reader allocates exactly one thing** — the node table — and it
//! grows by pushing what has already been accepted. Two shortcuts would
//! break that, and both look entirely reasonable written down: reserving
//! the table from the length of the input, and copying the bytes into a
//! `String` before scanning them. Either one makes a megabyte of garbage
//! refused at its second character cost megabytes to say no, and a
//! reservation taken from an untrusted length is worse than slow — on a
//! 32-bit target it is a `capacity overflow` abort rather than a
//! returned refusal, in a crate whose contract says nothing here panics.
//!
//! A comment saying "we do not reserve" is not a gate. Either shortcut
//! compiles cleanly and passes every other test in the crate, so the
//! property is measured here instead.
//!
//! Own process on purpose: the counters are process-wide, and this file
//! holds one test so that nothing allocates alongside the window being
//! measured. It compares a *peak*, not exact counts, because the
//! allocation either shortcut makes is a single enormous one that is
//! freed again as the refusal propagates — before-and-after totals would
//! show nothing.
//!
//! Probed by reserving the node table from the source length: "refusing
//! a 800003 byte document raised peak memory by 25600128 bytes" —
//! against an allowance of 65 536, and the message prints what each
//! shortcut would have cost so a reader can tell which one came back.

use renew_json::Json;
use renew_memory::{CountingAllocator, counters};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Enough elements that a per-element reservation is unmistakable
/// against the text itself, and still a document somebody could
/// plausibly be handed.
const ELEMENTS: usize = 400_000;

/// The allowance. The reader's honest cost here is one node — the array
/// it opened before it refused — in a vector that starts at whatever the
/// allocator's smallest useful block is. The shortcuts' cost is
/// megabytes. Anything between the two is a change worth looking at
/// rather than a threshold worth loosening.
const ALLOWED_RISE: usize = 64 * 1024;

#[test]
fn refusing_a_huge_document_never_holds_the_document() {
    // Exact capacity, filled once: a doubling string would raise the
    // very peak this test is about to measure against.
    let element = "1,";
    let mut text = String::with_capacity(2 + ELEMENTS * element.len() + 1);
    // A stray comma where the first element goes, so the refusal lands
    // at the second character and every byte after it is work the reader
    // must never have done.
    text.push_str("[,");
    for _ in 0..ELEMENTS {
        text.push_str(element);
    }
    text.push(']');

    let before = counters::snapshot();
    let error = Json::parse(text.as_bytes()).expect_err("a comma begins no value");
    let after = counters::snapshot();

    assert_eq!(error.at(), 1, "the refusal is at the second character");

    let rise = after.peak_bytes.saturating_sub(before.peak_bytes);
    // What the two shortcuts would have cost, computed here rather than
    // asserted as a magic number, so the margin is visible and stays
    // true if the node record changes size.
    let reserved = text.len() * size_of::<usize>() * 4;
    let copied = text.len();
    assert!(
        reserved > 16 * ALLOWED_RISE && copied > 8 * ALLOWED_RISE,
        "this test is only meaningful while the shortcuts it guards would be large: \
         a reserved table {reserved} bytes, a copy of the source {copied} bytes, \
         allowance {ALLOWED_RISE}"
    );
    assert!(
        rise <= ALLOWED_RISE,
        "refusing a {} byte document raised peak memory by {rise} bytes; a table reserved from \
         the source length would cost about {reserved} and a copy of the source about {copied}, \
         so this reads like one of them came back",
        text.len(),
    );
}
