//! The pack reader, against bytes nobody wrote on purpose.
//!
//! The reader takes a byte string from disk and hands back either a pack
//! or a refusal. Both are answers; a panic, a hang, or a read past the
//! end are not, and this target exists to look for those.
//!
//! The suite already asserts the same property over generated inputs with
//! a fixed budget. What this adds is coverage guidance — the fuzzer keeps
//! the inputs that reach new branches, so it works its way past the magic
//! number and the header into the entry table, where random bytes almost
//! never land.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The result is deliberately discarded: the property under test is
    // that the call returns at all. What it returns is the round-trip
    // suite's business, and asserting on it here would fail the corpus
    // rather than the code.
    let _ = renew_asset::Pack::read(data);
});
