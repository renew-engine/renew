//! Lockstep as a pure state machine.
//!
//! Local inputs and received datagram bytes in; confirmed per-tick input
//! sets and outbound datagram bytes out. **No socket, no clock, no
//! threads, and no allocation after construction** — the session cannot
//! reach any of them, and its storage is fixed inline arrays sized by the
//! crate's ceilings.
//!
//! # The one rule everything else serves
//!
//! **A tick runs only when every peer's input for it has arrived.** There
//! is no timeout that advances a tick, no "assume the last input", and no
//! way for arrival order, duplication or loss to reach a confirmed value.
//! Those are not omissions to be filled in later: each of them is a
//! divergence source dressed as a feature, and a lockstep session that
//! has one is a lockstep session that forks.
//!
//! # What this deliberately does not decide
//!
//! **When to give up on a silent peer.** Waiting is reported —
//! [`Advance::Stalled`] carries who is missing and how many pumps it has
//! been — and the *driver* decides, because the answer depends on a wall
//! clock and this crate may not read one. A pump budget living in here
//! would be wall-derived state inside the simulation zone with the power
//! to end the run.

use renew_frame::StateHash;

use crate::desync::DesyncReport;
use crate::params::ValidParams;
use crate::wire::{self, Body, WireError};
use crate::{
    DIGEST_HISTORY, INPUT_REDUNDANCY, INPUT_WINDOW, MAX_DATAGRAM_BYTES, MAX_INPUT_BYTES, MAX_PEERS,
    PeerId, PeerSet,
};

const PEERS: usize = MAX_PEERS as usize;
const WINDOW: usize = INPUT_WINDOW as usize;
const HISTORY: usize = DIGEST_HISTORY as usize;
const WIDTH: usize = MAX_INPUT_BYTES as usize;
const WINDOW_MASK: u64 = (INPUT_WINDOW as u64) - 1;
const HISTORY_MASK: u64 = (DIGEST_HISTORY as u64) - 1;

/// The ring slot a tick occupies.
///
/// A mask rather than a remainder, which is why both ring depths are
/// powers of two: `%` trips this crate's arithmetic deny and `&` does
/// not. The mask bounds the result below the window, so the narrowing
/// cannot lose a bit on any target this engine builds for.
#[allow(
    clippy::cast_possible_truncation,
    reason = "masked below INPUT_WINDOW (64) one operation earlier, so the value always fits"
)]
const fn slot(tick: u64) -> usize {
    (tick & WINDOW_MASK) as usize
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "masked below DIGEST_HISTORY (16) one operation earlier, so the value always fits"
)]
const fn digest_slot(tick: u64) -> usize {
    (tick & HISTORY_MASK) as usize
}

/// Counters a driver reports and a desync report carries as evidence.
///
/// **Every field here is arrival-order- or frame-rate-dependent, and none
/// of them may ever enter a digest.** Two healthy peers on two healthy
/// machines will disagree about all of them; that is what they are for.
/// This is the exclusion an implementer is most likely to get wrong, and
/// folding one produces a lane that diverges on three correct machines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionStats {
    /// Datagrams refused before they reached the session's state.
    pub datagrams_refused: u64,
    /// Frames that arrived for a tick already held, carrying the same
    /// bytes. The redundancy scheme working; not an error.
    pub frames_repeated: u64,
    /// Frames that arrived for a tick already held, carrying *different*
    /// bytes. Counted and dropped — see [`Delivery`].
    pub frames_contradicted: u64,
    /// Frames refused for naming a tick outside the window.
    pub frames_out_of_window: u64,
    /// Digests that arrived for a `(peer, tick)` already held and
    /// disagreed with it. Refused, never overwritten.
    pub digests_contradicted: u64,
    /// Pumps spent waiting on a tick that could not run.
    pub stall_pumps: u64,
}

/// What happened to a delivered datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Absorbed: new inputs, a new digest, or a departure noted.
    Accepted,
    /// Refused, counted, and dropped. **Never terminal**, including for a
    /// peer that contradicts itself: first-write-wins already makes the
    /// second frame a state no-op, so ending the session would add no
    /// safety and would hand any address-spoofing attacker a one-packet
    /// kill switch that names an innocent peer as the culprit. The digest
    /// exchange is what catches a genuine fork, at most `digest_period`
    /// ticks later.
    Refused(Refusal),
    /// The session is over and no further tick will be confirmed.
    Ends(Outcome),
}

