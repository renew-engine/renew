//! The codec's refusal table, its round-trip oracle, and two sweeps.
//!
//! An integration test rather than a unit one, deliberately: every rule
//! below is reachable through the public API, and a test that can only be
//! written from inside the crate is a test of an implementation rather
//! than of a contract. The one thing it costs is the crate root's
//! `indexing_slicing` deny, which does not reach here — and indexing is
//! the clearest way to say "this byte, that value" when the subject of
//! the test is one byte.

use renew_net::wire::{
    BYE_DATAGRAM_BYTES, Body, ByeBody, DIGEST_DATAGRAM_BYTES, DigestBody, HEADER_BYTES,
    HELLO_DATAGRAM_BYTES, Header, HelloBody, INPUTS_MIN_DATAGRAM_BYTES, Kind, WIRE_VERSION,
    WireError, WriteError, read, write_bye, write_digest, write_hello, write_inputs,
};
use renew_net::{
    INPUT_REDUNDANCY, INPUT_WINDOW, MAX_DATAGRAM_BYTES, MAX_INPUT_BYTES, MAX_PEERS, PeerId,
};

type Buffer = [u8; MAX_DATAGRAM_BYTES];

/// Body offsets, absolute in the datagram, so a test says where it pokes.
const HELLO_PEER_COUNT: usize = HEADER_BYTES + 32;
const HELLO_INPUT_BYTES: usize = HEADER_BYTES + 33;
const HELLO_INPUT_DELAY: usize = HEADER_BYTES + 34;
const HELLO_DIGEST_PERIOD: usize = HEADER_BYTES + 35;
const HELLO_PAD: usize = HEADER_BYTES + 36;
const INPUTS_FIRST_TICK: usize = HEADER_BYTES;
const INPUTS_COUNT: usize = HEADER_BYTES + 8;
const INPUTS_WIDTH: usize = HEADER_BYTES + 9;
const INPUTS_PAD: usize = HEADER_BYTES + 10;

// The three helpers below are the fixture, not the assertion: a refusal
// here is the fixture itself being wrong, which should stop the run with
// the reason at the top of the output rather than at whichever test
// happened to call it first. The same accommodation the trace codec's
// golden fixture makes.
#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn seat(index: u8) -> PeerId {
    PeerId::new(index).expect("a seat the test chose in range")
}

fn header(kind: Kind) -> Header {
    Header {
        kind,
        sender: seat(1),
        session: 0x0123_4567_89ab_cdef,
    }
}

fn hello_body() -> HelloBody {
    HelloBody {
        agreement_digest: 0xdead_beef_feed_face,
        content: 0x1111_2222_3333_4444,
        rules: 0x5555_6666_7777_8888,
        seed: 99,
        peer_count: 4,
        input_bytes: 3,
        input_delay: 2,
        digest_period: 30,
    }
}

fn a_hello() -> (Buffer, usize) {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = write_hello(&mut out, header(Kind::Hello), &hello_body());
    (out, len)
}

/// Three frames of two bytes each, from tick 4,000.
#[allow(
    clippy::expect_used,
    reason = "see `seat`: the fixture's own failure is the report"
)]
fn an_inputs() -> (Buffer, usize) {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = write_inputs(
        &mut out,
        header(Kind::Inputs),
        4_000,
        2,
        3,
        &[1, 2, 3, 4, 5, 6],
    )
    .expect("arguments inside every ceiling");
    (out, len)
}

fn a_digest() -> (Buffer, usize) {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let body = DigestBody {
        tick: 600,
        state_digest: 0xabcd,
        input_digest: 0x1234,
    };
    let len = write_digest(&mut out, header(Kind::Digest), &body);
    (out, len)
}

fn a_bye() -> (Buffer, usize) {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = write_bye(&mut out, header(Kind::Bye), &ByeBody { tick: 0x0102_0304 });
    (out, len)
}

#[allow(
    clippy::expect_used,
    reason = "a datagram that was accepted where a refusal was the subject of the test must say so here, naming the case"
)]
fn refusal(bytes: &[u8]) -> WireError {
    read(bytes).expect_err("these bytes should have been refused")
}

// ---- round trips: the canonical-encoding oracle ----

