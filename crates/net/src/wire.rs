//! The datagram codec: a pure function from bytes to a validated view,
//! and its exact inverse.
//!
//! **Everything decidable from the bytes alone is decided here.**
//! Session-relative refusals — wrong session, a sender outside *this*
//! roster, a tick past *this* window — belong to the session, which knows
//! the session. Keeping the split at that line is what makes this module
//! fuzzable and unit-testable with no session at all, and it is why a
//! `sender` past the type's ceiling is refused here while a `sender` who
//! is merely not playing is not.

use core::num::NonZeroU64;

use crate::{
    INPUT_REDUNDANCY, INPUT_WINDOW, MAX_DATAGRAM_BYTES, MAX_INPUT_BYTES, MAX_PEERS, PeerId,
};

/// Four bytes rather than the document format's eight: a datagram at
/// sixty hertz pays for every byte, and the session id below discriminates
/// far better than magic can.
pub const MAGIC: [u8; 4] = *b"RNWL";

/// Exactly one accepted value; everything else is refused outright.
///
/// Version negotiation is a writer's job, not a reader's. This number
/// moves when a byte layout or a vocabulary word moves, and **not** when
/// a caller adds a parameter — the trace codec's rule. It is folded into
/// the session's agreement digest, so a version skew becomes a named
/// handshake refusal at tick zero rather than a mystery at tick four
/// hundred.
pub const WIRE_VERSION: u16 = 1;

/// The bytes every datagram begins with, whatever its kind.
pub const HEADER_BYTES: usize = 16;

/// A `Hello` body.
pub const HELLO_BODY_BYTES: usize = 40;
/// An `Inputs` body's fixed part, before the frames it carries.
pub const INPUTS_BODY_BYTES: usize = 12;
/// A `Digest` body.
pub const DIGEST_BODY_BYTES: usize = 24;
/// A `Bye` body.
pub const BYE_BODY_BYTES: usize = 8;

/// A whole `Hello` datagram. Fixed: there is nothing in it that varies.
pub const HELLO_DATAGRAM_BYTES: usize = HEADER_BYTES + HELLO_BODY_BYTES;
/// A whole `Digest` datagram.
pub const DIGEST_DATAGRAM_BYTES: usize = HEADER_BYTES + DIGEST_BODY_BYTES;
/// A whole `Bye` datagram.
pub const BYE_DATAGRAM_BYTES: usize = HEADER_BYTES + BYE_BODY_BYTES;
/// The smallest an `Inputs` datagram can be: its fixed part, carrying no
/// frames at all — a shape [`read`] refuses, but only after proving the
/// fixed part is present, because the declared size is computed from two
/// bytes inside it.
pub const INPUTS_MIN_DATAGRAM_BYTES: usize = HEADER_BYTES + INPUTS_BODY_BYTES;

const MAGIC_AT: usize = 0;
const VERSION_AT: usize = 4;
const KIND_AT: usize = 6;
const SENDER_AT: usize = 7;
const SESSION_AT: usize = 8;

const HELLO_AGREEMENT_AT: usize = HEADER_BYTES;
const HELLO_CONTENT_AT: usize = HEADER_BYTES + 8;
const HELLO_RULES_AT: usize = HEADER_BYTES + 16;
const HELLO_SEED_AT: usize = HEADER_BYTES + 24;
const HELLO_PEER_COUNT_AT: usize = HEADER_BYTES + 32;
const HELLO_INPUT_BYTES_AT: usize = HEADER_BYTES + 33;
const HELLO_INPUT_DELAY_AT: usize = HEADER_BYTES + 34;
const HELLO_DIGEST_PERIOD_AT: usize = HEADER_BYTES + 35;
const HELLO_PAD_AT: usize = HEADER_BYTES + 36;
const HELLO_PAD_BYTES: usize = 4;

const INPUTS_FIRST_TICK_AT: usize = HEADER_BYTES;
const INPUTS_COUNT_AT: usize = HEADER_BYTES + 8;
const INPUTS_WIDTH_AT: usize = HEADER_BYTES + 9;
const INPUTS_PAD_AT: usize = HEADER_BYTES + 10;
const INPUTS_PAD_BYTES: usize = 2;
const INPUTS_FRAMES_AT: usize = HEADER_BYTES + INPUTS_BODY_BYTES;

const DIGEST_TICK_AT: usize = HEADER_BYTES;
const DIGEST_STATE_AT: usize = HEADER_BYTES + 8;
const DIGEST_INPUT_AT: usize = HEADER_BYTES + 16;

