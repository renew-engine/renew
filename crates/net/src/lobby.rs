//! Getting from "a player typed an address" to "everybody agrees who is
//! playing, and on what".
//!
//! # What this is for
//!
//! A [`Session`](crate::Session) begins already knowing everything: how
//! many seats, which one is ours, the seed, the content, the rules. Every
//! peer must arrive at that same answer independently, before tick zero
//! exists, or the first tick they run is already a fork. This module is
//! how they arrive at it.
//!
//! # Host is a deployment role, never an authority
//!
//! One machine binds a port and the others are told its address. That
//! machine takes seat zero, hands out the remaining seats in arrival
//! order, and decides when play begins. It has no other power, and the
//! separation is structural rather than promised: **the lobby is finished
//! before the session is constructed**, so there is no live channel
//! through which a host could influence a tick. The protocol underneath
//! stays a mesh — once play starts, every peer talks to every peer, and
//! seat zero is just a seat.
//!
//! What the host decides is *who is in the game*. What no one decides is
//! *what happens in it*: that is the confirmed input stream, and nothing
//! here can reach it.
//!
//! # Endpoints are opaque, and never self-reported
//!
//! A mesh needs every peer to know every other peer's address, and this
//! crate is forbidden from knowing what an address is — it declares
//! `simulation = true`, which denies it any path to the crate that owns
//! the socket. So endpoints travel as [`Endpoint`]: eighteen bytes this
//! module copies and never reads. The driver encodes them through the
//! platform seam, whose `peer_tag` produces exactly this shape.
//!
//! **Every endpoint is learned from the transport, never from a body.**
//! A peer's own idea of its address is the one address that will not
//! reach it through NAT, and a host that believed a body's claim would
//! send rosters to whatever address a one-packet sender named — a
//! reflector, aimed for free. So a joiner's address is the source its
//! `Join` arrived from, and the host's address is the source its `Roster`
//! arrived from. Seat zero's slot in a roster is therefore all zeroes: a
//! host cannot see itself from outside, and each joiner fills the slot in
//! with where the roster actually came from.
//!
//! # Delivery
//!
//! The same trade as everywhere else in this crate: **redundancy rather
//! than retransmission.** A joiner repeats its `Join` every pump until a
//! roster answers; a host repeats the roster to every seated peer every
//! pump, and its `Start` for a fixed run of pumps. Nothing is
//! acknowledged, nothing is retried on request, and a peer that loses
//! every datagram of a run does not play. A lobby is a handful of
//! datagrams a second between at most eight machines, so paying for
//! reliability with repetition costs nothing worth counting and needs no
//! round trip.
//!
//! # What this deliberately does not do
//!
//! No discovery — a player types an address and nothing broadcasts on a
//! subnet. No matchmaking. No mid-game join, and no rejoin after a drop:
//! the session ends for everyone and a fresh lobby is how it restarts.
//! Each is a feature with its own cost, not an oversight.

use core::num::NonZeroU64;

use crate::wire::{self, ENDPOINT_BYTES};
use crate::{MAX_DATAGRAM_BYTES, MAX_PEERS, MIN_PEERS, ParamsError, PeerId, SessionParams};

const PEERS: usize = MAX_PEERS as usize;
const TABLE_BYTES: usize = PEERS * ENDPOINT_BYTES;

/// One peer's address, as this crate carries it: eighteen opaque bytes.
///
/// Sixteen of address and two of port, as the platform seam's `peer_tag`
/// produces them. Nothing in this crate reads a byte of one.
pub type Endpoint = [u8; ENDPOINT_BYTES];

/// The endpoint that means "not known here": all zeroes.
///
/// A roster's seat-zero slot always holds this, because a host cannot see
/// its own address from outside and must not guess. A joiner replaces it
/// with the source the roster arrived from.
pub const UNKNOWN_ENDPOINT: Endpoint = [0u8; ENDPOINT_BYTES];

