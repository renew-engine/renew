//! What every peer must agree on before tick zero exists.
//!
//! Two types and one rule. [`SessionParams`] is what a caller writes
//! down; [`ValidParams`] is what a session holds, and it exists only
//! because a validated value that is indistinguishable from an
//! unvalidated one gets re-validated at every use, or worse, does not.
//!
//! The agreement digest is folded here, once, at validation. Every peer
//! folds it from its own parameters and puts the result in its `Hello`;
//! two peers who disagree about anything that matters therefore disagree
//! about one `u64` at the handshake, rather than about a world four
//! hundred ticks later.

use renew_frame::StateHash;

use crate::wire::WIRE_VERSION;
use crate::{INPUT_WINDOW, MAX_INPUT_BYTES, MAX_PEERS, PeerId, PeerSet};

/// The smallest roster that is a session. One peer is not multiplayer.
pub const MIN_PEERS: u8 = 2;

/// What a caller writes down to start a session.
///
/// Plain data with public fields, deliberately: this is the thing an
/// application builds from a lobby, a command line, or a save file, and
/// making it a builder would buy nothing but ceremony. It is validated
/// once, by [`SessionParams::validate`], and the validated form is what
/// a session takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionParams {
    /// How many seats are playing. `MIN_PEERS..=MAX_PEERS`.
    pub peer_count: u8,
    /// Which seat this machine is. Must be inside the roster.
    ///
    /// **Excluded from the agreement digest**, and it is the only field
    /// that is: it differs per peer by definition, so folding it would
    /// make every peer disagree with every other at the handshake.
    pub local: PeerId,
    /// How wide one peer's input is, per tick. `1..=MAX_INPUT_BYTES`.
    pub input_bytes: u8,
    /// How many ticks ahead of the confirmed tick a peer submits.
    ///
    /// This is the whole latency knob. Zero means a tick cannot run until
    /// its own inputs have crossed the wire; higher values buy the
    /// network that many ticks of slack, at the cost of that many ticks
    /// between pressing a key and seeing it. Below [`INPUT_WINDOW`].
    pub input_delay: u8,
    /// How often a digest is exchanged, in ticks. At least one.
    ///
    /// A bandwidth knob that is really a bisect knob: a divergence is
    /// located to within this many ticks, so at sixty hertz and thirty,
    /// the window a human has to search is half a second of simulation.
    pub digest_period: u8,
    /// The run's master seed.
    pub seed: u64,
    /// What content every peer is running — assets, levels, tables.
    ///
    /// **The engine cannot compute this and never validates it.** It
    /// proves only that everyone supplied the same number. What that
    /// number means is the application's, exactly as the trace header
    /// keeps caller-owned keys verbatim without reading them.
    pub content: u64,
    /// What rules every peer is running, as distinct from what assets.
    /// Two numbers rather than one so a refusal can name which half.
    pub rules: u64,
    /// The session identifier, shared by every peer and never zero.
    ///
    /// It is an admission token in the weakest sense: a datagram carrying
    /// the wrong one is dropped at the header, so an unrelated session on
    /// the same port cannot reach this one. **It is not a secret and not
    /// a defence** — the engine has no entropy source, so whatever
    /// resistance it has is whatever the application's own source of it
    /// provides.
    pub session: core::num::NonZeroU64,
}

/// Why a set of parameters could not start a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParamsError {
    PeerCount {
        saw: u8,
        floor: u8,
        ceiling: u8,
    },
    /// The local seat is not in the roster it claims to be part of.
    LocalNotInRoster {
        local: u8,
        peer_count: u8,
    },
    InputBytes {
        saw: u8,
        ceiling: u8,
    },
    InputDelay {
        saw: u8,
        window: u32,
    },
    DigestPeriodZero,
}

impl core::fmt::Display for ParamsError {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::PeerCount {
                saw,
                floor,
                ceiling,
            } => write!(out, "a roster of {saw} is outside {floor}..={ceiling}"),
            Self::LocalNotInRoster { local, peer_count } => {
                write!(out, "seat {local} is not inside a roster of {peer_count}")
            }
            Self::InputBytes { saw, ceiling } => {
                write!(out, "an input width of {saw} is not within 1..={ceiling}")
            }
            Self::InputDelay { saw, window } => write!(
                out,
                "an input delay of {saw} does not fit a window of {window}"
            ),
            Self::DigestPeriodZero => {
                write!(out, "a digest period of zero would digest every tick")
            }
        }
    }
}

