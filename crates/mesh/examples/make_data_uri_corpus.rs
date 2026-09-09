//! Write the `data:` URI seed corpus.
//!
//! **Every seed here is text this program wrote.** A URI carrying a real
//! resource would carry somebody's model or image with it; these carry
//! counting patterns and short words.
//!
//! A fuzzer finds `data:` quickly — fixed bytes at offset zero are what a
//! coverage-guided search is best at. What it does not find on its own is
//! **a payload that decodes**: four characters from a 64-character
//! alphabet, correctly padded, with the unused bits of the final group
//! zeroed. Random mutation produces a refusal essentially every time, so
//! without seeds that decode, the whole second half of the reader — the
//! bit assembly, the padding arithmetic, the canonical check — is reached
//! by luck rather than by search.
//!
//! The seeds below therefore come in pairs: for each rule, one text that
//! satisfies it and one that misses it by a single character. **A
//! mutation's starting point sits on both sides of every rule.**
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-mesh --example make_data_uri_corpus
//! ```
//!
//! Existing files are left alone. The fuzzer adds its own finds to this
//! directory over time, and this program must never delete them.

// The crate bans filesystem access because the library never touches a
// file -- a caller that reads one owns it, and owns the bound on reading
// it. This program is that caller: writing the corpus is its whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes, never a path.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;
use std::process::ExitCode;

/// The media type a buffer's payload is required to declare, which is
/// also the one a reader of these seeds will be checking for.
const BUFFER: &str = "application/octet-stream";

// The encoder is `crates/mesh/tests/shared/base64_encode.rs`, included
// here and by four other targets, so a seed can be described by the
// bytes it should decode to rather than by a string somebody typed.
#[path = "../tests/shared/base64_encode.rs"]
mod base64_encode;
use base64_encode::encode;

/// A URI with the buffer media type and the given payload text.
fn buffer(payload: &str) -> String {
    format!("data:{BUFFER};base64,{payload}")
}

/// Seeds that decode, which is what the interesting half of the reader
/// needs in order to be reached at all.
fn readable_seeds() -> Vec<(String, String)> {
    let mut seeds = vec![
        // **One seed per padding case**, because the three cases run
        // different arithmetic and only one of them has no canonical
        // check to make.
        ("no-padding".to_owned(), buffer(&encode(b"foo"))),
        ("one-pad".to_owned(), buffer(&encode(b"fo"))),
        ("two-pads".to_owned(), buffer(&encode(b"f"))),
        // Every alphabet entry, including `+` and `/`, which a payload
        // built from words never reaches.
        (
            "every-byte-value".to_owned(),
            buffer(&encode(&(0..=255).collect::<Vec<u8>>())),
        ),
        // Nothing at all, which is legal here and a question for
        // whoever wanted the resource.
        ("empty-payload".to_owned(), buffer("")),
        // The other media type a buffer may declare, so a reader
        // checking for one name is not the only path exercised.
        (
            "gltf-buffer-type".to_owned(),
            format!("data:application/gltf-buffer;base64,{}", encode(b"foo")),
        ),
        // A type with a parameter after it, and a parameter with no
        // type: the two shapes that make the split at the first `;`
        // worth having.
        (
            "type-with-parameter".to_owned(),
            format!("data:text/plain;charset=UTF-8;base64,{}", encode(b"hello!")),
        ),
        (
            "parameter-without-type".to_owned(),
            format!("data:;charset=UTF-8;base64,{}", encode(b"hello!")),
        ),
        (
            "no-type-at-all".to_owned(),
            format!("data:;base64,{}", encode(b"abc")),
        ),
        // The marker in a case no exporter writes, which is the one
        // leniency this reader allows and therefore the one that could
        // be removed without a seed here.
        (
            "marker-in-capitals".to_owned(),
            format!("data:{BUFFER};BASE64,{}", encode(b"foo")),
        ),
        // Long enough that a mutation lands in the middle of a group
        // rather than always at an edge.
        (
            "long-payload".to_owned(),
            buffer(&encode(
                &(0..96_u32)
                    .map(|n| u8::try_from(n * 5 % 251).unwrap_or_default())
                    .collect::<Vec<u8>>(),
            )),
        ),
    ];

    // A payload for each length modulo three, so the final group's
    // arithmetic is entered from every state.
    for len in 1_usize..=9 {
        let bytes: Vec<u8> = (0..len)
            .map(|n| u8::try_from(n * 37 % 256).unwrap_or_default())
            .collect();
        seeds.push((format!("length-{len}"), buffer(&encode(&bytes))));
    }
    seeds
}