/// The session identifier every `Join` carries, and the only one it may.
///
/// A joiner has no session yet — learning which one it is joining is what
/// the roster is *for* — but the header has a session field and that
/// field must hold something. It holds this. **Pinned rather than
/// ignored:** this crate admits exactly one byte string per fact, and a
/// field a sender could fill with any of 2⁶⁴−1 values while meaning
/// nothing would be that many spellings of "no session yet".
pub const UNSEATED_SESSION: NonZeroU64 = NonZeroU64::MIN;

/// The seat every `Join` is addressed from, and the only one it may be.
///
/// Same reasoning as [`UNSEATED_SESSION`], and the value is forced: seat
/// zero belongs to the host, so a joiner naming it as its own sender is
/// the one seat number that cannot be read as a claim to a seat.
const UNSEATED_SEAT: u8 = 0;

/// How many pumps the host repeats its `Start` for before considering the
/// lobby over.
///
/// The whole of the "did everyone hear go?" story. A joiner that loses
/// all of these does not play, and finds out by never seeing a tick — the
/// driver's timeout is what turns that into a message a player can read,
/// because a timeout needs a clock and this crate may not have one.
pub const START_REPEATS: u8 = 8;

/// Where a lobby is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LobbyState {
    /// Hosting, with this many seats taken including seat zero.
    Hosting { seated: u8 },
    /// Joining: repeating a `Join` at the host, no seat yet.
    Joining,
    /// The host answered. The roster may still grow, so the parameters
    /// are not final until `Start`.
    Seated { seat: PeerId },
    /// Agreed. [`Lobby::agreed`] now has the answer.
    Started,
}

/// What a lobby produces, and the only reason it exists.
///
/// Everything a driver needs to construct a [`Session`](crate::Session)
/// and route its datagrams: the validated parameters every peer agrees
/// on, and where each seat is.
#[derive(Clone, Copy, Debug)]
pub struct Agreed {
    params: crate::ValidParams,
    endpoints: [Endpoint; PEERS],
}

impl Agreed {
    /// The parameters, validated. Construct the session with these.
    #[must_use]
    pub const fn params(&self) -> &crate::ValidParams {
        &self.params
    }

    /// Where one seat is, or [`None`] past the roster.
    ///
    /// This machine's own seat answers [`UNKNOWN_ENDPOINT`]: nothing is
    /// ever sent to it, and a lobby has no way to learn how it looks from
    /// outside.
    #[must_use]
    pub fn endpoint(&self, seat: PeerId) -> Option<Endpoint> {
        if seat.index() >= self.params.peer_count() {
            return None;
        }
        self.endpoints.get(usize::from(seat.index())).copied()
    }
}

/// What a host writes down before opening its door.
///
/// The host owns every one of these numbers, which is what makes the
/// agreement possible at all: a value nobody chooses is a value peers can
/// disagree about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostSetup {
    /// Distinguishes this session from the last one on the same
    /// addresses. A stale datagram naming a different session is refused
    /// rather than mistaken for a live one.
    pub session: NonZeroU64,
    pub seed: u64,
    /// What content everyone must be running. A joiner that disagrees is
    /// refused at the door, with the differing half named.
    pub content: u64,
    pub rules: u64,
    pub input_bytes: u8,
    pub input_delay: u8,
    pub digest_period: u8,
}

/// What a joiner writes down: where the host is, and what it is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinSetup {
    /// The host's endpoint, as the player typed it and the driver
    /// encoded it.
    pub host: Endpoint,
    pub content: u64,
    pub rules: u64,
}

