//! Two sessions in one process, wired to each other through a link that
//! loses, reorders and duplicates.
//!
//! The claim under test is the one the whole crate exists for: **given the
//! same parameters and the same submissions, every peer's sequence of
//! confirmed input sets is bit-identical, whatever the datagrams did on
//! the way.** Arrival is the non-determinism; making it unobservable is
//! the job.

use core::num::NonZeroU64;

use renew_net::{
    Advance, Delivery, MAX_DATAGRAM_BYTES, Outcome, PeerId, Session, SessionParams, SubmitError,
    wire,
};

const SESSION: u64 = 0xfeed_face_dead_beef;

#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn seat(index: u8) -> PeerId {
    PeerId::new(index).expect("a seat the test chose in range")
}

#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn params(local: u8, delay: u8) -> SessionParams {
    SessionParams {
        peer_count: 2,
        local: seat(local),
        input_bytes: 1,
        input_delay: delay,
        digest_period: 4,
        seed: 7,
        content: 0xc0_ffee,
        rules: 0xba5e_ba11,
        session: NonZeroU64::new(SESSION).expect("not zero"),
    }
}

#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn session(local: u8, delay: u8) -> Session {
    Session::new(params(local, delay).validate().expect("valid parameters"))
}

/// A datagram in flight, with the seat that sent it.
type Wire = (u8, Vec<u8>);

/// One peer's confirmed run: every tick, and the inputs that made it.
type Run = Vec<(u64, Vec<u8>)>;

/// Both peers' runs, and each peer's input fingerprint **at exactly the
/// requested tick count**.
///
/// Snapshotted at that tick rather than read at the end, and the
/// distinction is load-bearing: the fold runs once per confirmed tick, so
/// two runs that overshoot the target by different amounts hold different
/// folds while having confirmed identical inputs. Comparing the end state
/// of two such runs compares how far each happened to get, which is a
/// property of the link and not of the simulation.
type Converged = ([Run; 2], [u64; 2]);

/// Drain one session's outbound queue into a list of datagrams.
fn pump(from: &mut Session, sender: u8) -> Vec<Wire> {
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let mut sent = Vec::new();
    while let Some(datagram) = from.next_outbound(&mut out) {
        sent.push((sender, datagram.bytes().to_vec()));
    }
    sent
}