/// Why a datagram was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// Malformed, by the codec's own rules.
    Malformed(WireError),
    /// A different session's datagram, arriving at this port.
    WrongSession { saw: u64 },
    /// The header's claimed sender disagreed with the seat the transport
    /// attributed the bytes to. Checked here so a driver that only looks
    /// at bodies cannot forget it.
    SenderNotSource { claimed: PeerId, source: PeerId },
    /// A seat outside this session's roster.
    NotInRoster { peer: PeerId },
    /// This machine's own seat, arriving from the network.
    FromSelf,
    /// A `Hello` whose parameters are not the ones this peer is playing.
    Disagreement { theirs: u64, ours: u64 },
    /// A frame for a tick already confirmed, or too far ahead to buffer.
    OutOfWindow { tick: u64, pending: u64 },
    /// A frame for a tick already held, carrying different bytes.
    Contradiction { peer: PeerId, tick: u64 },
    /// A frame whose width is not the width everyone agreed on.
    WrongWidth { saw: u8, agreed: u8 },
    /// A digest for a `(peer, tick)` already held, disagreeing with it.
    DigestContradiction { peer: PeerId, tick: u64 },
    /// A datagram the session does not handle, and must not.
    ///
    /// Today this is chat, and the refusal is the point rather than an
    /// omission: chat is not simulation state, so a session that absorbed
    /// one would be a session holding something that must never reach a
    /// digest. The driver routes it to [`crate::ChatChannel`] instead.
    /// The compiler enforces the split — `Body` is exhaustive, so a new
    /// kind cannot be quietly ignored here.
    NotSessionTraffic { kind: wire::Kind },
}

/// Why a session ended. Every arm is terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// A peer's world hashed differently from this one's.
    ///
    /// Terminal on purpose, and the argument is worth keeping: letting a
    /// majority continue while a minority plays a different game is a
    /// silent fork by vote, which is the exact thing inputs-only lockstep
    /// exists to prevent. What corroboration would buy is *blame*, and
    /// blame belongs in the report.
    Desynced { tick: u64 },
    /// A peer left, naming the last tick it confirmed.
    PeerLeft { peer: PeerId, tick: u64 },
    /// This machine left.
    LeftLocally { tick: u64 },
}

/// What the session can do right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advance {
    /// A tick is confirmed and must be run.
    Step(Step),
    /// Nothing to run: every confirmed tick has been taken and the next
    /// one is not complete. Ordinary, and not an error.
    Waiting,
    /// The next tick is missing at least one peer's input.
    ///
    /// Reported rather than acted on. `pumps` counts how many times the
    /// caller has asked while this tick stayed incomplete — a number
    /// derived from the caller's own frame rate, which is exactly why the
    /// decision to give up belongs to the caller and not to this crate.
    Stalled {
        tick: u64,
        waiting: PeerSet,
        pumps: u64,
    },
    /// The session is over.
    Ended(Outcome),
}

/// One confirmed tick, and the inputs that make it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    tick: u64,
    digest_due: bool,
    peer_count: u8,
    input_bytes: u8,
    frames: [[u8; WIDTH]; PEERS],
}

impl Step {
    #[must_use]
    pub const fn tick(&self) -> u64 {
        self.tick
    }

    /// Whether the caller owes a world digest to [`Session::commit`].
    #[must_use]
    pub const fn digest_due(&self) -> bool {
        self.digest_due
    }

    /// One peer's input for this tick, exactly `input_bytes` wide.
    #[must_use]
    pub fn input(&self, peer: PeerId) -> Option<&[u8]> {
        if peer.index() >= self.peer_count {
            return None;
        }
        self.frames
            .get(usize::from(peer.index()))?
            .get(..usize::from(self.input_bytes))
    }

    /// Every seat's input, **ascending by seat, always**.
    ///
    /// The order is the contract, not a consequence: a caller that
    /// applied inputs in arrival order would put the network into its
    /// world, and every peer would diverge while each ran correct code.
    pub fn inputs(&self) -> impl Iterator<Item = (PeerId, &[u8])> {
        PeerSet::of_count(self.peer_count)
            .iter()
            .filter_map(move |peer| self.input(peer).map(|bytes| (peer, bytes)))
    }
}

/// A datagram the caller should send, and who to.
#[derive(Clone, Copy, Debug)]
pub struct Outbound<'a> {
    peer: PeerId,
    bytes: &'a [u8],
}

impl<'a> Outbound<'a> {
    #[must_use]
    pub const fn peer(&self) -> PeerId {
        self.peer
    }

    #[must_use]
    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

/// Why a local submission was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubmitError {
    /// The input is not the width every peer agreed on.
    WrongWidth { saw: usize, agreed: u8 },
    /// The window is full: the local peer is `INPUT_WINDOW` ticks ahead
    /// of the slowest confirmed tick and may not run further ahead.
    WindowFull { tick: u64, pending: u64 },
    /// The session is over.
    Ended(Outcome),
}

/// Why a commit was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommitError {
    /// A tick was committed that was not the one handed out.
    OutOfOrder { saw: u64, expected: u64 },
    /// A digest was owed and not supplied, or supplied and not owed.
    /// Both directions are refused: a caller who does not know which
    /// ticks are digested has a caller-side bug that would surface later
    /// as a desync nobody could explain.
    DigestMismatch { tick: u64, owed: bool },
}

