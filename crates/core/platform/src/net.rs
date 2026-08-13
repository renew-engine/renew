//! UDP datagrams — the network seam, behind the `net` feature.
//!
//! **This module is the only place in the engine that names `std::net`.**
//! That is a zoning decision rather than a style one, and it is enforced
//! rather than merely stated:
//! `only_the_platform_socket_module_names_the_standard_network_types` in
//! `tools/cli/tests/workspace_lists.rs` walks every Rust file git lists
//! and fails on any but this one that names those types in code —
//! `std::net`, `core::net`, a brace group naming `net` however it is
//! wrapped, or a rename of the crate root. Comments are exempt, this one
//! among them.
//!
//! The graph rule is the other half: a crate that promises deterministic
//! simulation is denied a dependency path to this
//! one, at any depth and in any dependency kind, so the code that decides
//! what a tick means cannot reach a socket even by accident. What travels
//! over the wire is decided in a crate that has no way to send it, and
//! moved across by an application that holds both.
//!
//! What that split buys is not tidiness. Every later step of the security
//! story — an entropy source, a keyed authentication tag, encryption —
//! lands *here*, and changes no line of the code that runs the game.
//!
//! # No threads, and no blocking
//!
//! A socket is non-blocking from birth; there is no blocking mode to
//! reach. It is polled from the loop that already runs, and drains in
//! microseconds — a handful of small datagrams per tick has nothing for a
//! thread to overlap, and a receive thread would cost a shared queue, a
//! sanitizer obligation, and an argument about send/sync, to save nothing
//! measurable. The audio seam takes the mirror decision for the mirror
//! reason: there the operating system owns the thread, so this crate
//! spawns none; here there is no such thread, so this crate polls.
//!
//! # Failing recoverably
//!
//! A machine with no interface, no permission, or a port already taken
//! fails with [`NetError::Unavailable`] — the same graceful-skip seam the
//! audio and window modules offer, so a headless or firewalled run
//! reports an outcome instead of dying.

use core::fmt;
use std::io::ErrorKind;
use std::net::UdpSocket;

/// Re-exported so a consumer can name an address without importing
/// `std::net` itself. The doorway stays a doorway.
pub use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// The largest datagram this seam will hand back, in bytes.
///
/// A seam-level ceiling, deliberately independent of whatever protocol a
/// caller speaks: anything larger is refused as oversized rather than
/// delivered in pieces. Two kilobytes is comfortably above the smallest
/// maximum transmission unit any path is expected to carry, so a refusal
/// here means a peer sent something no ordinary route would deliver
/// whole.
pub const RECEIVE_CEILING: usize = 2048;

/// The Windows error a UDP receive raises when the datagram did not fit.
///
/// Named by number because `ErrorKind` folds it into `Uncategorized`,
/// which cannot be matched on stably. Winsock calls it WSAEMSGSIZE.
#[cfg(windows)]
const WSAEMSGSIZE: i32 = 10040;

/// Why a socket could not be opened or used.
///
/// Every variant names the address it was about, the way every
/// filesystem error here names its path: an error that does not say
/// *which* endpoint failed is one a caller cannot act on.
#[derive(Debug)]
#[non_exhaustive]
pub enum NetError {
    /// The machine, not the request: no interface, no permission, or the
    /// port already taken. Recoverable by design — a caller reports it
    /// and runs offline.
    Unavailable { addr: SocketAddr, kind: ErrorKind },
    /// The kernel refused a send for a reason that is not back-pressure.
    Send { to: SocketAddr, kind: ErrorKind },
    /// The kernel refused a receive for a reason that is not emptiness.
    Receive { kind: ErrorKind },
    /// The datagram was longer than the buffer offered.
    ///
    /// **Reported, never silently truncated.** A truncated datagram is a
    /// different message: a codec handed a prefix would refuse it as
    /// malformed, blaming the sender for the receiver's small buffer.
    Oversized { capacity: usize },
}

impl fmt::Display for NetError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { addr, kind } => {
                write!(out, "{addr} is unavailable: {kind:?}")
            }
            Self::Send { to, kind } => write!(out, "sending to {to} failed: {kind:?}"),
            Self::Receive { kind } => write!(out, "receiving failed: {kind:?}"),
            Self::Oversized { capacity } => write!(
                out,
                "a datagram was longer than the {capacity}-byte buffer offered, and a truncated \
                 datagram is a different message"
            ),
        }
    }
}

impl core::error::Error for NetError {}

/// Whether the kernel took the bytes.
///
/// A non-blocking socket with a full send buffer is a normal condition
/// and not an error. Nothing is lost by it: a lockstep session re-offers
/// whatever it still owes on the next pump, which is why this is a
/// reported outcome a caller can count rather than a failure it must
/// handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    Delivered,
    WouldBlock,
}

