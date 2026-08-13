//! Every way a session says no, and the accessors a driver reads.
//!
//! The happy path is covered by `session.rs`; this is the other half.
//! A refusal set with untested arms is one that can grow an arm that
//! absorbs what it should drop, and nothing would notice.

use core::num::NonZeroU64;

use renew_net::{
    Advance, Delivery, INPUT_WINDOW, MAX_DATAGRAM_BYTES, Outcome, PeerId, Refusal, Session,
    SessionParams, SubmitError, ValidParams, wire,
};

const SESSION: u64 = 0x5e55_1000;

#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn seat(index: u8) -> PeerId {
    PeerId::new(index).expect("in range")
}

#[allow(
    clippy::expect_used,
    reason = "see `seat`: the fixture's own failure is the report"
)]
fn params_of(local: u8, peers: u8, delay: u8) -> ValidParams {
    SessionParams {
        peer_count: peers,
        local: seat(local),
        input_bytes: 1,
        input_delay: delay,
        digest_period: 2,
        seed: 1,
        content: 2,
        rules: 3,
        session: NonZeroU64::new(SESSION).expect("not zero"),
    }
    .validate()
    .expect("valid parameters")
}

fn session_of(local: u8, peers: u8, delay: u8) -> Box<Session> {
    Box::new(Session::new(params_of(local, peers, delay)))
}

#[allow(
    clippy::expect_used,
    reason = "see `seat`: the fixture's own failure is the report"
)]
fn addressing(sender: u8, session: u64) -> wire::Addressing {
    wire::Addressing {
        sender: seat(sender),
        session: NonZeroU64::new(session).expect("not zero"),
    }
}

fn hello_body(agreement: u64) -> wire::HelloBody {
    wire::HelloBody {
        agreement_digest: agreement,
        content: 2,
        rules: 3,
        seed: 1,
        peer_count: 2,
        input_bytes: 1,
        input_delay: 0,
        digest_period: 2,
    }
}

// ---- the header gate ----

#[test]
fn malformed_bytes_are_refused_and_counted() {
    let mut session = session_of(0, 2, 0);
    let outcome = session.deliver(seat(1), b"not a datagram");
    assert!(matches!(outcome, Delivery::Refused(Refusal::Malformed(_))));
    assert_eq!(session.stats().datagrams_refused, 1);
}