/// One peer's fingerprints for one tick, as held in the ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    tick: u64,
    state: u64,
    input: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Waiting for every peer to say hello and for the local peer to fill
    /// the delay window. There is no synthetic neutral input to fill it
    /// with, deliberately: an all-zero frame is a value the game's decoder
    /// gives a meaning to, and inventing one here would make every peer do
    /// that thing for the whole window — identically, deterministically,
    /// and with no digest test able to see it.
    Joining,
    Playing,
    Over,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Emit {
    Hello,
    Inputs,
    Digest,
    Bye,
    Done,
}

/// A lockstep session: bytes and inputs in, confirmed ticks and bytes out.
pub struct Session {
    params: ValidParams,
    phase: Phase,

    /// Every peer's input for every tick in the window, by seat and slot.
    frames: [[[u8; WIDTH]; WINDOW]; PEERS],
    /// Which seats have submitted for the tick each slot holds.
    present: [PeerSet; WINDOW],
    /// Which tick each slot holds, so a stale slot is never read as a
    /// present one after the ring wraps.
    slot_tick: [u64; WINDOW],

    /// The next tick to hand out. Everything below it is history.
    pending: u64,
    /// The next tick the local peer will submit for.
    local_next: u64,

    /// The running fold over every confirmed input set.
    input_digest: StateHash,
    /// This peer's own fingerprints, and every remote's.
    mine: [Option<Fingerprint>; HISTORY],
    theirs: [[Option<Fingerprint>; HISTORY]; PEERS],
    /// A fingerprint owed to every remote and not yet sent.
    owed_digest: Option<Fingerprint>,

    /// Seats that have said hello, including this one.
    hello_seen: PeerSet,
    /// Seats seen *playing* — proved by any datagram only a playing peer
    /// sends. A peer cannot leave `Joining` without having heard every
    /// hello, so one of these is proof that peer heard ours, and that is
    /// the acknowledgement this handshake has instead of an ack.
    playing_seen: PeerSet,
    /// The tick handed out by `advance` and not yet committed.
    uncommitted: Option<(u64, bool)>,

    outcome: Option<Outcome>,
    stats: SessionStats,
    stalled_at: u64,

    emit: Emit,
    emit_peer: u8,
}

impl Session {
    /// A session that has not started: no tick exists until every peer has
    /// said hello and the local peer has filled the delay window.
    #[must_use]
    pub fn new(params: ValidParams) -> Self {
        Self {
            params,
            phase: Phase::Joining,
            frames: [[[0u8; WIDTH]; WINDOW]; PEERS],
            present: [PeerSet::EMPTY; WINDOW],
            slot_tick: [u64::MAX; WINDOW],
            pending: 0,
            local_next: 0,
            input_digest: StateHash::new(),
            mine: [None; HISTORY],
            theirs: [[None; HISTORY]; PEERS],
            owed_digest: None,
            hello_seen: PeerSet::EMPTY.with(params.local()),
            playing_seen: PeerSet::EMPTY,
            uncommitted: None,
            outcome: None,
            stats: SessionStats::default(),
            stalled_at: u64::MAX,
            emit: Emit::Hello,
            emit_peer: 0,
        }
    }

    #[must_use]
    pub const fn params(&self) -> &ValidParams {
        &self.params
    }

    #[must_use]
    pub const fn stats(&self) -> SessionStats {
        self.stats
    }

    /// The next tick that will be handed out — the confirmed frontier.
    ///
    /// **Do not key a local input on this.** It moves with the network:
    /// under loss it lags, and an input chosen from it is an input that
    /// depends on what the wire did. A scripted run that made that
    /// mistake would submit the same value for two different ticks and
    /// still see both peers agree, because they would agree on the wrong
    /// thing. [`Session::next_local_tick`] is the one to key on.
    #[must_use]
    pub const fn pending_tick(&self) -> u64 {
        self.pending
    }

    /// The tick the next [`Session::submit`] will fill.
    ///
    /// This is what a scripted or replayed run reads: it advances once
    /// per accepted submission and never moves for any other reason, so
    /// an input that is a function of it is a function of the tick alone
    /// — which is the property that makes a recorded run reproducible on
    /// a machine whose network behaved differently.
    #[must_use]
    pub const fn next_local_tick(&self) -> u64 {
        self.local_next
    }

    /// Whether every peer has said hello and the delay window is filled.
    #[must_use]
    pub const fn is_playing(&self) -> bool {
        matches!(self.phase, Phase::Playing)
    }

    #[must_use]
    pub const fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    /// The running fold over every confirmed input set so far.
    #[must_use]
    pub const fn input_digest(&self) -> u64 {
        self.input_digest.finish()
    }

