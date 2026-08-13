//! Text between players, and the one thing in this crate that is not
//! simulation state.
//!
//! # Why this is a separate type and not a corner of the session
//!
//! A chat message must never reach a digest, never gate a tick, and never
//! be able to desync anything. Those are easy sentences to write and easy
//! rules to break later, so the separation is **structural**: a
//! [`Session`](crate::Session) has no field that could hold a message and
//! refuses a `Chat` datagram by name. The two share a wire and nothing
//! else, and a driver routes between them. It is the same move the socket
//! split makes — the thing that must not happen is made unrepresentable
//! rather than forbidden.
//!
//! # Ordering, and why it is not the tick
//!
//! Chat carries its own per-sender counter. Borrowing the tick stream
//! would give a message the power to hold a tick up, which is exactly the
//! power it must not have. The cost, stated plainly: **two players'
//! messages have no global order.** Each sender's own messages arrive in
//! the order they were sent; between senders there is no truth to be had
//! without a clock or a consensus round, and this crate has neither.
//!
//! # Delivery
//!
//! Best effort, the same shape as the input stream: **redundancy rather
//! than retransmission.** A message is repeated for a few pumps and then
//! forgotten, and the receiver drops duplicates. Nothing is acknowledged
//! and nothing is guaranteed — a message can be lost, and the ADR records
//! that as accepted. Chat that must not be lost is a different feature
//! with a different cost.

use core::num::NonZeroU64;

use crate::wire::{self, MAX_CHAT_BYTES};
use crate::{MAX_DATAGRAM_BYTES, MAX_PEERS, PeerId, PeerSet};

const PEERS: usize = MAX_PEERS as usize;

/// How many pumps a message is repeated for before it is forgotten.
///
/// The whole of the loss story, and the same trade the input stream makes:
/// a handful of repeats costs a few hundred bytes and no round trip, where
/// an acknowledgement costs a round trip and a retransmit buffer an
/// attacker can grow.
pub const CHAT_REPEATS: u8 = 6;

/// How many of this peer's own messages may be in flight at once.
pub const CHAT_OUTBOX: usize = 4;

/// How many received messages are held before the oldest is dropped.
///
/// A ceiling, not a target: the driver is expected to drain this every
/// frame. It exists so a peer that types faster than the game reads cannot
/// grow this crate's memory.
pub const CHAT_INBOX: usize = 16;

/// One message, as held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    /// Who sent it.
    pub from: PeerId,
    /// The sender's own counter for it.
    pub sequence: u64,
    len: u8,
    text: [u8; MAX_CHAT_BYTES],
}

impl Message {
    /// The message bytes.
    ///
    /// **Not validated as text.** This crate does not know what encoding a
    /// game speaks; a decoder that guessed would refuse messages it should
    /// carry. A caller that wants UTF-8 checks for it here, where a
    /// refusal can say so to the player who typed it.
    #[must_use]
    pub fn text(&self) -> &[u8] {
        self.text.get(..usize::from(self.len)).unwrap_or_default()
    }
}

/// Counters a driver can report. **None of these may enter a digest** —
/// every one is arrival- and rate-dependent, like the session's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChatStats {
    pub sent: u64,
    pub received: u64,
    /// Repeats of a message already held. The redundancy working.
    pub duplicates: u64,
    /// Messages dropped because the inbox was full and nobody drained it.
    pub inbox_overflowed: u64,
    /// Datagrams refused, **not counting duplicates**.
    ///
    /// Zero on a healthy channel however much traffic crosses it, which
    /// is what makes it worth reading: a rising count is a peer sending
    /// something this one will not take, never the redundancy working.
    /// Repeats live in [`ChatStats::duplicates`].
    pub refused: u64,
}

/// Why a message could not be sent or was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChatRefusal {
    /// Empty, or longer than one datagram can carry.
    Length { saw: usize, ceiling: usize },
    /// This peer already has [`CHAT_OUTBOX`] messages still repeating.
    OutboxFull,
    /// Malformed bytes, by the codec's rules.
    Malformed(wire::WireError),
    /// A datagram for a different session.
    WrongSession { saw: u64 },
    /// The claimed sender disagreed with the seat the transport named.
    SenderNotSource { claimed: PeerId, source: PeerId },
    /// A seat outside the roster, or this machine's own.
    NotAPeer { peer: PeerId },
    /// Not a chat datagram at all. The session's traffic, misrouted.
    NotChatTraffic { kind: wire::Kind },
    /// A message this peer has already delivered, or one so old the
    /// duplicate window has forgotten it.
    AlreadySeen { peer: PeerId, sequence: u64 },
}

/// One peer's outgoing message, while it is still being repeated.
#[derive(Clone, Copy, Debug)]
struct Outgoing {
    sequence: u64,
    len: u8,
    text: [u8; MAX_CHAT_BYTES],
    repeats_left: u8,
}