#[test]
fn another_sessions_datagram_is_refused() {
    let mut session = session_of(0, 2, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_bye(&mut out, addressing(1, 0xdead), &wire::ByeBody { tick: 0 });
    assert!(matches!(
        session.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Delivery::Refused(Refusal::WrongSession { saw: 0xdead })
    ));
}

#[test]
fn a_header_that_disagrees_with_the_transport_is_refused() {
    let mut session = session_of(0, 3, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_bye(&mut out, addressing(1, SESSION), &wire::ByeBody { tick: 0 });
    // The bytes claim seat 1; the transport says they came from seat 2.
    assert!(matches!(
        session.deliver(seat(2), out.get(..len).unwrap_or_default()),
        Delivery::Refused(Refusal::SenderNotSource { .. })
    ));
}

#[test]
fn this_machines_own_seat_arriving_from_the_network_is_refused() {
    let mut session = session_of(0, 2, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_bye(&mut out, addressing(0, SESSION), &wire::ByeBody { tick: 0 });
    assert!(matches!(
        session.deliver(seat(0), out.get(..len).unwrap_or_default()),
        Delivery::Refused(Refusal::FromSelf)
    ));
}

#[test]
fn a_seat_outside_the_roster_is_refused() {
    // A two-seat session; seat 5 is a legal PeerId and not in this game.
    let mut session = session_of(0, 2, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_bye(&mut out, addressing(5, SESSION), &wire::ByeBody { tick: 0 });
    assert!(matches!(
        session.deliver(seat(5), out.get(..len).unwrap_or_default()),
        Delivery::Refused(Refusal::NotInRoster { peer: _ })
    ));
}

#[test]
fn a_peer_playing_different_parameters_is_refused_at_the_handshake() {
    let mut session = session_of(0, 2, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_hello(&mut out, addressing(1, SESSION), &hello_body(0xbad))
        .expect("a legal hello");
    let outcome = session.deliver(seat(1), out.get(..len).unwrap_or_default());
    let Delivery::Refused(Refusal::Disagreement { theirs, ours }) = outcome else {
        panic!("expected a disagreement, got {outcome:?}")
    };
    assert_eq!(theirs, 0xbad);
    assert_ne!(ours, 0xbad, "the refusal must name both sides");
    assert!(
        !session.is_playing(),
        "a disagreeing peer must not start a game"
    );
}

#[test]
fn a_matching_hello_starts_the_game() {
    // The anti-vacuity twin of the test above: if nothing could start,
    // every refusal here would pass for the wrong reason.
    let mut session = session_of(0, 2, 0);
    let agreement = session.params().agreement_digest();
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_hello(&mut out, addressing(1, SESSION), &hello_body(agreement))
        .expect("a legal hello");
    assert!(matches!(
        session.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Delivery::Accepted
    ));
    session.submit(&[1]).expect("a first input");
    assert!(
        session.is_playing(),
        "everyone said hello and the window is full"
    );
}

// ---- leaving ----

#[test]
fn a_peers_departure_ends_the_session_and_names_the_tick() {
    let mut session = session_of(0, 2, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_bye(
        &mut out,
        addressing(1, SESSION),
        &wire::ByeBody { tick: 41 },
    );
    let outcome = session.deliver(seat(1), out.get(..len).unwrap_or_default());
    assert_eq!(
        outcome,
        Delivery::Ends(Outcome::PeerLeft {
            peer: seat(1),
            tick: 41
        })
    );
    assert_eq!(
        session.outcome(),
        Some(Outcome::PeerLeft {
            peer: seat(1),
            tick: 41
        })
    );
    // Everything afterwards reports the end rather than acting.
    assert!(matches!(session.advance(), Advance::Ended(_)));
    assert!(matches!(
        session.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Delivery::Ends(_)
    ));
    assert!(!session.wants_local());
}

#[test]
fn leaving_twice_keeps_the_first_reason() {
    let mut session = session_of(0, 2, 0);
    let first = session.leave();
    let again = session.leave();
    assert_eq!(first, again, "a session's ending must not be rewritten");
}

#[test]
fn a_departing_peer_still_emits_its_farewell() {
    let mut session = session_of(0, 2, 0);
    session.leave();
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let mut sent = 0;
    while let Some(datagram) = session.next_outbound(&mut out) {
        let read = wire::read(datagram.bytes()).expect("a datagram this crate wrote");
        assert_eq!(read.header.kind, wire::Kind::Bye);
        assert_eq!(datagram.peer(), seat(1), "the farewell goes to the remote");
        sent += 1;
        assert!(sent < 8, "a farewell repeated forever");
    }
    assert_eq!(sent, 1, "one remote, one farewell");
}

// ---- submitting ----

#[test]
fn a_submitted_input_is_never_rewritten() {
    // This started as a test that a second submit for one tick is
    // REFUSED, and it found that the refusal it was written against
    // could not fire: `submit` always fills the next unsubmitted tick,
    // so there is no way to name an earlier one. The variant was removed
    // and this now pins the behaviour that makes it unnecessary.
    let mut session = session_of(0, 2, 0);
    assert_eq!(session.submit(&[1]).expect("the first input"), 0);
    assert_eq!(
        session
            .submit(&[2])
            .expect("the next tick, not the same one"),
        1,
        "a caller that could revise a submitted input could cheat"
    );
}

#[test]
fn submitting_past_the_window_is_refused() {
    // A wide delay lets the local peer run ahead until the ring is full.
    let mut session = session_of(0, 2, 63);
    let mut submitted = 0u64;
    loop {
        match session.submit(&[0]) {
            Ok(_) => submitted = submitted.saturating_add(1),
            Err(SubmitError::WindowFull { tick, pending }) => {
                assert_eq!(pending, 0, "nothing was confirmed, so the frontier is zero");
                assert_eq!(tick, u64::from(INPUT_WINDOW));
                break;
            }
            Err(other) => panic!("unexpected refusal: {other:?}"),
        }
        assert!(
            submitted <= u64::from(INPUT_WINDOW),
            "the window never filled"
        );
    }
    assert_eq!(
        submitted,
        u64::from(INPUT_WINDOW),
        "the window holds exactly its depth"
    );
}

// ---- what a confirmed tick hands back ----

#[test]
fn a_step_reports_only_the_seats_in_its_roster() {
    let mut here = session_of(0, 2, 0);
    let mut there = session_of(1, 2, 0);
    let agreement = here.params().agreement_digest();
    let mut out = [0u8; MAX_DATAGRAM_BYTES];

    for (session, from) in [(&mut here, 1u8), (&mut there, 0u8)] {
        let len = wire::write_hello(&mut out, addressing(from, SESSION), &hello_body(agreement))
            .expect("a legal hello");
        let _ = session.deliver(seat(from), out.get(..len).unwrap_or_default());
    }
    here.submit(&[7]).expect("an input");
    there.submit(&[9]).expect("an input");

    // Carry seat 1's inputs to seat 0 so a tick can be confirmed.
    let mut carried = Vec::new();
    while let Some(datagram) = there.next_outbound(&mut out) {
        carried.push(datagram.bytes().to_vec());
    }
    for bytes in carried {
        let _ = here.deliver(seat(1), &bytes);
    }

    let Advance::Step(step) = here.advance() else {
        panic!("both seats submitted, so a tick must be confirmed")
    };
    assert_eq!(step.tick(), 0);
    assert_eq!(step.input(seat(0)), Some(&[7u8][..]));
    assert_eq!(step.input(seat(1)), Some(&[9u8][..]));
    assert_eq!(
        step.input(seat(4)),
        None,
        "a seat outside the roster has no input, and asking is not a panic"
    );
    let seats: Vec<u8> = step.inputs().map(|(peer, _)| peer.index()).collect();
    assert_eq!(seats, vec![0, 1], "inputs are handed out ascending by seat");
}

// ---- committing ----

#[test]
fn committing_a_tick_that_was_not_handed_out_is_refused() {
    let mut session = session_of(0, 2, 0);
    // Nothing has been handed out at all.
    assert_eq!(
        session.commit(0, None),
        Err(renew_net::CommitError::OutOfOrder {
            saw: 0,
            expected: 0
        })
    );
}

/// Brings a two-seat pair to the point where seat 0 can confirm ticks.
#[allow(
    clippy::expect_used,
    reason = "see `seat`: the fixture's own failure is the report"
)]
fn started_pair() -> (Box<Session>, Box<Session>) {
    let mut here = session_of(0, 2, 0);
    let mut there = session_of(1, 2, 0);
    let agreement = here.params().agreement_digest();
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    for (session, from) in [(&mut here, 1u8), (&mut there, 0u8)] {
        let len = wire::write_hello(&mut out, addressing(from, SESSION), &hello_body(agreement))
            .expect("a legal hello");
        let _ = session.deliver(seat(from), out.get(..len).unwrap_or_default());
    }
    here.submit(&[1]).expect("an input");
    there.submit(&[2]).expect("an input");
    let mut carried = Vec::new();
    while let Some(datagram) = there.next_outbound(&mut out) {
        carried.push(datagram.bytes().to_vec());
    }
    for bytes in carried {
        let _ = here.deliver(seat(1), &bytes);
    }
    (here, there)
}

#[test]
fn a_digest_owed_and_not_supplied_is_refused_and_so_is_the_reverse() {
    let (mut here, _there) = started_pair();
    let Advance::Step(step) = here.advance() else {
        panic!("both seats submitted, so a tick must be confirmed")
    };
    let tick = step.tick();
    assert!(step.digest_due(), "period two means tick zero is digested");

    // Owed and withheld.
    assert_eq!(
        here.commit(tick, None),
        Err(renew_net::CommitError::DigestMismatch { tick, owed: true })
    );
    // The wrong tick entirely.
    assert!(matches!(
        here.commit(tick + 5, Some(1)),
        Err(renew_net::CommitError::OutOfOrder { .. })
    ));
    // And the right one, which must still work afterwards.
    here.commit(tick, Some(1)).expect("owed and supplied");
}

// ---- absorbing another peer's frames ----

#[test]
fn a_frame_of_the_wrong_width_is_refused() {
    let mut session = session_of(0, 2, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    // The session agreed one byte per input; this claims two.
    let len = wire::write_inputs(&mut out, addressing(1, SESSION), 0, 1, 2, &[1, 2])
        .expect("a legal datagram");
    assert!(matches!(
        session.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Delivery::Refused(Refusal::WrongWidth { saw: 2, agreed: 1 })
    ));
}

#[test]
fn a_frame_past_the_window_is_refused_and_counted() {
    let mut session = session_of(0, 2, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let far = u64::from(INPUT_WINDOW) + 100;
    let len = wire::write_inputs(&mut out, addressing(1, SESSION), far, 1, 1, &[9]).expect("legal");
    assert!(matches!(
        session.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Delivery::Refused(Refusal::OutOfWindow { .. })
    ));
    assert_eq!(session.stats().frames_out_of_window, 1);
}

#[test]
fn a_repeated_frame_is_counted_and_a_contradicting_one_is_dropped() {
    let mut session = session_of(0, 2, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];

    let len = wire::write_inputs(&mut out, addressing(1, SESSION), 0, 1, 1, &[7]).expect("legal");
    let first = out;
    assert!(matches!(
        session.deliver(seat(1), first.get(..len).unwrap_or_default()),
        Delivery::Accepted
    ));
    // The identical frame again: the redundancy working, not an error.
    assert!(matches!(
        session.deliver(seat(1), first.get(..len).unwrap_or_default()),
        Delivery::Accepted
    ));
    assert_eq!(session.stats().frames_repeated, 1);

    // A different frame for the same tick. First write wins, so this is
    // already a state no-op — counted and dropped, never fatal.
    let len = wire::write_inputs(&mut out, addressing(1, SESSION), 0, 1, 1, &[8]).expect("legal");
    assert!(matches!(
        session.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Delivery::Refused(Refusal::Contradiction { .. })
    ));
    assert_eq!(session.stats().frames_contradicted, 1);
    assert_eq!(
        session.outcome(),
        None,
        "a contradiction must never end a session: it would be a one-packet kill switch"
    );
}

#[test]
fn a_run_carrying_both_a_new_frame_and_a_refused_one_still_accepts() {
    let mut session = session_of(0, 2, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];

    // Seat 1's frame for tick 0 lands.
    let len = wire::write_inputs(&mut out, addressing(1, SESSION), 0, 1, 1, &[7]).expect("legal");
    let _ = session.deliver(seat(1), out.get(..len).unwrap_or_default());

    // Now a run covering ticks 0 and 1: tick 0 contradicts, tick 1 is new.
    // The useful half must still be absorbed.
    let len =
        wire::write_inputs(&mut out, addressing(1, SESSION), 0, 2, 1, &[8, 9]).expect("legal");
    assert!(matches!(
        session.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Delivery::Accepted
    ));
    assert_eq!(
        session.stats().frames_contradicted,
        1,
        "the refused half is still counted"
    );
}

#[test]
fn a_second_disagreeing_digest_for_one_tick_is_refused_never_overwritten() {
    let mut session = session_of(0, 2, 0);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let body = wire::DigestBody {
        tick: 4,
        state_digest: 111,
        input_digest: 222,
    };
    let len = wire::write_digest(&mut out, addressing(1, SESSION), &body);
    assert!(matches!(
        session.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Delivery::Accepted
    ));
    // The identical one again is simply accepted.
    assert!(matches!(
        session.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Delivery::Accepted
    ));

    // A different one for the same tick: refused, so a flood of forged
    // digests cannot evict a genuine entry and blind the detector.
    let forged = wire::DigestBody {
        tick: 4,
        state_digest: 999,
        input_digest: 222,
    };
    let len = wire::write_digest(&mut out, addressing(1, SESSION), &forged);
    assert!(matches!(
        session.deliver(seat(1), out.get(..len).unwrap_or_default()),
        Delivery::Refused(Refusal::DigestContradiction { .. })
    ));
    assert_eq!(session.stats().digests_contradicted, 1);
}

#[test]
fn a_peer_still_filling_its_window_keeps_saying_hello() {
    // The arm where the phase is still `Joining` but no hello is owed:
    // every remote has been seen playing, so they have heard this peer —
    // and yet this peer cannot start until its own delay window is full.
    // It must keep emitting a hello rather than falling silent, or a
    // remote that missed the first one would never hear another.
    let mut session = session_of(0, 2, 2);
    let mut out = [0u8; MAX_DATAGRAM_BYTES];

    // An inputs datagram is proof the sender is playing, which is what
    // stops a hello being owed.
    let len = wire::write_inputs(&mut out, addressing(1, SESSION), 0, 1, 1, &[3]).expect("legal");
    let _ = session.deliver(seat(1), out.get(..len).unwrap_or_default());

    session
        .submit(&[1])
        .expect("one input, of the three it needs");
    assert!(!session.is_playing(), "the delay window is not full yet");

    // Drain one whole pump first: the emission cursor is chosen afresh at
    // the START of each pump, so the interesting decision is the second
    // one, not the constructor's.
    while session.next_outbound(&mut out).is_some() {}

    let datagram = session
        .next_outbound(&mut out)
        .expect("a joining peer must still announce itself");
    let read = wire::read(datagram.bytes()).expect("a datagram this crate wrote");
    assert_eq!(
        read.header.kind,
        wire::Kind::Hello,
        "a peer that cannot play yet says hello, not inputs"
    );
}