    /// Whether the caller owes a local input right now.
    ///
    /// Ask once per pump and submit at most once. A caller that looped
    /// until this returned false would fill the whole window with one
    /// frame's held intent and make `input_delay` inert.
    #[must_use]
    pub fn wants_local(&self) -> bool {
        if matches!(self.phase, Phase::Over) {
            return false;
        }
        let horizon = self
            .pending
            .saturating_add(u64::from(self.params.input_delay()));
        self.local_next <= horizon
            && self.local_next.saturating_sub(self.pending) < u64::from(INPUT_WINDOW)
    }

    /// Offer this machine's input for the tick it is owed, returning that
    /// tick.
    ///
    /// **Never rewrites.** Each call fills the next unsubmitted tick, so a
    /// caller cannot revise an input after seeing another peer's — which
    /// is the shape of lookahead cheating, closed here by the state
    /// machine rather than by a rule.
    ///
    /// # Errors
    ///
    /// [`SubmitError`] for a wrong width, a tick already submitted, a full
    /// window, or a session that has ended.
    pub fn submit(&mut self, input: &[u8]) -> Result<u64, SubmitError> {
        if let Some(outcome) = self.outcome {
            return Err(SubmitError::Ended(outcome));
        }
        let agreed = self.params.input_bytes();
        if input.len() != usize::from(agreed) {
            return Err(SubmitError::WrongWidth {
                saw: input.len(),
                agreed,
            });
        }
        if self.local_next.saturating_sub(self.pending) >= u64::from(INPUT_WINDOW) {
            return Err(SubmitError::WindowFull {
                tick: self.local_next,
                pending: self.pending,
            });
        }
        // No "already submitted" refusal, and none is possible: this
        // always fills `local_next`, which only ever advances, and no
        // remote can write this seat's frame — a datagram claiming to be
        // from us is refused as `FromSelf`. First-write-wins holds by
        // construction rather than by a check, so a check here would
        // advertise a protection that could never fire.
        let tick = self.local_next;
        self.write_frame(self.params.local(), tick, input);
        self.local_next = self.local_next.saturating_add(1);
        self.maybe_start();
        Ok(tick)
    }

    /// Hand the session a datagram, attributed to the seat its transport
    /// says it came from.
    ///
    /// `source` is the seat the *transport* believes sent these bytes. The
    /// header's own claim is checked against it here, so a driver that
    /// only reads bodies cannot forget the check.
    pub fn deliver(&mut self, source: PeerId, bytes: &[u8]) -> Delivery {
        if let Some(outcome) = self.outcome {
            return Delivery::Ends(outcome);
        }
        let datagram = match wire::read(bytes) {
            Ok(datagram) => datagram,
            Err(error) => return self.refuse(Refusal::Malformed(error)),
        };
        let sender = datagram.header.sender;
        let session = datagram.header.session;
        if session != self.params.session() {
            return self.refuse(Refusal::WrongSession { saw: session.get() });
        }
        if sender != source {
            return self.refuse(Refusal::SenderNotSource {
                claimed: sender,
                source,
            });
        }
        if sender == self.params.local() {
            return self.refuse(Refusal::FromSelf);
        }
        if !self.params.roster().contains(sender) {
            return self.refuse(Refusal::NotInRoster { peer: sender });
        }

        match datagram.body {
            Body::Hello(body) => {
                if body.agreement_digest != self.params.agreement_digest() {
                    return self.refuse(Refusal::Disagreement {
                        theirs: body.agreement_digest,
                        ours: self.params.agreement_digest(),
                    });
                }
                self.hello_seen = self.hello_seen.with(sender);
                self.maybe_start();
                Delivery::Accepted
            }
            Body::Inputs(body) => {
                self.playing_seen = self.playing_seen.with(sender);
                self.absorb_inputs(sender, &body)
            }
            Body::Digest(body) => {
                self.playing_seen = self.playing_seen.with(sender);
                self.absorb_digest(sender, body.tick, body.state_digest, body.input_digest)
            }
            Body::Chat(_) => self.refuse(Refusal::NotSessionTraffic {
                kind: wire::Kind::Chat,
            }),
            Body::Bye(body) => {
                let outcome = Outcome::PeerLeft {
                    peer: sender,
                    tick: body.tick,
                };
                self.end(outcome);
                Delivery::Ends(outcome)
            }
        }
    }

