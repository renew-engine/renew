//! The codec's refusal table, its round-trip oracle, and two sweeps.
//!
//! An integration test rather than a unit one, deliberately: every rule
//! below is reachable through the public API, and a test that can only be
//! written from inside the crate is a test of an implementation rather
//! than of a contract. The one thing it costs is the crate root's
//! `indexing_slicing` deny, which does not reach here — and indexing is
//! the clearest way to say "this byte, that value" when the subject of
//! the test is one byte.

use core::num::NonZeroU64;

use renew_net::wire::{
    Addressing, BYE_DATAGRAM_BYTES, Body, ByeBody, DIGEST_DATAGRAM_BYTES, DigestBody, HEADER_BYTES,
    HELLO_DATAGRAM_BYTES, HelloBody, INPUTS_MIN_DATAGRAM_BYTES, Kind, MAGIC, MAX_CHAT_BYTES,
    WIRE_VERSION, WireError, WriteError, read, write_bye, write_chat, write_digest, write_hello,
    write_inputs,
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

const SESSION: u64 = 0x0123_4567_89ab_cdef;

#[allow(
    clippy::expect_used,
    reason = "see `seat`: the fixture's own failure is the report"
)]
fn addressing() -> Addressing {
    Addressing {
        sender: seat(1),
        session: NonZeroU64::new(SESSION).expect("the fixture's session is not zero"),
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

#[allow(
    clippy::expect_used,
    reason = "see `seat`: the fixture's own failure is the report"
)]
fn a_hello() -> (Buffer, usize) {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = write_hello(&mut out, addressing(), &hello_body())
        .expect("a body inside every documented range");
    (out, len)
}

/// Three frames of two bytes each, from tick 4,000.
#[allow(
    clippy::expect_used,
    reason = "see `seat`: the fixture's own failure is the report"
)]
fn an_inputs() -> (Buffer, usize) {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = write_inputs(&mut out, addressing(), 4_000, 3, 2, &[1, 2, 3, 4, 5, 6])
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
    let len = write_digest(&mut out, addressing(), &body);
    (out, len)
}

