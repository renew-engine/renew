//! Entity handles, and the allocator that recycles them.
//!
//! An entity is an index plus a generation. The index is a slot; the
//! generation counts how many times that slot has been reused. Both are
//! needed, and the reason is the bug the pair exists to prevent: without
//! a generation, a handle to a despawned entity silently becomes a handle
//! to whatever was spawned into its slot next — a use-after-free that the
//! borrow checker cannot see, because nothing here is a reference.

use core::fmt;

/// A handle to an entity, valid only while its generation matches.
///
/// `Copy` and 64 bits, so passing one costs nothing and storing a million
/// costs 8 MB. Deliberately not `Default`: a zeroed handle would name
/// slot 0 at generation 0, which is a real entity, and a defaulted handle
/// that accidentally works is worse than one that will not compile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    /// The slot this entity occupies. Stable for the entity's lifetime,
    /// and reused afterwards.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// How many times this slot had been used when the handle was issued.
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Construct a handle. Crate-internal: only the allocator may mint
    /// one, because a hand-made handle could name a live entity it has no
    /// right to.
    pub(crate) const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e{}v{}", self.index, self.generation)
    }
}

/// Hands out entity slots and recycles them.
#[derive(Debug, Default)]
pub struct Entities {
    /// Generation per slot. A slot is alive when its generation is even
    /// after the first spawn — see `alive`, which is tracked explicitly
    /// rather than encoded in parity, because parity is the kind of
    /// cleverness that is wrong once and then wrong forever.
    generations: Vec<u32>,
    alive: Vec<bool>,
    /// Slots ready for reuse, newest first.
    free: Vec<u32>,
    live_count: usize,
}