    /// Take the next confirmed tick, if there is one.
    pub fn advance(&mut self) -> Advance {
        if let Some(outcome) = self.outcome {
            return Advance::Ended(outcome);
        }
        if self.uncommitted.is_some() || !matches!(self.phase, Phase::Playing) {
            return Advance::Waiting;
        }
        let tick = self.pending;
        let roster = self.params.roster();
        let held = self.held_by(tick);
        if held != roster {
            if self.stalled_at != tick {
                self.stalled_at = tick;
                self.stats.stall_pumps = 0;
            }
            self.stats.stall_pumps = self.stats.stall_pumps.saturating_add(1);
            return Advance::Stalled {
                tick,
                waiting: roster.without_all(held),
                pumps: self.stats.stall_pumps,
            };
        }
        self.stalled_at = u64::MAX;

        let mut frames = [[0u8; WIDTH]; PEERS];
        let width = usize::from(self.params.input_bytes());
        let index = slot(tick);
        for peer in roster.iter() {
            let seat = usize::from(peer.index());
            if let (Some(destination), Some(source)) = (
                frames.get_mut(seat),
                self.frames.get(seat).and_then(|ring| ring.get(index)),
            ) && let (Some(destination), Some(source)) =
                (destination.get_mut(..width), source.get(..width))
            {
                destination.copy_from_slice(source);
            }
        }

        // Folded once, here, before the tick is handed out — a repeat
        // frame is proven identical before it is ignored, so absorbing it
        // again would move the digest for a datagram that moved no state.
        self.input_digest = self.input_digest.absorb_u64(tick);
        for peer in roster.iter() {
            if let Some(bytes) = frames
                .get(usize::from(peer.index()))
                .and_then(|frame| frame.get(..width))
            {
                self.input_digest = self.input_digest.absorb_bytes(bytes);
            }
        }

        let digest_due = tick
            .checked_rem(u64::from(self.params.digest_period()))
            .is_some_and(|remainder| remainder == 0);
        self.uncommitted = Some((tick, digest_due));
        Advance::Step(Step {
            tick,
            digest_due,
            peer_count: self.params.peer_count(),
            input_bytes: self.params.input_bytes(),
            frames,
        })
    }

    /// Report that the tick just handed out has been run.
    ///
    /// `world_digest` must be present exactly when [`Step::digest_due`]
    /// said so, and absent otherwise. Both directions are refused: a
    /// caller who does not know which ticks are digested has a bug that
    /// would otherwise surface much later as a desync nobody can explain.
    ///
    /// # Errors
    ///
    /// [`CommitError`] for a tick out of order, or a digest owed and not
    /// supplied — or supplied and not owed.
    pub fn commit(&mut self, tick: u64, world_digest: Option<u64>) -> Result<(), CommitError> {
        let Some((expected, owed)) = self.uncommitted else {
            return Err(CommitError::OutOfOrder {
                saw: tick,
                expected: self.pending,
            });
        };
        if tick != expected {
            return Err(CommitError::OutOfOrder {
                saw: tick,
                expected,
            });
        }
        if owed != world_digest.is_some() {
            return Err(CommitError::DigestMismatch { tick, owed });
        }

        self.uncommitted = None;
        self.pending = self.pending.saturating_add(1);

        if let Some(state) = world_digest {
            let mine = Fingerprint {
                tick,
                state,
                input: self.input_digest.finish(),
            };
            if let Some(cell) = self.mine.get_mut(digest_slot(tick)) {
                *cell = Some(mine);
            }
            self.owed_digest = Some(mine);
            // Compare against anything already heard for this tick: a
            // peer running ahead publishes before this machine gets here.
            self.judge(tick);
        }
        Ok(())
    }

    /// The report explaining a divergence, once one has been found.
    #[must_use]
    pub fn desync(&self) -> Option<DesyncReport> {
        let Some(Outcome::Desynced { tick }) = self.outcome else {
            return None;
        };
        Some(self.report_at(tick))
    }

    /// Leave, naming the last tick this peer confirmed.
    pub fn leave(&mut self) -> Outcome {
        let outcome = Outcome::LeftLocally {
            tick: self.pending.saturating_sub(1),
        };
        if self.outcome.is_none() {
            self.end(outcome);
            self.emit = Emit::Bye;
            self.emit_peer = 0;
        }
        self.outcome.unwrap_or(outcome)
    }

    /// The next datagram the caller should send, written into `out`.
    ///
    /// Call until it returns `None`, once per pump. **Nothing received
    /// causes anything to be sent** — every datagram here is rendered from
    /// session state on demand, never in reply, which is what keeps this
    /// port off a reflection list and is asserted by a test rather than
    /// left as a habit.
    pub fn next_outbound<'b>(
        &mut self,
        out: &'b mut [u8; MAX_DATAGRAM_BYTES],
    ) -> Option<Outbound<'b>> {
        loop {
            let Some(peer) = self.next_target() else {
                self.emit = self.first_emit();
                self.emit_peer = 0;
                return None;
            };
            let emit = self.emit;
            self.emit_peer = self.emit_peer.saturating_add(1);
            if let Some(len) = self.render(emit, out) {
                return Some(Outbound {
                    peer,
                    bytes: out.get(..len)?,
                });
            }
        }
    }
}