/// The duplicate filter for one sender.
///
/// A high-water sequence plus a bitmap of the sixty-four below it, which
/// is the standard shape and the reason it is bounded: a peer cannot make
/// this grow by sending, and a message older than the window is refused
/// rather than delivered twice.
#[derive(Clone, Copy, Debug, Default)]
struct Seen {
    high: u64,
    below: u64,
    any: bool,
}

impl Seen {
    /// Records `sequence`, returning `false` if it was already known.
    fn admit(&mut self, sequence: u64) -> bool {
        if !self.any {
            self.any = true;
            self.high = sequence;
            return true;
        }
        if sequence > self.high {
            let shift = sequence.saturating_sub(self.high);
            self.below = if shift >= 64 {
                0
            } else {
                (self.below << shift) | (1u64 << shift.saturating_sub(1))
            };
            self.high = sequence;
            return true;
        }
        if sequence == self.high {
            return false;
        }
        let back = self.high.saturating_sub(sequence);
        if back > 64 {
            return false;
        }
        let bit = 1u64 << back.saturating_sub(1);
        if self.below & bit != 0 {
            return false;
        }
        self.below |= bit;
        true
    }
}

/// Text between the peers of one session.
///
/// Held by the driver, beside the session and never inside it.
pub struct ChatChannel {
    local: PeerId,
    roster: PeerSet,
    session: NonZeroU64,
    next_sequence: u64,
    outbox: [Option<Outgoing>; CHAT_OUTBOX],
    inbox: [Option<Message>; CHAT_INBOX],
    /// Where the next received message lands, and where a drain starts.
    head: usize,
    len: usize,
    seen: [Seen; PEERS],
    stats: ChatStats,
    emit_peer: u8,
    emit_slot: usize,
}

impl ChatChannel {
    /// A channel for the session these parameters describe.
    ///
    /// Takes the same validated parameters the session does, so the two
    /// cannot disagree about who is playing or which session this is.
    #[must_use]
    pub fn new(params: &crate::ValidParams) -> Self {
        Self {
            local: params.local(),
            roster: params.roster(),
            session: params.session(),
            next_sequence: 0,
            outbox: [None; CHAT_OUTBOX],
            inbox: [None; CHAT_INBOX],
            head: 0,
            len: 0,
            seen: [Seen::default(); PEERS],
            stats: ChatStats::default(),
            emit_peer: 0,
            emit_slot: 0,
        }
    }

    #[must_use]
    pub const fn stats(&self) -> ChatStats {
        self.stats
    }

    /// How many received messages are waiting to be drained.
    #[must_use]
    pub const fn waiting(&self) -> usize {
        self.len
    }

    /// Queue a message for sending, returning its sequence number.
    ///
    /// # Errors
    ///
    /// [`ChatRefusal::Length`] for an empty or oversized message, and
    /// [`ChatRefusal::OutboxFull`] when this peer already has
    /// [`CHAT_OUTBOX`] messages in flight — a bound rather than a queue,
    /// so a player holding the key down cannot grow this crate's memory.
    pub fn send(&mut self, text: &[u8]) -> Result<u64, ChatRefusal> {
        if text.is_empty() || text.len() > MAX_CHAT_BYTES {
            return Err(ChatRefusal::Length {
                saw: text.len(),
                ceiling: MAX_CHAT_BYTES,
            });
        }
        let Ok(len) = u8::try_from(text.len()) else {
            return Err(ChatRefusal::Length {
                saw: text.len(),
                ceiling: MAX_CHAT_BYTES,
            });
        };
        let slot = self
            .outbox
            .iter()
            .position(Option::is_none)
            .ok_or(ChatRefusal::OutboxFull)?;

        let mut buffer = [0u8; MAX_CHAT_BYTES];
        if let Some(room) = buffer.get_mut(..text.len()) {
            room.copy_from_slice(text);
        }
        let sequence = self.next_sequence;
        if let Some(cell) = self.outbox.get_mut(slot) {
            *cell = Some(Outgoing {
                sequence,
                len,
                text: buffer,
                repeats_left: CHAT_REPEATS,
            });
        }
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.stats.sent = self.stats.sent.saturating_add(1);
        Ok(sequence)
    }

    /// Hand the channel a datagram the driver has decided is chat.
    ///
    /// # Errors
    ///
    /// [`ChatRefusal`] naming why it was dropped. Every refusal is
    /// counted and none is fatal: chat cannot end a session, which is the
    /// whole reason it lives out here.
    pub fn deliver(&mut self, source: PeerId, bytes: &[u8]) -> Result<(), ChatRefusal> {
        let refusal = self.absorb(source, bytes);
        // **A duplicate is not counted here.** It is a refusal in the
        // return type, because nothing was delivered, and it is the
        // protocol behaving rather than misbehaving: a message is
        // repeated a fixed number of times and never acknowledged, so
        // every message that arrives at all arrives several times over.
        // Folding those into `refused` gave that counter a healthy value
        // of five per message, which makes it useless for the one
        // question it is asked — is something wrong? They are in
        // [`ChatStats::duplicates`], which is where a reader looking for
        // the redundancy working will look.
        if matches!(refusal, Err(refused) if !matches!(refused, ChatRefusal::AlreadySeen { .. })) {
            self.stats.refused = self.stats.refused.saturating_add(1);
        }
        refusal
    }