/// Why a lobby datagram was dropped.
///
/// **None of these is fatal.** A lobby that ended on a bad datagram would
/// hand anyone who can send one packet the power to stop a game from
/// starting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LobbyRefusal {
    /// Malformed bytes, by the codec's rules.
    Malformed(wire::WireError),
    /// Session or chat traffic, misrouted here by the driver.
    NotLobbyTraffic { kind: wire::Kind },
    /// A `Join` at a joiner, or a roster at a host. Two hosts on one
    /// address, or a driver crossing its wires.
    WrongRole { kind: wire::Kind },
    /// A `Join` whose session field was not [`UNSEATED_SESSION`].
    NotUnseatedSession { saw: u64 },
    /// A `Join` addressed from a seat. A joiner has none to name.
    NotUnseatedSeat { saw: PeerId },
    /// A roster or a `Start` from somewhere other than the host this
    /// joiner was pointed at. The check that keeps a bystander from
    /// seating this peer in a game it never chose.
    NotFromHost,
    /// A roster or a `Start` not addressed from seat zero.
    HostNotSeatZero { saw: PeerId },
    /// A roster for a different session than the one already seated in.
    WrongSession { saw: u64, holding: u64 },
    /// The joiner is running different content than the host.
    ContentMismatch { ours: u64, theirs: u64 },
    /// The joiner is running different rules than the host.
    RulesMismatch { ours: u64, theirs: u64 },
    /// Every seat is taken.
    Full { ceiling: u8 },
    /// A roster whose numbers do not describe a session this crate can
    /// run. The host built it, so this is the host being wrong.
    Parameters(ParamsError),
    /// A roster that moved this peer to a different seat. Seats are
    /// assigned once; a host that reassigns is a host to walk away from.
    SeatMoved { held: PeerId, offered: PeerId },
    /// A `Start` before any roster: there is nothing to start yet.
    NotSeatedYet,
    /// A `Start` whose agreement fingerprint is not the one this peer
    /// would play under — almost always a roster this peer never
    /// received, so its idea of who is playing is one seat short.
    ///
    /// **Refusing here is the whole point of the fingerprint.** Starting
    /// anyway would be a fork agreed to in advance.
    AgreementMismatch { ours: u64, theirs: u64 },
    /// Traffic arriving after the lobby is over.
    AlreadyStarted,
    /// A datagram whose source is [`UNKNOWN_ENDPOINT`].
    ///
    /// Nothing can be sent back to "no address", so seating it would fill
    /// a seat that no roster could ever reach — and because seat zero's
    /// own slot holds exactly these bytes, admitting it would also read as
    /// the host having already seated itself.
    UnknownEndpoint,
}

/// Why a lobby could not be opened or started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LobbyError {
    /// `start` on a joiner. Only the host says go.
    NotTheHost,
    /// Fewer than [`MIN_PEERS`] seats taken. One peer is not multiplayer.
    NotEnoughPeers { seated: u8, floor: u8 },
    /// Already started.
    AlreadyStarted,
    /// The host's own numbers do not describe a runnable session.
    Parameters(ParamsError),
}

/// The agreement handshake, before a session exists.
///
/// Held by the driver, beside the session and never inside it — a lobby
/// holds addresses and a session must not.
pub struct Lobby {
    role: Role,
    state: LobbyState,
    /// The seat-indexed endpoint table, flat, so it can be written to the
    /// wire without a copy.
    table: [u8; TABLE_BYTES],
    seated: u8,
    agreed: Option<Agreed>,
    /// Where the emitter is in its walk over seats, per pump.
    emit_seat: u8,
    emit_start: bool,
    start_repeats: u8,
}

/// The two halves, and what only one of them holds.
enum Role {
    Host {
        setup: HostSetup,
    },
    Joiner {
        setup: JoinSetup,
        /// The most recent roster's contents, re-validated on arrival.
        /// [`None`] until the first one lands.
        held: Option<Held>,
    },
}

/// A joiner's latest roster, kept because the roster grows as peers
/// arrive and only the last one before `Start` is the truth.
#[derive(Clone, Copy)]
struct Held {
    seat: PeerId,
    params: crate::ValidParams,
}

impl Lobby {
    /// Open a lobby as the host: seat zero, taken.
    #[must_use]
    pub fn host(setup: HostSetup) -> Self {
        Self {
            role: Role::Host { setup },
            state: LobbyState::Hosting { seated: 1 },
            table: [0u8; TABLE_BYTES],
            seated: 1,
            agreed: None,
            emit_seat: 1,
            emit_start: false,
            start_repeats: 0,
        }
    }

