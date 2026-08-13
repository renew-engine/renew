//! The agreement handshake, driven end to end.
//!
//! Every test here runs real lobbies against a link that carries bytes by
//! endpoint, because the property under test is *agreement between
//! machines* and a single lobby inspected from outside cannot show it.

use core::num::NonZeroU64;

use renew_net::{
    HostSetup, JoinSetup, Lobby, LobbyError, LobbyRefusal, LobbyState, MAX_DATAGRAM_BYTES,
    MAX_PEERS, PeerId, START_REPEATS, UNKNOWN_ENDPOINT, UNSEATED_SESSION, wire,
};

type Endpoint = renew_net::Endpoint;

/// Endpoints are opaque, so any distinct bytes will do. The first byte is
/// the machine number purely so a failure prints something readable.
fn endpoint(machine: u8) -> Endpoint {
    let mut out = UNKNOWN_ENDPOINT;
    out[0] = machine;
    out[17] = 1;
    out
}

const HOST_AT: u8 = 1;

#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn session() -> NonZeroU64 {
    NonZeroU64::new(0x5e55_1017).expect("nonzero")
}

fn host_setup() -> HostSetup {
    HostSetup {
        session: session(),
        seed: 0xC0FF_EE00_1234_5678,
        content: 0xAAAA_BBBB,
        rules: 0x1111_2222,
        input_bytes: 2,
        input_delay: 2,
        digest_period: 8,
    }
}

fn join_setup() -> JoinSetup {
    JoinSetup {
        host: endpoint(HOST_AT),
        content: 0xAAAA_BBBB,
        rules: 0x1111_2222,
    }
}

/// One datagram in flight: who sent it, where to, and the bytes.
struct InFlight {
    from: Endpoint,
    to: Endpoint,
    bytes: Vec<u8>,
}

/// A host and its joiners, and a link between them.
struct World {
    host: Lobby,
    joiners: Vec<(Endpoint, Lobby)>,
    /// Endpoints whose datagrams are thrown away, in both directions.
    blackholed: Vec<Endpoint>,
    /// Endpoints that stop hearing rosters but keep hearing everything
    /// else. A blunt blackout would prove only that a peer hearing
    /// nothing does nothing; this leaves the peer listening, so what
    /// stops it is a check rather than a silence.
    roster_deaf: Vec<Endpoint>,
}

impl World {
    fn new(joiners: &[u8]) -> Self {
        Self {
            host: Lobby::host(host_setup()),
            joiners: joiners
                .iter()
                .map(|&machine| (endpoint(machine), Lobby::join(join_setup())))
                .collect(),
            blackholed: Vec::new(),
            roster_deaf: Vec::new(),
        }
    }

    fn drain(&mut self) -> Vec<InFlight> {
        let mut flight = Vec::new();
        let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
        let from = endpoint(HOST_AT);
        while let Some(out) = self.host.next_outbound(&mut buffer) {
            flight.push(InFlight {
                from,
                to: out.to(),
                bytes: out.bytes().to_vec(),
            });
        }
        for (at, lobby) in &mut self.joiners {
            while let Some(out) = lobby.next_outbound(&mut buffer) {
                flight.push(InFlight {
                    from: *at,
                    to: out.to(),
                    bytes: out.bytes().to_vec(),
                });
            }
        }
        flight
    }

    /// One pump: everybody emits, then everything lands.
    fn pump(&mut self) {
        for packet in self.drain() {
            if self.blackholed.contains(&packet.to) || self.blackholed.contains(&packet.from) {
                continue;
            }
            if self.roster_deaf.contains(&packet.to)
                && wire::read(&packet.bytes).is_ok_and(|d| d.header.kind == wire::Kind::Roster)
            {
                continue;
            }
            if packet.to == endpoint(HOST_AT) {
                let _ = self.host.deliver(packet.from, &packet.bytes);
                continue;
            }
            for (at, lobby) in &mut self.joiners {
                if *at == packet.to {
                    let _ = lobby.deliver(packet.from, &packet.bytes);
                }
            }
        }
    }

    fn pumps(&mut self, count: usize) {
        for _ in 0..count {
            self.pump();
        }
    }
}

#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn seat(index: u8) -> PeerId {
    PeerId::new(index).expect("a seat inside the roster")
}

// ---- the happy path ----

