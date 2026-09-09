//! The PNG decoder, against bytes nobody wrote on purpose.
//!
//! The decoder takes a byte string from disk and hands back either an
//! image or a named refusal. Both are answers; a panic, a hang, an
//! allocation sized by a number the file chose, or a read past the end
//! are not, and this target exists to look for those.
//!
//! It is the widest untrusted surface in the tree: a signature, a chunk
//! walk with per-chunk checksums, a header with six independent fields,
//! a palette, and then a from-scratch inflate feeding a filter loop that
//! reconstructs each row from the one above it. Almost every one of
//! those stages decides how much memory to reserve from a number the
//! file supplies, which is the shape this target is pointed at.
//!
//! The unit suite already asserts a named refusal for each malformed
//! variant it knows to write. What this adds is the ones nobody thought
//! of — the fuzzer keeps inputs that reach new branches, so it works its
//! way past the signature and the header into the inflate and the filter
//! loop, where random bytes almost never land on their own.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The result is deliberately discarded: the property under test is
    // that the call returns at all. Whether a given file decodes to the
    // right pixels is the unit suite's business, and asserting on it
    // here would fail the corpus rather than the code.
    let _ = renew_png::decode::decode(data);
});
