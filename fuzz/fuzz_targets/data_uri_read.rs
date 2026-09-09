//! `data:` URIs, against text nobody wrote on purpose.
//!
//! **The property under test is that text and bytes are one to one.** A
//! base64 decoder is easy to write leniently — skip the whitespace,
//! ignore the bits no output byte uses, accept a missing pad — and every
//! one of those leniencies makes two different texts decode to the same
//! resource. This target holds the decoder to the strict reading by
//! re-encoding whatever it accepted and demanding the original payload
//! back, character for character.
//!
//! That is a stronger claim than "it did not crash", and it is the claim
//! that a corpus can actually falsify: a decoder that quietly tolerated a
//! stray bit would return bytes whose re-encoding differs from the input
//! in exactly that character.
//!
//! **Every input is text here, and that costs something worth naming.**
//! A URI is text by definition, so the reader takes `&str` and these
//! bytes are converted lossily rather than discarded when they are not
//! UTF-8. That keeps every input useful, but it also means **a raw byte
//! above 0x7F never reaches the decoder as itself**: it arrives as the
//! replacement character, three bytes wide, sitting wherever the fuzzer
//! put a bad one. So what is under attack is the wide character landing
//! at an awkward offset — which is a real hazard for a reader that
//! slices strings — and not the byte the fuzzer chose. The alphabet's
//! rejection of `0x80..=0xFF` is reached by the suite beside the crate
//! rather than from here.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_mesh::data_uri;

// **The encoder is deliberately not the crate's**, because the crate has
// none: nothing in the engine writes a `data:` URI. Keeping it separate
// is also what makes the round trip evidence — a round trip through one
// body of code proves only that the code agrees with itself.
#[path = "../../crates/mesh/tests/shared/base64_encode.rs"]
mod base64_encode;
use base64_encode::encode;

/// Whether `part` is a subslice of `whole`, by address rather than by
/// content.
fn borrowed_from(part: &[u8], whole: &[u8]) -> bool {
    let base = whole.as_ptr() as usize;
    let start = part.as_ptr() as usize;
    let end = start.saturating_add(part.len());
    start >= base && end <= base.saturating_add(whole.len())
}

fuzz_target!(|data: &[u8]| {
    let uri = String::from_utf8_lossy(data);

    let Ok(read) = data_uri::read(&uri) else {
        // A refusal is an answer. Which refusal is the suite's business
        // beside the crate; that the call returned at all is this
        // target's.
        return;
    };

    // The text came back borrowed, which is the whole reason those two
    // fields are slices rather than owned strings.
    assert!(
        borrowed_from(read.media_type.as_bytes(), uri.as_bytes()),
        "the media type must borrow the URI"
    );
    assert!(
        borrowed_from(read.parameters.as_bytes(), uri.as_bytes()),
        "the parameters must borrow the URI"
    );

    let comma = uri.find(',').expect("a URI that read has a comma");
    let payload = &uri[comma + 1..];

    // Three bytes out per four characters in, less one for each pad.
    let pads = payload.bytes().filter(|&byte| byte == b'=').count();
    assert_eq!(
        read.bytes.len(),
        payload.len() / 4 * 3 - pads,
        "{} bytes out of a {}-character payload with {pads} pads",
        read.bytes.len(),
        payload.len()
    );

    // **The claim this target exists for.** Anything the decoder accepts
    // must be what an encoder would have written, or two texts decode to
    // one resource and a document's bytes stop determining what it means.
    assert_eq!(
        encode(&read.bytes),
        payload,
        "what decoded does not re-encode to itself"
    );

    // Reading twice answers the same, which is what makes a corpus worth
    // recording at all.
    let again = data_uri::read(&uri).expect("what read once reads again");
    assert_eq!(again, read, "the same text read to the same bytes");
});