    /// Open a lobby as a joiner, pointed at a host.
    #[must_use]
    pub fn join(setup: JoinSetup) -> Self {
        Self {
            role: Role::Joiner { setup, held: None },
            state: LobbyState::Joining,
            table: [0u8; TABLE_BYTES],
            seated: 0,
            agreed: None,
            emit_seat: 0,
            emit_start: false,
            start_repeats: 0,
        }
    }

    #[must_use]
    pub const fn state(&self) -> LobbyState {
        self.state
    }

    /// What was agreed, once the lobby is over.
    ///
    /// [`None`] until then, which is the type saying that a session
    /// cannot be constructed early.
    #[must_use]
    pub const fn agreed(&self) -> Option<&Agreed> {
        self.agreed.as_ref()
    }

    /// Say go. Host only.
    ///
    /// # Errors
    ///
    /// [`LobbyError::NotTheHost`] on a joiner,
    /// [`LobbyError::NotEnoughPeers`] below [`MIN_PEERS`],
    /// [`LobbyError::AlreadyStarted`] on a second call, and
    /// [`LobbyError::Parameters`] if the host's own numbers do not
    /// describe a runnable session — which is caught here, at the last
    /// moment before anyone commits to them, rather than at tick zero.
    pub fn start(&mut self) -> Result<(), LobbyError> {
        let Role::Host { setup } = &self.role else {
            return Err(LobbyError::NotTheHost);
        };
        if self.state == LobbyState::Started {
            return Err(LobbyError::AlreadyStarted);
        }
        if self.seated < MIN_PEERS {
            return Err(LobbyError::NotEnoughPeers {
                seated: self.seated,
                floor: MIN_PEERS,
            });
        }
        let seat = PeerId::new(0).ok_or(LobbyError::NotTheHost)?;
        let params = params_for(setup, self.seated, seat).map_err(LobbyError::Parameters)?;
        self.agreed = Some(Agreed {
            params,
            endpoints: self.table_snapshot(),
        });
        self.state = LobbyState::Started;
        self.emit_seat = 1;
        self.emit_start = false;
        self.start_repeats = START_REPEATS;
        Ok(())
    }

    /// Hand the lobby a datagram the driver has decided is lobby traffic,
    /// with the endpoint it came from.
    ///
    /// **`source` is the transport's answer, not the sender's.** Passing
    /// anything a datagram said about itself defeats the one check
    /// standing between this lobby and being used as a reflector.
    ///
    /// # Errors
    ///
    /// [`LobbyRefusal`] naming why it was dropped. Never fatal.
    pub fn deliver(&mut self, source: Endpoint, bytes: &[u8]) -> Result<(), LobbyRefusal> {
        if self.state == LobbyState::Started {
            return Err(LobbyRefusal::AlreadyStarted);
        }
        if source == UNKNOWN_ENDPOINT {
            return Err(LobbyRefusal::UnknownEndpoint);
        }
        let datagram = wire::read(bytes).map_err(LobbyRefusal::Malformed)?;
        match datagram.body {
            wire::Body::Join(body) => self.absorb_join(source, datagram.header, body),
            wire::Body::Roster(body) => self.absorb_roster(source, datagram.header, &body),
            wire::Body::Start(body) => self.absorb_start(source, datagram.header, body),
            wire::Body::Hello(_)
            | wire::Body::Inputs(_)
            | wire::Body::Digest(_)
            | wire::Body::Bye(_)
            | wire::Body::Chat(_) => Err(LobbyRefusal::NotLobbyTraffic {
                kind: datagram.header.kind,
            }),
        }
    }