const BYE_TICK_AT: usize = HEADER_BYTES;

/// The smallest roster a session can be: one peer is not multiplayer, and
/// a `Hello` claiming it is malformed rather than lonely.
const MIN_PEER_COUNT: u8 = 2;

/// What a datagram is.
///
/// Discriminants start at one, so an all-zero buffer names no kind — a
/// zeroed page and a dropped connection are the two things most likely to
/// arrive by accident, and neither should decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// I am here, and these are the parameters I believe we agreed.
    Hello = 1,
    /// My inputs for a run of consecutive ticks, oldest first.
    Inputs = 2,
    /// What my world hashed to after a tick, and what my inputs hashed to.
    Digest = 3,
    /// I am leaving, at this tick.
    Bye = 4,
}

impl Kind {
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// `None` for an unknown code. Closed, never skipped: skipping is how
    /// a format silently forks.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Hello),
            2 => Some(Self::Inputs),
            3 => Some(Self::Digest),
            4 => Some(Self::Bye),
            _ => None,
        }
    }

    /// The body length of a datagram of this kind, given the record counts
    /// it declares.
    ///
    /// `None` when the product would not fit a `u64` — which the ceilings
    /// make unreachable, and which is therefore *checked* rather than
    /// assumed, because a size computation that trusts its own ceilings is
    /// the one that stops being true when a ceiling moves.
    #[must_use]
    #[allow(
        clippy::cast_lossless,
        reason = "`u64::from` is not callable in a const fn while const trait impls are unstable, so the widening is written as a cast — the same accommodation the frame crate's digest makes"
    )]
    pub const fn body_bytes(self, count: u8, input_bytes: u8) -> Option<u64> {
        match self {
            Self::Hello => Some(HELLO_BODY_BYTES as u64),
            Self::Digest => Some(DIGEST_BODY_BYTES as u64),
            Self::Bye => Some(BYE_BODY_BYTES as u64),
            Self::Inputs => match (count as u64).checked_mul(input_bytes as u64) {
                Some(frames) => (INPUTS_BODY_BYTES as u64).checked_add(frames),
                None => None,
            },
        }
    }
}

/// Who a datagram is from, and which session it belongs to.
///
/// **The kind is deliberately not here.** Every writer knows the kind it
/// writes, so taking one as an argument would let a caller ask
/// [`write_hello`] for a datagram whose kind byte says `Bye` — a
/// disagreement between which function was called and what the bytes
/// claim, minted by a writer and refused by the reader. Splitting the
/// addressing out makes that unrepresentable rather than merely refused,
/// which is the difference between a contract and a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addressing {
    /// The claimed sender, written into every kind.
    ///
    /// A session refuses any datagram whose sender disagrees with the seat
    /// its transport attributed the bytes to, which is why this is in the
    /// header rather than in the three bodies that would otherwise need
    /// it: a check in one place cannot be forgotten by a reader that only
    /// looks at a body.
    pub sender: PeerId,
    /// Never zero, **by type**.
    ///
    /// Zero is the pinned illegal value on the wire, so a zeroed buffer
    /// that somehow cleared magic and version still dies at the reader.
    /// Making it `NonZeroU64` here means no writer needs a refusal for it
    /// and no caller can hold one that would be refused.
    pub session: NonZeroU64,
}

/// The sixteen bytes every datagram begins with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    /// The claimed sender.
    pub sender: PeerId,
    /// Never zero — the reader proves it, so the type carries the proof.
    pub session: NonZeroU64,
}

impl Header {
    /// The half of this header a writer takes.
    #[must_use]
    pub const fn addressing(self) -> Addressing {
        Addressing {
            sender: self.sender,
            session: self.session,
        }
    }
}

/// The parameters a peer claims it is playing under.
///
/// Redundant with `agreement_digest` by construction, and carried anyway:
/// **the digest decides, the plaintext explains.** A refusal can then name
/// *which* parameter differs instead of only reporting that two numbers
/// did not match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelloBody {
    pub agreement_digest: u64,
    /// What content this peer is running. **This crate cannot compute one
    /// and never validates it**; it carries the number and lets a session
    /// prove everyone supplied the same one.
    pub content: u64,
    /// What rules this peer is running, as distinct from what assets. Two
    /// numbers rather than one so a refusal can name which half.
    pub rules: u64,
    /// The run's master seed.
    pub seed: u64,
    /// `2..=MAX_PEERS`.
    pub peer_count: u8,
    /// `1..=MAX_INPUT_BYTES`.
    pub input_bytes: u8,
    /// Strictly below `INPUT_WINDOW`.
    pub input_delay: u8,
    /// At least one.
    pub digest_period: u8,
}

