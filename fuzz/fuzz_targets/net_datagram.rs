//! The lockstep datagram reader, against bytes nobody wrote on purpose.
//!
//! This is the first parser in the engine whose real input arrives from
//! another machine, so the bar it has to clear is the highest one: the
//! reader is documented as **total over every possible byte string**, and
//! a release build aborts on panic, which makes any counterexample a
//! remote process abort rather than a wrong answer.
//!
//! Like the document target, this one goes a step past "did it crash".
//! When the bytes DO read, it re-encodes the datagram and demands the
//! bytes back **exactly** — the canonical-encoding claim, which says the
//! format admits one spelling per fact. A fuzzer that finds two byte
//! strings decoding to the same datagram breaks that claim, and the
//! assertion below is where it would surface. That check is worth more
//! here than a crash oracle alone, because a second spelling is not a
//! crash: it is two peers agreeing to disagree, and it would show up in a
//! shipped game as a desync nobody could reproduce.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_net::MAX_DATAGRAM_BYTES;
use renew_net::wire::{self, Body};

fuzz_target!(|data: &[u8]| {
    let Ok(datagram) = wire::read(data) else {
        return;
    };

    let mut again = [0u8; MAX_DATAGRAM_BYTES];
    let addressing = datagram.header.addressing();
    let written = match datagram.body {
        Body::Hello(body) => wire::write_hello(&mut again, addressing, &body)
            .expect("what the reader accepted, the writer must accept"),
        Body::Digest(body) => wire::write_digest(&mut again, addressing, &body),
        Body::Bye(body) => wire::write_bye(&mut again, addressing, &body),
        Body::Chat(body) => wire::write_chat(&mut again, addressing, body.sequence, body.text())
            .expect("what the reader accepted, the writer must accept"),
        Body::Inputs(body) => wire::write_inputs(
            &mut again,
            addressing,
            body.first_tick,
            body.count,
            body.input_bytes,
            body.frames(),
        )
        .expect("what the reader accepted, the writer must accept"),
    };

    assert_eq!(
        &again[..written],
        data,
        "an accepted datagram must re-encode to the exact bytes it came from"
    );
});
