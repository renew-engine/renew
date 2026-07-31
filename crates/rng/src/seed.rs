//! The two identities a generator is built from: the run's master seed and
//! the stream that names one use of it.
//!
//! Separate newtypes over `u64` because `Rng::new(seed, stream)` takes two
//! of them and swapping the arguments is otherwise silent: the simulation
//! still runs, still reproduces, and quietly draws from a different
//! sequence than the recorded trace expects. Distinct types make that a
//! compile error.
//!
//! # Why a stream is a name and not a number a caller invents
//!
//! Two systems that both pick `1` share a sequence, and share it
//! invisibly: nothing fails, the numbers just correlate. Avoiding that
//! with integers means a central registry — one list every module has to
//! agree on and keep in step, which is a coupling this engine does not
//! want. [`StreamId::from_name`] removes the registry: every module names
//! its own streams in its own file, and the mixer makes collisions between
//! different names a birthday event over 64 bits rather than a namespace
//! problem.

use crate::mix::{GAMMA, mix};

/// The master seed for one run of a simulation.
///
/// Where it comes from is the application's business — a command-line
/// flag, a recorded input trace, a lobby handshake. This crate never
/// invents one, which is why it has no dependency that could supply
/// entropy: an unseeded run is not something a caller can reach by
/// forgetting an argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Seed(u64);

impl Seed {
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Names one independent sequence within a run: a system, a subsystem, or
/// one entity inside one of those.
///
/// Identity only — a `StreamId` holds no generator state and never
/// changes as the run proceeds. That is what makes derivation
/// order-independent: `Rng::new(seed, id)` answers the same way whether it
/// is called on frame 1 or frame 900, before or after every other stream
/// of the run, which is exactly what a replay needs when entities are
/// created in a different order than they were recorded in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct StreamId(u64);

impl StreamId {
    /// The stream a name refers to, computed at compile time when the
    /// name is a literal:
    ///
    /// ```
    /// use renew_rng::StreamId;
    /// const LOOT: StreamId = StreamId::from_name("loot");
    /// ```
    ///
    /// Each byte is folded through the mixer, so two names differing
    /// anywhere — including in length — land far apart rather than
    /// adjacent. This is a naming device, not a hash function for general
    /// use: it is not fast, it is not published, and nothing outside this
    /// crate should depend on the exact value it produces.
    #[must_use]
    pub const fn from_name(name: &str) -> Self {
        let bytes = name.as_bytes();
        let mut accumulator = GAMMA;
        let mut index = 0;
        while index < bytes.len() {
            // `u64::from` is not callable in a `const fn` (const trait
            // impls are unstable), so the widening is written as a cast.
            #[allow(clippy::cast_lossless)]
            let byte = bytes[index] as u64;
            accumulator = mix(accumulator ^ byte);
            index += 1;
        }
        Self(accumulator)
    }

    /// A stream chosen by number rather than by name — for identifiers
    /// that are already numbers, such as a recorded trace's stream table.
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    /// The stream belonging to one index under this one: per-entity,
    /// per-tile, per-shot.
    ///
    /// Injective in `index` for a fixed parent, so no two entities under
    /// one system can share a sequence. Different parents with different
    /// indices *can* in principle land on the same child — 128 bits of
    /// input do not fit in 64 bits of output — and the honest bound is the
    /// birthday one: a run using a million distinct streams has roughly a
    /// one-in-forty-million chance of any collision at all.
    #[must_use]
    pub const fn child(self, index: u64) -> Self {
        Self(mix(self.0 ^ mix(index).wrapping_mul(GAMMA)))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Seed, StreamId};

    #[test]
    fn a_seed_round_trips() {
        assert_eq!(Seed::from_u64(0).get(), 0);
        assert_eq!(Seed::from_u64(u64::MAX).get(), u64::MAX);
        assert!(Seed::from_u64(1) > Seed::from_u64(0));
    }

    #[test]
    fn a_numeric_stream_id_round_trips() {
        assert_eq!(StreamId::from_u64(7).get(), 7);
        assert_ne!(StreamId::from_u64(7), StreamId::from_u64(8));
    }

    #[test]
    fn a_named_stream_is_computable_at_compile_time() {
        const LOOT: StreamId = StreamId::from_name("loot");
        assert_eq!(LOOT, StreamId::from_name("loot"));
    }

    /// Names that a careless hash would collide on: one-character
    /// differences, a suffix, an anagram, and the empty name.
    #[test]
    fn near_miss_names_land_in_different_streams() {
        let names = [
            "",
            "a",
            "b",
            "ai",
            "ia",
            "loot",
            "loots",
            "loo",
            "physics",
            "physic",
            "spawn",
            "spawns",
            "npc_spawn",
            "spawn_npc",
        ];
        let ids: std::collections::BTreeSet<u64> = names
            .iter()
            .map(|name| StreamId::from_name(name).get())
            .collect();
        assert_eq!(ids.len(), names.len());
    }

    #[test]
    fn children_of_one_parent_are_distinct() {
        let parent = StreamId::from_name("enemies");
        let ids: std::collections::BTreeSet<u64> = (0..20_000u64)
            .map(|index| parent.child(index).get())
            .collect();
        assert_eq!(ids.len(), 20_000);
    }

    /// Two systems that happen to use the same entity index must not share
    /// a sequence — the whole reason a child is derived from its parent
    /// rather than from the index alone.
    #[test]
    fn the_same_index_under_different_parents_is_a_different_stream() {
        let index = 42;
        assert_ne!(
            StreamId::from_name("enemies").child(index),
            StreamId::from_name("loot").child(index)
        );
    }

    #[test]
    fn children_nest() {
        let grandchild = StreamId::from_name("enemies").child(3).child(9);
        assert_ne!(grandchild, StreamId::from_name("enemies").child(9).child(3));
        assert_ne!(grandchild, StreamId::from_name("enemies").child(3));
    }
}