/// A run of consecutive per-tick inputs from one peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputsBody<'a> {
    /// The tick the first frame belongs to.
    pub first_tick: u64,
    /// How many frames follow — the whole run, newest last.
    /// `1..=INPUT_REDUNDANCY`.
    pub count: u8,
    /// How wide each frame is. `1..=MAX_INPUT_BYTES`.
    pub input_bytes: u8,
    frames: &'a [u8],
}

impl<'a> InputsBody<'a> {
    /// The `index`-th frame, or `None` at or past [`InputsBody::count`].
    #[must_use]
    pub fn frame(&self, index: u8) -> Option<&'a [u8]> {
        if index >= self.count {
            return None;
        }
        let width = usize::from(self.input_bytes);
        let start = usize::from(index).checked_mul(width)?;
        self.frames.get(start..)?.get(..width)
    }

    /// Every frame as `(tick, bytes)`, ascending from
    /// [`InputsBody::first_tick`].
    pub fn iter(&self) -> impl Iterator<Item = (u64, &'a [u8])> + 'a {
        let first = self.first_tick;
        let count = self.count;
        let width = usize::from(self.input_bytes);
        let frames = self.frames;
        (0..u32::from(count)).filter_map(move |index| {
            let start = usize::try_from(index).ok()?.checked_mul(width)?;
            let bytes = frames.get(start..)?.get(..width)?;
            Some((first.checked_add(u64::from(index))?, bytes))
        })
    }

    /// The frame bytes as one run, for a caller that wants to copy them
    /// whole rather than one tick at a time.
    #[must_use]
    pub const fn frames(&self) -> &'a [u8] {
        self.frames
    }
}

/// One peer's fingerprints for one tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DigestBody {
    pub tick: u64,
    /// The consuming simulation's own digest after `tick`.
    pub state_digest: u64,
    /// This peer's running fold of every confirmed input set up to and
    /// including `tick`. Eight bytes, and they are the difference between
    /// "we diverged" and "we diverged, and here is which half".
    pub input_digest: u64,
}

/// A departure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByeBody {
    /// The last tick this peer confirmed.
    ///
    /// Tick-addressed on purpose: a departure naming no tick would leave
    /// every remaining peer to guess when it happened, which is the
    /// consensus problem this design declines to solve.
    pub tick: u64,
}

/// What a validated datagram turned out to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Body<'a> {
    Hello(HelloBody),
    Inputs(InputsBody<'a>),
    Digest(DigestBody),
    Bye(ByeBody),
}

/// A validated datagram, borrowing the bytes it was read from.
///
/// Validation happens once, at [`read`]. Every accessor below it is total
/// because of that, and nothing here re-checks what the reader proved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Datagram<'a> {
    pub header: Header,
    pub body: Body<'a>,
}

/// Why a datagram was refused. One variant per rule, each carrying what
/// was seen — "invalid" teaches a reader nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WireError {
    /// Shorter than the region a reader needed.
    ///
    /// Raised by the header-length check, and — unreachably, by
    /// construction — by any later read whose region the size check has
    /// already proven present. That second road exists so an impossible
    /// miss becomes a refusal rather than a panic: fail closed, not
    /// sideways.
    TooShort {
        len: usize,
    },
    /// Longer than any legal datagram, checked before anything that could
    /// depend on a length.
    TooLong {
        len: usize,
    },
    BadMagic {
        saw: [u8; 4],
    },
    BadVersion {
        saw: u16,
    },
    UnknownKind {
        saw: u8,
    },
    /// The claimed sender is at or past `MAX_PEERS`. Decidable from the
    /// bytes; whether that seat is in *this session's* roster is not, and
    /// belongs to the session.
    SenderPastCeiling {
        saw: u8,
        ceiling: u8,
    },
    /// Zero is the pinned illegal session id.
    SessionZero,
    /// The declared datagram length is not the actual one.
    ///
    /// Equality, never a lower bound: trailing bytes are a refusal,
    /// because a format that tolerates them admits two spellings of one
    /// fact. The arithmetic is done in `u64` so the check does not depend
    /// on the ceilings staying small forever.
    SizeMismatch {
        kind: Kind,
        declared: u64,
        actual: usize,
    },
    /// A byte the semantics do not read was not zero.
    ///
    /// Every reserved region is pinned, which is what stops a second
    /// spelling of the same datagram existing — and closes the
    /// covert-channel road as a side effect.
    PadNotZero {
        offset: usize,
        saw: u8,
    },
    /// An `Inputs` run of zero frames: a datagram that says nothing and
    /// costs a parse. Refused rather than accepted as empty.
    FrameCountZero,
    FrameCountPastRedundancy {
        saw: u8,
        ceiling: u8,
    },
    InputBytesZero,
    InputBytesPastCeiling {
        saw: u8,
        ceiling: u8,
    },
    /// `first_tick + count` would leave `u64`.
    TickOverflow {
        first_tick: u64,
        count: u8,
    },
    /// A `Hello` declaring a roster this crate cannot hold, or one too
    /// small to be a session at all.
    PeerCountOutOfRange {
        saw: u8,
        floor: u8,
        ceiling: u8,
    },
    /// A `Hello` declaring a delay this crate cannot buffer.
    InputDelayPastWindow {
        saw: u8,
        window: u32,
    },
    /// A `Hello` declaring a zero digest period: every tick would owe a
    /// digest, which is a bandwidth decision nobody makes on purpose and a
    /// division by zero if it were honoured.
    DigestPeriodZero,
}

