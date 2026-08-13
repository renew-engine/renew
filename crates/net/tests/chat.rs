//! Chat, and the property that matters more than any of its features:
//! **it cannot touch the simulation.**

use core::num::NonZeroU64;

use renew_net::{
    Advance, CHAT_INBOX, CHAT_OUTBOX, CHAT_REPEATS, ChatChannel, ChatRefusal, Delivery,
    MAX_DATAGRAM_BYTES, PeerId, Refusal, Session, SessionParams, ValidParams, wire,
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
    {
        // One remote, so this pump holds exactly one datagram — and the
        // caller is told which seat it is addressed to.
        let mut out = [0u8; MAX_DATAGRAM_BYTES];
        let addressed = here
            .next_outbound(&mut out)
            .expect("a queued message must be emitted");
        assert_eq!(addressed.peer(), seat(1));
        assert!(
            here.next_outbound(&mut out).is_none(),
            "one remote means one datagram per pump"
        );
    }
    // The following pumps carry the repeats.
    let datagrams = drain(&mut here);
    assert!(!datagrams.is_empty(), "the repeats follow");

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

// ---- the duplicate window, which is where reordering lives ----

/// Deliver one message from seat 0, at a chosen sequence number.
#[allow(
    clippy::expect_used,
    reason = "see `params`: the fixture's own failure is the report"
)]
fn deliver_at(there: &mut ChatChannel, sequence: u64, text: &[u8]) -> Result<(), ChatRefusal> {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let addressing = wire::Addressing {
        sender: seat(0),
        session: NonZeroU64::new(SESSION).expect("not zero"),
    };
    let len = wire::write_chat(&mut out, addressing, sequence, text).expect("a legal message");
    there.deliver(seat(0), out.get(..len).unwrap_or_default())
}

#[test]
fn messages_that_arrive_out_of_order_all_arrive() {
    let mut there = ChatChannel::new(&params(1));
    // 5 first, then the ones it overtook. All are new, none is a repeat.
    for sequence in [5u64, 1, 3, 0, 4, 2] {
        deliver_at(&mut there, sequence, b"x").unwrap_or_else(|error| {
            panic!("sequence {sequence} was refused as {error:?}");
        });
    }
    assert_eq!(there.waiting(), 6, "every distinct message must arrive");
    assert_eq!(there.stats().duplicates, 0);
}

#[test]
fn a_repeat_is_refused_whether_it_is_the_newest_or_an_older_one() {
    let mut there = ChatChannel::new(&params(1));
    deliver_at(&mut there, 7, b"x").expect("the first");
    deliver_at(&mut there, 3, b"x").expect("an earlier one, still new");

    // The high-water mark itself.
    assert!(matches!(
        deliver_at(&mut there, 7, b"x"),
        Err(ChatRefusal::AlreadySeen { sequence: 7, .. })
    ));
    // And one inside the window that was already filled in.
    assert!(matches!(
        deliver_at(&mut there, 3, b"x"),
        Err(ChatRefusal::AlreadySeen { sequence: 3, .. })
    ));
    assert_eq!(there.stats().duplicates, 2);
}

#[test]
fn a_message_older_than_the_window_is_refused_rather_than_delivered_twice() {
    let mut there = ChatChannel::new(&params(1));
    deliver_at(&mut there, 1_000, b"x").expect("the first");
    // Further back than the sixty-four the filter remembers. Refusing is
    // the safe answer: delivering it might be a duplicate, and this
    // channel would rather drop a very old message than repeat one.
    assert!(matches!(
        deliver_at(&mut there, 1, b"x"),
        Err(ChatRefusal::AlreadySeen { sequence: 1, .. })
    ));
}

#[test]
fn a_jump_past_the_window_clears_it_rather_than_shifting_nonsense() {
    let mut there = ChatChannel::new(&params(1));
    deliver_at(&mut there, 0, b"x").expect("the first");
    deliver_at(&mut there, 1, b"x").expect("the second");
    // A jump of more than sixty-four: everything below is forgotten in
    // one move rather than shifted out a bit at a time.
    deliver_at(&mut there, 500, b"x").expect("a long jump");
    assert!(matches!(
        deliver_at(&mut there, 500, b"x"),
        Err(ChatRefusal::AlreadySeen { .. })
    ));
    // And the window around the new high water still works.
    deliver_at(&mut there, 499, b"x").expect("inside the new window");
    assert!(matches!(
        deliver_at(&mut there, 499, b"x"),
        Err(ChatRefusal::AlreadySeen { .. })
    ));
}

