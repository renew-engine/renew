//! Chat, and the property that matters more than any of its features:
//! **it cannot touch the simulation.**

use core::num::NonZeroU64;

use renew_net::{
    Advance, CHAT_INBOX, CHAT_OUTBOX, ChatChannel, ChatRefusal, Delivery, MAX_DATAGRAM_BYTES,
    PeerId, Refusal, Session, SessionParams, ValidParams, wire,
};

const SESSION: u64 = 0x0bad_c0de_0bad_c0de;

#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn params(local: u8) -> ValidParams {
    SessionParams {
        peer_count: 2,
        local: PeerId::new(local).expect("in range"),
        input_bytes: 1,
        input_delay: 1,
        digest_period: 4,
        seed: 5,
        content: 1,
        rules: 2,
        session: NonZeroU64::new(SESSION).expect("not zero"),
    }
    .validate()
    .expect("valid parameters")
}

#[allow(
    clippy::expect_used,
    reason = "see `params`: the fixture's own failure is the report"
)]
fn seat(index: u8) -> PeerId {
    PeerId::new(index).expect("in range")
}

/// Drain one channel's outbox into owned datagrams.
fn drain(channel: &mut ChatChannel) -> Vec<Vec<u8>> {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let mut sent = Vec::new();
    while let Some(datagram) = channel.next_outbound(&mut out) {
        sent.push(datagram.bytes().to_vec());
    }
    sent
}

#[test]
fn a_message_crosses_and_arrives_once() {
    let mut here = ChatChannel::new(&params(0));
    let mut there = ChatChannel::new(&params(1));

    here.send(b"ready when you are").expect("a legal message");
    let datagrams = drain(&mut here);
    assert!(!datagrams.is_empty(), "a queued message must be emitted");

    // Every repeat is delivered. Exactly one message must come out.
    for bytes in &datagrams {
        let _ = there.deliver(seat(0), bytes);
    }
    let first = there.next_message().expect("the message arrived");
    assert_eq!(first.from, seat(0));
    assert_eq!(first.text(), b"ready when you are");
    assert!(
        there.next_message().is_none(),
        "a repeated message was delivered twice"
    );
}

#[test]
fn redundancy_repeats_a_message_and_then_forgets_it() {
    let mut here = ChatChannel::new(&params(0));
    here.send(b"hello").expect("a legal message");

    let mut pumps = 0;
    loop {
        let sent = drain(&mut here);
        if sent.is_empty() {
            break;
        }
        pumps += 1;
        assert!(pumps <= 32, "a message repeated forever");
    }
    assert!(
        pumps > 1,
        "a message sent once is a message one lost datagram erases"
    );
}

#[test]
fn a_lost_repeat_costs_nothing() {
    let mut here = ChatChannel::new(&params(0));
    let mut there = ChatChannel::new(&params(1));
    here.send(b"still here").expect("a legal message");

    // Every datagram of the first pump is dropped; the next one lands.
    let _dropped = drain(&mut here);
    for bytes in drain(&mut here) {
        let _ = there.deliver(seat(0), bytes.as_slice());
    }
    let arrived = there.next_message().expect("a later repeat carried it");
    assert_eq!(arrived.text(), b"still here");
}

#[test]
fn the_outbox_is_a_bound_and_not_a_queue() {
    let mut here = ChatChannel::new(&params(0));
    for index in 0..CHAT_OUTBOX {
        here.send(format!("message {index}").as_bytes())
            .expect("room in the outbox");
    }
    assert_eq!(
        here.send(b"one too many"),
        Err(ChatRefusal::OutboxFull),
        "a player holding the key down must not grow this crate's memory"
    );
}

#[test]
fn an_empty_or_oversized_message_is_refused() {
    let mut here = ChatChannel::new(&params(0));
    assert!(matches!(
        here.send(b""),
        Err(ChatRefusal::Length { saw: 0, .. })
    ));
    let huge = vec![b'x'; wire::MAX_CHAT_BYTES + 1];
    assert!(matches!(here.send(&huge), Err(ChatRefusal::Length { .. })));
    // The ceiling itself is legal, so the refusal pins a range.
    let widest = vec![b'x'; wire::MAX_CHAT_BYTES];
    assert!(here.send(&widest).is_ok());
}

#[test]
fn the_inbox_drops_the_oldest_rather_than_refusing_the_newest() {
    let mut there = ChatChannel::new(&params(1));
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let addressing = wire::Addressing {
        sender: seat(0),
        session: NonZeroU64::new(SESSION).expect("not zero"),
    };

    for sequence in 0..(CHAT_INBOX as u64 + 4) {
        let text = format!("{sequence}");
        let len = wire::write_chat(&mut out, addressing, sequence, text.as_bytes())
            .expect("a legal message");
        let _ = there.deliver(seat(0), out.get(..len).unwrap_or_default());
    }
    assert_eq!(there.waiting(), CHAT_INBOX);
    assert!(there.stats().inbox_overflowed > 0);

    // What survived is the RECENT conversation, not the opening of it: a
    // player who looked away should come back to what was just said.
    let first = there.next_message().expect("something is waiting");
    assert_eq!(
        first.text(),
        b"4",
        "the oldest four should have been dropped"
    );
}

#[test]
fn a_stranger_and_a_misdirected_datagram_are_both_refused() {
    let mut there = ChatChannel::new(&params(1));
    let mut out = [0u8; MAX_DATAGRAM_BYTES];

    // A different session.
    let len = wire::write_chat(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: NonZeroU64::new(0xdead).expect("not zero"),
        },
        0,
        b"hi",
    )
    .expect("legal");
    assert!(matches!(
        there.deliver(seat(0), out.get(..len).unwrap_or_default()),
        Err(ChatRefusal::WrongSession { .. })
    ));

    // A sender that disagrees with the transport.
    let len = wire::write_chat(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: NonZeroU64::new(SESSION).expect("not zero"),
        },
        0,
        b"hi",
    )
    .expect("legal");
    assert!(matches!(
        there.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Err(ChatRefusal::SenderNotSource { .. })
    ));

    assert_eq!(there.waiting(), 0, "no refusal reached the inbox");
}

// ---- the property the whole design exists for ----

#[test]
fn the_session_refuses_chat_and_a_flood_of_it_changes_nothing() {
    let mut session = Box::new(Session::new(params(0)));
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let addressing = wire::Addressing {
        sender: seat(1),
        session: NonZeroU64::new(SESSION).expect("not zero"),
    };

    let before_tick = session.pending_tick();
    let before_digest = session.input_digest();

    for sequence in 0..200u64 {
        let len =
            wire::write_chat(&mut out, addressing, sequence, b"spam").expect("a legal message");
        let outcome = session.deliver(seat(1), out.get(..len).unwrap_or_default());
        assert!(
            matches!(
                outcome,
                Delivery::Refused(Refusal::NotSessionTraffic { .. })
            ),
            "the session must refuse chat by name, not absorb it"
        );
    }

    assert_eq!(
        session.pending_tick(),
        before_tick,
        "chat moved the simulation's tick"
    );
    assert_eq!(
        session.input_digest(),
        before_digest,
        "chat reached the input fingerprint — the one thing it must never touch"
    );
    assert!(
        matches!(session.advance(), Advance::Waiting),
        "chat must not confirm a tick"
    );
    assert_eq!(session.outcome(), None, "chat must not end a session");
}