impl core::fmt::Display for WireError {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::TooShort { len } => {
                write!(
                    out,
                    "{len} bytes is shorter than the {HEADER_BYTES}-byte header"
                )
            }
            Self::TooLong { len } => {
                write!(
                    out,
                    "{len} bytes is longer than the {MAX_DATAGRAM_BYTES}-byte ceiling"
                )
            }
            Self::BadMagic { saw } => write!(out, "magic {saw:?}, expected {MAGIC:?}"),
            Self::BadVersion { saw } => {
                write!(
                    out,
                    "wire version {saw}, and only {WIRE_VERSION} is accepted"
                )
            }
            Self::UnknownKind { saw } => write!(out, "datagram kind {saw} names nothing"),
            Self::SenderPastCeiling { saw, ceiling } => {
                write!(
                    out,
                    "sender seat {saw} is at or past the ceiling of {ceiling}"
                )
            }
            Self::SessionZero => write!(out, "session id zero is the pinned illegal value"),
            Self::SizeMismatch {
                kind,
                declared,
                actual,
            } => write!(
                out,
                "a {kind:?} declaring {declared} bytes arrived as {actual}"
            ),
            Self::PadNotZero { offset, saw } => {
                write!(
                    out,
                    "reserved byte at offset {offset} is {saw}, and must be zero"
                )
            }
            Self::FrameCountZero => write!(out, "an inputs run of zero frames says nothing"),
            Self::FrameCountPastRedundancy { saw, ceiling } => {
                write!(
                    out,
                    "{saw} frames is past the redundancy ceiling of {ceiling}"
                )
            }
            Self::InputBytesZero => write!(out, "an input width of zero carries no input"),
            Self::InputBytesPastCeiling { saw, ceiling } => {
                write!(
                    out,
                    "an input width of {saw} is past the ceiling of {ceiling}"
                )
            }
            Self::TickOverflow { first_tick, count } => {
                write!(
                    out,
                    "{count} frames from tick {first_tick} would leave the tick space"
                )
            }
            Self::PeerCountOutOfRange {
                saw,
                floor,
                ceiling,
            } => {
                write!(out, "a roster of {saw} is outside {floor}..={ceiling}")
            }
            Self::InputDelayPastWindow { saw, window } => {
                write!(
                    out,
                    "an input delay of {saw} does not fit a window of {window}"
                )
            }
            Self::DigestPeriodZero => {
                write!(out, "a digest period of zero would digest every tick")
            }
        }
    }
}

impl core::error::Error for WireError {}

/// Why a writer refused. Four rules, each carrying what was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// Zero frames, or more than the redundancy ceiling. One variant for
    /// both because they are one mistake: a run that is not a run.
    FrameCount {
        saw: u8,
        ceiling: u8,
    },
    /// Zero width, or more than the input ceiling.
    InputBytes {
        saw: u8,
        ceiling: u8,
    },
    FramesLength {
        saw: usize,
        expected: usize,
    },
    TickOverflow {
        first_tick: u64,
        count: u8,
    },
    /// A roster the reader would refuse: below two, or past the ceiling.
    PeerCount {
        saw: u8,
        floor: u8,
        ceiling: u8,
    },
    /// A delay the input window could not buffer.
    InputDelay {
        saw: u8,
        window: u32,
    },
    /// Every tick would owe a digest, and the period would divide by zero.
    DigestPeriodZero,
}