impl Entities {
    /// An allocator with no slots yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many entities are alive.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live_count
    }

    /// Whether no entity is alive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live_count == 0
    }

    /// The highest slot ever allocated, which bounds an ordered walk.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.generations.len()
    }

    /// Allocate an entity, reusing a free slot when one exists.
    ///
    /// Reuse is newest-first, which keeps the index range compact: an
    /// ordered query walks slots rather than entities, so a store whose
    /// indices are spread thin pays for the gaps.
    ///
    /// # Panics
    ///
    /// Never in practice: the slot count is bounded by `u32::MAX`, and a
    /// tree with four billion live entities has other problems. Written
    /// as a saturating count rather than an unwrap so there is no panic
    /// to reason about.
    pub fn spawn(&mut self) -> Entity {
        self.live_count = self.live_count.saturating_add(1);
        if let Some(index) = self.free.pop() {
            let slot = index as usize;
            if let Some(alive) = self.alive.get_mut(slot) {
                *alive = true;
            }
            let generation = self.generations.get(slot).copied().unwrap_or_default();
            return Entity::new(index, generation);
        }
        let index = u32::try_from(self.generations.len()).unwrap_or(u32::MAX);
        self.generations.push(0);
        self.alive.push(true);
        Entity::new(index, 0)
    }

    /// Whether this exact handle still names a live entity.
    ///
    /// Both halves are checked. A handle whose slot is alive but whose
    /// generation is stale names an entity that no longer exists, and is
    /// the whole reason the generation is there.
    #[must_use]
    pub fn is_alive(&self, entity: Entity) -> bool {
        let slot = entity.index() as usize;
        self.alive.get(slot).copied().unwrap_or(false)
            && self.generations.get(slot).copied() == Some(entity.generation())
    }

    /// Free an entity's slot. Returns whether it was alive to begin with.
    ///
    /// Despawning an already-dead handle is a no-op rather than an error:
    /// it is the natural outcome of two systems both deciding something
    /// should go, and turning it into a failure would make every caller
    /// check first.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        let slot = entity.index() as usize;
        if let Some(alive) = self.alive.get_mut(slot) {
            *alive = false;
        }
        if let Some(generation) = self.generations.get_mut(slot) {
            // Wrapping is the honest choice: at four billion reuses of one
            // slot a handle from the first pass could alias, and there is
            // no cheaper fix that does not leak slots forever. Recorded
            // rather than hidden behind a saturating counter that would
            // silently stop detecting staleness at the same point.
            *generation = generation.wrapping_add(1);
        }
        self.free.push(entity.index());
        self.live_count = self.live_count.saturating_sub(1);
        true
    }

    /// Every live entity, in ascending slot order.
    ///
    /// The order is part of the contract, not an accident of the
    /// representation: see the crate docs.
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.alive
            .iter()
            .enumerate()
            .filter(|(_, alive)| **alive)
            .filter_map(|(slot, _)| {
                let index = u32::try_from(slot).ok()?;
                let generation = self.generations.get(slot).copied()?;
                Some(Entity::new(index, generation))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_allocator_is_empty() {
        let entities = Entities::new();
        assert!(entities.is_empty());
        assert_eq!(entities.len(), 0);
        assert_eq!(entities.capacity(), 0);
    }

    #[test]
    fn spawning_hands_out_distinct_slots() {
        let mut entities = Entities::new();
        let first = entities.spawn();
        let second = entities.spawn();
        assert_ne!(first, second);
        assert_eq!(first.index(), 0);
        assert_eq!(second.index(), 1);
        assert_eq!(entities.len(), 2);
        assert!(entities.is_alive(first));
        assert!(entities.is_alive(second));
    }

    /// The bug the generation exists to prevent: a stale handle must not
    /// name whatever took its slot.
    #[test]
    fn a_stale_handle_does_not_name_the_slots_new_owner() {
        let mut entities = Entities::new();
        let old = entities.spawn();
        assert!(entities.despawn(old));

        let new = entities.spawn();
        assert_eq!(new.index(), old.index(), "the slot must be reused");
        assert_ne!(new.generation(), old.generation());

        assert!(!entities.is_alive(old), "the stale handle must be dead");
        assert!(entities.is_alive(new));
    }

    #[test]
    fn despawning_twice_is_a_no_op_the_second_time() {
        let mut entities = Entities::new();
        let entity = entities.spawn();
        assert!(entities.despawn(entity));
        assert!(!entities.despawn(entity));
        assert_eq!(entities.len(), 0);
    }

    #[test]
    fn a_handle_from_another_allocator_is_not_alive_here() {
        let mut one = Entities::new();
        let mut other = Entities::new();
        let _ = one.spawn();
        let stranger = other.spawn();
        // Same slot, same generation, different world. This is the one
        // case the generation cannot catch, and the test exists to say so
        // rather than to claim otherwise.
        assert!(one.is_alive(stranger), "documented limit, not a promise");
    }

    #[test]
    fn iteration_is_in_ascending_slot_order() {
        let mut entities = Entities::new();
        let made: Vec<Entity> = (0..8).map(|_| entities.spawn()).collect();
        // Despawn a scattering, so the live set has gaps.
        for index in [1usize, 4, 5] {
            assert!(entities.despawn(made[index]));
        }
        let seen: Vec<u32> = entities.iter().map(Entity::index).collect();
        assert_eq!(seen, vec![0, 2, 3, 6, 7]);
    }

    #[test]
    fn reuse_keeps_the_slot_range_compact() {
        let mut entities = Entities::new();
        let first = entities.spawn();
        let second = entities.spawn();
        entities.despawn(first);
        entities.despawn(second);
        let a = entities.spawn();
        let b = entities.spawn();
        assert_eq!(entities.capacity(), 2, "no new slots were needed");
        assert!(entities.is_alive(a));
        assert!(entities.is_alive(b));
    }

    #[test]
    fn a_handle_prints_its_slot_and_generation() {
        let mut entities = Entities::new();
        let entity = entities.spawn();
        assert_eq!(entity.to_string(), "e0v0");
    }
}
