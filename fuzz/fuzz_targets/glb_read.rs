//! The binary glTF container, against bytes nobody wrote on purpose.
//!
//! **This target fuzzes framing and nothing else.** The reader hands
//! back two byte slices and parses neither, so what is under test is
//! exactly the arithmetic: a twelve-byte header whose third field is a
//! total length, then a chain of chunks each located by the length of
//! the one before it. Every one of those numbers arrives from the file.
//!
//! That chain is what makes this worth a target of its own. A single
//! wrong length does not merely produce one bad slice — it moves the
//! cursor, so every chunk after it is read at an offset the writer never
//! intended, and a reader that recovered by scanning would happily find
//! a "chunk" in the middle of somebody's vertex data.
//!
//! **The properties asserted below are the ones a caller relies on**,
//! and they are all about borrowing: whatever comes back must be a
//! subslice of the input, inside it, and accounted for by the header's
//! own total. A container that read but handed back a slice reaching
//! past what it validated would be the bug this target exists to find,
//! and it would not show up as a panic on its own.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_mesh::glb;

/// Whether `part` is a subslice of `whole`, by address rather than by
/// content.
///
/// Content equality would pass for a copy, and a copy is precisely what
/// this reader promises not to make.
fn borrowed_from(part: &[u8], whole: &[u8]) -> bool {
    let base = whole.as_ptr() as usize;
    let start = part.as_ptr() as usize;
    let end = start.saturating_add(part.len());
    start >= base && end <= base.saturating_add(whole.len())
}

fuzz_target!(|data: &[u8]| {
    let Ok(container) = glb::read(data) else {
        // A refusal is an answer. Which refusal is the suite's business
        // beside the crate; that the call returned at all is this
        // target's.
        return;
    };

    assert!(
        borrowed_from(container.json, data),
        "the JSON chunk must borrow the caller's bytes, not copy them"
    );
    assert_eq!(
        container.json.len() % 4,
        0,
        "a chunk that read is a whole number of four-byte words"
    );

    if let Some(binary) = container.binary {
        assert!(
            borrowed_from(binary, data),
            "the binary chunk must borrow the caller's bytes"
        );
        assert_eq!(
            binary.len() % 4,
            0,
            "a chunk that read is a whole number of four-byte words"
        );
    }

    // **The chunks and their headers fit inside the file**, which is the
    // arithmetic claim: twelve bytes of preamble, eight per chunk
    // header, and the payloads. A reader that let a length wrap would
    // hand back slices whose total exceeded what it was given.
    let payloads = container.json.len() + container.binary.map_or(0, <[u8]>::len);
    let headers = 8 * (1 + usize::from(container.binary.is_some()));
    assert!(
        payloads + headers + 12 <= data.len(),
        "what came back does not fit in what went in: {payloads} + {headers} + 12 > {}",
        data.len()
    );

    // Reading twice answers the same, which is what makes the refusals
    // above worth anything: a reader whose answer depended on anything
    // but its input could not be reasoned about from a corpus at all.
    let again = glb::read(data).expect("what read once reads again");
    assert_eq!(again, container, "the same bytes read to the same container");
});
