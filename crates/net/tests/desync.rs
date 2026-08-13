//! A divergence, caught and written down.
//!
//! The report is the one artifact this crate produces for a human, so it
//! is tested the way a human reads it: force two peers to disagree about
//! the world while agreeing about the inputs, and check that the report
//! says exactly that — because "the inputs matched and the states did not"
//! is the sentence that sends someone to the simulation instead of to the
//! network.

use core::num::NonZeroU64;

use renew_net::{
    Advance, Delivery, MAX_DATAGRAM_BYTES, Outcome, PeerId, Session, SessionParams, ValidParams,
};

const SESSION: u64 = 0x0dd1_c0de;

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
fn params(local: u8) -> ValidParams {
    SessionParams {
        peer_count: 2,
        local: seat(local),
        input_bytes: 1,
        input_delay: 0,
        digest_period: 1,
        seed: 3,
        content: 0xc0_11,
        rules: 0xba_11,
        session: NonZeroU64::new(SESSION).expect("not zero"),
    }
    .validate()
    .expect("valid parameters")
}

/// Runs the pair until one of them ends, or the budget is spent.
///
/// `world_of` decides what each peer's world hashed to, which is how a
/// disagreement is injected without any dishonesty on the wire: both peers
/// send perfectly well-formed digests, they simply differ.
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "the harness's own failure is the report, and names the step it broke at"
)]
fn run_until_end(world_of: impl Fn(usize, u64) -> u64) -> [Box<Session>; 2] {
    let mut peers = [
        Box::new(Session::new(params(0))),
        Box::new(Session::new(params(1))),
    ];
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let mut inflight: Vec<(u8, Vec<u8>)> = Vec::new();

    // A budget, not an expectation. Whether ending is the right outcome
    // is the caller's question: the healthy control asserts the pair does
    // NOT end, and a harness that demanded an ending could never express
    // that.
    for _ in 0..400 {
        let carried = std::mem::take(&mut inflight);
        for (from, bytes) in carried {
            let target = usize::from(from ^ 1);
            if let Some(peer) = peers.get_mut(target)
                && matches!(peer.deliver(seat(from), &bytes), Delivery::Ends(_))
            {
                return peers;
            }
        }
        for index in 0..2usize {
            let Some(peer) = peers.get_mut(index) else {
                continue;
            };
            if peer.wants_local() {
                let tick = peer.next_local_tick();
                let _ = peer.submit(&[u8::try_from(tick & 0xff).unwrap_or(0)]);
            }
            loop {
                match peer.advance() {
                    Advance::Step(confirmed) => {
                        let tick = confirmed.tick();
                        let world = confirmed.digest_due().then(|| world_of(index, tick));
                        peer.commit(tick, world).expect("the tick just handed out");
                    }
                    Advance::Ended(_) => return peers,
                    _ => break,
                }
            }
            let sender = u8::try_from(index).unwrap_or(0);
            while let Some(datagram) = peer.next_outbound(&mut out) {
                inflight.push((sender, datagram.bytes().to_vec()));
            }
        }
    }
    peers
}

#[test]
fn agreeing_worlds_never_produce_a_report() {
    // The anti-vacuity half: if this ended on its own, the test below
    // would prove nothing about disagreement.
    let peers = run_until_end(|_, tick| tick.wrapping_mul(31));
    assert_eq!(peers[0].outcome(), None, "healthy peers must not end");
    assert!(peers[0].desync().is_none());
    assert!(
        peers[0].pending_tick() > 4,
        "the pair must actually have run"
    );
}

#[test]
fn a_diverging_world_is_caught_and_explained() {
    // Seat 1's world hashes differently from tick 3 onward. Their inputs
    // are identical throughout — which is the whole point.
    let peers = run_until_end(|index, tick| {
        if index == 1 && tick >= 3 {
            tick.wrapping_mul(31).wrapping_add(1)
        } else {
            tick.wrapping_mul(31)
        }
    });

    let ended = peers
        .iter()
        .find(|peer| matches!(peer.outcome(), Some(Outcome::Desynced { .. })))
        .expect("a divergence must be caught");
    let report = ended.desync().expect("an ended session explains itself");

    assert!(report.witnessed(), "the report compared nothing");
    assert!(
        report.inputs_agree(),
        "the inputs were identical, so the report must exonerate the network"
    );
    assert!(
        !report.dissenters().is_empty(),
        "a divergence must name who disagreed"
    );
    assert_eq!(report.peer_count, 2);
    assert_eq!(report.content, 0xc0_11);
    assert_eq!(report.rules, 0xba_11);
    assert_eq!(report.seed, 3);
    assert!(
        report
            .last_agreed_tick
            .is_some_and(|tick| tick < report.tick),
        "the divergence must be bracketed below the tick it was caught at"
    );
}

