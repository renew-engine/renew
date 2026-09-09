//! The JSON reader, against bytes nobody wrote on purpose.
//!
//! The reader takes a byte string from wherever a caller got one and
//! hands back either a document or a named refusal. Both are answers; a
//! panic, a hang, a stack overflow, an allocation sized by a number the
//! document chose, or a read past the end are not, and this target
//! exists to look for those.
//!
//! **The bytes go in unfiltered, invalid UTF-8 included.** The
//! text-format target beside this one guards its input with
//! `from_utf8` — a reader that takes text has no other honest choice —
//! and that leaves the "is this even text" question untested. This
//! reader takes bytes precisely so a caller holding one chunk of a
//! larger file does not have to answer it, so the fuzzer must be allowed
//! to ask.
//!
//! **A document that parses is then walked**, and every typed question
//! is asked of every value in it. Validation claims that an accepted
//! document has nothing deferred in it: no escape left to trip over, no
//! span that can run off the end, no container whose children outrun the
//! table they live in. The walk is what puts that claim under the
//! fuzzer rather than under a comment — and it is written with an
//! explicit stack, because a recursive walk would meet a document nested
//! sixty-four deep with the stack overflow the reader itself refuses to
//! have.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_json::{Json, Kind, Value};

/// Ask every question of every value, and discard every answer.
///
/// The results are deliberately thrown away: the property under test is
/// that the calls return at all. Whether a given document holds the
/// right numbers is the unit suite's business, and asserting on it here
/// would fail the corpus rather than the code.
fn walk(root: Value<'_>) {
    let mut pending = vec![root];
    while let Some(value) = pending.pop() {
        let _ = value.at();
        let _ = value.text();
        let _ = value.kind();
        let _ = value.is_null();
        let _ = value.len();
        let _ = value.is_empty();
        let _ = value.as_bool();
        let _ = value.as_u32();
        let _ = value.as_u64();
        let _ = value.as_i64();
        let _ = value.as_f32();
        let _ = value.as_f64();
        let _ = value.index(0);
        let _ = value.get("");
        if let Ok(text) = value.as_str() {
            // Decoding is the half the parse deferred, so it is the half
            // most worth reaching: every escape it walks was validated
            // and never read back until here.
            let _ = text.as_plain();
            let _ = text.eq_str("");
            let mut buffer = String::new();
            text.decode_into(&mut buffer);
        }
        match value.kind() {
            Kind::Array => {
                if let Ok(elements) = value.elements() {
                    pending.extend(elements);
                }
            }
            Kind::Object => {
                if let Ok(entries) = value.entries() {
                    for (name, member) in entries {
                        // Looking a member up by the name the document
                        // just handed over is what exercises the
                        // comparison path, escapes and all.
                        let _ = value.get_all(&name.decode()).count();
                        pending.push(member);
                    }
                }
            }
            _ => {}
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(document) = Json::parse(data) {
        walk(document.root());
    }
});