#[test]
fn a_hello_round_trips_to_the_same_bytes() {
    let (bytes, len) = a_hello();
    assert_eq!(len, HELLO_DATAGRAM_BYTES);

    let datagram = read(&bytes[..len]).expect("a datagram this crate wrote");
    assert_eq!(datagram.header, header(Kind::Hello));
    let Body::Hello(body) = datagram.body else {
        panic!("a Hello decoded as something else")
    };
    assert_eq!(body, hello_body());

    let mut again = [0u8; MAX_DATAGRAM_BYTES];
    let again_len = write_hello(&mut again, datagram.header, &body);
    assert_eq!((&again[..again_len], again_len), (&bytes[..len], len));
}

#[test]
fn a_digest_round_trips_to_the_same_bytes() {
    let (bytes, len) = a_digest();
    assert_eq!(len, DIGEST_DATAGRAM_BYTES);

    let datagram = read(&bytes[..len]).expect("a datagram this crate wrote");
    let Body::Digest(body) = datagram.body else {
        panic!("a Digest decoded as something else")
    };
    assert_eq!(
        (body.tick, body.state_digest, body.input_digest),
        (600, 0xabcd, 0x1234)
    );

    let mut again = [0u8; MAX_DATAGRAM_BYTES];
    let again_len = write_digest(&mut again, datagram.header, &body);
    assert_eq!(&again[..again_len], &bytes[..len]);
}

#[test]
fn a_bye_round_trips_to_the_same_bytes() {
    let (bytes, len) = a_bye();
    assert_eq!(len, BYE_DATAGRAM_BYTES);

    let datagram = read(&bytes[..len]).expect("a datagram this crate wrote");
    let Body::Bye(body) = datagram.body else {
        panic!("a Bye decoded as something else")
    };
    assert_eq!(body.tick, 0x0102_0304);

    let mut again = [0u8; MAX_DATAGRAM_BYTES];
    assert_eq!(write_bye(&mut again, datagram.header, &body), len);
    assert_eq!(&again[..len], &bytes[..len]);
}

#[test]
fn an_inputs_run_round_trips_with_every_frame_at_its_own_tick() {
    let (bytes, len) = an_inputs();
    assert_eq!(len, INPUTS_MIN_DATAGRAM_BYTES + 6);

    let datagram = read(&bytes[..len]).expect("a datagram this crate wrote");
    let Body::Inputs(body) = datagram.body else {
        panic!("an Inputs decoded as something else")
    };
    assert_eq!(
        (body.first_tick, body.count, body.input_bytes),
        (4_000, 3, 2)
    );

    let seen: Vec<(u64, Vec<u8>)> = body
        .iter()
        .map(|(tick, frame)| (tick, frame.to_vec()))
        .collect();
    assert_eq!(
        seen,
        vec![
            (4_000, vec![1, 2]),
            (4_001, vec![3, 4]),
            (4_002, vec![5, 6])
        ],
        "frames ascend from first_tick, and each is exactly input_bytes wide"
    );

    let mut again = [0u8; MAX_DATAGRAM_BYTES];
    let again_len = write_inputs(
        &mut again,
        datagram.header,
        body.first_tick,
        body.input_bytes,
        body.count,
        body.frames(),
    )
    .expect("what the reader accepted, the writer must be able to write");
    assert_eq!(&again[..again_len], &bytes[..len]);
}

#[test]
fn a_frame_index_past_the_count_is_none_rather_than_a_panic() {
    let (bytes, len) = an_inputs();
    let datagram = read(&bytes[..len]).expect("valid");
    let Body::Inputs(body) = datagram.body else {
        panic!("wrong kind")
    };
    assert_eq!(body.frame(0), Some(&[1u8, 2][..]));
    assert_eq!(body.frame(2), Some(&[5u8, 6][..]));
    assert_eq!(body.frame(3), None);
    assert_eq!(body.frame(u8::MAX), None);
}

// ---- the refusal table: one case per rule ----

#[test]
fn a_datagram_shorter_than_a_header_is_refused() {
    let (bytes, _) = a_hello();
    assert_eq!(
        refusal(&bytes[..HEADER_BYTES - 1]),
        WireError::TooShort { len: 15 }
    );
    assert_eq!(refusal(&[]), WireError::TooShort { len: 0 });
}