impl core::fmt::Display for WriteError {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::FrameCount { saw, ceiling } => {
                write!(out, "{saw} frames is not within 1..={ceiling}")
            }
            Self::InputBytes { saw, ceiling } => {
                write!(out, "an input width of {saw} is not within 1..={ceiling}")
            }
            Self::FramesLength { saw, expected } => {
                write!(
                    out,
                    "{saw} bytes of frames where the counts imply {expected}"
                )
            }
            Self::TickOverflow { first_tick, count } => {
                write!(
                    out,
                    "{count} frames from tick {first_tick} would leave the tick space"
                )
            }
            Self::PeerCount {
                saw,
                floor,
                ceiling,
            } => {
                write!(out, "a roster of {saw} is outside {floor}..={ceiling}")
            }
            Self::InputDelay { saw, window } => {
                write!(
                    out,
                    "an input delay of {saw} does not fit a window of {window}"
                )
            }
            Self::DigestPeriodZero => {
                write!(out, "a digest period of zero would digest every tick")
            }
        }
    }
}

impl core::error::Error for WriteError {}

/// Read a datagram.
///
/// Everything decidable from the bytes alone, in the order a reader should
/// decide it: cheapest and most discriminating first, and region slicing
/// only after a size equality has proven both bounds.
///
/// **This function is total over every possible byte string.** It
/// allocates nothing, panics on nothing, and touches no state.
///
/// # Errors
///
/// One [`WireError`] variant per rule, each carrying what was seen.
pub fn read(bytes: &[u8]) -> Result<Datagram<'_>, WireError> {
    let len = bytes.len();
    if len < HEADER_BYTES {
        return Err(WireError::TooShort { len });
    }
    if len > MAX_DATAGRAM_BYTES {
        return Err(WireError::TooLong { len });
    }

    let magic = region(bytes, MAGIC_AT, MAGIC.len()).ok_or(WireError::TooShort { len })?;
    if magic != MAGIC {
        let mut saw = [0u8; 4];
        // Proven four bytes wide one line above; a mismatch here would be
        // the slice helper lying, so it fails closed onto the same refusal.
        if let Some(seen) = magic.get(..4) {
            saw.copy_from_slice(seen);
        }
        return Err(WireError::BadMagic { saw });
    }

    let version = u16_at(bytes, VERSION_AT).ok_or(WireError::TooShort { len })?;
    if version != WIRE_VERSION {
        return Err(WireError::BadVersion { saw: version });
    }

    let kind_code = byte_at(bytes, KIND_AT).ok_or(WireError::TooShort { len })?;
    let kind = Kind::from_code(kind_code).ok_or(WireError::UnknownKind { saw: kind_code })?;

    let sender_index = byte_at(bytes, SENDER_AT).ok_or(WireError::TooShort { len })?;
    let sender = PeerId::new(sender_index).ok_or(WireError::SenderPastCeiling {
        saw: sender_index,
        ceiling: MAX_PEERS,
    })?;

    let claimed = u64_at(bytes, SESSION_AT).ok_or(WireError::TooShort { len })?;
    let session = NonZeroU64::new(claimed).ok_or(WireError::SessionZero)?;

    let header = Header {
        kind,
        sender,
        session,
    };
    let body = match kind {
        Kind::Hello => Body::Hello(read_hello(bytes, len)?),
        Kind::Inputs => Body::Inputs(read_inputs(bytes, len)?),
        Kind::Digest => Body::Digest(read_digest(bytes, len)?),
        Kind::Bye => Body::Bye(read_bye(bytes, len)?),
    };
    Ok(Datagram { header, body })
}

