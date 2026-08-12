# renew-net

The lockstep datagram codec: a self-describing wire format for inputs-only
multiplayer, and a reader that refuses everything it does not understand.

A world too large to replicate as state is nearly free to replicate as
"everybody pressed these buttons". That is the whole premise of lockstep,
and this crate is the half of it that turns button presses into bytes and
back without ever learning what a button means.

```rust
use renew_net::{MAX_DATAGRAM_BYTES, PeerId, wire};

let sender = PeerId::new(0).expect("seat zero is always in range");
let header = wire::Header { kind: wire::Kind::Inputs, sender, session: 0x51e3 };

// Three ticks of one-byte inputs, sent as one datagram. The last eight
// frames ride along in every send, so eight consecutive losses of this
// peer's stream cost nothing and no round trip recovers them.
let mut out = [0u8; MAX_DATAGRAM_BYTES];
let len = wire::write_inputs(&mut out, header, 4_000, 1, 3, &[0b0001, 0b0001, 0b0101])?;

let wire::Body::Inputs(body) = wire::read(&out[..len])?.body else { unreachable!() };
for (tick, frame) in body.iter() {
    // (4000, [0b0001]), (4001, [0b0001]), (4002, [0b0101])
    let _ = (tick, frame);
}
# Ok::<(), Box<dyn core::error::Error>>(())
```

## Status

**`bootstrap`.** Interface churn expected; breaking its API costs nothing
yet. What exists today is the wire format alone — the session state
machine, the input ring and the digest exchange arrive next, and the
socket that carries any of it is not here and never will be (see *The
socket is somewhere else*).

The manifest in [`Cargo.toml`](Cargo.toml) is authoritative for maturity,
dependencies and core status. This file does not restate them.

## Contract

- **State never crosses this wire.** Only inputs do. What a legal input
  *is* remains the game's question: inputs are opaque fixed-width bytes
  here.
- **Every frame is addressed by an absolute tick**, never a delta and
  never a sequence number. Arrival order, duplication and loss cannot
  reach any value this crate hands out.
- **One byte string per fact.** The format admits exactly one spelling of
  anything it can carry, and `read` *proves* that rather than trusting it:
  length equality rather than a lower bound, one accepted version, closed
  enumerations, and every reserved byte proven zero. A test flips every
  bit of every byte of every kind and asserts that no mutation yields a
  second spelling of the same datagram.
- **The reader is total** over every possible byte string. It allocates
  nothing, panics on nothing, and holds no state.
- **A writer cannot mint what the reader would refuse** — the ceilings are
  enforced in argument types where they can be, and in a refusal where
  they cannot. `write_inputs` refuses rather than truncating, because a
  silently shorter run would be a second spelling of a shorter fact.
- **This crate owns no socket, reads no clock, and spawns nothing.**

## Ceilings, and which of them are types

| | | |
|---|---|---|
| `MAX_PEERS` | 8 | **a type decision.** `PeerSet` is one byte because of it, and so is a tick's arrival mask. A ninth seat is a change to those types, not an edit to a number |
| `MAX_INPUT_BYTES` | 16 | the widest one peer's input may be, per tick |
| `INPUT_REDUNDANCY` | 8 | how many past frames every datagram repeats — the whole of the loss story |
| `INPUT_WINDOW` | 64 | how far ahead of the pending tick a peer's inputs may buffer; a power of two so a ring index is a mask |
| `MAX_DATAGRAM_BYTES` | 156 | derived from the four above, not chosen, and asserted against the composition |

156 bytes sits well inside `MTU_FLOOR`, the 1,200-byte path this protocol
assumes. Nothing enforces that at run time — it is asserted at compile
time, because a ceiling raised past it would produce datagrams that vanish
on some paths and arrive on others, and that reads as a bug in the
simulation rather than one in the network.

## The socket is somewhere else, and that is the design

This crate declares itself simulation code. The engine's structure check
therefore denies it a dependency path to the platform crate at any depth,
in any dependency kind — a graph property rather than a lint, because a
lint matching a definition path is exactly one wrapper deep. So the socket
cannot be here, and it is not: it lives behind a default-off feature on
the platform crate, with no dependency edge between the two halves in
either direction. An application holds both and moves bytes across.

What that buys is not tidiness. It is that every step of the security
roadmap — an entropy source, a keyed authentication tag, encryption —
lands at the socket seam and changes no line of the code that decides what
a tick means.

## What this is not, stated plainly

**There is no authentication, no confidentiality, and no integrity
protection.** Peer identity is a source address plus a session id.
Anything on the path, or anything able to spoof a rostered peer's address,
is indistinguishable from that peer. Nothing is encrypted, and nothing
detects a datagram modified in flight.

**The deployment this is built for is a LAN, or an explicit invitation
among people who trust each other, over a path the participants control.**
Not public matchmaking, not untrusted peers, not a listening port on an
open host.

**Cheating is out of scope, and specifically:** information cheating —
seeing what you should not — is *unpreventable* in any inputs-only design,
because every peer necessarily holds the whole world. That is the premise
that makes a large world affordable at all. What the model does give is
narrower and real: state never crosses the wire, so **a cheater can lie to
himself and cannot lie to anyone else.** Nothing a peer sends can change
another peer's world except an input the rules would have accepted from
them.

A reader who assumes lockstep implies anti-cheat will be wrong in an
expensive way, which is why this section is here rather than in a document
nobody downstream reads.

## The format

Little-endian throughout, fixed width, no varints, no optional fields.
Every datagram opens with the same sixteen bytes:

| off | size | field |
|---|---|---|
| 0 | 4 | magic `RNWL` |
| 4 | 2 | wire version — exactly one value is accepted |
| 6 | 1 | kind — `1` Hello, `2` Inputs, `3` Digest, `4` Bye |
| 7 | 1 | the claimed sender's seat |
| 8 | 8 | session id — never zero |

Discriminants start at one and the session id is never zero, so an
all-zero buffer names nothing. There is no padding in the header: the
sender byte occupies what would otherwise be one.

Four bodies follow it — parameters at the handshake, a run of consecutive
per-tick inputs, a pair of fingerprints, and a departure that names the
tick it happened at. The exact layouts are in
[`wire.rs`](src/wire.rs), one named constant per offset.
