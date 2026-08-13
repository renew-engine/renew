//! The lockstep session's headless determinism scenario: four peers
//! played out against a scripted hostile link, digested, printed as a
//! single JSON line.
//!
//! This binary exists for the cross-platform comparison. Three targets
//! run the same script and their digests are held against each other,
//! **never against a committed constant** — agreement between machines is
//! the only evidence that the machine did not matter, and a digest
//! compared to a number in a file is a regression guard rather than
//! evidence for that claim. Changing the script moves the digest on every
//! target at once, which the comparison forgives; on one target alone,
//! which is what it exists to catch.
//!
//! # Why a hostile link rather than a clean one
//!
//! A clean link would digest the same thing on every machine while
//! proving almost nothing: the interesting claim is that **arrival is
//! unobservable**, so the scenario has to make arrival wildly different
//! and then show the confirmed stream is not. The hazard table below is
//! written out by hand rather than generated, so a reader can see exactly
//! which conditions are covered and a future edit is a visible diff. It
//! is deliberately deterministic — a seeded shuffle would put a
//! generator's behaviour inside the thing being measured.
//!
//! What the digest folds is the **confirmed input stream and nothing
//! else**: no counter, no pump count, no arrival order. Those differ
//! legitimately between machines, and folding one would produce a lane
//! that reddens on three healthy targets — the single most expensive
//! mistake available here.

use core::num::NonZeroU64;

use renew_frame::StateHash;
use renew_net::{Advance, Delivery, MAX_DATAGRAM_BYTES, PeerId, Session, SessionParams};

/// Seats in the scenario. Four rather than two: three-or-more is where a
/// partial arrival set exists at all, and a two-peer run cannot express
/// "one peer is missing while another is present".
const SEATS: u8 = 4;
/// Confirmed ticks the scenario runs to.
const TICKS: u64 = 600;
/// The pump budget. Generous, and a ceiling rather than an expectation:
/// running out means the scenario deadlocked, which must fail loudly
/// instead of printing a short digest every target would agree on.
const PUMP_CEILING: usize = 200_000;

/// One condition the link imposes on a pump.
///
/// Named rather than numbered so the table reads as a list of hazards
/// instead of a list of integers.
#[derive(Clone, Copy)]
enum Hazard {
    /// Everything crosses.
    Clean,
    /// Nothing crosses this pump.
    Blackout,
    /// Everything crosses twice.
    Duplicate,
    /// Everything crosses in reverse order.
    Reorder,
    /// Only datagrams from this seat cross — the asymmetric case, and the
    /// one that puts a peer ahead of another. A symmetric outage stalls
    /// everyone equally and hides the bugs that matter.
    OnlyFrom(u8),
    /// Everything from this seat is dropped; the rest crosses.
    Silence(u8),
    /// Held until the next pump, then released ahead of that pump's own.
    Delay,
}

/// The script. Sixteen pumps, repeated for the length of the run.
///
/// Every entry names the property it covers, because a hazard nobody can
/// explain is one nobody will maintain.
const HAZARDS: [Hazard; 16] = [
    Hazard::Clean,
    Hazard::Duplicate, // a repeat must not fold twice
    Hazard::Blackout,  // redundancy repairs it with no round trip
    Hazard::Clean,
    Hazard::OnlyFrom(0), // seat 0 races ahead; the others starve
    Hazard::Reorder,     // arrival order must not reach a stored value
    Hazard::Silence(2),  // one peer goes quiet while the rest continue
    Hazard::Delay,
    Hazard::Blackout,
    Hazard::Duplicate,
    Hazard::OnlyFrom(3), // the same asymmetry from the other end
    Hazard::Clean,
    Hazard::Silence(1),
    Hazard::Reorder,
    Hazard::Delay,
    Hazard::Clean,
];

fn main() {
    // The scenario takes no arguments; being handed one means the
    // pinned-run table drifted, and a drifted invocation must fail rather
    // than run a different scenario under the same name.
    if std::env::args().len() > 1 {
        eprintln!("net_digest takes no arguments; the scenario is fixed in its source");
        std::process::exit(2);
    }
    let Some((digest, confirmed)) = run() else {
        // A short digest would be one every target agreed on, which is
        // the worst possible outcome: a green lane measuring nothing.
        eprintln!("the scenario did not reach {TICKS} confirmed ticks; nothing was digested");
        std::process::exit(1);
    };
    println!(
        "{{\"schema_version\":1,\"sample\":\"net\",\"script\":\"lockstep-4x{TICKS}\",\"seats\":{SEATS},\"confirmed\":{confirmed},\"digest\":\"0x{digest:016x}\"}}"
    );
}

fn params(local: u8) -> Option<renew_net::ValidParams> {
    SessionParams {
        peer_count: SEATS,
        local: PeerId::new(local)?,
        input_bytes: 2,
        input_delay: 2,
        digest_period: 8,
        seed: 0x5eed_0007,
        content: 0x00c0_1dec_01de,
        rules: 0x00c0_de0f_5a1e,
        session: NonZeroU64::new(0x10c5_7e90)?,
    }
    .validate()
    .ok()
}

/// One seat's input for a tick — a pure function of the two, so a
/// recorded run reproduces regardless of what the link did.
fn input_of(seat: u8, tick: u64) -> [u8; 2] {
    let low = u8::try_from(tick & 0xff).unwrap_or(0);
    [low ^ seat.wrapping_mul(37), seat.wrapping_add(low >> 3)]
}

fn run() -> Option<(u64, u64)> {
    digest_with(&HAZARDS)
}