fn read_hello(bytes: &[u8], len: usize) -> Result<HelloBody, WireError> {
    expect_exactly(Kind::Hello, HELLO_DATAGRAM_BYTES, len)?;

    let peer_count = byte_at(bytes, HELLO_PEER_COUNT_AT).ok_or(WireError::TooShort { len })?;
    if !(MIN_PEER_COUNT..=MAX_PEERS).contains(&peer_count) {
        return Err(WireError::PeerCountOutOfRange {
            saw: peer_count,
            floor: MIN_PEER_COUNT,
            ceiling: MAX_PEERS,
        });
    }

    let input_bytes = byte_at(bytes, HELLO_INPUT_BYTES_AT).ok_or(WireError::TooShort { len })?;
    check_input_width(input_bytes)?;

    let input_delay = byte_at(bytes, HELLO_INPUT_DELAY_AT).ok_or(WireError::TooShort { len })?;
    if u32::from(input_delay) >= INPUT_WINDOW {
        return Err(WireError::InputDelayPastWindow {
            saw: input_delay,
            window: INPUT_WINDOW,
        });
    }

    let digest_period =
        byte_at(bytes, HELLO_DIGEST_PERIOD_AT).ok_or(WireError::TooShort { len })?;
    if digest_period == 0 {
        return Err(WireError::DigestPeriodZero);
    }

    expect_zeroes(bytes, HELLO_PAD_AT, HELLO_PAD_BYTES)?;

    Ok(HelloBody {
        agreement_digest: u64_at(bytes, HELLO_AGREEMENT_AT).ok_or(WireError::TooShort { len })?,
        content: u64_at(bytes, HELLO_CONTENT_AT).ok_or(WireError::TooShort { len })?,
        rules: u64_at(bytes, HELLO_RULES_AT).ok_or(WireError::TooShort { len })?,
        seed: u64_at(bytes, HELLO_SEED_AT).ok_or(WireError::TooShort { len })?,
        peer_count,
        input_bytes,
        input_delay,
        digest_period,
    })
}

fn read_inputs(bytes: &[u8], len: usize) -> Result<InputsBody<'_>, WireError> {
    // The declared size is a function of two bytes inside the fixed part,
    // so the fixed part's presence is proven before either is read. This
    // is the one kind whose length check cannot come first, and the
    // ordering is stated here rather than left to be re-derived.
    if len < INPUTS_MIN_DATAGRAM_BYTES {
        return Err(WireError::SizeMismatch {
            kind: Kind::Inputs,
            declared: widen(INPUTS_MIN_DATAGRAM_BYTES),
            actual: len,
        });
    }

    let count = byte_at(bytes, INPUTS_COUNT_AT).ok_or(WireError::TooShort { len })?;
    if count == 0 {
        return Err(WireError::FrameCountZero);
    }
    if count > INPUT_REDUNDANCY {
        return Err(WireError::FrameCountPastRedundancy {
            saw: count,
            ceiling: INPUT_REDUNDANCY,
        });
    }

    let input_bytes = byte_at(bytes, INPUTS_WIDTH_AT).ok_or(WireError::TooShort { len })?;
    check_input_width(input_bytes)?;

    // Both ceilings are proven above, so the product cannot overflow — and
    // it is computed in `u64` and checked anyway, because a size
    // computation that trusts its own ceilings stops being true the day
    // one of them moves.
    let body = Kind::Inputs
        .body_bytes(count, input_bytes)
        .ok_or(WireError::SizeMismatch {
            kind: Kind::Inputs,
            declared: u64::MAX,
            actual: len,
        })?;
    let declared = widen(HEADER_BYTES)
        .checked_add(body)
        .ok_or(WireError::SizeMismatch {
            kind: Kind::Inputs,
            declared: u64::MAX,
            actual: len,
        })?;
    if declared != widen(len) {
        return Err(WireError::SizeMismatch {
            kind: Kind::Inputs,
            declared,
            actual: len,
        });
    }

    expect_zeroes(bytes, INPUTS_PAD_AT, INPUTS_PAD_BYTES)?;

    let first_tick = u64_at(bytes, INPUTS_FIRST_TICK_AT).ok_or(WireError::TooShort { len })?;
    if first_tick.checked_add(u64::from(count)).is_none() {
        return Err(WireError::TickOverflow { first_tick, count });
    }

    let width = usize::from(count)
        .checked_mul(usize::from(input_bytes))
        .ok_or(WireError::SizeMismatch {
            kind: Kind::Inputs,
            declared,
            actual: len,
        })?;
    let frames = region(bytes, INPUTS_FRAMES_AT, width).ok_or(WireError::TooShort { len })?;

    Ok(InputsBody {
        first_tick,
        count,
        input_bytes,
        frames,
    })
}

fn read_digest(bytes: &[u8], len: usize) -> Result<DigestBody, WireError> {
    expect_exactly(Kind::Digest, DIGEST_DATAGRAM_BYTES, len)?;
    Ok(DigestBody {
        tick: u64_at(bytes, DIGEST_TICK_AT).ok_or(WireError::TooShort { len })?,
        state_digest: u64_at(bytes, DIGEST_STATE_AT).ok_or(WireError::TooShort { len })?,
        input_digest: u64_at(bytes, DIGEST_INPUT_AT).ok_or(WireError::TooShort { len })?,
    })
}