/// Run both sessions to a fixed number of confirmed ticks, applying
/// `hazard` to every datagram before it is delivered. Returns each
/// session's sequence of (tick, inputs) and its final input digest.
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "the harness's own failure is the report; a panic here names which invariant the two sessions broke, at the point it broke"
)]
fn converge(delay: u8, ticks: u64, mut hazard: impl FnMut(usize, &Wire) -> Vec<Wire>) -> Converged {
    // Boxed because a Session holds its whole input window inline —
    // roughly eight kilobytes — and two of them overflow the stack-array
    // ceiling the workspace lints at. A real driver keeps one, for the
    // lifetime of the session, which is why the type is shaped this way.
    let mut peers = [Box::new(session(0, delay)), Box::new(session(1, delay))];
    let mut confirmed: [Run; 2] = [Vec::new(), Vec::new()];
    let mut digest_at: [Option<u64>; 2] = [None, None];
    let mut inflight: Vec<Wire> = Vec::new();
    let mut step = 0usize;

    while confirmed.iter().any(|run| (run.len() as u64) < ticks) {
        step = step.saturating_add(1);
        assert!(step < 20_000, "the two sessions never converged");

        // Deliver everything the link decided to carry this pump.
        let carried: Vec<Wire> = inflight.drain(..).flat_map(|w| hazard(step, &w)).collect();
        for (sender, bytes) in carried {
            let target = usize::from(sender ^ 1);
            let source = seat(sender);
            if let Some(peer) = peers.get_mut(target) {
                match peer.deliver(source, &bytes) {
                    Delivery::Ends(outcome) => panic!("unexpected end: {outcome:?}"),
                    Delivery::Accepted | Delivery::Refused(_) => {}
                }
            }
        }

        for (index, peer) in peers.iter_mut().enumerate() {
            // One local input per pump, never a loop: a caller that
            // looped here would fill the window with one held intent.
            if peer.wants_local() {
                // Keyed on the tick being SUBMITTED for, never on the
                // confirmed frontier: an input chosen from the frontier
                // moves with the network, and the oracle below would then
                // be comparing two runs that were fed different things.
                let value = u8::try_from(index).unwrap_or(0).wrapping_mul(16);
                let tick = peer.next_local_tick();
                let byte = value.wrapping_add(u8::try_from(tick & 0x0f).unwrap_or(0));
                match peer.submit(&[byte]) {
                    Ok(_) | Err(SubmitError::WindowFull { .. }) => {}
                    Err(error) => panic!("submit refused: {error:?}"),
                }
            }

            loop {
                match peer.advance() {
                    Advance::Step(confirmed_step) => {
                        let tick = confirmed_step.tick();
                        let mut inputs = Vec::new();
                        for (_, bytes) in confirmed_step.inputs() {
                            inputs.extend_from_slice(bytes);
                        }
                        if let Some(run) = confirmed.get_mut(index) {
                            run.push((tick, inputs.clone()));
                        }
                        // A world digest that is a pure function of the
                        // confirmed inputs: two peers that confirmed the
                        // same thing must produce the same number, which
                        // is what makes a desync here mean something.
                        let world = confirmed.get(index).map_or(0u64, |run| {
                            run.iter().fold(0u64, |acc, (t, bytes)| {
                                bytes
                                    .iter()
                                    .fold(acc ^ t, |a, b| a.rotate_left(7) ^ u64::from(*b))
                            })
                        });
                        peer.commit(tick, confirmed_step.digest_due().then_some(world))
                            .expect("the tick just handed out");
                        if confirmed
                            .get(index)
                            .is_some_and(|run| run.len() as u64 == ticks)
                            && let Some(slot) = digest_at.get_mut(index)
                            && slot.is_none()
                        {
                            *slot = Some(peer.input_digest());
                        }
                    }
                    Advance::Waiting | Advance::Stalled { .. } => break,
                    Advance::Ended(outcome) => panic!("unexpected end: {outcome:?}"),
                }
            }

            let sender = u8::try_from(index).unwrap_or(0);
            inflight.extend(pump(peer, sender));
        }
    }

    // Truncated to exactly the requested count, and that is the claim
    // being made rather than a convenience. Peers run at their own pace,
    // so the loop stops when BOTH have reached the target and one may have
    // taken a further tick inside that last pump. "Both stopped at the
    // same instant" is not a property of lockstep and could not be: what
    // is promised is that the sequences agree wherever both have one.
    for run in &mut confirmed {
        run.truncate(usize::try_from(ticks).unwrap_or(usize::MAX));
    }
    let digests = [
        digest_at[0].unwrap_or_else(|| peers[0].input_digest()),
        digest_at[1].unwrap_or_else(|| peers[1].input_digest()),
    ];
    (confirmed, digests)
}

/// The link is perfect. The floor everything else is measured against.
#[test]
fn two_peers_confirm_the_same_ticks_over_a_perfect_link() {
    let (runs, digests) = converge(2, 40, |_, wire| vec![wire.clone()]);
    assert_eq!(runs[0], runs[1], "the two peers confirmed different inputs");
    assert_eq!(digests[0], digests[1], "the input digests diverged");
    assert!(runs[0].len() >= 40);
    // Ticks are consecutive from zero, with no gap and no repeat.
    for (position, (tick, _)) in runs[0].iter().enumerate() {
        assert_eq!(*tick, position as u64, "ticks are consecutive from zero");
    }
}

/// Every third datagram is dropped. The redundancy repairs it with no
/// round trip, and no confirmed value moves.
#[test]
fn loss_changes_nothing_a_peer_confirms() {
    let (runs, digests) = converge(2, 40, |step, wire| {
        if step % 3 == 0 {
            Vec::new()
        } else {
            vec![wire.clone()]
        }
    });
    assert_eq!(runs[0], runs[1]);
    assert_eq!(digests[0], digests[1]);
    let (perfect, perfect_digests) = converge(2, 40, |_, wire| vec![wire.clone()]);
    assert_eq!(
        runs[0][..40],
        perfect[0][..40],
        "loss moved a confirmed input, which is the one thing it must never do"
    );
    assert_eq!(digests[0], perfect_digests[0]);
}

