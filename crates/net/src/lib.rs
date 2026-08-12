//! The lockstep datagram codec: a self-describing wire format for
//! inputs-only multiplayer, and a reader that refuses everything it does
//! not understand.
//!
//! A world too large to replicate as state is nearly free to replicate as
//! "everybody pressed these buttons". That is the whole premise, and this
//! crate is the half of it that turns button presses into bytes and back
//! without ever learning what a button means.
//!
//! ```
//! use renew_net::{PeerId, wire};
//!
//! let sender = PeerId::new(0).expect("seat zero is always in range");
//! let header = wire::Header { kind: wire::Kind::Bye, sender, session: 7 };
//!
//! let mut out = [0u8; renew_net::MAX_DATAGRAM_BYTES];
//! let len = wire::write_bye(&mut out, header, &wire::ByeBody { tick: 900 });
//!
//! let read = wire::read(&out[..len]).expect("a datagram this crate wrote");
//! assert_eq!(read.header, header);
//! ```
//!
//! # Contract
//!
//! - **State never crosses this wire.** Only inputs do. What a legal
//!   input *is* remains the game's question: inputs are opaque
//!   fixed-width bytes here, exactly as the trace codec declines to know
//!   what a seed does.
//! - **Every frame is addressed by an absolute tick**, never by a delta
//!   and never by a sequence number. Arrival order, duplication and loss
//!   therefore cannot reach any value this crate hands out.
//! - **One byte string per fact.** The format admits exactly one spelling
//!   of anything it can carry, and [`wire::read`] proves that rather than
//!   trusting it: length equality rather than a lower bound, one accepted
//!   version, closed enumerations, and every byte the semantics do not
//!   read proven zero.
//! - **The reader is total.** It is a pure function over every possible
//!   byte string. It allocates nothing, panics on nothing, and holds no
//!   state. Everything decidable from the bytes alone is decided here;
//!   everything session-relative — whether this is *our* session, whether
//!   that seat is in *our* roster — belongs to the session, which knows
//!   the session.
//! - **A writer cannot mint what the reader would refuse.** Every ceiling
//!   the reader enforces, a writer enforces first, in its argument types
//!   where it can and in a refusal where it cannot.
//! - **This crate owns no socket, reads no clock, and spawns nothing.**
//!   It cannot: it declares `simulation = true`, which denies it a
//!   dependency path to the platform crate at any depth, in any
//!   dependency kind. Bytes arrive as slices and leave as slices.
//!
//! Machine-readable facts about this crate — maturity, dependencies, core
//! status, whether it is simulation code — live in `Cargo.toml` under
//! `[package.metadata.renew]`, which is authoritative. This file does not
//! restate them.

// The engine-crate pair: diagnostics leave through sinks, never through a
// process's standard streams.
#![deny(clippy::print_stdout, clippy::print_stderr)]
// Required today by nothing that runs: the float-closure rule asks a
// crate a simulation *reaches* for this deny and never asks the
// declaring crate itself. Written by hand until that rule gains the half
// it is missing.
#![deny(clippy::float_arithmetic)]
// Every byte this crate reads can come off a hostile wire, and a release
// build aborts on panic. An unchecked index is therefore a remote process
// abort and an unchecked add is a remote abort in debug and a wrong
// answer in release — and the workspace lint table denies neither. Both
// bans are load-bearing rather than stylistic, which is why they sit at
// the root instead of at the two functions that obviously need them.
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects)]

mod peer;
pub mod wire;

pub use peer::{PeerId, PeerSet, peers};

/// The most peers one session may hold.
///
/// Eight is the roster co-operative play is built for, and it is a **type
/// decision rather than a tunable**: [`PeerSet`] is one byte because of
/// it, and so is a tick's arrival mask. Nine peers is a change to those
/// types, not an edit to this number, and the README says so where a
/// reader will meet it.
pub const MAX_PEERS: u8 = 8;

/// The widest input one peer may submit per tick.
///
/// A ceiling, not a target: two trits and three bits fit in one byte, and
/// a quantised aim pair adds two. The reader refuses past it *before
/// multiplying anything by anything*.
pub const MAX_INPUT_BYTES: u8 = 16;

/// How many past frames every `Inputs` datagram repeats, and therefore
/// how many consecutive losses of one peer's stream cost nothing.
///
/// This is the whole of the loss story: no acknowledgements, no
/// retransmit requests, no sequence windows. Seven bytes of tail on a
/// one-byte input repairs seven consecutive losses with zero round trips,
/// where a retransmit protocol would spend a round trip recovering data
/// that has already expired.
pub const INPUT_REDUNDANCY: u8 = 8;

/// The depth of a peer's input ring, and the ceiling on how far ahead of
/// the pending tick that peer's inputs may be buffered.
///
/// Flow control and memory bound in one: a frame past it is refused
/// rather than stored, which is what stops the fastest machine spending
/// the slowest machine's memory. **A power of two on purpose** — a ring
/// index is `tick & (INPUT_WINDOW - 1)`, and `%` would trip this crate's
/// arithmetic deny where a mask does not.
pub const INPUT_WINDOW: u32 = 64;

/// The largest datagram this protocol can produce, in bytes.
///
/// Derived, not chosen: a header plus the widest body, which is an
/// `Inputs` datagram at both of its ceilings. A unit test asserts the
/// composition, so raising a ceiling cannot silently mint a datagram no
/// path carries.
pub const MAX_DATAGRAM_BYTES: usize = wire::HEADER_BYTES
    + wire::INPUTS_BODY_BYTES
    + (INPUT_REDUNDANCY as usize) * (MAX_INPUT_BYTES as usize);

/// The smallest maximum transmission unit this protocol assumes a path
/// will carry, in bytes: the IPv6 minimum, less generous room for headers
/// and tunnels.
///
/// **Nothing enforces this at run time** — there is no path-MTU discovery
/// here, and a datagram is never fragmented by this crate. It exists so
/// that a unit test can hold [`MAX_DATAGRAM_BYTES`] against it, because a
/// ceiling raised past this line would produce datagrams that vanish on
/// some paths and arrive on others, which is the worst failure this
/// protocol could have: one that looks like a bug in the simulation.
pub const MTU_FLOOR: usize = 1200;

// Two facts a test could only report after the build that broke them
// succeeded. They are asserted where they are decided instead: raising a
// ceiling past either of these lines fails compilation, with the reason
// printed at the constant that caused it.
const _: () = assert!(
    MAX_DATAGRAM_BYTES <= MTU_FLOOR,
    "a datagram wider than the smallest assumed path would vanish on some routes and arrive on \
     others, which reads as a bug in the simulation rather than one in the network"
);
const _: () = assert!(
    MAX_PEERS <= 8,
    "PeerSet and a tick's arrival mask are each one byte; a ninth seat is a change to those types, \
     not an edit to this constant"
);

#[cfg(test)]
mod tests {
    use super::{INPUT_REDUNDANCY, MAX_DATAGRAM_BYTES, MAX_INPUT_BYTES, wire};

    #[test]
    fn the_widest_datagram_is_the_sum_of_its_parts() {
        let widest = wire::HEADER_BYTES
            + wire::INPUTS_BODY_BYTES
            + usize::from(INPUT_REDUNDANCY) * usize::from(MAX_INPUT_BYTES);
        assert_eq!(
            MAX_DATAGRAM_BYTES, widest,
            "the ceiling and the composition it is derived from have parted"
        );
        assert_eq!(
            MAX_DATAGRAM_BYTES, 156,
            "the number a reader can check by hand"
        );
    }
}