    fn absorb_join(
        &mut self,
        source: Endpoint,
        header: wire::Header,
        body: wire::JoinBody,
    ) -> Result<(), LobbyRefusal> {
        let Role::Host { setup } = &self.role else {
            return Err(LobbyRefusal::WrongRole {
                kind: wire::Kind::Join,
            });
        };
        if header.session != UNSEATED_SESSION {
            return Err(LobbyRefusal::NotUnseatedSession {
                saw: header.session.get(),
            });
        }
        if header.sender.index() != UNSEATED_SEAT {
            return Err(LobbyRefusal::NotUnseatedSeat { saw: header.sender });
        }
        if body.content != setup.content {
            return Err(LobbyRefusal::ContentMismatch {
                ours: setup.content,
                theirs: body.content,
            });
        }
        if body.rules != setup.rules {
            return Err(LobbyRefusal::RulesMismatch {
                ours: setup.rules,
                theirs: body.rules,
            });
        }

        // Already seated? Then this is the redundancy working, not a
        // second player: a joiner repeats until a roster answers, so
        // every seated peer sends several of these. Idempotent by
        // endpoint, which is also what makes a joiner that restarted on
        // the same port get its own seat back rather than a second one.
        if self.seat_of(source).is_some() {
            return Ok(());
        }
        if self.seated >= MAX_PEERS {
            return Err(LobbyRefusal::Full { ceiling: MAX_PEERS });
        }
        let at = usize::from(self.seated)
            .checked_mul(ENDPOINT_BYTES)
            .ok_or(LobbyRefusal::Full { ceiling: MAX_PEERS })?;
        if let Some(slot) = self
            .table
            .get_mut(at..)
            .and_then(|t| t.get_mut(..ENDPOINT_BYTES))
        {
            slot.copy_from_slice(&source);
        }
        self.seated = self.seated.saturating_add(1);
        self.state = LobbyState::Hosting {
            seated: self.seated,
        };
        Ok(())
    }

    fn absorb_roster(
        &mut self,
        source: Endpoint,
        header: wire::Header,
        body: &wire::RosterBody<'_>,
    ) -> Result<(), LobbyRefusal> {
        let Role::Joiner { setup, held } = &mut self.role else {
            return Err(LobbyRefusal::WrongRole {
                kind: wire::Kind::Roster,
            });
        };
        let setup = *setup;
        let holding = *held;
        if source != setup.host {
            return Err(LobbyRefusal::NotFromHost);
        }
        if header.sender.index() != 0 {
            return Err(LobbyRefusal::HostNotSeatZero { saw: header.sender });
        }
        if let Some(current) = holding.filter(|c| c.params.session() != header.session) {
            return Err(LobbyRefusal::WrongSession {
                saw: header.session.get(),
                holding: current.params.session().get(),
            });
        }
        if body.content != setup.content {
            return Err(LobbyRefusal::ContentMismatch {
                ours: setup.content,
                theirs: body.content,
            });
        }
        if body.rules != setup.rules {
            return Err(LobbyRefusal::RulesMismatch {
                ours: setup.rules,
                theirs: body.rules,
            });
        }
        let seat = PeerId::new(body.seat).ok_or(LobbyRefusal::Parameters(
            ParamsError::LocalNotInRoster {
                local: body.seat,
                peer_count: body.peer_count,
            },
        ))?;
        if let Some(current) = holding.filter(|c| c.seat != seat) {
            return Err(LobbyRefusal::SeatMoved {
                held: current.seat,
                offered: seat,
            });
        }
        let params = SessionParams {
            peer_count: body.peer_count,
            local: seat,
            input_bytes: body.input_bytes,
            input_delay: body.input_delay,
            digest_period: body.digest_period,
            seed: body.seed,
            content: body.content,
            rules: body.rules,
            session: header.session,
        }
        .validate()
        .map_err(LobbyRefusal::Parameters)?;

        // The table is rebuilt from every roster rather than merged into:
        // a later roster is strictly better informed than an earlier one,
        // and merging would let a seat that vanished from the roster
        // survive in this peer's routing table.
        self.table = [0u8; TABLE_BYTES];
        // Chunked rather than indexed. The reader has already proven this
        // roster's endpoint region is exactly `peer_count × ENDPOINT_BYTES`
        // wide, so a zip over equal-width chunks copies every seat and can
        // name none that is not there — where indexing would need two
        // bounds checks whose failure arms no test could reach and no
        // reader could evaluate.
        for (slot, endpoint) in self
            .table
            .chunks_exact_mut(ENDPOINT_BYTES)
            .zip(body.endpoints.chunks_exact(ENDPOINT_BYTES))
        {
            slot.copy_from_slice(endpoint);
        }
        // Seat zero's slot is always zeroes on the wire — a host cannot
        // see itself from outside. Where the roster actually came from is
        // the answer, and it is the one address this peer has already
        // proven it can reach.
        if let Some(slot) = self.table.get_mut(..ENDPOINT_BYTES) {
            slot.copy_from_slice(&setup.host);
        }

        *held = Some(Held { seat, params });
        self.seated = body.peer_count;
        self.state = LobbyState::Seated { seat };
        Ok(())
    }