    fn absorb(&mut self, source: PeerId, bytes: &[u8]) -> Result<(), ChatRefusal> {
        let datagram = wire::read(bytes).map_err(ChatRefusal::Malformed)?;
        if datagram.header.session != self.session {
            return Err(ChatRefusal::WrongSession {
                saw: datagram.header.session.get(),
            });
        }
        if datagram.header.sender != source {
            return Err(ChatRefusal::SenderNotSource {
                claimed: datagram.header.sender,
                source,
            });
        }
        if source == self.local || !self.roster.contains(source) {
            return Err(ChatRefusal::NotAPeer { peer: source });
        }
        let wire::Body::Chat(body) = datagram.body else {
            return Err(ChatRefusal::NotChatTraffic {
                kind: datagram.header.kind,
            });
        };

        let admitted = self
            .seen
            .get_mut(usize::from(source.index()))
            .is_some_and(|seen| seen.admit(body.sequence));
        if !admitted {
            self.stats.duplicates = self.stats.duplicates.saturating_add(1);
            return Err(ChatRefusal::AlreadySeen {
                peer: source,
                sequence: body.sequence,
            });
        }

        let text = body.text();
        let Ok(len) = u8::try_from(text.len()) else {
            return Err(ChatRefusal::Length {
                saw: text.len(),
                ceiling: MAX_CHAT_BYTES,
            });
        };
        let mut buffer = [0u8; MAX_CHAT_BYTES];
        if let Some(room) = buffer.get_mut(..text.len()) {
            room.copy_from_slice(text);
        }

        // The oldest is dropped rather than the newest refused: a player
        // who looks away should come back to recent conversation, not to
        // the first thing anyone said and then silence.
        if self.len == CHAT_INBOX {
            self.stats.inbox_overflowed = self.stats.inbox_overflowed.saturating_add(1);
            self.head = self.head.saturating_add(1) % CHAT_INBOX;
            self.len = self.len.saturating_sub(1);
        }
        let at = self.head.saturating_add(self.len) % CHAT_INBOX;
        if let Some(cell) = self.inbox.get_mut(at) {
            *cell = Some(Message {
                from: source,
                sequence: body.sequence,
                len,
                text: buffer,
            });
            self.len = self.len.saturating_add(1);
        }
        self.stats.received = self.stats.received.saturating_add(1);
        Ok(())
    }

    /// Take the oldest received message, if there is one.
    pub fn next_message(&mut self) -> Option<Message> {
        if self.len == 0 {
            return None;
        }
        let taken = self.inbox.get_mut(self.head).and_then(Option::take);
        self.head = self.head.saturating_add(1) % CHAT_INBOX;
        self.len = self.len.saturating_sub(1);
        taken
    }

    /// The next chat datagram to send, written into `out`.
    ///
    /// Call until it returns `None`, once per pump. Like the session's
    /// outbox, **nothing received causes anything to be sent**: every
    /// datagram here is rendered from state this peer already holds.
    pub fn next_outbound<'b>(
        &mut self,
        out: &'b mut [u8; MAX_DATAGRAM_BYTES],
    ) -> Option<Outbound<'b>> {
        loop {
            let Some(peer) = self.next_target() else {
                self.emit_peer = 0;
                self.emit_slot = 0;
                self.expire();
                return None;
            };
            let slot = self.emit_slot;
            self.emit_peer = self.emit_peer.saturating_add(1);
            let message = self.outbox.get(slot).copied().flatten();
            if let Some(message) = message {
                let text = message.text.get(..usize::from(message.len))?;
                let len = wire::write_chat(
                    out,
                    wire::Addressing {
                        sender: self.local,
                        session: self.session,
                    },
                    message.sequence,
                    text,
                )
                .ok()?;
                return Some(Outbound {
                    peer,
                    bytes: out.get(..len)?,
                });
            }
        }
    }

    /// Walk every remote for the current message, then move to the next.
    fn next_target(&mut self) -> Option<PeerId> {
        loop {
            if self.emit_slot >= CHAT_OUTBOX {
                return None;
            }
            let remotes = self.roster.without(self.local);
            while let Some(peer) = PeerId::new(self.emit_peer) {
                if remotes.contains(peer) {
                    return Some(peer);
                }
                self.emit_peer = self.emit_peer.saturating_add(1);
            }
            self.emit_peer = 0;
            self.emit_slot = self.emit_slot.saturating_add(1);
        }
    }

    /// One repeat spent per message, per pump; a message out of repeats is
    /// forgotten. Charged at the end of a pump rather than per datagram,
    /// so the count means pumps and not peers.
    fn expire(&mut self) {
        for slot in &mut self.outbox {
            if let Some(message) = slot {
                message.repeats_left = message.repeats_left.saturating_sub(1);
                if message.repeats_left == 0 {
                    *slot = None;
                }
            }
        }
    }
}

/// A chat datagram to send, and who to.
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