/// Everything below is private: the parts a consumer never names.
impl Session {
    /// Which seats have a frame for `tick`, or none if the slot has been
    /// reused by a later tick.
    fn held_by(&self, tick: u64) -> PeerSet {
        let index = slot(tick);
        if self.slot_tick.get(index).copied() == Some(tick) {
            self.present.get(index).copied().unwrap_or(PeerSet::EMPTY)
        } else {
            PeerSet::EMPTY
        }
    }

    /// The frame a seat submitted for `tick`, if the slot still holds it.
    fn frame_of(&self, peer: PeerId, tick: u64) -> Option<&[u8]> {
        if !self.held_by(tick).contains(peer) {
            return None;
        }
        let width = usize::from(self.params.input_bytes());
        self.frames
            .get(usize::from(peer.index()))?
            .get(slot(tick))?
            .get(..width)
    }

    /// Claim a slot for `tick` if a previous tick still holds it, then
    /// record one seat's frame in it.
    fn write_frame(&mut self, peer: PeerId, tick: u64, input: &[u8]) {
        let index = slot(tick);
        if self.slot_tick.get(index).copied() != Some(tick) {
            if let Some(cell) = self.slot_tick.get_mut(index) {
                *cell = tick;
            }
            if let Some(cell) = self.present.get_mut(index) {
                *cell = PeerSet::EMPTY;
            }
        }
        let width = usize::from(self.params.input_bytes());
        if let Some(destination) = self
            .frames
            .get_mut(usize::from(peer.index()))
            .and_then(|ring| ring.get_mut(index))
            .and_then(|frame| frame.get_mut(..width))
            && let Some(source) = input.get(..width)
        {
            destination.copy_from_slice(source);
            if let Some(cell) = self.present.get_mut(index) {
                *cell = cell.with(peer);
            }
        }
    }

    /// Leave `Joining` once every peer has said hello and the local peer
    /// has submitted through the delay window.
    fn maybe_start(&mut self) {
        if matches!(self.phase, Phase::Joining)
            && self.hello_seen == self.params.roster()
            && self.local_next > u64::from(self.params.input_delay())
        {
            self.phase = Phase::Playing;
        }
    }

    fn refuse(&mut self, refusal: Refusal) -> Delivery {
        self.stats.datagrams_refused = self.stats.datagrams_refused.saturating_add(1);
        match refusal {
            Refusal::Contradiction { .. } => {
                self.stats.frames_contradicted = self.stats.frames_contradicted.saturating_add(1);
            }
            Refusal::OutOfWindow { .. } => {
                self.stats.frames_out_of_window = self.stats.frames_out_of_window.saturating_add(1);
            }
            Refusal::DigestContradiction { .. } => {
                self.stats.digests_contradicted = self.stats.digests_contradicted.saturating_add(1);
            }
            _ => {}
        }
        Delivery::Refused(refusal)
    }

    fn end(&mut self, outcome: Outcome) {
        if self.outcome.is_none() {
            self.outcome = Some(outcome);
            self.phase = Phase::Over;
        }
    }

    fn absorb_inputs(&mut self, sender: PeerId, body: &wire::InputsBody<'_>) -> Delivery {
        let agreed = self.params.input_bytes();
        if body.input_bytes != agreed {
            return self.refuse(Refusal::WrongWidth {
                saw: body.input_bytes,
                agreed,
            });
        }

        let mut accepted = false;
        let mut refusal = None;
        for (tick, bytes) in body.iter() {
            // Below the frontier is history the session has already
            // folded; past the window is a peer spending this machine's
            // memory. Neither is an error worth ending a session for, and
            // the first is the ordinary case: redundancy repeats frames
            // that have already been used.
            if tick < self.pending {
                continue;
            }
            if tick.saturating_sub(self.pending) >= u64::from(INPUT_WINDOW) {
                refusal = Some(Refusal::OutOfWindow {
                    tick,
                    pending: self.pending,
                });
                continue;
            }
            match self.frame_of(sender, tick) {
                Some(held) if held == bytes => {
                    self.stats.frames_repeated = self.stats.frames_repeated.saturating_add(1);
                }
                Some(_) => {
                    // First write wins, so this is already a state no-op.
                    // Counted and dropped rather than fatal: see `Delivery`.
                    refusal = Some(Refusal::Contradiction { peer: sender, tick });
                }
                None => {
                    self.write_frame(sender, tick, bytes);
                    accepted = true;
                }
            }
        }

        match refusal {
            Some(refusal) if !accepted => self.refuse(refusal),
            Some(refusal) => {
                // Something useful arrived alongside something refused;
                // the counter still moves so the evidence survives.
                let _ = self.refuse(refusal);
                Delivery::Accepted
            }
            None => Delivery::Accepted,
        }
    }

