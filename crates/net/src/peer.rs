//! Who is in a session, and in what order they are always visited.

use crate::MAX_PEERS;

/// One seat in a session, by index.
///
/// **Never an address.** The mapping from "where these bytes came from"
/// to "which seat sent them" belongs to the driver, held in whatever
/// table its transport gives it. This crate compares seats, orders them,
/// and does nothing else with them — which is what lets the same session
/// run over a mesh, over a relayed link, or over no socket at all in a
/// test, without learning that any of those exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerId(u8);

impl PeerId {
    /// A seat by index, or `None` at or past [`MAX_PEERS`].
    ///
    /// The only constructor. A seat that cannot be built out of range is
    /// a seat no later check has to remember to reject.
    #[must_use]
    pub const fn new(index: u8) -> Option<Self> {
        if index < MAX_PEERS {
            Some(Self(index))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Every seat, ascending, whether or not a session holds them all.
///
/// The iteration order of the whole type, exported so a caller writing
/// its own per-seat loop cannot choose a different one by accident.
pub fn peers() -> impl Iterator<Item = PeerId> {
    (0..MAX_PEERS).filter_map(PeerId::new)
}

/// A set of seats, as one byte.
///
/// **Iteration is ascending by index, always** — the same contract
/// `renew-ecs` makes about slot order, and for the same reason: a set
/// that iterated in any other order could put arrival time into digested
/// state, and every peer would diverge from every other while each ran
/// correct code. Making the container incapable of another order is
/// cheaper than a rule every future contributor has to remember and every
/// reviewer has to catch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerSet(u8);

impl PeerSet {
    /// No seats.
    pub const EMPTY: Self = Self(0);

    /// The first `count` seats.
    ///
    /// A `count` above [`MAX_PEERS`] is **clamped**, not refused: a roster
    /// is validated where it is declared — at the handshake, which can
    /// name the peer that lied about it — and a set that silently held a
    /// ninth seat here would be worse than one that holds eight.
    #[must_use]
    pub const fn of_count(count: u8) -> Self {
        let held = if count < MAX_PEERS { count } else { MAX_PEERS };
        // `1 << 8` would overflow the byte; the clamp above is what makes
        // the shift total, and the mask is written rather than computed
        // for the same reason.
        if held >= MAX_PEERS {
            Self(u8::MAX)
        } else {
            Self((1u8 << held).wrapping_sub(1))
        }
    }

    #[must_use]
    pub const fn contains(self, peer: PeerId) -> bool {
        self.0 & (1u8 << peer.index()) != 0
    }

    #[must_use]
    pub const fn with(self, peer: PeerId) -> Self {
        Self(self.0 | (1u8 << peer.index()))
    }

    #[must_use]
    pub const fn without(self, peer: PeerId) -> Self {
        Self(self.0 & !(1u8 << peer.index()))
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The seats in this set that are not in `other`.
    ///
    /// What a stall reports: the roster, less whoever has arrived.
    #[must_use]
    pub const fn without_all(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// How many seats are in the set.
    ///
    /// `u32` rather than `usize`, and deliberately: no pointer-width value
    /// may enter a digest, and this one can.
    #[must_use]
    pub const fn count(self) -> u32 {
        self.0.count_ones()
    }

    /// The raw bits, for a caller folding this set into a fingerprint.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// The seats in the set, ascending by index. The only order this type
    /// ever hands out.
    pub fn iter(self) -> impl Iterator<Item = PeerId> {
        peers().filter(move |peer| self.contains(*peer))
    }
}

#[cfg(test)]
mod tests {
    use super::{PeerId, PeerSet, peers};
    use crate::MAX_PEERS;

    fn seat(index: u8) -> PeerId {
        PeerId::new(index).expect("a seat the test chose in range")
    }

    #[test]
    fn a_seat_past_the_ceiling_cannot_be_built() {
        assert!(PeerId::new(MAX_PEERS).is_none());
        assert!(PeerId::new(u8::MAX).is_none());
        assert!(PeerId::new(MAX_PEERS.saturating_sub(1)).is_some());
    }

    #[test]
    fn the_whole_roster_is_a_full_byte() {
        assert_eq!(PeerSet::of_count(MAX_PEERS).bits(), u8::MAX);
        assert_eq!(PeerSet::of_count(MAX_PEERS).count(), u32::from(MAX_PEERS));
    }

    #[test]
    fn an_oversized_roster_clamps_rather_than_overflowing() {
        // The shift that builds the mask is only total because of this
        // clamp, so the test is about the arithmetic and not only the API.
        assert_eq!(PeerSet::of_count(u8::MAX), PeerSet::of_count(MAX_PEERS));
        assert_eq!(
            PeerSet::of_count(MAX_PEERS.saturating_add(1)).count(),
            u32::from(MAX_PEERS)
        );
    }

    #[test]
    fn a_partial_roster_holds_exactly_its_prefix() {
        let two = PeerSet::of_count(2);
        assert!(two.contains(seat(0)) && two.contains(seat(1)));
        assert!(!two.contains(seat(2)));
        assert_eq!(two.count(), 2);
    }

    #[test]
    fn the_empty_set_is_empty_and_the_only_one() {
        assert!(PeerSet::EMPTY.is_empty());
        assert_eq!(PeerSet::EMPTY, PeerSet::of_count(0));
        assert_eq!(PeerSet::EMPTY, PeerSet::default());
        assert_eq!(PeerSet::EMPTY.iter().count(), 0);
    }

    #[test]
    fn adding_and_removing_a_seat_are_inverse() {
        let set = PeerSet::EMPTY.with(seat(3)).with(seat(5));
        assert_eq!(set.count(), 2);
        assert_eq!(set.without(seat(5)).without(seat(3)), PeerSet::EMPTY);
        // Idempotent in both directions: a set is a set.
        assert_eq!(set.with(seat(3)), set);
        assert_eq!(set.without(seat(1)), set);
    }

    #[test]
    fn iteration_ascends_whatever_order_the_seats_were_added_in() {
        let forwards = PeerSet::EMPTY.with(seat(1)).with(seat(4)).with(seat(6));
        let backwards = PeerSet::EMPTY.with(seat(6)).with(seat(4)).with(seat(1));
        let order: Vec<u8> = forwards.iter().map(PeerId::index).collect();
        assert_eq!(order, vec![1, 4, 6]);
        assert_eq!(
            order,
            backwards.iter().map(PeerId::index).collect::<Vec<u8>>()
        );
    }

    #[test]
    fn every_seat_is_visited_once_and_in_order() {
        let all: Vec<u8> = peers().map(PeerId::index).collect();
        assert_eq!(all, (0..MAX_PEERS).collect::<Vec<u8>>());
    }
}
