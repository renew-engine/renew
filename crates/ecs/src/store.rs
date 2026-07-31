//! A sparse set: one component type, stored densely, addressed by entity.
//!
//! Three arrays. `sparse` maps an entity slot to a dense position, `dense`
//! maps back, and `values` sits alongside `dense`. Insert and remove are
//! constant time; removal swaps the last element into the hole, which is
//! what keeps `values` contiguous for iteration.
//!
//! **That swap is why iteration order is not free**, and why this file has
//! two iterators rather than one. After any churn the dense array is in
//! no useful order at all, so a query that walked it would visit entities
//! in an order decided by their removal history — reproducible only if
//! every prior operation was. The engine defines an order instead
//! (by recorded decision), and [`Store::iter`] provides it by walking `sparse`.

/// Components of one type, addressed by entity slot.
#[derive(Debug)]
pub struct Store<T> {
    /// Entity slot to dense position. `u32::MAX` means absent, which
    /// costs four bytes per slot rather than the eight an `Option<u32>`
    /// would take at this alignment.
    sparse: Vec<u32>,
    dense: Vec<u32>,
    values: Vec<T>,
}

/// The sentinel for "this slot holds nothing".
const ABSENT: u32 = u32::MAX;

impl<T> Default for Store<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Store<T> {
    /// An empty store.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sparse: Vec::new(),
            dense: Vec::new(),
            values: Vec::new(),
        }
    }

    /// How many components are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the store holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Whether this entity slot has a component here.
    #[must_use]
    pub fn contains(&self, slot: u32) -> bool {
        self.position(slot).is_some()
    }

    /// The dense position for a slot, if any.
    fn position(&self, slot: u32) -> Option<usize> {
        match self.sparse.get(slot as usize).copied() {
            Some(ABSENT) | None => None,
            Some(position) => Some(position as usize),
        }
    }

    /// The component for this slot.
    #[must_use]
    pub fn get(&self, slot: u32) -> Option<&T> {
        self.values.get(self.position(slot)?)
    }

    /// The component for this slot, mutably.
    pub fn get_mut(&mut self, slot: u32) -> Option<&mut T> {
        let position = self.position(slot)?;
        self.values.get_mut(position)
    }

    /// Store a component, returning the one it replaced.
    pub fn insert(&mut self, slot: u32, value: T) -> Option<T> {
        if let Some(position) = self.position(slot) {
            let existing = self.values.get_mut(position)?;
            return Some(core::mem::replace(existing, value));
        }
        let needed = (slot as usize).checked_add(1)?;
        if self.sparse.len() < needed {
            self.sparse.resize(needed, ABSENT);
        }
        let position = u32::try_from(self.dense.len()).ok()?;
        if let Some(entry) = self.sparse.get_mut(slot as usize) {
            *entry = position;
        }
        self.dense.push(slot);
        self.values.push(value);
        None
    }

    /// Remove a component, returning it.
    ///
    /// The last element is swapped into the hole, so this is constant
    /// time and `values` stays contiguous — at the cost of `dense` losing
    /// any order it had, which is the trade [`Store::iter`] pays for.
    pub fn remove(&mut self, slot: u32) -> Option<T> {
        let position = self.position(slot)?;
        let last = self.dense.len().checked_sub(1)?;
        self.dense.swap(position, last);
        self.values.swap(position, last);

        // The element now at `position` used to be last; point its slot
        // at its new home. Done before the pop so a store of one element
        // reads its own slot rather than a stale one.
        if let Some(moved) = self.dense.get(position).copied()
            && position != last
            && let Some(entry) = self.sparse.get_mut(moved as usize)
        {
            *entry = u32::try_from(position).unwrap_or(ABSENT);
        }
        if let Some(entry) = self.sparse.get_mut(slot as usize) {
            *entry = ABSENT;
        }
        self.dense.pop();
        self.values.pop()
    }

    /// Every component, in ascending entity-slot order.
    ///
    /// **This is the order the engine promises**, and it is why the store
    /// exists in this shape. It walks `sparse`, so its cost is
    /// proportional to the highest occupied slot rather than to the number
    /// of components — a store scattered across a wide slot range pays for
    /// the gaps. That is the measured cost of a defined order, and the
    /// entity allocator reuses low slots first precisely to keep it small.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &T)> + '_ {
        self.sparse
            .iter()
            .enumerate()
            .filter(|(_, position)| **position != ABSENT)
            .filter_map(move |(slot, position)| {
                let value = self.values.get(*position as usize)?;
                Some((u32::try_from(slot).ok()?, value))
            })
    }

    /// Every component in slot order, mutably.
    ///
    /// Collects the visit order first, because handing out `&mut` while
    /// borrowing `sparse` to decide the order is not something the borrow
    /// checker will allow — and the honest fix is one allocation per
    /// call, not `unsafe`.
    pub fn for_each_mut(&mut self, mut visit: impl FnMut(u32, &mut T)) {
        let order: Vec<(u32, u32)> = self
            .sparse
            .iter()
            .enumerate()
            .filter(|(_, position)| **position != ABSENT)
            .filter_map(|(slot, position)| Some((u32::try_from(slot).ok()?, *position)))
            .collect();
        for (slot, position) in order {
            if let Some(value) = self.values.get_mut(position as usize) {
                visit(slot, value);
            }
        }
    }

    /// The components in storage order, which is **unspecified**.
    ///
    /// Offered because it is what a system that does not care about order
    /// should use: it is a flat walk of a contiguous array, with none of
    /// the gap-skipping [`Store::iter`] pays for. Any system whose result
    /// depends on the order it sees is wrong to use this, and the name is
    /// the warning.
    pub fn iter_unordered(&self) -> impl Iterator<Item = &T> + '_ {
        self.values.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_store_is_empty() {
        let store: Store<u32> = Store::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert!(store.get(0).is_none());
        assert!(!store.contains(0));
    }

    /// `Default` is what a struct holding a store will use, so it has to
    /// agree with `new` rather than merely compile.
    #[test]
    fn default_and_new_agree() {
        let made: Store<u32> = Store::default();
        assert!(made.is_empty());
        assert_eq!(made.len(), Store::<u32>::new().len());
    }

    #[test]
    fn insert_then_get_returns_the_value() {
        let mut store = Store::new();
        assert!(store.insert(3, "three").is_none());
        assert_eq!(store.get(3), Some(&"three"));
        assert!(store.contains(3));
        assert_eq!(store.len(), 1);
        // A slot below the inserted one exists in `sparse` and is empty.
        assert!(!store.contains(0));
    }

    #[test]
    fn inserting_twice_replaces_and_returns_the_old_value() {
        let mut store = Store::new();
        store.insert(1, 10);
        assert_eq!(store.insert(1, 20), Some(10));
        assert_eq!(store.get(1), Some(&20));
        assert_eq!(store.len(), 1, "replacing must not grow the store");
    }

    #[test]
    fn get_mut_edits_in_place() {
        let mut store = Store::new();
        store.insert(2, 5);
        if let Some(value) = store.get_mut(2) {
            *value += 1;
        }
        assert_eq!(store.get(2), Some(&6));
        assert!(store.get_mut(9).is_none());
    }

    /// The swap-remove has to fix up the moved element's back-pointer.
    /// Getting this wrong is the classic sparse-set bug and it only shows
    /// when the removed element is not the last one.
    #[test]
    fn removing_from_the_middle_keeps_every_other_lookup_correct() {
        let mut store = Store::new();
        for slot in 0..5 {
            store.insert(slot, slot * 100);
        }
        assert_eq!(store.remove(1), Some(100));
        assert_eq!(store.len(), 4);
        assert!(!store.contains(1));
        for slot in [0u32, 2, 3, 4] {
            assert_eq!(store.get(slot), Some(&(slot * 100)), "slot {slot}");
        }
    }

    #[test]
    fn removing_the_last_element_is_also_correct() {
        let mut store = Store::new();
        store.insert(0, 'a');
        store.insert(1, 'b');
        assert_eq!(store.remove(1), Some('b'));
        assert_eq!(store.get(0), Some(&'a'));
        assert!(!store.contains(1));
        assert_eq!(store.remove(0), Some('a'));
        assert!(store.is_empty());
    }

    #[test]
    fn removing_what_is_not_there_returns_nothing() {
        let mut store: Store<u8> = Store::new();
        assert!(store.remove(7).is_none());
        store.insert(0, 1);
        assert!(store.remove(7).is_none());
        assert_eq!(store.len(), 1);
    }

    /// The contract: iteration is by ascending slot however the store was
    /// churned. Built by inserting out of order and removing from the
    /// middle, which is exactly what leaves `dense` scrambled.
    #[test]
    fn iteration_is_by_slot_whatever_the_churn() {
        let mut store = Store::new();
        for slot in [5u32, 1, 9, 3, 7] {
            store.insert(slot, slot);
        }
        store.remove(3);
        store.insert(2, 2);
        store.remove(9);

        let seen: Vec<u32> = store.iter().map(|(slot, _)| slot).collect();
        assert_eq!(seen, vec![1, 2, 5, 7]);

        // And the dense order is genuinely different, or the test above
        // would prove nothing.
        let dense: Vec<u32> = store.iter_unordered().copied().collect();
        assert_ne!(
            dense, seen,
            "dense order happened to match; pick harsher churn"
        );
    }

    #[test]
    fn for_each_mut_visits_in_slot_order_and_can_edit() {
        let mut store = Store::new();
        for slot in [4u32, 0, 2] {
            store.insert(slot, slot);
        }
        let mut order = Vec::new();
        store.for_each_mut(|slot, value| {
            order.push(slot);
            *value += 1;
        });
        assert_eq!(order, vec![0, 2, 4]);
        assert_eq!(store.get(0), Some(&1));
        assert_eq!(store.get(4), Some(&5));
    }

    #[test]
    fn the_store_survives_a_long_churn() {
        let mut store = Store::new();
        for round in 0..50u32 {
            for slot in 0..20u32 {
                store.insert(slot, round * 100 + slot);
            }
            for slot in (0..20u32).step_by(3) {
                store.remove(slot);
            }
        }
        let seen: Vec<u32> = store.iter().map(|(slot, _)| slot).collect();
        let expected: Vec<u32> = (0..20).filter(|slot| slot % 3 != 0).collect();
        assert_eq!(seen, expected);
        assert_eq!(store.len(), expected.len());
    }
}