/// Every datagram is delivered twice. A repeat is proven identical and
/// ignored, and must not fold into the digest a second time.
#[test]
fn duplication_changes_nothing_a_peer_confirms() {
    let (runs, digests) = converge(2, 40, |_, wire| vec![wire.clone(), wire.clone()]);
    let (perfect, perfect_digests) = converge(2, 40, |_, wire| vec![wire.clone()]);
    assert_eq!(runs[0], runs[1]);
    assert_eq!(runs[0][..40], perfect[0][..40]);
    assert_eq!(
        digests[0], perfect_digests[0],
        "a repeated frame moved the digest, so it was absorbed twice"
    );
}

/// Loss, duplication and delay together.
#[test]
fn a_hostile_link_changes_nothing_a_peer_confirms() {
    let mut held: Vec<Wire> = Vec::new();
    let (runs, digests) = converge(3, 60, move |step, wire| {
        let mut carried = Vec::new();
        // Delay: hold this one and release what was held before.
        carried.append(&mut held);
        match step % 4 {
            0 => {}                       // dropped
            1 => held.push(wire.clone()), // delayed
            2 => {
                carried.push(wire.clone());
                carried.push(wire.clone()); // duplicated
            }
            _ => carried.push(wire.clone()),
        }
        carried.reverse(); // reordered
        carried
    });
    let (perfect, perfect_digests) = converge(3, 60, |_, wire| vec![wire.clone()]);
    assert_eq!(runs[0], runs[1]);
    assert_eq!(
        runs[0][..60],
        perfect[0][..60],
        "the link reached a confirmed value"
    );
    assert_eq!(digests[0], perfect_digests[0]);
}

/// The delay parameter changes when a tick can run, and nothing else.
#[test]
fn the_input_delay_does_not_reach_a_confirmed_value() {
    let (zero, zero_digests) = converge(0, 30, |_, wire| vec![wire.clone()]);
    let (five, five_digests) = converge(5, 30, |_, wire| vec![wire.clone()]);
    assert_eq!(
        zero[0][..30],
        five[0][..30],
        "input_delay is a latency knob, not a simulation one"
    );
    assert_eq!(zero_digests[0], five_digests[0]);
}

// ---- the state machine's own rules ----

#[test]
fn no_tick_exists_until_everyone_has_said_hello() {
    let mut alone = session(0, 0);
    assert!(!alone.is_playing());
    alone.submit(&[1]).expect("a first input is always welcome");
    assert!(
        !alone.is_playing(),
        "a session with an unheard peer must not start"
    );
    assert!(matches!(alone.advance(), Advance::Waiting));
}

#[test]
fn a_local_input_cannot_be_revised_after_it_is_submitted() {
    let mut peer = session(0, 4);
    peer.submit(&[1]).expect("first");
    let second = peer.submit(&[2]).expect("the next tick, not the same one");
    assert_eq!(
        second, 1,
        "submit advances to the next tick, never rewrites"
    );
}

#[test]
fn an_input_of_the_wrong_width_is_refused() {
    let mut peer = session(0, 0);
    assert_eq!(
        peer.submit(&[1, 2]),
        Err(SubmitError::WrongWidth { saw: 2, agreed: 1 })
    );
    assert_eq!(
        peer.submit(&[]),
        Err(SubmitError::WrongWidth { saw: 0, agreed: 1 })
    );
}

#[test]
fn leaving_is_terminal_and_names_the_last_confirmed_tick() {
    let mut peer = session(0, 0);
    let outcome = peer.leave();
    assert!(matches!(outcome, Outcome::LeftLocally { .. }));
    assert_eq!(peer.outcome(), Some(outcome));
    assert!(matches!(peer.advance(), Advance::Ended(_)));
    assert!(!peer.wants_local());
    assert!(matches!(peer.submit(&[0]), Err(SubmitError::Ended(_))));
}

