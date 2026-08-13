//! The refusal path holds nothing proportional to the file it refuses.
//!
//! This is a regression test for a fixed defect, and the defect is worth
//! stating because the code that had it looked entirely reasonable: the
//! reader collected every line of the input into a vector and then
//! reserved an event vector from that line count, both before validating
//! anything. On a megabyte of garbage refused at its second line, that
//! held tens of megabytes to say "no" — and the reservation was a length
//! taken from untrusted input, which on a 32-bit target is a `capacity
//! overflow` abort rather than a returned refusal, in a crate whose
//! contract says nothing here panics.
//!
//! The fix is small — walk the iterator, start the vector empty — and a
//! comment saying so is not a gate. Reinstating either half compiles
//! cleanly and passes every other test in the crate, so the property is
//! measured here instead.
//!
//! Own process on purpose: the counters are process-wide, and this file
//! holds one test so that nothing allocates alongside the window being
//! measured. It compares a *peak*, not exact counts, because the
//! allocation the defect makes is a single enormous one that is freed
//! again as the refusal propagates — before-and-after totals would show
//! nothing. That is also why this test needs no exemption from
//! instrumented runs the way an exact-count test does: it asks whether
//! several megabytes appeared, and the answer to that does not depend on
//! whose allocator is underneath.

use renew_memory::{CountingAllocator, counters};
use renew_trace::{TraceEvent, parse};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const HEADER: &str = "renew-trace 1 sample=input_echo ticks=30 timestep_ns=16666667 budget=5\n";

/// Enough lines that a per-line reservation is unmistakable against the
/// text itself, and still a file a person could plausibly be handed.
const LINES: usize = 200_000;

/// The allowance. The reader's honest cost here is one small string for
/// the sample name and one for the refusal — hundreds of bytes. The
/// defect's cost is megabytes. Anything between the two is a change
/// worth looking at rather than a threshold worth loosening.
const ALLOWED_RISE: usize = 64 * 1024;

#[test]
fn refusing_a_huge_file_never_holds_the_file() {
    let junk = "nope\n";
    // Exact capacity, filled once: a doubling string would raise the
    // very peak this test is about to measure against.
    let mut text = String::with_capacity(HEADER.len() + LINES * junk.len());
    text.push_str(HEADER);
    for _ in 0..LINES {
        text.push_str(junk);
    }

    let before = counters::snapshot();
    let error = parse(&text).expect_err("`nope` is not a line keyword this reader knows");
    let after = counters::snapshot();

    // The refusal is on the *second* line, so every line after it is
    // work the reader must never have done.
    assert_eq!(error.line(), 2);

    let rise = after.peak_bytes.saturating_sub(before.peak_bytes);
    // What the two halves of the defect would have cost, computed here
    // rather than asserted as a magic number, so the margin is visible
    // and stays true if the event type changes size.
    let reserved = LINES * size_of::<(u64, TraceEvent)>();
    let collected = LINES * size_of::<&str>();
    assert!(
        reserved > 16 * ALLOWED_RISE && collected > 16 * ALLOWED_RISE,
        "this test is only meaningful while the defect it guards would be large: \
         reservation {reserved} bytes, line buffer {collected} bytes, allowance {ALLOWED_RISE}"
    );
    assert!(
        rise <= ALLOWED_RISE,
        "refusing a {} byte file raised peak memory by {rise} bytes; a per-line reservation \
         would cost about {reserved} and a buffer of every line about {collected}, so this \
         reads like one of them came back",
        text.len(),
    );
}