#[test]
fn a_host_and_two_joiners_reach_one_agreement() {
    let mut world = World::new(&[2, 3]);
    world.pumps(3);
    assert_eq!(world.host.state(), LobbyState::Hosting { seated: 3 });

    world.host.start().expect("three seats is a session");
    world.pumps(2);

    let agreed = world.host.agreed().expect("the host agreed with itself");
    assert_eq!(agreed.params().peer_count(), 3);
    assert_eq!(agreed.params().local(), seat(0));

    // The property the whole module exists for: every peer computed the
    // same agreement independently, and each knows which seat it is.
    for (index, (_, joiner)) in world.joiners.iter().enumerate() {
        let theirs = joiner.agreed().expect("a joiner that heard go");
        assert_eq!(
            theirs.params().agreement_digest(),
            agreed.params().agreement_digest(),
            "joiner {index} agreed to a different game than the host"
        );
        assert_eq!(theirs.params().peer_count(), 3);
        assert_eq!(theirs.params().get().seed, agreed.params().get().seed);
    }
    let seats: Vec<u8> = world
        .joiners
        .iter()
        .map(|(_, lobby)| lobby.agreed().expect("agreed").params().local().index())
        .collect();
    assert_eq!(seats, vec![1, 2], "seats are handed out in arrival order");
}

#[test]
fn every_peer_learns_where_every_other_peer_is() {
    let mut world = World::new(&[2, 3]);
    world.pumps(3);
    world.host.start().expect("start");
    world.pumps(2);

    // The host learned both joiners from the transport.
    let host = world.host.agreed().expect("agreed");
    assert_eq!(host.endpoint(seat(1)), Some(endpoint(2)));
    assert_eq!(host.endpoint(seat(2)), Some(endpoint(3)));
    assert_eq!(
        host.endpoint(seat(0)),
        Some(UNKNOWN_ENDPOINT),
        "a host cannot see its own address from outside, and must not guess"
    );

    let (_, first) = world.joiners.first().expect("a joiner");
    let table = first.agreed().expect("agreed");
    assert_eq!(
        table.endpoint(seat(0)),
        Some(endpoint(HOST_AT)),
        "seat zero is filled in from where the roster actually arrived"
    );
    assert_eq!(table.endpoint(seat(2)), Some(endpoint(3)));
    assert_eq!(table.endpoint(seat(3)), None, "past the roster is nothing");
}

#[test]
fn a_seated_joiner_keeps_asking_until_it_starts() {
    // It used to go quiet the moment it held any roster, and that was a
    // wedge with no way out: one stale or forged roster set the held
    // state, the joiner never spoke again, and every genuine roster after
    // it was refused while the peer sat in `Seated` looking healthy.
    // Asking until the go-ahead is what makes a bad roster survivable.
    let mut world = World::new(&[2]);
    world.pumps(3);
    assert!(matches!(
        world.joiners[0].1.state(),
        LobbyState::Seated { .. }
    ));
    assert!(
        world
            .drain()
            .into_iter()
            .any(|packet| packet.from == endpoint(2)),
        "a seated joiner still has something to say: it has not started yet"
    );

    world.host.start().expect("start");
    world.pumps(2);
    assert!(world.joiners[0].1.agreed().is_some(), "it started");
    assert!(
        !world
            .drain()
            .into_iter()
            .any(|packet| packet.from == endpoint(2)),
        "and then it stops, because the lobby is over and the session owns the wire"
    );
}

#[test]
fn the_start_run_is_finite() {
    let mut world = World::new(&[2]);
    world.pumps(2);
    world.host.start().expect("start");
    world.pumps(usize::from(START_REPEATS) + 2);
    assert!(
        world.drain().is_empty(),
        "a lobby that never goes quiet is a lobby still sending during play"
    );
}

// ---- the roster grows, and a stale peer refuses rather than forks ----