impl core::error::Error for ParamsError {}

impl SessionParams {
    /// Check every field, and fold the agreement digest once.
    ///
    /// # Errors
    ///
    /// [`ParamsError`] naming the first field that is out of range, in
    /// declaration order, so the message is stable rather than dependent
    /// on how many things are wrong.
    pub fn validate(self) -> Result<ValidParams, ParamsError> {
        if !(MIN_PEERS..=MAX_PEERS).contains(&self.peer_count) {
            return Err(ParamsError::PeerCount {
                saw: self.peer_count,
                floor: MIN_PEERS,
                ceiling: MAX_PEERS,
            });
        }
        if self.local.index() >= self.peer_count {
            return Err(ParamsError::LocalNotInRoster {
                local: self.local.index(),
                peer_count: self.peer_count,
            });
        }
        if self.input_bytes == 0 || self.input_bytes > MAX_INPUT_BYTES {
            return Err(ParamsError::InputBytes {
                saw: self.input_bytes,
                ceiling: MAX_INPUT_BYTES,
            });
        }
        if u32::from(self.input_delay) >= INPUT_WINDOW {
            return Err(ParamsError::InputDelay {
                saw: self.input_delay,
                window: INPUT_WINDOW,
            });
        }
        if self.digest_period == 0 {
            return Err(ParamsError::DigestPeriodZero);
        }
        Ok(ValidParams {
            params: self,
            agreement: self.agreement_digest(),
        })
    }

    /// The fingerprint every peer compares at the handshake.
    ///
    /// Absorbed in written order, so a reordering is a visible diff
    /// rather than a silently moved digest — the same discipline the
    /// frame crate's fingerprint is built on. **`local` is excluded and
    /// `session` is excluded**: the first differs per peer by definition,
    /// and the second cannot reach a confirmed frame, because two peers
    /// holding different ones drop each other's datagrams at the header.
    #[must_use]
    fn agreement_digest(self) -> u64 {
        StateHash::new()
            .absorb_u32(u32::from(WIRE_VERSION))
            .absorb_u64(self.content)
            .absorb_u64(self.rules)
            .absorb_u64(self.seed)
            .absorb_u32(u32::from(self.peer_count))
            .absorb_u32(u32::from(self.input_bytes))
            .absorb_u32(u32::from(self.input_delay))
            .absorb_u32(u32::from(self.digest_period))
            .finish()
    }
}

/// Parameters that have been checked, and the digest folded from them.
///
/// A session takes this rather than [`SessionParams`], so "has this been
/// validated?" is answered by the type instead of by a comment. The
/// inner value is readable and the digest is not recomputable from
/// outside — there is one fold, at one moment, and everything downstream
/// reads its result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidParams {
    params: SessionParams,
    agreement: u64,
}

impl ValidParams {
    #[must_use]
    pub const fn get(&self) -> &SessionParams {
        &self.params
    }

    /// The fingerprint every peer puts in its `Hello`.
    #[must_use]
    pub const fn agreement_digest(&self) -> u64 {
        self.agreement
    }

    /// Every seat in the session, as a set.
    #[must_use]
    pub const fn roster(&self) -> PeerSet {
        PeerSet::of_count(self.params.peer_count)
    }

    /// Every seat except this machine's — the peers a datagram goes to.
    #[must_use]
    pub const fn remotes(&self) -> PeerSet {
        self.roster().without(self.params.local)
    }

    #[must_use]
    pub const fn local(&self) -> PeerId {
        self.params.local
    }

    #[must_use]
    pub const fn peer_count(&self) -> u8 {
        self.params.peer_count
    }

    #[must_use]
    pub const fn input_bytes(&self) -> u8 {
        self.params.input_bytes
    }

    #[must_use]
    pub const fn input_delay(&self) -> u8 {
        self.params.input_delay
    }

    #[must_use]
    pub const fn digest_period(&self) -> u8 {
        self.params.digest_period
    }

    #[must_use]
    pub const fn session(&self) -> core::num::NonZeroU64 {
        self.params.session
    }
}
