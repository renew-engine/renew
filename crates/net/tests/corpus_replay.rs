//! The recorded fuzz corpus, replayed on the stable toolchain.
//!
//! The fuzz lane runs on a schedule and needs nightly. This runs on every
//! push, on stable, and holds the same two claims the target holds: every
//! committed input gets an *answer* — a datagram or a named refusal, never
//! a panic — and every input that reads back re-encodes to the exact bytes
//! it came from.
//!
//! The second claim is the one worth gating on. A crash is loud wherever
//! it happens; a second byte string decoding to the same datagram is
//! silent, and it would reach a player as a desync nobody can reproduce.

// The tripwire ban on filesystem access protects engine code; replaying
// committed corpus artifacts is this test's whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes and never a path,
// but this harness's whole subject is a directory of committed files.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;

use renew_net::MAX_DATAGRAM_BYTES;
use renew_net::wire::{self, Body};

/// The committed corpus never shrinks below this without someone
/// noticing. Set below the count that shipped — 33 after minimisation —
/// with room for a future `cmin` to drop redundant inputs, and far enough
/// above zero that an empty or unreadable directory fails loudly rather
/// than passing with nothing to say.
const LOW_WATER: usize = 20;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/net_datagram")
}

#[test]
fn every_recorded_corpus_input_gets_an_answer() {
    let dir = corpus_dir();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("the committed corpus at {dir:?} must exist: {error}"));

    let mut replayed = 0usize;
    let mut accepted = 0usize;
    for entry in entries {
        let entry = entry.expect("corpus entries are readable");
        let bytes = std::fs::read(entry.path()).expect("corpus files are readable");

        if let Ok(datagram) = wire::read(&bytes) {
            let mut again = [0u8; MAX_DATAGRAM_BYTES];
            let addressing = datagram.header.addressing();
            let written = match datagram.body {
                Body::Hello(body) => wire::write_hello(&mut again, addressing, &body)
                    .expect("what the reader accepted, the writer must accept"),
                Body::Digest(body) => wire::write_digest(&mut again, addressing, &body),
                Body::Bye(body) => wire::write_bye(&mut again, addressing, &body),
                Body::Chat(body) => {
                    wire::write_chat(&mut again, addressing, body.sequence, body.text())
                        .expect("what the reader accepted, the writer must accept")
                }
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
                &bytes[..],
                "{:?} was accepted but did not re-encode to its own bytes — the format has two \
                 spellings of one fact",
                entry.path()
            );
            accepted += 1;
        }
        replayed += 1;
    }

    assert!(
        replayed >= LOW_WATER,
        "{replayed} corpus files replayed, below the committed floor of {LOW_WATER} — a corpus \
         that quietly emptied would leave this gate green and measuring nothing"
    );
    assert!(
        accepted >= 1,
        "{replayed} corpus files replayed and not one of them read as a datagram. A corpus made \
         entirely of refusals exercises the reader's first few branches and nothing else, and the \
         re-encoding claim above would never run — the same drift tripwire the document gate keeps"
    );
}