    fn absorb_digest(&mut self, sender: PeerId, tick: u64, state: u64, input: u64) -> Delivery {
        let seat = usize::from(sender.index());
        let index = digest_slot(tick);
        let incoming = Fingerprint { tick, state, input };
        let held = self
            .theirs
            .get(seat)
            .and_then(|ring| ring.get(index))
            .copied()
            .flatten();

        match held {
            // First write wins per (peer, tick): a flood of forged
            // digests cannot evict a genuine one and blind the detector.
            Some(previous) if previous.tick == tick && previous != incoming => {
                return self.refuse(Refusal::DigestContradiction { peer: sender, tick });
            }
            Some(previous) if previous == incoming => return Delivery::Accepted,
            _ => {}
        }

        if let Some(cell) = self
            .theirs
            .get_mut(seat)
            .and_then(|ring| ring.get_mut(index))
        {
            *cell = Some(incoming);
        }
        if let Some(outcome) = self.judge(tick) {
            return Delivery::Ends(outcome);
        }
        Delivery::Accepted
    }

    /// Compare every fingerprint held for `tick`. A tick this peer never
    /// digested compares nothing and says so by returning `None` — the
    /// anti-vacuity arm, so a report about an unwitnessed tick can never
    /// read as agreement.
    fn judge(&mut self, tick: u64) -> Option<Outcome> {
        let mine = self
            .mine
            .get(digest_slot(tick))
            .copied()
            .flatten()
            .filter(|held| held.tick == tick)?;
        for peer in self.params.remotes().iter() {
            let theirs = self
                .theirs
                .get(usize::from(peer.index()))
                .and_then(|ring| ring.get(digest_slot(tick)))
                .copied()
                .flatten();
            if let Some(theirs) = theirs
                && theirs.tick == tick
                && theirs.state != mine.state
            {
                let outcome = Outcome::Desynced { tick };
                self.end(outcome);
                return Some(outcome);
            }
        }
        None
    }

    fn report_at(&self, tick: u64) -> DesyncReport {
        let index = digest_slot(tick);
        let mine = self.mine.get(index).copied().flatten();
        let mut peer_state = [None; PEERS];
        let mut peer_input = [None; PEERS];
        for peer in self.params.remotes().iter() {
            let seat = usize::from(peer.index());
            let held = self
                .theirs
                .get(seat)
                .and_then(|ring| ring.get(index))
                .copied()
                .flatten()
                .filter(|held| held.tick == tick);
            if let (Some(held), Some(state), Some(input)) =
                (held, peer_state.get_mut(seat), peer_input.get_mut(seat))
            {
                *state = Some(held.state);
                *input = Some(held.input);
            }
        }
        DesyncReport {
            tick,
            local: self.params.local(),
            peer_count: self.params.peer_count(),
            local_state_digest: mine.map_or(0, |held| held.state),
            local_input_digest: mine.map_or(0, |held| held.input),
            peer_state_digests: peer_state,
            peer_input_digests: peer_input,
            last_agreed_tick: self.last_agreed_before(tick),
            agreement_digest: self.params.agreement_digest(),
            content: self.params.get().content,
            rules: self.params.get().rules,
            seed: self.params.get().seed,
            stats: self.stats,
        }
    }

    /// The most recent digested tick below `tick` at which every
    /// reporting peer agreed. It bounds the search for the divergence to
    /// at most `digest_period` ticks, which is what turns that parameter
    /// from a bandwidth knob into a bisect knob.
    fn last_agreed_before(&self, tick: u64) -> Option<u64> {
        let mut best: Option<u64> = None;
        for held in self.mine.iter().flatten() {
            if held.tick >= tick {
                continue;
            }
            let agreed = self.params.remotes().iter().all(|peer| {
                self.theirs
                    .get(usize::from(peer.index()))
                    .and_then(|ring| ring.get(digest_slot(held.tick)))
                    .copied()
                    .flatten()
                    .filter(|theirs| theirs.tick == held.tick)
                    .is_none_or(|theirs| theirs.state == held.state)
            });
            if agreed && best.is_none_or(|previous| held.tick > previous) {
                best = Some(held.tick);
            }
        }
        best
    }

    /// Whether this peer still owes anyone a hello.
    ///
    /// **Not "am I joining".** A hello that is lost while the sender goes
    /// on to play is a hello never sent again, and the peer that missed it
    /// waits in `Joining` forever holding a full set of that sender's
    /// input frames — stalled on a handshake rather than on data, with no
    /// error raised anywhere. So a hello keeps going out until every
    /// remote has been *seen playing*, which is proof it heard this one.
    /// That terminates: a peer cannot play without having heard everyone.
    fn hello_owed(&self) -> bool {
        !matches!(self.phase, Phase::Over) && self.playing_seen != self.params.remotes()
    }