#[test]
fn a_joiner_that_missed_the_last_roster_refuses_to_start() {
    let mut world = World::new(&[2]);
    world.pumps(3);
    let first = world.joiners[0].1.agreed();
    assert!(first.is_none(), "seated is not started");

    // A second player arrives, so the roster the first joiner holds is
    // now one seat short — and from here that joiner hears no further
    // roster. It still hears everything else, which is what makes this a
    // test of the fingerprint rather than of silence: the `Start` lands,
    // and the joiner has to be the thing that refuses it.
    world.joiners.push((endpoint(3), Lobby::join(join_setup())));
    world.roster_deaf.push(endpoint(2));
    world.pumps(3);
    world.host.start().expect("start");
    world.pumps(3);

    // The host and the peer that kept hearing agree; the stale one did
    // not start at all, which is the only safe answer. Starting on a
    // two-seat roster while everyone else runs three is a fork agreed to
    // in advance, and the agreement fingerprint is what refuses it.
    assert_eq!(
        world.host.agreed().expect("agreed").params().peer_count(),
        3
    );
    assert!(world.joiners[1].1.agreed().is_some());
    assert!(
        world.joiners[0].1.agreed().is_none(),
        "a peer holding a stale roster must not start"
    );

    // And it refused for the stated reason. Without this the test would
    // pass just as well against a peer that heard nothing at all.
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_start(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: session(),
        },
        &wire::StartBody {
            agreement_digest: world
                .host
                .agreed()
                .expect("agreed")
                .params()
                .agreement_digest(),
        },
    );
    let refusal = world.joiners[0]
        .1
        .deliver(endpoint(HOST_AT), out.get(..len).expect("written"))
        .expect_err("a three-seat go, at a peer holding two seats");
    assert!(
        matches!(refusal, LobbyRefusal::AgreementMismatch { .. }),
        "expected the fingerprint to be what refused, got {refusal:?}"
    );
}

#[test]
fn a_start_whose_fingerprint_disagrees_is_refused_by_name() {
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(join_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];

    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();
    host.deliver(endpoint(2), &join).expect("a good join");
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let roster = host
        .next_outbound(&mut buffer)
        .expect("a roster")
        .bytes()
        .to_vec();
    joiner
        .deliver(endpoint(HOST_AT), &roster)
        .expect("a good roster");

    // A `Start` the host never sent, carrying a fingerprint for some
    // other agreement.
    let mut forged = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_start(
        &mut forged,
        wire::Addressing {
            sender: seat(0),
            session: session(),
        },
        &wire::StartBody {
            agreement_digest: 0xDEAD_BEEF,
        },
    );
    let refusal = joiner
        .deliver(endpoint(HOST_AT), forged.get(..len).expect("written"))
        .expect_err("a fingerprint that is not ours");
    assert!(matches!(
        refusal,
        LobbyRefusal::AgreementMismatch {
            theirs: 0xDEAD_BEEF,
            ..
        }
    ));
    assert!(joiner.agreed().is_none());
}

// ---- the door ----

#[test]
fn a_joiner_running_other_content_is_refused_at_the_door() {
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(JoinSetup {
        content: 0x0BAD_0BAD,
        ..join_setup()
    });
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();

    let refusal = host
        .deliver(endpoint(2), &join)
        .expect_err("different content is not the same game");
    assert!(matches!(
        refusal,
        LobbyRefusal::ContentMismatch {
            ours: 0xAAAA_BBBB,
            theirs: 0x0BAD_0BAD
        }
    ));
    assert_eq!(
        host.state(),
        LobbyState::Hosting { seated: 1 },
        "a refused joiner must not take a seat"
    );
}

#[test]
fn a_joiner_running_other_rules_is_refused_at_the_door() {
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(JoinSetup {
        rules: 0x9999,
        ..join_setup()
    });
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();
    assert!(matches!(
        host.deliver(endpoint(2), &join),
        Err(LobbyRefusal::RulesMismatch { theirs: 0x9999, .. })
    ));
}