/// Seeds that are refused, one per rule, each missing by one character.
fn refused_seeds() -> Vec<(String, String)> {
    let good = encode(b"foo");
    vec![
        // Not a URI of this kind at all: the relative path a document
        // uses to name a second file.
        ("relative-path".to_owned(), "geometry.bin".to_owned()),
        (
            "absolute-url".to_owned(),
            "https://example.invalid/a.bin".to_owned(),
        ),
        // Everything before the payload, and no payload.
        ("no-comma".to_owned(), format!("data:{BUFFER};base64")),
        // The percent-encoded spelling, which RFC 2397 allows and this
        // reader does not.
        (
            "percent-encoded".to_owned(),
            format!("data:{BUFFER},%00%01"),
        ),
        // `base64` as part of the type rather than as a parameter.
        (
            "marker-without-semicolon".to_owned(),
            "data:base64,Zm9v".to_owned(),
        ),
        // A character outside the alphabet, and the two that belong to
        // the other alphabet.
        ("space-in-payload".to_owned(), buffer("Zm v")),
        ("newline-in-payload".to_owned(), buffer("Zm\nv")),
        ("url-safe-dash".to_owned(), buffer("Zm9-")),
        ("url-safe-underscore".to_owned(), buffer("Zm9_")),
        // Lengths that are not whole groups, on both sides of one.
        ("three-characters".to_owned(), buffer("Zm9")),
        ("five-characters".to_owned(), buffer("Zm9vZ")),
        // Padding where padding cannot be.
        ("padding-mid-payload".to_owned(), buffer("Zg==Zg==")),
        ("padding-at-the-front".to_owned(), buffer("=m9v")),
        ("four-pads".to_owned(), buffer("Zm9v====")),
        ("digit-after-padding".to_owned(), buffer("Zm=v")),
        // **The rule a lenient decoder drops.** `QR==` would decode to
        // the same single byte as `QQ==`; the bits that differ reach no
        // output byte.
        ("stray-bits-two-pads".to_owned(), buffer("QR==")),
        ("stray-bits-one-pad".to_owned(), buffer("Zm9=")),
        // A near miss on the scheme, which is what a truncated or
        // reassembled document looks like.
        (
            "scheme-misspelt".to_owned(),
            format!("dat:{BUFFER};base64,{good}"),
        ),
        ("scheme-truncated".to_owned(), "data".to_owned()),
        // A wide character where the scheme's fifth byte would be: no
        // character boundary there, and a reader that sliced instead of
        // comparing bytes would end the process rather than refuse.
        (
            "wide-character-at-the-scheme".to_owned(),
            "dat\u{e9}foo".to_owned(),
        ),
        (
            "wide-character-before-the-comma".to_owned(),
            "data:text/\u{e9}\u{e9}\u{e9}\u{e9},Zm9v".to_owned(),
        ),
    ]
}

/// Seeds that are not text, which is the shape a `String` cannot hold.
///
/// **Every other seed here is valid UTF-8, and that was a gap.** A URI
/// arrives as bytes; whoever reads it has to turn those into text first,
/// and the byte strings that cannot be turned into text cleanly are
/// exactly the ones where that conversion does something. A corpus of
/// nothing but well-formed text never replays that step.
fn raw_seeds() -> Vec<(String, Vec<u8>)> {
    let head = format!("data:{BUFFER};base64,").into_bytes();
    let with = |tail: &[u8]| {
        let mut bytes = head.clone();
        bytes.extend_from_slice(tail);
        bytes
    };
    vec![
        // A continuation byte with nothing to continue, in the payload.
        (
            "lone-continuation-byte".to_owned(),
            with(&[0xFF, b'm', b'9', b'v']),
        ),
        // A truncated multi-byte sequence, which is what a file cut in
        // half in the wrong place looks like.
        ("truncated-sequence".to_owned(), with(&[0xE2, 0x82])),
        // And one before the comma, where the media type is read.
        (
            "invalid-bytes-in-the-type".to_owned(),
            b"data:text/\xC3;base64,Zm9v".to_vec(),
        ),
    ]
}

fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/data_uri_read");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {error}", dir.display());
        return ExitCode::FAILURE;
    }

    let mut written = 0usize;
    let mut kept = 0usize;
    let seeds = readable_seeds()
        .into_iter()
        .chain(refused_seeds())
        .map(|(name, text)| (name, text.into_bytes()))
        .chain(raw_seeds());
    for (name, text) in seeds {
        let path = dir.join(format!("{name}.uri"));
        if path.exists() {
            kept += 1;
            continue;
        }
        // **A generator that swallows a write failure and then reports
        // success is worse than one that crashes**: the caller sees a
        // count and believes the corpus is whole.
        if let Err(error) = std::fs::write(&path, &text) {
            eprintln!("cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        written += 1;
    }

    println!(
        "{written} written, {kept} already present, in {}",
        dir.display()
    );
    ExitCode::SUCCESS
}