#[test]
fn a_datagram_past_the_ceiling_is_refused_before_anything_reads_it() {
    let oversized = [0u8; MAX_DATAGRAM_BYTES + 1];
    assert_eq!(
        refusal(&oversized),
        WireError::TooLong {
            len: MAX_DATAGRAM_BYTES + 1
        },
        "the ceiling is checked before magic, so an oversized buffer costs one comparison"
    );
}

#[test]
fn an_all_zero_buffer_names_no_datagram() {
    let zeroes = [0u8; HELLO_DATAGRAM_BYTES];
    assert_eq!(refusal(&zeroes), WireError::BadMagic { saw: [0, 0, 0, 0] });
}

#[test]
fn wrong_magic_is_refused_and_reports_what_it_saw() {
    let (mut bytes, len) = a_hello();
    bytes[0] = b'X';
    assert_eq!(
        refusal(&bytes[..len]),
        WireError::BadMagic { saw: *b"XNWL" }
    );
}

#[test]
fn any_version_but_the_one_is_refused_including_zero() {
    for version in [0u16, WIRE_VERSION + 1, u16::MAX] {
        let (mut bytes, len) = a_hello();
        bytes[4..6].copy_from_slice(&version.to_le_bytes());
        assert_eq!(
            refusal(&bytes[..len]),
            WireError::BadVersion { saw: version }
        );
    }
}

#[test]
fn an_unknown_kind_is_refused_rather_than_skipped() {
    for code in [0u8, 5, u8::MAX] {
        let (mut bytes, len) = a_hello();
        bytes[6] = code;
        assert_eq!(refusal(&bytes[..len]), WireError::UnknownKind { saw: code });
    }
}

#[test]
fn a_sender_past_the_seat_ceiling_is_refused() {
    for claimed in [MAX_PEERS, u8::MAX] {
        let (mut bytes, len) = a_hello();
        bytes[7] = claimed;
        assert_eq!(
            refusal(&bytes[..len]),
            WireError::SenderPastCeiling {
                saw: claimed,
                ceiling: MAX_PEERS
            }
        );
    }
}

#[test]
fn session_zero_is_the_pinned_illegal_value() {
    let (mut bytes, len) = a_hello();
    bytes[8..16].copy_from_slice(&0u64.to_le_bytes());
    assert_eq!(refusal(&bytes[..len]), WireError::SessionZero);
}

#[test]
fn a_trailing_byte_is_a_refusal_and_not_a_tolerance() {
    let (bytes, len) = a_hello();
    let longer = &bytes[..=len];
    assert_eq!(
        refusal(longer),
        WireError::SizeMismatch {
            kind: Kind::Hello,
            declared: HELLO_DATAGRAM_BYTES as u64,
            actual: len + 1,
        },
        "equality, never a lower bound: a tolerated tail is a second spelling of one fact"
    );
}

#[test]
fn an_inputs_run_whose_frames_do_not_match_its_counts_is_refused() {
    let (bytes, len) = an_inputs();
    assert_eq!(
        refusal(&bytes[..len - 1]),
        WireError::SizeMismatch {
            kind: Kind::Inputs,
            declared: len as u64,
            actual: len - 1
        }
    );
}

#[test]
fn an_inputs_datagram_too_short_to_declare_its_own_size_is_refused() {
    let (bytes, _) = an_inputs();
    assert_eq!(
        refusal(&bytes[..INPUTS_MIN_DATAGRAM_BYTES - 1]),
        WireError::SizeMismatch {
            kind: Kind::Inputs,
            declared: INPUTS_MIN_DATAGRAM_BYTES as u64,
            actual: INPUTS_MIN_DATAGRAM_BYTES - 1,
        },
        "the two bytes the size is computed from are proven present before either is read"
    );
}

#[test]
fn a_nonzero_reserved_byte_is_refused_in_both_pads() {
    for step in 0..4 {
        let (mut bytes, len) = a_hello();
        bytes[HELLO_PAD + step] = 1;
        assert_eq!(
            refusal(&bytes[..len]),
            WireError::PadNotZero {
                offset: HELLO_PAD + step,
                saw: 1
            }
        );
    }
    for step in 0..2 {
        let (mut bytes, len) = an_inputs();
        bytes[INPUTS_PAD + step] = 0x80;
        assert_eq!(
            refusal(&bytes[..len]),
            WireError::PadNotZero {
                offset: INPUTS_PAD + step,
                saw: 0x80
            }
        );
    }
}