#[test]
fn one_endpoint_takes_one_seat_however_often_it_asks() {
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(join_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();

    for _ in 0..20 {
        host.deliver(endpoint(2), &join)
            .expect("a repeat is not a refusal");
    }
    assert_eq!(
        host.state(),
        LobbyState::Hosting { seated: 2 },
        "the redundancy that makes a Join arrive must not fill the lobby with one player"
    );
}

#[test]
fn the_lobby_fills_and_then_refuses() {
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(join_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();

    for machine in 2..=MAX_PEERS {
        host.deliver(endpoint(machine), &join)
            .expect("room for this one");
    }
    assert_eq!(host.state(), LobbyState::Hosting { seated: MAX_PEERS });
    assert!(matches!(
        host.deliver(endpoint(MAX_PEERS + 1), &join),
        Err(LobbyRefusal::Full { ceiling: 8 })
    ));
}

// ---- the checks that keep this from being a weapon ----

#[test]
fn a_join_is_never_smaller_than_the_roster_it_provokes() {
    // **The anti-amplification property, measured rather than asserted.**
    // The first version of this module claimed removing a self-reported
    // address had closed a reflector. It had not: forging a UDP source
    // needs an unfiltered uplink and nothing else, and a host that seats
    // whatever asks then sends it a roster every pump for the life of the
    // lobby — one small packet buying an unbounded reply aimed anywhere.
    //
    // Two things bound it now, and this measures both: a `Join` is padded
    // to the datagram ceiling, so no answer can exceed it; and a seat's
    // roster is paid for in credit that decays, so one datagram cannot
    // buy a subscription.
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(join_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();

    // One spoofed join, from a source that will never speak again.
    let victim = endpoint(99);
    host.deliver(victim, &join).expect("seated");

    let mut sent = 0usize;
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    for _ in 0..500 {
        while let Some(out) = host.next_outbound(&mut buffer) {
            if out.to() == victim {
                sent = sent.saturating_add(out.bytes().len());
            }
        }
    }
    assert!(
        sent <= join.len(),
        "one {}-byte join bought {sent} bytes aimed at a third party — that is a reflector",
        join.len()
    );

    // And the bound is not vacuous: a peer that keeps asking keeps being
    // answered, which is the whole reason the credit exists rather than a
    // flat refusal.
    let mut answered = 0usize;
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    for _ in 0..500 {
        host.deliver(victim, &join).expect("still asking");
        while let Some(out) = host.next_outbound(&mut buffer) {
            if out.to() == victim {
                answered = answered.saturating_add(1);
            }
        }
    }
    assert!(
        answered > 400,
        "a peer that asks must be answered: {answered}"
    );
}

#[test]
fn a_roster_from_a_bystander_is_refused() {
    let mut joiner = Lobby::join(join_setup());
    let mut host = Lobby::host(host_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();
    host.deliver(endpoint(2), &join).expect("seated");
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let roster = host
        .next_outbound(&mut buffer)
        .expect("a roster")
        .bytes()
        .to_vec();

    // Byte-identical to the roster the real host sent, arriving from
    // somewhere else. Without this check anyone who can send one packet
    // can seat a player in a game they never chose.
    assert!(matches!(
        joiner.deliver(endpoint(99), &roster),
        Err(LobbyRefusal::NotFromHost)
    ));
    assert_eq!(joiner.state(), LobbyState::Joining);
    joiner
        .deliver(endpoint(HOST_AT), &roster)
        .expect("the real host");
    assert_eq!(joiner.state(), LobbyState::Seated { seat: seat(1) });
}

#[test]
fn a_join_must_be_spelled_exactly_one_way() {
    let mut host = Lobby::host(host_setup());
    let body = wire::JoinBody {
        content: 0xAAAA_BBBB,
        rules: 0x1111_2222,
    };

    // A session field a sender could fill freely would be 2^64-1
    // spellings of "no session yet", which this crate does not admit.
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_join(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: session(),
        },
        &body,
    );
    assert!(matches!(
        host.deliver(endpoint(2), out.get(..len).expect("written")),
        Err(LobbyRefusal::NotUnseatedSession { .. })
    ));

    // And a joiner has no seat to name.
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_join(
        &mut out,
        wire::Addressing {
            sender: seat(3),
            session: UNSEATED_SESSION,
        },
        &body,
    );
    assert!(matches!(
        host.deliver(endpoint(2), out.get(..len).expect("written")),
        Err(LobbyRefusal::NotUnseatedSeat { .. })
    ));
    assert_eq!(
        host.state(),
        LobbyState::Hosting { seated: 1 },
        "neither misspelling may seat anyone"
    );
}

#[test]
fn session_traffic_is_refused_by_name_rather_than_ignored() {
    let mut host = Lobby::host(host_setup());
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_bye(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: session(),
        },
        &wire::ByeBody { tick: 12 },
    );
    assert!(matches!(
        host.deliver(endpoint(2), out.get(..len).expect("written")),
        Err(LobbyRefusal::NotLobbyTraffic {
            kind: wire::Kind::Bye
        })
    ));
}

#[test]
fn a_host_refuses_a_roster_and_a_joiner_refuses_a_join() {
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(join_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();

    assert!(matches!(
        joiner.deliver(endpoint(3), &join),
        Err(LobbyRefusal::WrongRole {
            kind: wire::Kind::Join
        })
    ));

    host.deliver(endpoint(2), &join).expect("seated");
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let roster = host
        .next_outbound(&mut buffer)
        .expect("a roster")
        .bytes()
        .to_vec();
    assert!(matches!(
        host.deliver(endpoint(3), &roster),
        Err(LobbyRefusal::WrongRole {
            kind: wire::Kind::Roster
        })
    ));
}

#[test]
fn a_seat_is_assigned_once() {
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(join_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();
    host.deliver(endpoint(2), &join).expect("seated");
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let roster = host
        .next_outbound(&mut buffer)
        .expect("a roster")
        .bytes()
        .to_vec();
    joiner.deliver(endpoint(HOST_AT), &roster).expect("seated");

    // A second host, or the same one having forgotten, offering a
    // different seat. Taking it would mean two peers submitting inputs
    // for one seat and neither for another.
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let endpoints = [0u8; 18 * 3];
    let len = wire::write_roster(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: session(),
        },
        &wire::RosterBody {
            seat: 2,
            peer_count: 3,
            input_bytes: 2,
            input_delay: 2,
            digest_period: 8,
            seed: 0xC0FF_EE00_1234_5678,
            content: 0xAAAA_BBBB,
            rules: 0x1111_2222,
            endpoints: &endpoints,
        },
    )
    .expect("a well-formed roster");
    assert!(matches!(
        joiner.deliver(endpoint(HOST_AT), out.get(..len).expect("written")),
        Err(LobbyRefusal::SeatMoved { .. })
    ));
    assert_eq!(joiner.state(), LobbyState::Seated { seat: seat(1) });
}

#[test]
fn a_start_before_a_roster_has_nothing_to_start() {
    let mut joiner = Lobby::join(join_setup());
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_start(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: session(),
        },
        &wire::StartBody {
            agreement_digest: 7,
        },
    );
    assert!(matches!(
        joiner.deliver(endpoint(HOST_AT), out.get(..len).expect("written")),
        Err(LobbyRefusal::NotSeatedYet)
    ));
}

// ---- saying go ----

#[test]
fn one_peer_is_not_multiplayer() {
    let mut host = Lobby::host(host_setup());
    assert!(matches!(
        host.start(),
        Err(LobbyError::NotEnoughPeers {
            seated: 1,
            floor: 2
        })
    ));
}

#[test]
fn only_the_host_says_go() {
    let mut joiner = Lobby::join(join_setup());
    assert_eq!(joiner.start(), Err(LobbyError::NotTheHost));
}

#[test]
fn go_is_said_once() {
    let mut world = World::new(&[2]);
    world.pumps(2);
    world.host.start().expect("start");
    assert_eq!(world.host.start(), Err(LobbyError::AlreadyStarted));
}

#[test]
fn traffic_after_the_lobby_is_over_is_refused() {
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(join_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();
    host.deliver(endpoint(2), &join).expect("seated");
    host.start().expect("start");
    assert!(matches!(
        host.deliver(endpoint(3), &join),
        Err(LobbyRefusal::AlreadyStarted)
    ));
}

#[test]
fn a_hosts_own_numbers_are_checked_before_anyone_commits_to_them() {
    let mut host = Lobby::host(HostSetup {
        input_bytes: 0,
        ..host_setup()
    });
    let mut joiner = Lobby::join(join_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();
    host.deliver(endpoint(2), &join).expect("seated");
    assert!(matches!(host.start(), Err(LobbyError::Parameters(_))));
    assert_eq!(
        host.state(),
        LobbyState::Hosting { seated: 2 },
        "a host that cannot start must not believe it did"
    );
}

// ---- what a joiner refuses, one arm each ----

/// A seated joiner, and the host that seated it.
#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn seated_pair() -> (Lobby, Lobby) {
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(join_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();
    host.deliver(endpoint(2), &join).expect("seated");
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let roster = host
        .next_outbound(&mut buffer)
        .expect("a roster")
        .bytes()
        .to_vec();
    joiner
        .deliver(endpoint(HOST_AT), &roster)
        .expect("a good roster");
    (host, joiner)
}

/// A roster the host never sent, so each field can be varied one at a
/// time. Every argument defaults through [`host_setup`], so a test names
/// only the thing it is changing.
#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn forged_roster(
    out: &mut [u8; MAX_DATAGRAM_BYTES],
    sender: PeerId,
    session: NonZeroU64,
    content: u64,
    rules: u64,
) -> usize {
    let endpoints = [0u8; 18 * 2];
    wire::write_roster(
        out,
        wire::Addressing { sender, session },
        &wire::RosterBody {
            seat: 1,
            peer_count: 2,
            input_bytes: 2,
            input_delay: 2,
            digest_period: 8,
            seed: 0xC0FF_EE00_1234_5678,
            content,
            rules,
            endpoints: &endpoints,
        },
    )
    .expect("a well-formed roster")
}

#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn forged_start(out: &mut [u8; MAX_DATAGRAM_BYTES], sender: PeerId, session: NonZeroU64) -> usize {
    wire::write_start(
        out,
        wire::Addressing { sender, session },
        &wire::StartBody {
            agreement_digest: 0,
        },
    )
}

#[test]
fn a_roster_not_from_seat_zero_is_refused() {
    let mut joiner = Lobby::join(join_setup());
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = forged_roster(&mut out, seat(4), session(), 0xAAAA_BBBB, 0x1111_2222);
    assert!(matches!(
        joiner.deliver(endpoint(HOST_AT), out.get(..len).expect("written")),
        Err(LobbyRefusal::HostNotSeatZero { .. })
    ));
}

#[test]
fn a_roster_for_another_session_is_refused_once_seated() {
    let (_, mut joiner) = seated_pair();
    let other = NonZeroU64::new(0x9999).expect("nonzero");
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = forged_roster(&mut out, seat(0), other, 0xAAAA_BBBB, 0x1111_2222);
    // A host restarted between the roster this peer holds and this one.
    // Following it would put this peer in a game the others left.
    assert!(matches!(
        joiner.deliver(endpoint(HOST_AT), out.get(..len).expect("written")),
        Err(LobbyRefusal::WrongSession { .. })
    ));
}

#[test]
fn a_roster_describing_other_content_or_rules_is_refused() {
    let mut joiner = Lobby::join(join_setup());
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = forged_roster(&mut out, seat(0), session(), 0x0BAD, 0x1111_2222);
    assert!(matches!(
        joiner.deliver(endpoint(HOST_AT), out.get(..len).expect("written")),
        Err(LobbyRefusal::ContentMismatch { theirs: 0x0BAD, .. })
    ));

    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = forged_roster(&mut out, seat(0), session(), 0xAAAA_BBBB, 0x0BAD);
    assert!(matches!(
        joiner.deliver(endpoint(HOST_AT), out.get(..len).expect("written")),
        Err(LobbyRefusal::RulesMismatch { theirs: 0x0BAD, .. })
    ));
    assert_eq!(
        joiner.state(),
        LobbyState::Joining,
        "neither may seat this peer"
    );
}

#[test]
fn a_host_refuses_a_start() {
    let (mut host, _) = seated_pair();
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = forged_start(&mut out, seat(0), session());
    assert!(matches!(
        host.deliver(endpoint(2), out.get(..len).expect("written")),
        Err(LobbyRefusal::WrongRole {
            kind: wire::Kind::Start
        })
    ));
}

#[test]
fn a_start_from_a_bystander_or_a_non_host_seat_is_refused() {
    let (_, mut joiner) = seated_pair();
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = forged_start(&mut out, seat(0), session());
    assert!(matches!(
        joiner.deliver(endpoint(99), out.get(..len).expect("written")),
        Err(LobbyRefusal::NotFromHost)
    ));

    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = forged_start(&mut out, seat(1), session());
    assert!(matches!(
        joiner.deliver(endpoint(HOST_AT), out.get(..len).expect("written")),
        Err(LobbyRefusal::HostNotSeatZero { .. })
    ));
    assert!(joiner.agreed().is_none());
}

#[test]
fn a_start_for_another_session_is_refused() {
    let (_, mut joiner) = seated_pair();
    let other = NonZeroU64::new(0x9999).expect("nonzero");
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = forged_start(&mut out, seat(0), other);
    assert!(matches!(
        joiner.deliver(endpoint(HOST_AT), out.get(..len).expect("written")),
        Err(LobbyRefusal::WrongSession { .. })
    ));
    assert!(joiner.agreed().is_none());
}

#[test]
fn a_datagram_from_no_address_is_refused() {
    // Seat zero's own slot holds these exact bytes, so admitting them
    // would read as the host having already seated itself — and nothing
    // could be sent back to the seat it took either way.
    let mut host = Lobby::host(host_setup());
    let mut joiner = Lobby::join(join_setup());
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    let join = joiner
        .next_outbound(&mut buffer)
        .expect("a join")
        .bytes()
        .to_vec();
    assert!(matches!(
        host.deliver(UNKNOWN_ENDPOINT, &join),
        Err(LobbyRefusal::UnknownEndpoint)
    ));
    assert_eq!(host.state(), LobbyState::Hosting { seated: 1 });
}

// ---- the wall between the lobby and the simulation ----

/// The structural claim, from the session's side. A lobby datagram that
/// reaches a session is a routing defect, and the session says so by name
/// rather than falling through — which is what makes the wall visible in a
/// log instead of only in a design note.
#[test]
fn a_session_refuses_all_three_lobby_kinds_by_name() {
    use renew_net::{Delivery, Refusal, Session, SessionParams};

    let params = SessionParams {
        peer_count: 2,
        local: seat(0),
        input_bytes: 1,
        input_delay: 1,
        digest_period: 4,
        seed: 5,
        content: 1,
        rules: 1,
        session: session(),
    }
    .validate()
    .expect("a runnable session");
    let mut live = Session::new(params);

    let from = seat(1);
    let addressing = wire::Addressing {
        sender: from,
        session: session(),
    };
    let endpoints = [0u8; 18 * 2];

    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    // Addressed with the live session's own identifier rather than
    // [`UNSEATED_SESSION`], which no lobby would ever write — because the
    // session checks whose session a datagram names *before* it looks at
    // the kind, and a Join carrying the joining sentinel is refused as
    // somebody else's session and never reaches the arm under test.
    let len = wire::write_join(
        &mut out,
        addressing,
        &wire::JoinBody {
            content: 1,
            rules: 1,
        },
    );
    assert!(matches!(
        live.deliver(from, out.get(..len).expect("written")),
        Delivery::Refused(Refusal::NotSessionTraffic {
            kind: wire::Kind::Join
        })
    ));

    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_roster(
        &mut out,
        addressing,
        &wire::RosterBody {
            seat: 0,
            peer_count: 2,
            input_bytes: 1,
            input_delay: 1,
            digest_period: 4,
            seed: 5,
            content: 1,
            rules: 1,
            endpoints: &endpoints,
        },
    )
    .expect("a well-formed roster");
    assert!(matches!(
        live.deliver(from, out.get(..len).expect("written")),
        Delivery::Refused(Refusal::NotSessionTraffic {
            kind: wire::Kind::Roster
        })
    ));

    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_start(
        &mut out,
        addressing,
        &wire::StartBody {
            agreement_digest: params.agreement_digest(),
        },
    );
    assert!(matches!(
        live.deliver(from, out.get(..len).expect("written")),
        Delivery::Refused(Refusal::NotSessionTraffic {
            kind: wire::Kind::Start
        })
    ));

    // And none of it moved the session: the wall is not merely a message.
    assert_eq!(live.pending_tick(), 0);
    assert!(!live.is_playing());
}

#[test]
fn a_peer_that_never_hears_a_roster_keeps_asking_and_takes_no_second_seat() {
    // The failure this guards is not loud: the host seats a peer, every
    // roster to it is lost, the host says go, and the session then holds a
    // seat that will never submit an input. The lobby cannot fix that — a
    // timeout needs a clock it may not read — but it must not make it
    // worse by handing the same machine a second seat every time it asks
    // again, which would quietly shrink everyone else's roster to nothing.
    let mut world = World::new(&[2, 3]);
    world.roster_deaf.push(endpoint(2));
    world.pumps(6);
    assert_eq!(
        world.host.state(),
        LobbyState::Hosting { seated: 3 },
        "one deaf peer, one seat"
    );

    // And the deaf peer is still asking rather than having given up
    // silently, so a driver watching its own outbox can see the state it
    // is in.
    let asking = world
        .drain()
        .into_iter()
        .any(|packet| packet.from == endpoint(2));
    assert!(asking, "a joiner with no roster has nothing to do but ask");
}

#[test]
fn the_start_run_carries_the_roster_beside_it() {
    // A joiner that lost every roster before `start` still has eight
    // pumps to be seated and start, because the run keeps sending both.
    // Without this a lost roster is unrecoverable the instant the host
    // says go, which is the moment it is most likely to matter.
    let mut world = World::new(&[2]);
    world.roster_deaf.push(endpoint(2));
    world.pumps(3);
    assert_eq!(world.joiners[0].1.state(), LobbyState::Joining);

    world.host.start().expect("start");
    world.roster_deaf.clear();
    world.pumps(3);
    assert!(
        world.joiners[0].1.agreed().is_some(),
        "the roster rides with the start run so a late peer can still make it"
    );
}

#[test]
fn a_roster_offering_seat_zero_is_refused() {
    // Seat zero is the host's. A roster handing it away puts two machines
    // on one seat, each submitting as sender zero and each refusing the
    // other's inputs, so no tick ever confirms and a third peer sees two
    // contradicting streams from one seat. Nothing downstream recovers
    // from that, which is why it is refused where it can still be seen.
    let mut joiner = Lobby::join(join_setup());
    let endpoints = [0u8; 18 * 2];
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_roster(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: session(),
        },
        &wire::RosterBody {
            seat: 0,
            peer_count: 2,
            input_bytes: 2,
            input_delay: 2,
            digest_period: 8,
            seed: 0xC0FF_EE00_1234_5678,
            content: 0xAAAA_BBBB,
            rules: 0x1111_2222,
            endpoints: &endpoints,
        },
    )
    .expect("the wire permits it; the lobby must not");
    assert!(matches!(
        joiner.deliver(endpoint(HOST_AT), out.get(..len).expect("written")),
        Err(LobbyRefusal::SeatZeroOffered)
    ));
    assert_eq!(joiner.state(), LobbyState::Joining);
}

#[test]
fn a_roster_that_went_backwards_is_refused() {
    // Ordinary reordering, not an attack: the host's three-seat roster
    // overtakes its two-seat one. Taking the older one would move this
    // peer back onto a fingerprint the others have left behind, and it
    // would then refuse the go-ahead and stall everybody at tick zero.
    let (_, mut joiner) = seated_pair();
    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let three = [0u8; 18 * 3];
    let len = wire::write_roster(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: session(),
        },
        &wire::RosterBody {
            seat: 1,
            peer_count: 3,
            input_bytes: 2,
            input_delay: 2,
            digest_period: 8,
            seed: 0xC0FF_EE00_1234_5678,
            content: 0xAAAA_BBBB,
            rules: 0x1111_2222,
            endpoints: &three,
        },
    )
    .expect("a three-seat roster");
    joiner
        .deliver(endpoint(HOST_AT), out.get(..len).expect("written"))
        .expect("the roster grew, which is normal");

    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let two = [0u8; 18 * 2];
    let len = wire::write_roster(
        &mut out,
        wire::Addressing {
            sender: seat(0),
            session: session(),
        },
        &wire::RosterBody {
            seat: 1,
            peer_count: 2,
            input_bytes: 2,
            input_delay: 2,
            digest_period: 8,
            seed: 0xC0FF_EE00_1234_5678,
            content: 0xAAAA_BBBB,
            rules: 0x1111_2222,
            endpoints: &two,
        },
    )
    .expect("a two-seat roster");
    assert!(matches!(
        joiner.deliver(endpoint(HOST_AT), out.get(..len).expect("written")),
        Err(LobbyRefusal::RosterWentBackwards {
            held: 3,
            offered: 2
        })
    ));
}