fn a_bye() -> (Buffer, usize) {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = write_bye(&mut out, addressing(), &ByeBody { tick: 0x0102_0304 });
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
    assert_eq!(datagram.header.kind, Kind::Hello);
    assert_eq!(datagram.header.addressing(), addressing());
    let Body::Hello(body) = datagram.body else {
        panic!("a Hello decoded as something else")
    };
    assert_eq!(body, hello_body());

    let mut again = [0u8; MAX_DATAGRAM_BYTES];
    let again_len = write_hello(&mut again, datagram.header.addressing(), &body)
        .expect("what the reader accepted, the writer must be able to write");
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
    let again_len = write_digest(&mut again, datagram.header.addressing(), &body);
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
    assert_eq!(
        write_bye(&mut again, datagram.header.addressing(), &body),
        len
    );
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
        datagram.header.addressing(),
        body.first_tick,
        body.count,
        body.input_bytes,
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
    // The vocabulary now runs to 8; 9 is the first code past it.
    for code in [0u8, 9, u8::MAX] {
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
    let head = addressing();

    assert_eq!(
        write_inputs(&mut out, head, 0, 0, 2, &[]),
        Err(WriteError::FrameCount {
            saw: 0,
            ceiling: INPUT_REDUNDANCY
        })
    );
    assert_eq!(
        write_inputs(&mut out, head, 0, INPUT_REDUNDANCY + 1, 1, &[0; 9]),
        Err(WriteError::FrameCount {
            saw: INPUT_REDUNDANCY + 1,
            ceiling: INPUT_REDUNDANCY
        })
    );
    assert_eq!(
        write_inputs(&mut out, head, 0, 1, 0, &[]),
        Err(WriteError::InputBytes {
            saw: 0,
            ceiling: MAX_INPUT_BYTES
        })
    );
    assert_eq!(
        write_inputs(&mut out, head, 0, 1, MAX_INPUT_BYTES + 1, &[0; 17]),
        Err(WriteError::InputBytes {
            saw: MAX_INPUT_BYTES + 1,
            ceiling: MAX_INPUT_BYTES
        })
    );
    assert_eq!(
        write_inputs(&mut out, head, 0, 3, 2, &[0; 5]),
        Err(WriteError::FramesLength {
            saw: 5,
            expected: 6
        }),
        "refused rather than truncated: a short write is a second spelling of a shorter fact"
    );
    assert_eq!(
        write_inputs(&mut out, head, u64::MAX, 2, 1, &[0; 2]),
        Err(WriteError::TickOverflow {
            first_tick: u64::MAX,
            count: 2
        })
    );
}

#[test]
fn the_hello_writer_refuses_every_body_the_reader_would_reject() {
    // The four ranges read_hello enforces, enforced here too — which is
    // what makes "a writer cannot mint what the reader would refuse" a
    // contract rather than an aspiration. Each case is also driven
    // through `read` to prove the refusal was not merely conservative:
    // the datagram it declined really would have been declined.
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let head = addressing();

    for count in [0u8, 1, MAX_PEERS + 1] {
        let body = HelloBody {
            peer_count: count,
            ..hello_body()
        };
        assert_eq!(
            write_hello(&mut out, head, &body),
            Err(WriteError::PeerCount {
                saw: count,
                floor: 2,
                ceiling: MAX_PEERS
            })
        );
    }

    for width in [0u8, MAX_INPUT_BYTES + 1] {
        let body = HelloBody {
            input_bytes: width,
            ..hello_body()
        };
        assert_eq!(
            write_hello(&mut out, head, &body),
            Err(WriteError::InputBytes {
                saw: width,
                ceiling: MAX_INPUT_BYTES
            })
        );
    }

    let past = u8::try_from(INPUT_WINDOW).expect("the window fits a byte today");
    let body = HelloBody {
        input_delay: past,
        ..hello_body()
    };
    assert_eq!(
        write_hello(&mut out, head, &body),
        Err(WriteError::InputDelay {
            saw: past,
            window: INPUT_WINDOW
        })
    );

    let body = HelloBody {
        digest_period: 0,
        ..hello_body()
    };
    assert_eq!(
        write_hello(&mut out, head, &body),
        Err(WriteError::DigestPeriodZero)
    );

    // The boundaries the other way, so the refusals pin a range rather
    // than only its outside.
    for body in [
        HelloBody {
            peer_count: 2,
            ..hello_body()
        },
        HelloBody {
            peer_count: MAX_PEERS,
            ..hello_body()
        },
        HelloBody {
            input_bytes: 1,
            ..hello_body()
        },
        HelloBody {
            input_bytes: MAX_INPUT_BYTES,
            ..hello_body()
        },
        HelloBody {
            input_delay: past - 1,
            ..hello_body()
        },
        HelloBody {
            digest_period: 1,
            ..hello_body()
        },
    ] {
        let len = write_hello(&mut out, head, &body).expect("a body at its boundary is legal");
        assert!(
            read(&out[..len]).is_ok(),
            "the writer accepted {body:?} and the reader did not"
        );
    }
}

#[test]
fn the_widest_legal_run_writes_exactly_the_ceiling() {
    let frames = [7u8; (INPUT_REDUNDANCY as usize) * (MAX_INPUT_BYTES as usize)];
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = write_inputs(
        &mut out,
        addressing(),
        0,
        INPUT_REDUNDANCY,
        MAX_INPUT_BYTES,
        &frames,
    )
    .expect("both ceilings, exactly");
    assert_eq!(
        len,
        HEADER_BYTES + 12 + usize::from(INPUT_REDUNDANCY) * usize::from(MAX_INPUT_BYTES),
        "an inputs run at both ceilings is its own composition, to the byte"
    );
    assert!(
        len < MAX_DATAGRAM_BYTES,
        "an inputs run stopped being the widest kind when the lobby landed, and this test says so \
         rather than quietly tracking whichever kind happens to be widest"
    );
    assert!(
        read(&out[..len]).is_ok(),
        "and the reader accepts what the writer just minted"
    );
}

#[test]
fn refusals_display_their_evidence() {
    // Every arm, not a sample. A refusal set with untested arms is one
    // that can grow an arm printing nothing, and nothing would notice —
    // and three of twenty-one is a sample however the comment reads.
    let wire_errors = vec![
        WireError::TooShort { len: 15 },
        WireError::TooLong { len: 157 },
        WireError::BadMagic { saw: *b"XNWL" },
        WireError::BadVersion { saw: 2 },
        WireError::UnknownKind { saw: 9 },
        WireError::SenderPastCeiling { saw: 8, ceiling: 8 },
        WireError::SessionZero,
        WireError::SizeMismatch {
            kind: Kind::Hello,
            declared: 56,
            actual: 57,
        },
        WireError::PadNotZero { offset: 52, saw: 1 },
        WireError::FrameCountZero,
        WireError::FrameCountPastRedundancy { saw: 9, ceiling: 8 },
        WireError::InputBytesZero,
        WireError::InputBytesPastCeiling {
            saw: 17,
            ceiling: 16,
        },
        WireError::TickOverflow {
            first_tick: u64::MAX,
            count: 3,
        },
        WireError::PeerCountOutOfRange {
            saw: 1,
            floor: 2,
            ceiling: 8,
        },
        WireError::InputDelayPastWindow {
            saw: 64,
            window: 64,
        },
        WireError::DigestPeriodZero,
        WireError::ChatEmpty,
        WireError::ChatTooLong {
            saw: 200,
            ceiling: 128,
        },
    ];
    let write_errors = vec![
        WriteError::FrameCount { saw: 9, ceiling: 8 },
        WriteError::InputBytes {
            saw: 17,
            ceiling: 16,
        },
        WriteError::FramesLength {
            saw: 5,
            expected: 6,
        },
        WriteError::TickOverflow {
            first_tick: u64::MAX,
            count: 2,
        },
        WriteError::PeerCount {
            saw: 1,
            floor: 2,
            ceiling: 8,
        },
        WriteError::InputDelay {
            saw: 64,
            window: 64,
        },
        WriteError::DigestPeriodZero,
        WriteError::ChatLength {
            saw: 0,
            ceiling: 128,
        },
    ];

    // Five refusals name their value in words rather than in digits,
    // because the value IS zero and "0" would read worse. They are held
    // to that spelling by name rather than exempted from the rule, so the
    // rule keeps its teeth for every refusal that carries a number — and
    // the five stay consistent with each other, which is why the chat one
    // says "zero bytes" rather than "no bytes".
    let spelled_out = |text: &str| assert!(text.contains("zero"), "expected the word: \"{text}\"");

    for case in wire_errors {
        let text = case.to_string();
        assert!(!text.is_empty(), "{case:?} printed nothing");
        match case {
            WireError::SessionZero
            | WireError::FrameCountZero
            | WireError::InputBytesZero
            | WireError::DigestPeriodZero
            | WireError::ChatEmpty => spelled_out(&text),
            _ => assert!(
                text.chars().any(|character| character.is_ascii_digit()),
                "{case:?} printed no number: \"{text}\" — a refusal that names no value teaches                  a reader nothing"
            ),
        }
    }
    for case in write_errors {
        let text = case.to_string();
        assert!(!text.is_empty(), "{case:?} printed nothing");
        if matches!(case, WriteError::DigestPeriodZero) {
            spelled_out(&text);
        } else {
            assert!(
                text.chars().any(|character| character.is_ascii_digit()),
                "{case:?} printed no number: \"{text}\""
            );
        }
    }
}

#[test]
fn every_body_size_is_a_function_of_its_kind() {
    // The three fixed kinds ignore both counts; only Inputs reads them.
    assert_eq!(Kind::Hello.body_bytes(0, 0), Some(40));
    assert_eq!(Kind::Digest.body_bytes(9, 9), Some(24));
    assert_eq!(Kind::Bye.body_bytes(u8::MAX, u8::MAX), Some(8));
    assert_eq!(Kind::Inputs.body_bytes(3, 2), Some(18));
    // A chat body's length rides in `count`: one record, its own width.
    assert_eq!(Kind::Chat.body_bytes(0, 0), Some(12));
    assert_eq!(Kind::Chat.body_bytes(40, 9), Some(52));
    assert_eq!(
        Kind::Inputs.body_bytes(u8::MAX, u8::MAX),
        Some(12 + 255 * 255),
        "the product is computed in u64, so even the widest pair of bytes fits"
    );
}

// ---- the golden: bytes a human typed, from the page a reader reads ----

#[test]
fn a_hand_built_datagram_writes_back_to_itself() {
    // Every other assertion in this file is the writer against the
    // reader, and a writer and a reader that made the SAME mistake are
    // still exact inverses of each other — the argument the trace codec's
    // golden already writes down. These bytes were typed from the layout
    // tables in README.md, not from wire.rs, so they are an independent
    // referent: a field that moved would have to move on the front page
    // too before this test agreed with it again.
    #[rustfmt::skip]
    let hand_built: [u8; 40] = [
        // header, 16 bytes
        b'R', b'N', b'W', b'L',   // magic
        0x03, 0x00,               // wire version 3, little-endian
        0x03,                     // kind 3 = Digest
        0x01,                     // sender: seat 1
        0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01, // session 0x0123456789abcdef
        // Digest body, 24 bytes
        0x58, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // tick 600
        0xcd, 0xab, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // state digest 0xabcd
        0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // input digest 0x1234
    ];

    let datagram = read(&hand_built).expect("bytes typed from the documented layout");
    assert_eq!(datagram.header.kind, Kind::Digest);
    assert_eq!(datagram.header.sender.index(), 1);
    assert_eq!(datagram.header.session.get(), SESSION);
    let Body::Digest(body) = datagram.body else {
        panic!("the kind byte says Digest")
    };
    assert_eq!(
        (body.tick, body.state_digest, body.input_digest),
        (600, 0xabcd, 0x1234)
    );

    // And the inverse: what the reader accepted, the writer reproduces
    // byte for byte. This is the half a writer-against-reader round trip
    // cannot give, because both halves would be wrong together.
    let mut written = [0u8; MAX_DATAGRAM_BYTES];
    let len = write_digest(&mut written, datagram.header.addressing(), &body);
    assert_eq!(&written[..len], &hand_built[..]);

    // The fixture agrees with the constants it was not typed from — which
    // is what makes a disagreement point at the layout rather than at a
    // typo in the test.
    assert_eq!(&hand_built[..4], &MAGIC[..]);
    assert_eq!(len, DIGEST_DATAGRAM_BYTES);
    assert_eq!(
        u16::from_le_bytes([hand_built[4], hand_built[5]]),
        WIRE_VERSION
    );
    assert_eq!(hand_built[6], Kind::Digest.code());
    assert_eq!(HEADER_BYTES, 16);
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

// ---- chat, the kind that is not simulation state ----

#[test]
fn a_chat_datagram_round_trips_and_carries_bytes_that_are_not_text() {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    // Deliberately not UTF-8: this codec does not know what encoding a
    // game speaks, and a reader that guessed would refuse real messages.
    let raw = [0x00u8, 0x80, 0xc0, 0xff];
    let len = write_chat(&mut out, addressing(), 42, &raw).expect("a legal message");

    let datagram = read(&out[..len]).expect("a datagram this crate wrote");
    assert_eq!(datagram.header.kind, Kind::Chat);
    let Body::Chat(body) = datagram.body else {
        panic!("a Chat decoded as something else")
    };
    assert_eq!(body.sequence, 42);
    assert_eq!(body.text(), &raw[..]);

    let mut again = [0u8; MAX_DATAGRAM_BYTES];
    let again_len = write_chat(
        &mut again,
        datagram.header.addressing(),
        body.sequence,
        body.text(),
    )
    .expect("what the reader accepted, the writer must accept");
    assert_eq!(&again[..again_len], &out[..len]);
}

#[test]
fn the_chat_writer_refuses_what_the_reader_would() {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    assert_eq!(
        write_chat(&mut out, addressing(), 0, b""),
        Err(WriteError::ChatLength {
            saw: 0,
            ceiling: MAX_CHAT_BYTES
        })
    );
    let huge = [b'x'; MAX_CHAT_BYTES + 1];
    assert_eq!(
        write_chat(&mut out, addressing(), 0, &huge),
        Err(WriteError::ChatLength {
            saw: MAX_CHAT_BYTES + 1,
            ceiling: MAX_CHAT_BYTES
        })
    );
    // The ceiling itself is legal, so the refusal pins a range.
    let widest = [b'x'; MAX_CHAT_BYTES];
    let len = write_chat(&mut out, addressing(), 0, &widest).expect("the ceiling is legal");
    assert_eq!(
        len, MAX_DATAGRAM_BYTES,
        "the widest chat fills a datagram exactly"
    );
    assert!(read(&out[..len]).is_ok());
}

#[test]
fn an_empty_or_oversized_chat_on_the_wire_is_refused() {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = write_chat(&mut out, addressing(), 7, b"hi").expect("legal");

    // Length zero, hand-poked: the writer will not mint one.
    let mut empty = out;
    empty[HEADER_BYTES + 8] = 0;
    assert_eq!(refusal(&empty[..len]), WireError::ChatEmpty);

    // A length past the ceiling, likewise.
    let mut oversized = out;
    oversized[HEADER_BYTES + 8] = 200;
    assert_eq!(
        refusal(&oversized[..len]),
        WireError::ChatTooLong {
            saw: 200,
            ceiling: MAX_CHAT_BYTES
        }
    );

    // A length that disagrees with the datagram is a size mismatch.
    let mut wrong = out;
    wrong[HEADER_BYTES + 8] = 3;
    assert!(matches!(
        refusal(&wrong[..len]),
        WireError::SizeMismatch {
            kind: Kind::Chat,
            ..
        }
    ));

    // And its reserved bytes are pinned like every other kind's.
    for step in 0..3 {
        let mut padded = out;
        padded[HEADER_BYTES + 9 + step] = 1;
        assert_eq!(
            refusal(&padded[..len]),
            WireError::PadNotZero {
                offset: HEADER_BYTES + 9 + step,
                saw: 1
            }
        );
    }

    // Too short to declare its own size.
    assert!(matches!(
        refusal(&out[..HEADER_BYTES + 11]),
        WireError::SizeMismatch {
            kind: Kind::Chat,
            ..
        }
    ));
}

// ---- the lobby's three kinds ----

/// The size table, asked directly. `read` reaches the roster arm and no
/// other: a `Join` and a `Start` are fixed-width, so their readers check a
/// constant and never come here. Asked anyway, because a size table one
/// caller happens not to use is a table that can drift from the layout
/// beneath it without anything noticing.
#[test]
fn every_lobby_kind_declares_the_body_it_writes() {
    use renew_net::wire::{ENDPOINT_BYTES, JOIN_BODY_BYTES, ROSTER_BODY_BYTES, START_BODY_BYTES};
    assert_eq!(Kind::Join.body_bytes(0, 0), Some(JOIN_BODY_BYTES as u64));
    assert_eq!(Kind::Start.body_bytes(0, 0), Some(START_BODY_BYTES as u64));
    for seats in 0..=MAX_PEERS {
        assert_eq!(
            Kind::Roster.body_bytes(seats, 0),
            Some(ROSTER_BODY_BYTES as u64 + u64::from(seats) * ENDPOINT_BYTES as u64),
            "a roster's width is its seat count"
        );
    }
}

#[test]
fn a_roster_hands_back_each_seats_endpoint_and_nothing_past_them() {
    use renew_net::wire::{ENDPOINT_BYTES, RosterBody, write_roster};
    let mut endpoints = [0u8; 18 * 3];
    for (seat, chunk) in endpoints
        .as_chunks_mut::<ENDPOINT_BYTES>()
        .0
        .iter_mut()
        .enumerate()
    {
        chunk.fill(u8::try_from(seat).expect("three seats") + 10);
    }
    let mut out: Buffer = [0; MAX_DATAGRAM_BYTES];
    let len = write_roster(
        &mut out,
        Addressing {
            sender: PeerId::new(0).expect("seat zero"),
            session: NonZeroU64::new(9).expect("nonzero"),
        },
        &RosterBody {
            seat: 2,
            peer_count: 3,
            input_bytes: 1,
            input_delay: 0,
            digest_period: 1,
            seed: 1,
            content: 2,
            rules: 3,
            endpoints: &endpoints,
        },
    )
    .expect("a well-formed roster");

    let Body::Roster(body) = read(out.get(..len).expect("written"))
        .expect("reads back")
        .body
    else {
        panic!("a roster must read back as one");
    };
    assert_eq!(body.endpoint(0), Some([10u8; ENDPOINT_BYTES]));
    assert_eq!(body.endpoint(2), Some([12u8; ENDPOINT_BYTES]));
    assert_eq!(
        body.endpoint(3),
        None,
        "a seat past the roster has no endpoint, and must not read the next one"
    );
}

/// Every way a roster can be asked for that the reader would refuse.
/// The writer has to refuse each one first, or it can mint a datagram
/// nothing accepts.
#[test]
fn a_roster_a_reader_would_refuse_cannot_be_written() {
    use renew_net::wire::{RosterBody, write_roster};
    let addressing = Addressing {
        sender: PeerId::new(0).expect("seat zero"),
        session: NonZeroU64::new(9).expect("nonzero"),
    };
    let endpoints = [0u8; 18 * 2];
    let good = RosterBody {
        seat: 1,
        peer_count: 2,
        input_bytes: 1,
        input_delay: 0,
        digest_period: 1,
        seed: 1,
        content: 2,
        rules: 3,
        endpoints: &endpoints,
    };
    let mut out: Buffer = [0; MAX_DATAGRAM_BYTES];
    write_roster(&mut out, addressing, &good).expect("the fixture must be writable");

    let cases: [(RosterBody<'_>, &str); 6] = [
        (
            RosterBody {
                peer_count: 1,
                ..good
            },
            "one peer is not a session",
        ),
        (RosterBody { seat: 2, ..good }, "a seat outside the roster"),
        (
            RosterBody {
                input_bytes: 0,
                ..good
            },
            "a zero-width input",
        ),
        (
            RosterBody {
                input_delay: u8::try_from(INPUT_WINDOW).unwrap_or(u8::MAX),
                ..good
            },
            "a delay past the window",
        ),
        (
            RosterBody {
                digest_period: 0,
                ..good
            },
            "a period that digests every tick",
        ),
        (
            RosterBody {
                peer_count: 3,
                ..good
            },
            "endpoints that do not match the seat count",
        ),
    ];
    for (body, why) in cases {
        let mut out: Buffer = [0; MAX_DATAGRAM_BYTES];
        let refusal = write_roster(&mut out, addressing, &body)
            .expect_err(&format!("{why} must be refused, not written"));
        // Every refusal has to say enough to act on, which is the whole
        // reason they are typed rather than a bare unit.
        let mut text = String::new();
        core::fmt::Write::write_fmt(&mut text, format_args!("{refusal}")).expect("formats");
        assert!(!text.is_empty(), "{why}");
    }
}

/// The two `SeatNotInRoster` arms — one on the reader's error, one on the
/// writer's. They say the same sentence and are two types, so a change to
/// one that skipped the other would go unnoticed.
#[test]
fn a_seat_outside_the_roster_reads_the_same_from_either_side() {
    use renew_net::wire::{RosterBody, write_roster};
    let endpoints = [0u8; 18 * 2];
    let mut out: Buffer = [0; MAX_DATAGRAM_BYTES];
    let write_side = write_roster(
        &mut out,
        Addressing {
            sender: PeerId::new(0).expect("seat zero"),
            session: NonZeroU64::new(9).expect("nonzero"),
        },
        &RosterBody {
            seat: 5,
            peer_count: 2,
            input_bytes: 1,
            input_delay: 0,
            digest_period: 1,
            seed: 1,
            content: 2,
            rules: 3,
            endpoints: &endpoints,
        },
    )
    .expect_err("seat five of two");
    assert!(matches!(
        write_side,
        WriteError::SeatNotInRoster {
            seat: 5,
            peer_count: 2
        }
    ));
    let mut written = String::new();
    core::fmt::Write::write_fmt(&mut written, format_args!("{write_side}")).expect("formats");
    assert!(written.contains('5') && written.contains('2'), "{written}");

    // And the same claim from the reader, on bytes a writer would not
    // have produced.
    let mut hand_built: Buffer = [0; MAX_DATAGRAM_BYTES];
    let len = write_roster(
        &mut hand_built,
        Addressing {
            sender: PeerId::new(0).expect("seat zero"),
            session: NonZeroU64::new(9).expect("nonzero"),
        },
        &RosterBody {
            seat: 1,
            peer_count: 2,
            input_bytes: 1,
            input_delay: 0,
            digest_period: 1,
            seed: 1,
            content: 2,
            rules: 3,
            endpoints: &endpoints,
        },
    )
    .expect("a well-formed roster");
    if let Some(cell) = hand_built.get_mut(HEADER_BYTES) {
        *cell = 5;
    }
    let read_side = read(hand_built.get(..len).expect("written")).expect_err("seat five of two");
    assert!(matches!(
        read_side,
        WireError::SeatNotInRoster {
            seat: 5,
            peer_count: 2
        }
    ));
    let mut got = String::new();
    core::fmt::Write::write_fmt(&mut got, format_args!("{read_side}")).expect("formats");
    assert_eq!(got, written, "one sentence, said by both sides");
}