/// The regression for the deadlock the allocation gate found.
///
/// An earlier `render_inputs` bounded its redundancy run by the *sender's*
/// confirmed frontier. The moment a peer confirmed a tick it stopped
/// repeating it — so a peer that had never received that frame could never
/// be sent it again. Both then waited on each other forever, and nothing
/// raised an error: no refusal, no desync, no timeout. Two sessions
/// politely stalled, which is the worst shape a bug can take here.
///
/// **The hazard has to be one-way, and that is the whole point.** A
/// symmetric blackout stalls both peers equally and never reproduces it —
/// the first version of this test did exactly that and passed against the
/// bug. The deadlock needs one peer to run *ahead*: seat 1 keeps hearing
/// seat 0 and confirms ticks, while seat 0 hears nothing and starves. When
/// the link heals, seat 1 must still be willing to repeat frames it has
/// long since confirmed, or seat 0 never catches up.
#[test]
fn a_starved_peer_still_catches_up_after_a_one_way_blackout() {
    // Seat 0's datagrams always arrive. Seat 1's are dropped for the first
    // stretch, so seat 1 races ahead while seat 0 waits on tick zero.
    let (runs, digests) = converge(2, 30, |step, wire| {
        let (sender, _) = wire;
        if *sender == 1 && step <= 10 {
            Vec::new()
        } else {
            vec![wire.clone()]
        }
    });
    assert_eq!(runs[0], runs[1], "the two peers confirmed different inputs");
    assert_eq!(digests[0], digests[1]);

    let (perfect, perfect_digests) = converge(2, 30, |_, wire| vec![wire.clone()]);
    assert_eq!(
        runs[0][..30],
        perfect[0][..30],
        "a one-way blackout reached a confirmed value"
    );
    assert_eq!(digests[0], perfect_digests[0]);
}

/// **A goodbye does not discard a tick the roster already agreed to.**
///
/// Every tick this session has confirmed was confirmed with the departing
/// peer's own input in it — the roster had to be complete or `pending`
/// would never have reached it. Reporting the ending before handing those
/// out throws away work every participant agreed to, including the one
/// that left.
///
/// The visible cost is two peers reporting different tick counts for the
/// same session: whoever consumed the last step before the goodbye landed
/// is one ahead of whoever did not, and two worlds one step apart agree
/// about nothing. That reads as a desync and is not one.
///
/// Written against the old behaviour first, where it failed with zero
/// steps drained: `advance` returned `Ended` on its first line, before
/// `pending` was looked at.
#[test]
fn a_goodbye_hands_out_the_ticks_it_already_confirmed() {
    let mut local = Box::new(session(0, 2));
    let mut far = Box::new(session(1, 2));

    // Both peers up and running, with a window of confirmed ticks that
    // neither has consumed. The far peer's inputs are what make them
    // confirmable, so what follows is agreed by the peer about to leave.
    let mut inflight: Vec<Wire> = Vec::new();
    for _ in 0..24 {
        for (index, peer) in [&mut local, &mut far].into_iter().enumerate() {
            if peer.wants_local() {
                let byte = u8::try_from(index).unwrap_or(0);
                let _ = peer.submit(&[byte]);
            }
        }
        inflight.extend(pump(&mut local, 0));
        inflight.extend(pump(&mut far, 1));
        for (sender, bytes) in inflight.drain(..) {
            let target: &mut Session = if sender == 0 { &mut far } else { &mut local };
            let _ = target.deliver(seat(sender), &bytes);
        }
    }

    // How much the local peer is holding, unconsumed, before anything
    // leaves. Counted by draining a clone-free dry run is not available,
    // so it is counted by draining for real below and compared against
    // this floor instead.
    let held_before = local.outcome();
    assert!(
        held_before.is_none(),
        "the session ended before the test began: {held_before:?}"
    );

    // The far peer says goodbye at the last tick it confirmed.
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let addressing = wire::Addressing {
        sender: seat(1),
        session: NonZeroU64::new(SESSION).expect("not zero"),
    };
    let len = wire::write_bye(&mut out, addressing, &wire::ByeBody { tick: 0 });
    assert!(
        matches!(local.deliver(seat(1), &out[..len]), Delivery::Ends(_)),
        "the goodbye was not taken as an ending"
    );

    // Now drain. Every step handed out here was agreed before the
    // goodbye; the ending must come after them, not instead of them.
    let mut drained = 0u64;
    loop {
        match local.advance() {
            Advance::Step(step) => {
                let tick = step.tick();
                // A digest is owed on the period, and a drained tick is
                // no different from any other in that respect.
                let owed = step.digest_due().then_some(tick);
                drained = drained.saturating_add(1);
                local.commit(tick, owed).expect("a step it just handed out");
            }
            Advance::Ended(Outcome::PeerLeft { peer, .. }) => {
                assert_eq!(peer, seat(1), "the wrong seat was named as having left");
                break;
            }
            other => panic!("a drained session should end, not {other:?}"),
        }
    }

    assert!(
        drained > 0,
        "the goodbye discarded every confirmed tick the roster had agreed to"
    );
}