/// A bound UDP socket, owned by whoever bound it.
///
/// No ambient state, no background thread, and no queue this crate
/// manages — the buffers belong to the caller, which is what lets a
/// consumer keep its whole steady state allocation-free.
#[derive(Debug)]
pub struct Socket {
    inner: UdpSocket,
}

impl Socket {
    /// Bind, and switch to non-blocking.
    ///
    /// Port zero binds an ephemeral port; [`Socket::local_addr`] reports
    /// which one, which is how a test gets two sockets without agreeing a
    /// number in advance.
    ///
    /// # Errors
    ///
    /// [`NetError::Unavailable`] when the address cannot be bound, or
    /// when the socket cannot be switched out of blocking mode — the two
    /// are one outcome for a caller, because a blocking socket is not a
    /// thing this seam is willing to hand back.
    pub fn bind(addr: SocketAddr) -> Result<Self, NetError> {
        let inner = UdpSocket::bind(addr).map_err(|error| NetError::Unavailable {
            addr,
            kind: error.kind(),
        })?;
        inner
            .set_nonblocking(true)
            .map_err(|error| NetError::Unavailable {
                addr,
                kind: error.kind(),
            })?;
        Ok(Self { inner })
    }

    /// The address actually bound, which is how an ephemeral port is
    /// discovered.
    ///
    /// # Errors
    ///
    /// [`NetError::Unavailable`] if the kernel will not report it.
    pub fn local_addr(&self) -> Result<SocketAddr, NetError> {
        self.inner
            .local_addr()
            .map_err(|error| NetError::Unavailable {
                addr: SocketAddr::from(([0, 0, 0, 0], 0)),
                kind: error.kind(),
            })
    }

    /// Send one datagram.
    ///
    /// # Errors
    ///
    /// [`NetError::Send`] for anything that is not back-pressure;
    /// back-pressure is [`Sent::WouldBlock`] and not an error.
    pub fn send_to(&self, bytes: &[u8], to: SocketAddr) -> Result<Sent, NetError> {
        match self.inner.send_to(bytes, to) {
            Ok(_) => Ok(Sent::Delivered),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(Sent::WouldBlock),
            Err(error) => Err(NetError::Send {
                to,
                kind: error.kind(),
            }),
        }
    }