#[test]
fn an_empty_inputs_run_is_refused() {
    let (mut bytes, len) = an_inputs();
    bytes[INPUTS_COUNT] = 0;
    assert_eq!(refusal(&bytes[..len]), WireError::FrameCountZero);
}

#[test]
fn a_run_past_the_redundancy_ceiling_is_refused_before_any_multiply() {
    let (mut bytes, len) = an_inputs();
    bytes[INPUTS_COUNT] = INPUT_REDUNDANCY + 1;
    assert_eq!(
        refusal(&bytes[..len]),
        WireError::FrameCountPastRedundancy {
            saw: INPUT_REDUNDANCY + 1,
            ceiling: INPUT_REDUNDANCY
        }
    );
}

#[test]
fn an_input_width_outside_its_range_is_refused_in_both_kinds() {
    let (mut bytes, len) = an_inputs();
    bytes[INPUTS_WIDTH] = 0;
    assert_eq!(refusal(&bytes[..len]), WireError::InputBytesZero);

    let (mut bytes, len) = an_inputs();
    bytes[INPUTS_WIDTH] = MAX_INPUT_BYTES + 1;
    assert_eq!(
        refusal(&bytes[..len]),
        WireError::InputBytesPastCeiling {
            saw: MAX_INPUT_BYTES + 1,
            ceiling: MAX_INPUT_BYTES
        }
    );

    let (mut bytes, len) = a_hello();
    bytes[HELLO_INPUT_BYTES] = 0;
    assert_eq!(refusal(&bytes[..len]), WireError::InputBytesZero);

    let (mut bytes, len) = a_hello();
    bytes[HELLO_INPUT_BYTES] = MAX_INPUT_BYTES + 1;
    assert_eq!(
        refusal(&bytes[..len]),
        WireError::InputBytesPastCeiling {
            saw: MAX_INPUT_BYTES + 1,
            ceiling: MAX_INPUT_BYTES
        }
    );
}