#[test]
fn each_sender_has_its_own_sequence_space() {
    // Three seats, so two remotes can both use sequence zero.
    let params = SessionParams {
        peer_count: 3,
        local: seat(2),
        input_bytes: 1,
        input_delay: 1,
        digest_period: 4,
        seed: 5,
        content: 1,
        rules: 2,
        session: NonZeroU64::new(SESSION).expect("not zero"),
    }
    .validate()
    .expect("valid");
    let mut there = ChatChannel::new(&params);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];

    for from in [0u8, 1] {
        let addressing = wire::Addressing {
            sender: seat(from),
            session: NonZeroU64::new(SESSION).expect("not zero"),
        };
        let len = wire::write_chat(&mut out, addressing, 0, b"hi").expect("legal");
        there
            .deliver(seat(from), out.get(..len).unwrap_or_default())
            .unwrap_or_else(|error| panic!("seat {from} was refused as {error:?}"));
    }
    assert_eq!(
        there.waiting(),
        2,
        "two senders' sequence zero are two different messages"
    );
}

#[test]
fn traffic_that_is_not_chat_is_refused_by_the_channel_too() {
    // The mirror of the session refusing chat: each side refuses the
    // other's traffic by name, so a misrouting driver is told rather than
    // silently ignored.
    let mut there = ChatChannel::new(&params(1));
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_bye(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: NonZeroU64::new(SESSION).expect("not zero"),
        },
        &wire::ByeBody { tick: 3 },
    );
    assert!(matches!(
        there.deliver(seat(0), out.get(..len).unwrap_or_default()),
        Err(ChatRefusal::NotChatTraffic {
            kind: wire::Kind::Bye
        })
    ));
    assert_eq!(there.stats().refused, 1);
}

#[test]
fn malformed_bytes_are_refused_by_the_channel() {
    let mut there = ChatChannel::new(&params(1));
    assert!(matches!(
        there.deliver(seat(0), b"nonsense"),
        Err(ChatRefusal::Malformed(_))
    ));
}

#[test]
fn a_message_from_this_machine_arriving_from_the_network_is_refused() {
    let mut there = ChatChannel::new(&params(1));
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_chat(
        &mut out,
        wire::Addressing {
            sender: seat(1),
            session: NonZeroU64::new(SESSION).expect("not zero"),
        },
        0,
        b"echo",
    )
    .expect("legal");
    assert!(matches!(
        there.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Err(ChatRefusal::NotAPeer { .. })
    ));
}

#[test]
fn a_healthy_channel_reports_no_refusals_however_many_repeats_arrive() {
    // **The counter exists to answer one question — is something wrong —
    // and it can only answer it if the healthy value is zero.** A message
    // is repeated a fixed number of times and never acknowledged, so
    // every message that arrives at all arrives several times over.
    // Counting those as refusals gave the counter a healthy value of five
    // per message, and two separate consumers downstream then read a
    // clean run as a peer refusing nearly everything it heard.
    let mut sender = ChatChannel::new(&params(0));
    let mut receiver = ChatChannel::new(&params(1));
    sender.send(b"hello").expect("a short message");

    let from = PeerId::new(0).expect("seat zero");
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let mut carried = 0usize;
    // Every repeat the sender offers, delivered — which is what a link
    // with no loss does, and the case the counter has to survive.
    for _ in 0..CHAT_REPEATS {
        while let Some(out) = sender.next_outbound(&mut buffer) {
            let _ = receiver.deliver(from, out.bytes());
            carried += 1;
        }
    }
    assert!(carried > 1, "the redundancy must have actually repeated");

    let stats = receiver.stats();
    assert_eq!(stats.received, 1, "one message, however many datagrams");
    assert!(
        stats.duplicates > 0,
        "the repeats must be counted somewhere"
    );
    assert_eq!(
        stats.refused, 0,
        "a channel that lost nothing and refused nothing must say so: {stats:?}"
    );

    // And the counter still moves for something genuinely wrong, or the
    // fix above would have been to delete it.
    let mut nonsense = [0u8; MAX_DATAGRAM_BYTES];
    nonsense[0] = 0xff;
    let _ = receiver.deliver(from, &nonsense[..32]);
    assert_eq!(
        receiver.stats().refused,
        1,
        "malformed bytes are still a refusal"
    );
}
