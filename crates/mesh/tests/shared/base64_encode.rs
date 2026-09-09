//! Canonical base64, written once for the four harnesses that need it.
//!
//! **This file is included by five targets in two crates**, with
//! `#[path]`, because none of them can reach the others' code any other
//! way:
//!
//! * `crates/mesh/examples/make_data_uri_corpus.rs` writes the seeds.
//! * `crates/mesh/tests/data_uri.rs` builds fixtures and round-trips.
//! * `crates/mesh/tests/properties.rs` generates URIs to attack.
//! * `crates/mesh/tests/corpus_replay.rs` replays the seeds at every
//!   merge.
//! * `fuzz/fuzz_targets/data_uri_read.rs` mutates them.
//!
//! Cargo compiles a `tests/` subdirectory for nobody, which is what
//! makes this a shared file rather than a sixth target.
//!
//! # Why the engine does not ship this
//!
//! Nothing in the engine writes a `data:` URI. An encoder in the crate
//! would be code with no caller, and the crate is a reader of things
//! other tools produce.
//!
//! It also has to stay separate from the decoder to be worth anything.
//! **A round trip through one body of code proves only that the code
//! agrees with itself**; this encoder is written from the specification's
//! description rather than by reading the decoder backwards, so a shared
//! misunderstanding does not cancel out.
//!
//! # What "canonical" means here, and why it is the whole point
//!
//! Three bytes become four characters. When the input does not divide by
//! three, the final group is short, and the bits no input byte reached
//! are **written as zero** and the group is padded to four with `=`.
//!
//! Those zero bits are what make the encoding reversible. A decoder that
//! ignores them accepts `QQ==` and `QR==` as the same single byte `A`,
//! and at that moment two different texts name one resource. This
//! encoder never writes the second spelling, which is why "re-encoding
//! what decoded reproduces the input exactly" is a claim worth asserting.

#![allow(
    dead_code,
    reason = "the alphabet is an implementation detail of the one function here"
)]

/// The standard alphabet. Not base64url: `+` and `/`, never `-` and `_`.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes as canonical, padded base64.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        // The group's bytes, left-aligned in twenty-four bits. Whatever
        // is missing stays zero, which is exactly the rule above.
        let mut bits = 0_u32;
        for (index, &byte) in group.iter().enumerate() {
            bits |= u32::from(byte) << (16 - index * 8);
        }
        // One character per six bits, and one more character than the
        // group has bytes: three bytes give four, two give three, one
        // gives two.
        for index in 0..=group.len() {
            let sextet = (bits >> (18 - index * 6)) & 0b11_1111;
            out.push(char::from(ALPHABET[sextet as usize]));
        }
        for _ in group.len()..3 {
            out.push('=');
        }
    }
    out
}