#[test]
fn a_run_that_would_leave_the_tick_space_is_refused() {
    // Hand-built, because the writer refuses to mint one. The reader's
    // rule has to be tested against bytes a hostile peer could send, not
    // only against bytes this crate can produce.
    let (mut bytes, len) = an_inputs();
    bytes[INPUTS_FIRST_TICK..INPUTS_FIRST_TICK + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(
        refusal(&bytes[..len]),
        WireError::TickOverflow {
            first_tick: u64::MAX,
            count: 3
        }
    );
}

#[test]
fn a_roster_outside_its_range_is_refused_at_both_ends() {
    for count in [0u8, 1, MAX_PEERS + 1, u8::MAX] {
        let (mut bytes, len) = a_hello();
        bytes[HELLO_PEER_COUNT] = count;
        assert_eq!(
            refusal(&bytes[..len]),
            WireError::PeerCountOutOfRange {
                saw: count,
                floor: 2,
                ceiling: MAX_PEERS
            },
            "one peer is not multiplayer, and nine does not fit the set"
        );
    }
}

#[test]
fn a_delay_the_window_cannot_buffer_is_refused() {
    let past = u8::try_from(INPUT_WINDOW).expect("the window fits a byte today");
    let (mut bytes, len) = a_hello();
    bytes[HELLO_INPUT_DELAY] = past;
    assert_eq!(
        refusal(&bytes[..len]),
        WireError::InputDelayPastWindow {
            saw: past,
            window: INPUT_WINDOW
        }
    );

    // The boundary the other way: one below the window is accepted, so the
    // test pins a range rather than only its outside.
    let (mut bytes, len) = a_hello();
    bytes[HELLO_INPUT_DELAY] = past - 1;
    assert!(read(&bytes[..len]).is_ok());
}

#[test]
fn a_zero_digest_period_is_refused() {
    let (mut bytes, len) = a_hello();
    bytes[HELLO_DIGEST_PERIOD] = 0;
    assert_eq!(refusal(&bytes[..len]), WireError::DigestPeriodZero);
}

// ---- the writers cannot mint what the reader refuses ----

#[test]
fn the_inputs_writer_refuses_every_argument_the_reader_would_reject() {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let head = header(Kind::Inputs);

    assert_eq!(
        write_inputs(&mut out, head, 0, 2, 0, &[]),
        Err(WriteError::FrameCount {
            saw: 0,
            ceiling: INPUT_REDUNDANCY
        })
    );
    assert_eq!(
        write_inputs(&mut out, head, 0, 1, INPUT_REDUNDANCY + 1, &[0; 9]),
        Err(WriteError::FrameCount {
            saw: INPUT_REDUNDANCY + 1,
            ceiling: INPUT_REDUNDANCY
        })
    );
    assert_eq!(
        write_inputs(&mut out, head, 0, 0, 1, &[]),
        Err(WriteError::InputBytes {
            saw: 0,
            ceiling: MAX_INPUT_BYTES
        })
    );
    assert_eq!(
        write_inputs(&mut out, head, 0, MAX_INPUT_BYTES + 1, 1, &[0; 17]),
        Err(WriteError::InputBytes {
            saw: MAX_INPUT_BYTES + 1,
            ceiling: MAX_INPUT_BYTES
        })
    );
    assert_eq!(
        write_inputs(&mut out, head, 0, 2, 3, &[0; 5]),
        Err(WriteError::FramesLength {
            saw: 5,
            expected: 6
        }),
        "refused rather than truncated: a short write is a second spelling of a shorter fact"
    );
    assert_eq!(
        write_inputs(&mut out, head, u64::MAX, 1, 2, &[0; 2]),
        Err(WriteError::TickOverflow {
            first_tick: u64::MAX,
            count: 2
        })
    );
}

#[test]
fn the_widest_legal_run_writes_exactly_the_ceiling() {
    let frames = [7u8; (INPUT_REDUNDANCY as usize) * (MAX_INPUT_BYTES as usize)];
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = write_inputs(
        &mut out,
        header(Kind::Inputs),
        0,
        MAX_INPUT_BYTES,
        INPUT_REDUNDANCY,
        &frames,
    )
    .expect("both ceilings, exactly");
    assert_eq!(
        len, MAX_DATAGRAM_BYTES,
        "the widest datagram is the ceiling, to the byte"
    );
    assert!(
        read(&out[..len]).is_ok(),
        "and the reader accepts what the writer just minted"
    );
}

#[test]
fn every_refusal_says_something_a_reader_can_act_on() {
    // A refusal set whose Display arms are untested is a set that can grow
    // an arm printing nothing, and nothing would notice.
    let cases: Vec<WireError> = vec![
        refusal(&[]),
        refusal(&[0u8; MAX_DATAGRAM_BYTES + 1]),
        refusal(&[0u8; HELLO_DATAGRAM_BYTES]),
    ];
    for case in cases {
        let text = case.to_string();
        assert!(!text.is_empty(), "{case:?} printed nothing");
        assert!(
            text.chars().any(|character| character.is_ascii_digit()),
            "{case:?} printed no number: \"{text}\" — a refusal that names no value teaches a \
             reader nothing"
        );
    }
}

// ---- sweeps ----

#[test]
fn every_truncation_of_every_kind_is_refused() {
    for (bytes, len) in [a_hello(), an_inputs(), a_digest(), a_bye()] {
        for cut in 0..len {
            assert!(
                read(&bytes[..cut]).is_err(),
                "a datagram cut to {cut} of {len} bytes was accepted"
            );
        }
        assert!(
            read(&bytes[..len]).is_ok(),
            "and the whole one is still accepted"
        );
    }
}

#[test]
fn no_single_bit_flip_produces_a_second_spelling_of_the_same_datagram() {
    // The canonical-encoding claim, tested the only way it can be: no
    // mutation may yield a different byte string that decodes to the same
    // datagram. Every kind, every byte, every bit.
    for (original, len) in [a_hello(), an_inputs(), a_digest(), a_bye()] {
        let truth = read(&original[..len]).expect("valid before mutation");
        for offset in 0..len {
            for bit in 0..8u8 {
                let mut mutated = original;
                mutated[offset] ^= 1 << bit;
                if let Ok(seen) = read(&mutated[..len]) {
                    assert!(
                        (seen.header, seen.body) != (truth.header, truth.body),
                        "byte {offset} bit {bit} produced a second spelling of one datagram"
                    );
                }
            }
        }
    }
}