    /// The first thing a pump emits, given the phase.
    fn first_emit(&self) -> Emit {
        match self.phase {
            Phase::Over => Emit::Bye,
            _ if self.hello_owed() => Emit::Hello,
            Phase::Playing => Emit::Inputs,
            Phase::Joining => Emit::Hello,
        }
    }

    /// Walk remotes for the current emission, then move to the next kind.
    fn next_target(&mut self) -> Option<PeerId> {
        loop {
            if matches!(self.emit, Emit::Done) {
                return None;
            }
            let remotes = self.params.remotes();
            while let Some(peer) = PeerId::new(self.emit_peer) {
                if remotes.contains(peer) {
                    return Some(peer);
                }
                self.emit_peer = self.emit_peer.saturating_add(1);
            }
            self.emit_peer = 0;
            self.emit = match self.emit {
                // A hello no longer ends the pump: a peer that is playing
                // and still owes a hello sends both, or its inputs would
                // stop while it re-announced.
                Emit::Hello if matches!(self.phase, Phase::Playing) => Emit::Inputs,
                Emit::Inputs => Emit::Digest,
                _ => Emit::Done,
            };
            if matches!(self.emit, Emit::Digest) && self.owed_digest.is_none() {
                self.emit = Emit::Done;
            }
        }
    }

    /// Render one datagram, or `None` when there is nothing of this kind
    /// to say right now.
    fn render(&mut self, emit: Emit, out: &mut [u8; MAX_DATAGRAM_BYTES]) -> Option<usize> {
        let addressing = wire::Addressing {
            sender: self.params.local(),
            session: self.params.session(),
        };
        match emit {
            Emit::Hello => {
                let body = wire::HelloBody {
                    agreement_digest: self.params.agreement_digest(),
                    content: self.params.get().content,
                    rules: self.params.get().rules,
                    seed: self.params.get().seed,
                    peer_count: self.params.peer_count(),
                    input_bytes: self.params.input_bytes(),
                    input_delay: self.params.input_delay(),
                    digest_period: self.params.digest_period(),
                };
                wire::write_hello(out, addressing, &body).ok()
            }
            Emit::Inputs => self.render_inputs(addressing, out),
            Emit::Digest => {
                let owed = self.owed_digest?;
                Some(wire::write_digest(
                    out,
                    addressing,
                    &wire::DigestBody {
                        tick: owed.tick,
                        state_digest: owed.state,
                        input_digest: owed.input,
                    },
                ))
            }
            Emit::Bye => Some(wire::write_bye(
                out,
                addressing,
                &wire::ByeBody {
                    tick: self.pending.saturating_sub(1),
                },
            )),
            Emit::Done => None,
        }
    }

    /// The newest local frames, oldest first — the redundancy that makes
    /// loss free without a round trip.
    fn render_inputs(
        &self,
        addressing: wire::Addressing,
        out: &mut [u8; MAX_DATAGRAM_BYTES],
    ) -> Option<usize> {
        if self.local_next == 0 {
            return None;
        }
        let newest = self.local_next.saturating_sub(1);
        let want = u64::from(INPUT_REDUNDANCY);
        let local = self.params.local();

        // **Bounded by what this peer has submitted, never by what it has
        // confirmed.** Clamping the run to the local frontier is the
        // obvious thing to write and it deadlocks: the moment this peer
        // confirms a tick it would stop repeating it, so a peer that never
        // received that frame could never be sent it again — each waiting
        // on the other, forever, with no error raised anywhere. A sender's
        // own progress says nothing about what a receiver still needs.
        //
        // Walked backwards from the newest and stopped at the first gap,
        // so a frame the ring has already recycled ends the run instead of
        // cancelling the whole datagram.
        let mut oldest = newest;
        let mut span = 1u64;
        while span < want {
            let Some(earlier) = oldest.checked_sub(1) else {
                break;
            };
            if self.frame_of(local, earlier).is_none() {
                break;
            }
            oldest = earlier;
            span = span.saturating_add(1);
        }
        let count = u8::try_from(span).ok()?;
        let width = usize::from(self.params.input_bytes());

        let mut run = [0u8; (INPUT_REDUNDANCY as usize) * WIDTH];
        for step in 0..u64::from(count) {
            let tick = oldest.checked_add(step)?;
            let bytes = self.frame_of(local, tick)?;
            let at = usize::try_from(step).ok()?.checked_mul(width)?;
            let destination = run.get_mut(at..)?.get_mut(..width)?;
            destination.copy_from_slice(bytes);
        }
        let total = usize::from(count).checked_mul(width)?;
        wire::write_inputs(
            out,
            addressing,
            oldest,
            count,
            self.params.input_bytes(),
            run.get(..total)?,
        )
        .ok()
    }
}