    fn absorb_start(
        &mut self,
        source: Endpoint,
        header: wire::Header,
        body: wire::StartBody,
    ) -> Result<(), LobbyRefusal> {
        let Role::Joiner { setup, held } = &self.role else {
            return Err(LobbyRefusal::WrongRole {
                kind: wire::Kind::Start,
            });
        };
        if source != setup.host {
            return Err(LobbyRefusal::NotFromHost);
        }
        if header.sender.index() != 0 {
            return Err(LobbyRefusal::HostNotSeatZero { saw: header.sender });
        }
        let Some(current) = held else {
            return Err(LobbyRefusal::NotSeatedYet);
        };
        if current.params.session() != header.session {
            return Err(LobbyRefusal::WrongSession {
                saw: header.session.get(),
                holding: current.params.session().get(),
            });
        }
        let ours = current.params.agreement_digest();
        if ours != body.agreement_digest {
            return Err(LobbyRefusal::AgreementMismatch {
                ours,
                theirs: body.agreement_digest,
            });
        }
        self.agreed = Some(Agreed {
            params: current.params,
            endpoints: self.table_snapshot(),
        });
        self.state = LobbyState::Started;
        Ok(())
    }

    /// The next lobby datagram to send, written into `out`.
    ///
    /// Call until it returns [`None`], once per pump. Like the session's
    /// outbox, **nothing received causes anything to be sent**: every
    /// datagram here is rendered from state this peer already holds, so
    /// there is no input that makes this lobby emit more than its own
    /// steady rate. That is what stops it being an amplifier.
    pub fn next_outbound<'b>(
        &mut self,
        out: &'b mut [u8; MAX_DATAGRAM_BYTES],
    ) -> Option<Outbound<'b>> {
        match &self.role {
            Role::Host { setup } => self.host_outbound(*setup, out),
            Role::Joiner { setup, held } => {
                // A seated joiner has what it came for and goes quiet;
                // the host has no acknowledgement to wait for, so it
                // keeps sending until it says go.
                if held.is_some() {
                    return None;
                }
                let setup = *setup;
                if self.emit_seat > 0 {
                    self.emit_seat = 0;
                    return None;
                }
                self.emit_seat = 1;
                let len = wire::write_join(
                    out,
                    wire::Addressing {
                        sender: PeerId::new(UNSEATED_SEAT)?,
                        session: UNSEATED_SESSION,
                    },
                    &wire::JoinBody {
                        content: setup.content,
                        rules: setup.rules,
                    },
                );
                Some(Outbound {
                    to: setup.host,
                    bytes: out.get(..len)?,
                })
            }
        }
    }

    fn host_outbound<'b>(
        &mut self,
        setup: HostSetup,
        out: &'b mut [u8; MAX_DATAGRAM_BYTES],
    ) -> Option<Outbound<'b>> {
        if self.state == LobbyState::Started && self.start_repeats == 0 {
            return None;
        }
        if self.emit_seat >= self.seated {
            // One pump spent. A `Start` run is the only thing that
            // ever ends: the roster repeats for as long as the lobby
            // is open, because a host has no way to learn that a
            // joiner heard it.
            self.emit_seat = 1;
            self.emit_start = false;
            if self.state == LobbyState::Started {
                self.start_repeats = self.start_repeats.saturating_sub(1);
            }
            return None;
        }
        let seat = self.emit_seat;
        // Every seated peer gets the roster, and during the start run
        // gets a `Start` behind it — the roster stays in the run so a
        // joiner that lost every earlier one can still be seated by
        // the last pump and start on the datagram after it.
        let send_start = self.state == LobbyState::Started && self.emit_start;
        if self.state == LobbyState::Started && !self.emit_start {
            self.emit_start = true;
        } else {
            self.emit_start = false;
            self.emit_seat = self.emit_seat.saturating_add(1);
        }

        let to = self.endpoint_at(seat);
        let sender = PeerId::new(0)?;
        let addressing = wire::Addressing {
            sender,
            session: setup.session,
        };
        let len = if send_start {
            let digest = self.agreed.as_ref()?.params().agreement_digest();
            wire::write_start(
                out,
                addressing,
                &wire::StartBody {
                    agreement_digest: digest,
                },
            )
        } else {
            let endpoints = self.roster_endpoints()?;
            wire::write_roster(
                out,
                addressing,
                &wire::RosterBody {
                    seat,
                    peer_count: self.seated,
                    input_bytes: setup.input_bytes,
                    input_delay: setup.input_delay,
                    digest_period: setup.digest_period,
                    seed: setup.seed,
                    content: setup.content,
                    rules: setup.rules,
                    endpoints,
                },
            )
            .ok()?
        };
        Some(Outbound {
            to,
            bytes: out.get(..len)?,
        })
    }

    /// The flat endpoint table for the seats currently taken.
    ///
    /// Seat zero's eighteen bytes are zero and stay zero: see
    /// [`UNKNOWN_ENDPOINT`].
    fn roster_endpoints(&self) -> Option<&[u8]> {
        let width = usize::from(self.seated).checked_mul(ENDPOINT_BYTES)?;
        self.table.get(..width)
    }

    /// One seat's slot. **Total**: a seat past the table reads as
    /// [`UNKNOWN_ENDPOINT`], which is what an unfilled slot holds anyway,
    /// so there is no second answer for a caller to handle and no arm for
    /// a test to be unable to reach.
    fn endpoint_at(&self, seat: u8) -> Endpoint {
        let mut out = UNKNOWN_ENDPOINT;
        if let Some(chunk) = self
            .table
            .chunks_exact(ENDPOINT_BYTES)
            .nth(usize::from(seat))
        {
            out.copy_from_slice(chunk);
        }
        out
    }

    /// Which seat an endpoint already holds, if any.
    ///
    /// [`UNKNOWN_ENDPOINT`] is never matched here even though seat zero's
    /// slot holds it: "no address" is not an address, and treating it as
    /// one would report the host's own seat to a caller asking about a
    /// peer. [`Lobby::deliver`] refuses it before this is ever asked, and
    /// this is the second half of the same statement.
    fn seat_of(&self, endpoint: Endpoint) -> Option<u8> {
        if endpoint == UNKNOWN_ENDPOINT {
            return None;
        }
        (0..self.seated).find(|&seat| self.endpoint_at(seat) == endpoint)
    }

    fn table_snapshot(&self) -> [Endpoint; PEERS] {
        let mut out = [UNKNOWN_ENDPOINT; PEERS];
        for (slot, held) in out.iter_mut().zip(self.table.chunks_exact(ENDPOINT_BYTES)) {
            slot.copy_from_slice(held);
        }
        out
    }
}

/// The host's parameters for a roster of `seated`, from its own seat.
fn params_for(
    setup: &HostSetup,
    seated: u8,
    local: PeerId,
) -> Result<crate::ValidParams, ParamsError> {
    SessionParams {
        peer_count: seated,
        local,
        input_bytes: setup.input_bytes,
        input_delay: setup.input_delay,
        digest_period: setup.digest_period,
        seed: setup.seed,
        content: setup.content,
        rules: setup.rules,
        session: setup.session,
    }
    .validate()
}

/// A lobby datagram to send, and where to.
#[derive(Clone, Copy, Debug)]
pub struct Outbound<'a> {
    to: Endpoint,
    bytes: &'a [u8],
}

impl<'a> Outbound<'a> {
    /// Where to send it. Opaque here; the driver knows what it means.
    #[must_use]
    pub const fn to(&self) -> Endpoint {
        self.to
    }

    #[must_use]
    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}