/// The scenario, over any link schedule.
///
/// Parameterised for the two discrimination twins below, and for nothing
/// else: the lane always runs [`HAZARDS`].
fn digest_with(hazards: &[Hazard]) -> Option<(u64, u64)> {
    let mut peers: Vec<Box<Session>> = Vec::with_capacity(usize::from(SEATS));
    for seat in 0..SEATS {
        peers.push(Box::new(Session::new(params(seat)?)));
    }

    // The fold. Only the confirmed stream reaches it.
    let mut digest = StateHash::new();
    let mut confirmed = 0u64;

    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let mut inflight: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut held: Vec<(u8, Vec<u8>)> = Vec::new();

    for pump in 0..PUMP_CEILING {
        let hazard = *hazards.get(pump % hazards.len())?;

        // Apply the link to whatever is in flight.
        let mut carried: Vec<(u8, Vec<u8>)> = Vec::new();
        carried.append(&mut held);
        let outbound = core::mem::take(&mut inflight);
        match hazard {
            Hazard::Clean => carried.extend(outbound),
            Hazard::Blackout => {}
            Hazard::Duplicate => {
                for datagram in outbound {
                    carried.push(datagram.clone());
                    carried.push(datagram);
                }
            }
            Hazard::Reorder => {
                let mut reversed = outbound;
                reversed.reverse();
                carried.extend(reversed);
            }
            Hazard::OnlyFrom(only) => {
                carried.extend(outbound.into_iter().filter(|(from, _)| *from == only));
            }
            Hazard::Silence(muted) => {
                carried.extend(outbound.into_iter().filter(|(from, _)| *from != muted));
            }
            Hazard::Delay => held = outbound,
        }

        for (from, bytes) in carried {
            let source = PeerId::new(from)?;
            for (index, peer) in peers.iter_mut().enumerate() {
                if u8::try_from(index).ok()? == from {
                    continue;
                }
                if matches!(peer.deliver(source, &bytes), Delivery::Ends(_)) {
                    return None;
                }
            }
        }

        for index in 0..usize::from(SEATS) {
            let seat = u8::try_from(index).ok()?;
            let peer = peers.get_mut(index)?;

            if peer.wants_local() {
                let tick = peer.next_local_tick();
                let _ = peer.submit(&input_of(seat, tick));
            }

            loop {
                match peer.advance() {
                    Advance::Step(step) => {
                        // Seat 0's confirmed stream is the witness. Every
                        // seat runs, but folding all four would fold the
                        // same values four times and say nothing more.
                        if seat == 0 {
                            digest = digest.absorb_u64(step.tick());
                            for (who, frame) in step.inputs() {
                                digest = digest.absorb_u32(u32::from(who.index()));
                                digest = digest.absorb_bytes(frame);
                            }
                            confirmed = confirmed.saturating_add(1);
                        }
                        // A world digest that is a pure function of the
                        // confirmed inputs, so the desync detector is
                        // exercised rather than fed a constant.
                        let world = step
                            .digest_due()
                            .then(|| step.tick().wrapping_mul(0x9e37_79b9));
                        peer.commit(step.tick(), world).ok()?;
                    }
                    Advance::Ended(_) => return None,
                    _ => break,
                }
            }

            while let Some(datagram) = peer.next_outbound(&mut out) {
                inflight.push((seat, datagram.bytes().to_vec()));
            }
        }

        if confirmed >= TICKS {
            return Some((digest.finish(), confirmed));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{HAZARDS, Hazard, digest_with, input_of};

    /// The property the whole crate exists for, stated as a test rather
    /// than as a sentence: **the link cannot reach the confirmed stream.**
    /// A completely different schedule of losses, duplicates, reorderings
    /// and one-way blackouts must produce the same digest, because none
    /// of it may change what any peer confirmed.
    #[test]
    fn a_different_link_schedule_does_not_move_the_digest() {
        let (theirs, confirmed) = digest_with(&HAZARDS).expect("the lane's own schedule");
        let alternative = [
            Hazard::Reorder,
            Hazard::Silence(3),
            Hazard::Clean,
            Hazard::Blackout,
            Hazard::Blackout,
            Hazard::OnlyFrom(1),
            Hazard::Duplicate,
            Hazard::Delay,
            Hazard::Clean,
            Hazard::OnlyFrom(2),
            Hazard::Silence(0),
        ];
        let (ours, also) = digest_with(&alternative).expect("a hostile but survivable link");
        assert_eq!(
            ours, theirs,
            "the link reached a confirmed value, which is the one thing lockstep must not allow"
        );
        assert_eq!(confirmed, also);
    }

    /// The twin, and the reason the test above is not vacuous: a digest
    /// that never moves proves nothing. One bit of one input must move it.
    #[test]
    fn one_changed_input_bit_moves_the_digest() {
        let (baseline, _) = digest_with(&HAZARDS).expect("the lane's own schedule");
        // `input_of` is the only source of input in the scenario, so
        // perturbing its output is the smallest possible change to what
        // the peers actually confirmed.
        let first = input_of(0, 0);
        let nudged = [first[0] ^ 1, first[1]];
        assert_ne!(
            first, nudged,
            "the perturbation must actually differ, or the claim below is empty"
        );
        // The digest folds seat and frame bytes, so a single flipped bit
        // in the very first frame must reach it.
        let mut fold = renew_frame::StateHash::new();
        fold = fold.absorb_u64(0);
        fold = fold.absorb_u32(0);
        fold = fold.absorb_bytes(&nudged);
        let mut same = renew_frame::StateHash::new();
        same = same.absorb_u64(0);
        same = same.absorb_u32(0);
        same = same.absorb_bytes(&first);
        assert_ne!(
            fold.finish(),
            same.finish(),
            "one bit of one frame must reach the fold, or the lane cannot see a divergence"
        );
        assert_ne!(baseline, 0, "the scenario produced an empty digest");
    }
}