    /// Take one datagram, if one is waiting.
    ///
    /// `Ok(None)` means nothing was there. Never blocks, never allocates,
    /// and never partially fills a buffer.
    ///
    /// **Oversized is detected identically on every platform, and it
    /// takes work to make that true.** The two families disagree: Windows
    /// raises an error, while Unix silently truncates and reports the
    /// clamped length, so a naive `len > buffer.len()` check can never
    /// fire there and would hand back a prefix as though it were whole.
    /// This reads into a buffer larger than the caller's and compares, so
    /// truncation is *observed* rather than trusted to be reported.
    ///
    /// **Two kinds read as "nothing waiting", and the second is the
    /// interesting one.** `WouldBlock` is the obvious empty case. On
    /// Windows a UDP socket also surfaces `ConnectionReset` *on receive*
    /// to report an ICMP port-unreachable provoked by an earlier *send* —
    /// to a peer whose process has not started yet, which is the ordinary
    /// case at the beginning of a session. A receive loop that treated it
    /// as fatal would die reliably on one platform and never on the
    /// others, which is the worst shape a bug can have.
    ///
    /// # Errors
    ///
    /// [`NetError::Oversized`] when the datagram would not fit, and
    /// [`NetError::Receive`] for any other kind.
    pub fn recv_from(&self, into: &mut [u8]) -> Result<Option<(usize, SocketAddr)>, NetError> {
        // One byte of headroom is what makes truncation visible: a
        // datagram that fills this exactly is one the kernel may have
        // cut, and is refused rather than guessed at.
        let mut scratch = [0u8; RECEIVE_CEILING + 1];
        // A loop, not a single attempt, and the difference matters. A
        // consumed ICMP error is not an empty queue: returning `None` for
        // one would end the caller's drain early and leave real datagrams
        // sitting behind it until the next pump. On Windows a peer that
        // is not listening yet produces one of these per send, so the
        // rate is set by the network rather than by this machine — a
        // drain that gave up on each would fall behind exactly when a
        // session is starting. Only `WouldBlock` means "nothing there".
        loop {
            match self.inner.recv_from(&mut scratch) {
                Ok((len, from)) => {
                    if len > into.len() || len > RECEIVE_CEILING {
                        return Err(NetError::Oversized {
                            capacity: into.len().min(RECEIVE_CEILING),
                        });
                    }
                    let (Some(source), Some(destination)) =
                        (scratch.get(..len), into.get_mut(..len))
                    else {
                        return Err(NetError::Oversized {
                            capacity: into.len(),
                        });
                    };
                    destination.copy_from_slice(source);
                    return Ok(Some((len, from)));
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(None),
                // Consumed and stepped over. The empty arm is the loop
                // going round again: an ICMP error is not a datagram and
                // not an empty queue, so the only correct response is to
                // ask once more.
                Err(error) if error.kind() == ErrorKind::ConnectionReset => {}
                Err(error) => {
                    // Windows reports "it did not fit" as an error rather
                    // than by truncating, and folds it into an ErrorKind that
                    // cannot be matched on. The number is the only stable
                    // handle, and mapping it here is what keeps one datagram
                    // one outcome on every platform.
                    #[cfg(windows)]
                    if error.raw_os_error() == Some(WSAEMSGSIZE) {
                        return Err(NetError::Oversized {
                            capacity: into.len().min(RECEIVE_CEILING),
                        });
                    }
                    return Err(NetError::Receive { kind: error.kind() });
                }
            }
        }
    }
}

/// A stable, comparable tag for an address.
///
/// **A v4-mapped IPv6 address folds to its v4 form**, and that is the
/// whole reason this exists rather than every driver keying on
/// `SocketAddr` directly. The same peer can reach one machine as
/// `::ffff:1.2.3.4` and another as `1.2.3.4`; a roster keyed on the raw
/// address would see two peers, hand out two seats, and the resulting
/// divergence would surface hundreds of ticks later as a desync with no
/// obvious cause. Canonicalising belongs in the crate that owns the
/// address type, not in each consumer that keys by one.
///
/// The layout is sixteen bytes of address, then the port, big-endian: two
/// tags compare and sort without anyone deciding a byte order twice.
///
/// **This is an encoding, not a hash.** [`peer_addr`] reverses it, which
/// is what lets a tag travel somewhere that may not name an address and
/// come back somewhere that must send to one — a roster of peers, for
/// instance, carried by a crate forbidden from knowing what an address
/// is. Nothing is lost but the distinction between a v4 address and its
/// v4-mapped spelling, which is the distinction this function exists to
/// erase.
#[must_use]
pub fn peer_tag(addr: SocketAddr) -> [u8; 18] {
    let mut tag = [0u8; 18];
    let canonical = match addr.ip() {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        // `to_ipv4_mapped` returns Some only for the ::ffff:a.b.c.d form,
        // which is exactly the case that must fold. A native v6 address
        // stays a v6 address.
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
    };
    // **Both families fill all sixteen bytes**, and that is the whole of
    // the correctness argument. Writing a v4 address into the first four
    // and leaving twelve zero puts the two families in one key space with
    // no discriminator: `32.1.13.184` and the native `2001:db8::` would
    // then produce identical tags, so two genuinely different peers would
    // take one seat — the exact failure this function exists to prevent,
    // inverted. Re-emitting v4 in its mapped form keeps the two disjoint
    // by construction, because the fold above has already turned every
    // mapped v6 into a v4.
    let address = match canonical {
        IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
        IpAddr::V6(v6) => v6.octets(),
    };
    if let Some(room) = tag.get_mut(..16) {
        room.copy_from_slice(&address);
    }
    if let Some(room) = tag.get_mut(16..18) {
        room.copy_from_slice(&addr.port().to_be_bytes());
    }
    tag
}

/// The address a tag names: the inverse of [`peer_tag`].
///
/// **A tag has to be routable again or it is only half a value.** The
/// tag exists so an endpoint can cross a boundary that may not name a
/// `SocketAddr` — a peer roster, a save file, a wire format written by a
/// crate denied any path to this one. Every one of those hands the bytes
/// back to something that must then *send* to them, and without this
/// function that last step is impossible: the roster arrives, and there
/// is nowhere to put a datagram.
///
/// **A v4-mapped tag comes back as a v4 address, not as the mapped v6
/// spelling it was stored in.** That is deliberate and it is load-bearing
/// rather than cosmetic: a socket bound to a v4 address refuses a send to
/// `::ffff:a.b.c.d` on several platforms, so returning the literal
/// sixteen bytes would produce an address that compares equal to the
/// right one and cannot be sent to. Unfolding here means the pair reads
/// as an encoding in one direction and a canonicalisation in the other:
///
/// - `peer_tag(peer_addr(t)) == t` for every tag, exactly.
/// - `peer_addr(peer_tag(a)) == a` for every address already canonical,
///   and equal to its v4 form for one written as v4-mapped v6 — which is
///   the same folding [`peer_tag`] does, observed from the other side.
///
/// Both properties are held by tests rather than by this paragraph.
#[must_use]
pub fn peer_addr(tag: [u8; 18]) -> SocketAddr {
    let mut address = [0u8; 16];
    if let Some(bytes) = tag.get(..16) {
        address.copy_from_slice(bytes);
    }
    let mut port = [0u8; 2];
    if let Some(bytes) = tag.get(16..18) {
        port.copy_from_slice(bytes);
    }
    let v6 = Ipv6Addr::from(address);
    // The same fold `peer_tag` applies on the way in, so a round trip
    // through the pair is idempotent rather than alternating between two
    // spellings of one peer.
    let ip = v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4);
    SocketAddr::new(ip, u16::from_be_bytes(port))
}