fn read_bye(bytes: &[u8], len: usize) -> Result<ByeBody, WireError> {
    expect_exactly(Kind::Bye, BYE_DATAGRAM_BYTES, len)?;
    Ok(ByeBody {
        tick: u64_at(bytes, BYE_TICK_AT).ok_or(WireError::TooShort { len })?,
    })
}

fn check_input_width(input_bytes: u8) -> Result<(), WireError> {
    if input_bytes == 0 {
        return Err(WireError::InputBytesZero);
    }
    if input_bytes > MAX_INPUT_BYTES {
        return Err(WireError::InputBytesPastCeiling {
            saw: input_bytes,
            ceiling: MAX_INPUT_BYTES,
        });
    }
    Ok(())
}

fn expect_exactly(kind: Kind, declared: usize, actual: usize) -> Result<(), WireError> {
    if declared == actual {
        Ok(())
    } else {
        Err(WireError::SizeMismatch {
            kind,
            declared: widen(declared),
            actual,
        })
    }
}

fn expect_zeroes(bytes: &[u8], offset: usize, count: usize) -> Result<(), WireError> {
    let pad = region(bytes, offset, count).ok_or(WireError::TooShort { len: bytes.len() })?;
    for (step, byte) in pad.iter().enumerate() {
        if *byte != 0 {
            return Err(WireError::PadNotZero {
                offset: offset.saturating_add(step),
                saw: *byte,
            });
        }
    }
    Ok(())
}

fn region(bytes: &[u8], offset: usize, count: usize) -> Option<&[u8]> {
    bytes.get(offset..)?.get(..count)
}

fn byte_at(bytes: &[u8], offset: usize) -> Option<u8> {
    bytes.get(offset).copied()
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        region(bytes, offset, 2)?.try_into().ok()?,
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        region(bytes, offset, 8)?.try_into().ok()?,
    ))
}

/// A length as the size arithmetic sees it. Saturating rather than
/// panicking, and upward: an impossible conversion becomes a size that
/// matches nothing, which the equality check then refuses.
fn widen(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Fills a caller's buffer in order, and refuses to write past its end.
///
/// A region that does not fit is not written and does not advance the
/// cursor, so an impossible miss yields a **short** datagram that [`read`]
/// refuses on its size equality — never a full-length one carrying a hole.
/// Fail closed, not sideways.
struct Cursor<'a> {
    out: &'a mut [u8],
    at: usize,
}