#[test]
fn the_report_says_so_when_the_inputs_are_what_differ() {
    // Hand-built rather than driven: two peers whose input folds differ
    // is a transport or submit-discipline bug, and the report must point
    // there instead of at the simulation.
    let peers = run_until_end(|index, tick| {
        if index == 1 && tick >= 3 {
            tick.wrapping_mul(31).wrapping_add(1)
        } else {
            tick.wrapping_mul(31)
        }
    });
    let ended = peers
        .iter()
        .find(|peer| peer.outcome().is_some())
        .expect("an end");
    let mut report = ended.desync().expect("a report");

    // Move one peer's input fingerprint and the verdict must flip.
    report.local_input_digest = report.local_input_digest.wrapping_add(1);
    assert!(
        !report.inputs_agree(),
        "differing input folds must not read as agreement"
    );
}

#[test]
fn a_report_nobody_witnessed_does_not_read_as_agreement() {
    let peers = run_until_end(|index, tick| {
        if index == 1 && tick >= 3 {
            tick.wrapping_mul(31).wrapping_add(1)
        } else {
            tick.wrapping_mul(31)
        }
    });
    let ended = peers
        .iter()
        .find(|peer| peer.outcome().is_some())
        .expect("an end");
    let mut report = ended.desync().expect("a report");

    report.peer_state_digests = [None; 8];
    report.peer_input_digests = [None; 8];
    assert!(
        !report.witnessed(),
        "a report with no peer entries witnessed nothing and must say so"
    );
    assert!(
        report.dissenters().is_empty(),
        "nobody can dissent in a report nobody sent"
    );
}

#[test]
fn the_report_renders_as_machine_readable_json() {
    let peers = run_until_end(|index, tick| {
        if index == 1 && tick >= 3 {
            tick.wrapping_mul(31).wrapping_add(1)
        } else {
            tick.wrapping_mul(31)
        }
    });
    let ended = peers
        .iter()
        .find(|peer| peer.outcome().is_some())
        .expect("an end");
    let report = ended.desync().expect("a report");
    let json = report.json().to_string();

    // Shape, not prettiness: a tool reads this.
    assert!(json.starts_with(r#"{"schema_version":1,"#), "got: {json}");
    assert!(json.ends_with('}'), "got: {json}");
    for key in [
        "\"tick\":",
        "\"local\":",
        "\"peer_count\":",
        "\"inputs_agree\":",
        "\"witnessed\":",
        "\"local_state_digest\":",
        "\"local_input_digest\":",
        "\"last_agreed_tick\":",
        "\"dissenters\":[",
        "\"peers\":[",
        "\"agreement_digest\":",
        "\"content\":",
        "\"rules\":",
        "\"seed\":",
        "\"stats\":{",
        "\"datagrams_refused\":",
        "\"stall_pumps\":",
    ] {
        assert!(json.contains(key), "the JSON is missing {key}: {json}");
    }
    assert_eq!(
        json.matches("\"seat\":").count(),
        2,
        "one entry per seat, whether or not that seat reported"
    );
}

#[test]
fn a_report_with_no_agreed_tick_renders_null_rather_than_a_number() {
    let peers = run_until_end(|index, tick| {
        if index == 1 && tick >= 3 {
            tick.wrapping_mul(31).wrapping_add(1)
        } else {
            tick.wrapping_mul(31)
        }
    });
    let ended = peers
        .iter()
        .find(|peer| peer.outcome().is_some())
        .expect("an end");
    let mut report = ended.desync().expect("a report");

    report.last_agreed_tick = None;
    report.peer_state_digests = [None; 8];
    report.peer_input_digests = [None; 8];
    let json = report.json().to_string();
    assert!(json.contains(r#""last_agreed_tick":null"#), "got: {json}");
    assert!(json.contains(r#""state_digest":null"#), "got: {json}");
    assert!(json.contains(r#""input_digest":null"#), "got: {json}");
    assert!(json.contains(r#""dissenters":[]"#), "got: {json}");
}

#[test]
fn a_report_naming_two_dissenters_separates_them() {
    // The JSON list separator only runs with more than one dissenter, and
    // a one-element list would leave it untested — which is how a
    // malformed array reaches a tool that reads this.
    let peers = run_until_end(|index, tick| {
        if index == 1 && tick >= 3 {
            tick.wrapping_mul(31).wrapping_add(1)
        } else {
            tick.wrapping_mul(31)
        }
    });
    let ended = peers
        .iter()
        .find(|peer| peer.outcome().is_some())
        .expect("an end");
    let mut report = ended.desync().expect("a report");

    // Widen the roster and disagree with two of its seats.
    report.peer_count = 4;
    report.local = seat(0);
    report.local_state_digest = 1;
    report.peer_state_digests = [None, Some(2), Some(3), None, None, None, None, None];
    assert_eq!(report.dissenters().count(), 2);

    let json = report.json().to_string();
    let start = json.find(r#""dissenters":["#).expect("the list is present");
    let tail = json.get(start..).unwrap_or_default();
    let end = tail.find(']').expect("the list closes");
    let list = tail.get(0..end).unwrap_or_default();
    assert!(
        list.contains("1,2"),
        "two dissenters must be comma-separated: {list}"
    );
}