#[test]
fn a_joiners_own_seat_answers_unknown_like_the_hosts_does() {
    // The contract `Agreed::endpoint` states, which held on the host and
    // failed on every joiner: the roster carries the recipient's own
    // endpoint, because the host observed it and sends the whole table
    // back. A driver that skips itself by this sentinel would pass its
    // tests as a host and loop every datagram back to itself as a joiner.
    let mut world = World::new(&[2, 3]);
    world.pumps(3);
    world.host.start().expect("start");
    world.pumps(2);

    for (index, (_, joiner)) in world.joiners.iter().enumerate() {
        let agreed = joiner.agreed().expect("agreed");
        let own = agreed.params().local();
        assert_eq!(
            agreed.endpoint(own),
            Some(UNKNOWN_ENDPOINT),
            "joiner {index} was handed its own address for its own seat"
        );
    }
}

#[test]
fn the_start_run_is_as_long_as_it_says_it_is() {
    // `the_start_run_is_finite` pins the ceiling and nothing else, so the
    // run could be cut from eight pumps to one and stay green — throwing
    // away seven eighths of the only loss protection the go-ahead has,
    // silently. This pins the floor.
    let mut world = World::new(&[2]);
    world.pumps(2);
    world.host.start().expect("start");

    let mut starts = 0usize;
    let mut buffer = [0u8; MAX_DATAGRAM_BYTES];
    for _ in 0..(usize::from(START_REPEATS) + 4) {
        while let Some(out) = world.host.next_outbound(&mut buffer) {
            if wire::read(out.bytes()).is_ok_and(|d| d.header.kind == wire::Kind::Start) {
                starts = starts.saturating_add(1);
            }
        }
    }
    assert_eq!(
        starts,
        usize::from(START_REPEATS),
        "the run must be exactly as long as the constant promises"
    );
}