impl Cursor<'_> {
    fn bytes(&mut self, source: &[u8]) {
        if let Some(tail) = self.out.get_mut(self.at..)
            && let Some(slot) = tail.get_mut(..source.len())
        {
            slot.copy_from_slice(source);
            self.at = self.at.saturating_add(source.len());
        }
    }

    fn byte(&mut self, value: u8) {
        self.bytes(&[value]);
    }

    fn u16(&mut self, value: u16) {
        self.bytes(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn zeroes(&mut self, count: usize) {
        const BLANK: [u8; 8] = [0; 8];
        if let Some(source) = BLANK.get(..count) {
            self.bytes(source);
        }
    }

    /// The kind is the writer's, never the caller's — see [`Addressing`].
    fn header(&mut self, kind: Kind, addressing: Addressing) {
        self.bytes(&MAGIC);
        self.u16(WIRE_VERSION);
        self.byte(kind.code());
        self.byte(addressing.sender.index());
        self.u64(addressing.session.get());
    }
}

/// Write a `Hello`, returning the byte count written.
///
/// # Errors
///
/// [`WriteError`] for each of the four parameter ranges [`read`] enforces
/// on a `Hello`. They are checked here rather than left to a caller
/// because the alternative is a writer that mints a datagram the reader
/// refuses, which would make this crate's central claim false — and a
/// claim under a **Contract** heading is intent, so the code moves to meet
/// it rather than the sentence retreating to meet the code.
pub fn write_hello(
    out: &mut [u8; MAX_DATAGRAM_BYTES],
    addressing: Addressing,
    body: &HelloBody,
) -> Result<usize, WriteError> {
    if !(MIN_PEER_COUNT..=MAX_PEERS).contains(&body.peer_count) {
        return Err(WriteError::PeerCount {
            saw: body.peer_count,
            floor: MIN_PEER_COUNT,
            ceiling: MAX_PEERS,
        });
    }
    if body.input_bytes == 0 || body.input_bytes > MAX_INPUT_BYTES {
        return Err(WriteError::InputBytes {
            saw: body.input_bytes,
            ceiling: MAX_INPUT_BYTES,
        });
    }
    if u32::from(body.input_delay) >= INPUT_WINDOW {
        return Err(WriteError::InputDelay {
            saw: body.input_delay,
            window: INPUT_WINDOW,
        });
    }
    if body.digest_period == 0 {
        return Err(WriteError::DigestPeriodZero);
    }

    let mut cursor = Cursor { out, at: 0 };
    cursor.header(Kind::Hello, addressing);
    cursor.u64(body.agreement_digest);
    cursor.u64(body.content);
    cursor.u64(body.rules);
    cursor.u64(body.seed);
    cursor.byte(body.peer_count);
    cursor.byte(body.input_bytes);
    cursor.byte(body.input_delay);
    cursor.byte(body.digest_period);
    cursor.zeroes(HELLO_PAD_BYTES);
    Ok(cursor.at)
}

/// Write a `Digest`, returning the byte count written.
///
/// Nothing here can be refused: the kind is this function's, the session
/// is non-zero by type, and both remaining fields are opaque `u64`s the
/// reader accepts whatever their value. That is what "enforced in the
/// argument types where it can be" buys, and it is why this one returns no
/// `Result` while [`write_hello`] does.
#[must_use]
pub fn write_digest(
    out: &mut [u8; MAX_DATAGRAM_BYTES],
    addressing: Addressing,
    body: &DigestBody,
) -> usize {
    let mut cursor = Cursor { out, at: 0 };
    cursor.header(Kind::Digest, addressing);
    cursor.u64(body.tick);
    cursor.u64(body.state_digest);
    cursor.u64(body.input_digest);
    cursor.at
}

/// Write a `Bye`, returning the byte count written. Unrefusable for the
/// same reason as [`write_digest`].
#[must_use]
pub fn write_bye(
    out: &mut [u8; MAX_DATAGRAM_BYTES],
    addressing: Addressing,
    body: &ByeBody,
) -> usize {
    let mut cursor = Cursor { out, at: 0 };
    cursor.header(Kind::Bye, addressing);
    cursor.u64(body.tick);
    cursor.at
}

/// Write an `Inputs` run, returning the byte count written.
///
/// `frames` is exactly `count × input_bytes` bytes, ascending from
/// `first_tick`. **`count` precedes `input_bytes`** — the order the two
/// bytes appear in on the wire, the order [`InputsBody`] declares them,
/// and the order [`Kind::body_bytes`] takes them; two `u8`s whose swap no
/// compiler can catch are worth spelling the same way everywhere.
///
/// # Errors
///
/// [`WriteError`] when the arguments could not produce a datagram the
/// reader would accept. **Refused rather than truncated**, because a
/// writer that silently produced a shorter run would be a second spelling
/// of a shorter fact — and the reader's whole canonical-encoding argument
/// rests on there being no second spelling of anything.
pub fn write_inputs(
    out: &mut [u8; MAX_DATAGRAM_BYTES],
    addressing: Addressing,
    first_tick: u64,
    count: u8,
    input_bytes: u8,
    frames: &[u8],
) -> Result<usize, WriteError> {
    if count == 0 || count > INPUT_REDUNDANCY {
        return Err(WriteError::FrameCount {
            saw: count,
            ceiling: INPUT_REDUNDANCY,
        });
    }
    if input_bytes == 0 || input_bytes > MAX_INPUT_BYTES {
        return Err(WriteError::InputBytes {
            saw: input_bytes,
            ceiling: MAX_INPUT_BYTES,
        });
    }
    let expected = usize::from(count)
        .checked_mul(usize::from(input_bytes))
        .ok_or(WriteError::FramesLength {
            saw: frames.len(),
            expected: usize::MAX,
        })?;
    if frames.len() != expected {
        return Err(WriteError::FramesLength {
            saw: frames.len(),
            expected,
        });
    }
    if first_tick.checked_add(u64::from(count)).is_none() {
        return Err(WriteError::TickOverflow { first_tick, count });
    }

    let mut cursor = Cursor { out, at: 0 };
    cursor.header(Kind::Inputs, addressing);
    cursor.u64(first_tick);
    cursor.byte(count);
    cursor.byte(input_bytes);
    cursor.zeroes(INPUTS_PAD_BYTES);
    cursor.bytes(frames);
    Ok(cursor.at)
}
