//! Fixed-capacity object pool with generation-checked handles.

/// A handle into a [`Pool`]. Stale handles (the slot was freed or reused)
/// are detected by generation and miss. The generation is 32 bits: a
/// stale handle could false-hit only after the *same slot* is recycled
/// 2³² times while that exact handle is retained — accepted for now and
/// revisited if a consumer holds handles across billions of recycles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    index: u32,
    generation: u32,
}

/// One storage slot. `value` is `Some` exactly while the slot is
/// occupied; `generation` counts releases, so a handle issued for an
/// earlier occupant of the same slot misses.
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// A fixed-capacity pool: all storage acquired at construction, no
/// growth, O(1) insert/remove/lookup. Entirely safe code.
///
/// Thread affinity: `Pool<T>` is `Send`/`Sync` exactly when `T` is (the
/// auto traits are the ground truth); there is no interior mutability,
/// so cross-thread use still requires external `&mut` discipline.
pub struct Pool<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    live: usize,
}

impl<T> Pool<T> {
    /// A pool with room for `capacity` values. Handles index with 32
    /// bits, so capacities above `u32::MAX` are clamped to `u32::MAX`
    /// (and are almost certainly a bug upstream — debug assertion).
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        debug_assert!(
            u32::try_from(capacity).is_ok(),
            "pool capacity exceeds the 32-bit handle index space"
        );
        let clamped = u32::try_from(capacity).unwrap_or(u32::MAX);
        let mut slots = Vec::with_capacity(clamped as usize);
        for _ in 0..clamped {
            slots.push(Slot {
                generation: 0,
                value: None,
            });
        }
        // Reverse order so `pop` hands out index 0 first.
        let free: Vec<u32> = (0..clamped).rev().collect();
        Self {
            slots,
            free,
            live: 0,
        }
    }

    /// Insert a value.
    ///
    /// # Errors
    ///
    /// `Err(value)` when the pool is at capacity — the value comes back
    /// untouched so the caller keeps ownership.
    pub fn insert(&mut self, value: T) -> Result<Handle, T> {
        let claimed = self.free.pop();
        // A free-list entry always indexes an existing, vacant slot:
        // entries come from `0..capacity` at construction and from an
        // index that already passed a bounds check in `remove`. The
        // lookup below therefore misses only when the list was empty —
        // the pool is full — and the value goes back to the caller.
        debug_assert!(
            claimed.is_none_or(|index| (index as usize) < self.slots.len()),
            "the free list must only hold indices of existing slots"
        );
        let slots = &mut self.slots;
        let Some((index, slot)) =
            claimed.and_then(|index| slots.get_mut(index as usize).map(|slot| (index, slot)))
        else {
            return Err(value);
        };
        // Occupying a slot that already holds a value would drop the
        // previous occupant silently and hand out a second handle with
        // its exact identity — two live handles onto one slot. The
        // free-list discipline makes that unreachable, so this refuses
        // rather than defends: a graceful full-pool answer costs one
        // cold branch and cannot become aliasing if the discipline ever
        // breaks.
        debug_assert!(slot.value.is_none(), "the free list held an occupied slot");
        if slot.value.is_some() {
            return Err(value);
        }
        let generation = slot.generation;
        slot.value = Some(value);
        self.live += 1;
        Ok(Handle { index, generation })
    }

    /// Remove and return the value, or `None` for stale/invalid handles.
    pub fn remove(&mut self, handle: Handle) -> Option<T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        // A live handle's generation always names an *occupied* slot:
        // releasing bumps the generation past it. A vacant slot here is
        // therefore a miss, not a removal.
        let value = slot.value.take()?;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(handle.index);
        self.live -= 1;
        Some(value)
    }

    /// Borrow the value, or `None` for stale/invalid handles.
    #[must_use]
    pub fn get(&self, handle: Handle) -> Option<&T> {
        let slot = self.slots.get(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_ref()
    }

    /// Mutably borrow the value, or `None` for stale/invalid handles.
    #[must_use]
    pub fn get_mut(&mut self, handle: Handle) -> Option<&mut T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_mut()
    }

    /// Live value count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Total slot capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of a caught panic, whichever payload shape it carries.
    fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
            .unwrap_or_default()
    }

    #[test]
    fn insert_get_remove_round_trip() {
        let mut pool: Pool<u64> = Pool::with_capacity(4);
        let handle = pool.insert(7).expect("has room");
        assert_eq!(pool.get(handle), Some(&7));
        *pool.get_mut(handle).expect("live") += 1;
        assert_eq!(pool.remove(handle), Some(8));
        assert!(pool.is_empty());
    }

    #[test]
    fn stale_handles_miss_after_remove_and_after_reuse() {
        let mut pool: Pool<&str> = Pool::with_capacity(2);
        let first = pool.insert("first").expect("has room");
        assert_eq!(pool.remove(first), Some("first"));
        assert_eq!(pool.get(first), None);
        assert_eq!(pool.remove(first), None);

        // The slot comes back with a bumped generation: the old handle
        // still misses even though the index is occupied again.
        let second = pool.insert("second").expect("has room");
        assert_eq!(pool.get(first), None);
        assert_eq!(pool.get(second), Some(&"second"));
    }

    #[test]
    fn stale_handles_miss_through_get_mut_too() {
        // The mutable accessor is a separate path and must reject the
        // same handles the shared one does — a stale `get_mut` would
        // hand out a live reference to another occupant's value.
        let mut pool: Pool<u32> = Pool::with_capacity(1);
        let first = pool.insert(1).expect("has room");
        assert_eq!(pool.remove(first), Some(1));
        assert!(pool.get_mut(first).is_none(), "freed slot");

        let second = pool.insert(2).expect("slot recycled");
        assert!(
            pool.get_mut(first).is_none(),
            "reused slot, bumped generation"
        );
        assert_eq!(pool.get_mut(second).copied(), Some(2));
    }

    #[test]
    fn handles_from_a_bigger_pool_miss_instead_of_indexing_out_of_bounds() {
        let mut big: Pool<u8> = Pool::with_capacity(4);
        let _first = big.insert(1).expect("has room");
        let far = big.insert(2).expect("has room");

        let mut small: Pool<u8> = Pool::with_capacity(1);
        assert!(small.get(far).is_none());
        assert!(small.get_mut(far).is_none());
        assert_eq!(small.remove(far), None);
        assert!(small.is_empty(), "a foreign handle changes nothing");
    }

    #[test]
    fn a_full_pool_returns_the_value() {
        let mut pool: Pool<u8> = Pool::with_capacity(1);
        let _keep = pool.insert(1).expect("has room");
        assert_eq!(pool.insert(2), Err(2));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn capacity_zero_is_always_full() {
        let mut pool: Pool<u8> = Pool::with_capacity(0);
        assert_eq!(pool.insert(1), Err(1));
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn slots_recycle_through_the_free_list() {
        let mut pool: Pool<u32> = Pool::with_capacity(2);
        let a = pool.insert(1).expect("room");
        let b = pool.insert(2).expect("room");
        assert!(pool.insert(3).is_err());
        assert_eq!(pool.remove(a), Some(1));
        let c = pool.insert(3).expect("slot recycled");
        assert_eq!(pool.get(c), Some(&3));
        assert_eq!(pool.get(b), Some(&2));
        assert_eq!(pool.len(), 2);
    }

    /// Removal must consult the *value* before touching the generation
    /// or the free list: a handle whose generation happens to match a
    /// VACANT slot is a miss, not a removal. Get that order wrong and
    /// the slot is pushed onto the free list twice, which later hands
    /// out two live handles with the same identity. A foreign handle
    /// from another pool is the cheapest way to present a matching
    /// generation over an empty slot.
    #[test]
    fn a_handle_matching_a_vacant_slots_generation_removes_nothing() {
        let mut donor: Pool<u8> = Pool::with_capacity(2);
        let foreign = donor.insert(1).expect("room");

        let mut pool: Pool<u8> = Pool::with_capacity(2);
        assert_eq!(
            pool.remove(foreign),
            None,
            "vacant slot, matching generation"
        );

        // The free list must still hold exactly two entries.
        let a = pool.insert(10).expect("room");
        let b = pool.insert(20).expect("room");
        assert_eq!(pool.insert(30), Err(30), "capacity is still two");
        assert_eq!(pool.get(a), Some(&10));
        assert_eq!(pool.get(b), Some(&20));
        assert_eq!(pool.len(), 2);
    }

    /// `insert`'s bounds check guards a state the public API cannot
    /// produce — the free list is private and only ever receives indices
    /// that already passed a bounds check. Corrupting the list from
    /// inside the crate is the only way to drive the guard, and it must
    /// name the broken invariant rather than let the lookup fall through
    /// to the indistinguishable "pool is full" answer.
    ///
    /// This proves the ASSERTION, not the `Err` return below it: the two
    /// share one condition (`index < slots.len()`), so wherever the
    /// return would fire the assertion fires first. The `Err` itself is
    /// reached by the other route — an empty free list — which
    /// `a_full_pool_returns_the_value` already covers.
    #[test]
    fn a_free_list_entry_outside_the_slot_array_is_named_not_swallowed() {
        let mut pool: Pool<u8> = Pool::with_capacity(1);
        pool.free.push(9);

        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.insert(7)));
        // The refusal is an assertion with debug assertions on and a
        // returned Err without them; the suite runs under both, and the
        // arm that does not apply is selected out rather than left in
        // the binary as a region no coverage run can enter.
        #[cfg(debug_assertions)]
        {
            let message = panic_text(
                refused
                    .expect_err("an out-of-range free-list entry must be refused")
                    .as_ref(),
            );
            assert!(
                message.contains("the free list must only hold indices of existing slots"),
                "unexpected payload: {message}"
            );
        }
        #[cfg(not(debug_assertions))]
        assert_eq!(
            refused.expect("with assertions off the entry is refused by returning"),
            Err(7),
            "the value comes back to the caller"
        );

        // Nothing was stored, and the sound part of the pool survived the
        // refusal: the one real slot is still free and still the only one.
        assert_eq!(pool.len(), 0, "the refused insert stored nothing");
        let handle = pool.insert(7).expect("the real slot is still free");
        assert_eq!(pool.get(handle), Some(&7));
        assert_eq!(pool.insert(8), Err(8), "capacity is still one");
    }

    /// A free-list entry naming an OCCUPIED slot is the corruption that
    /// costs memory safety at the API level: filling that slot would drop
    /// the previous occupant without anyone calling `remove`, and hand
    /// back a second handle carrying the live one's exact identity —
    /// index and generation. `String` occupants make a silent
    /// displacement observable: the survivor is read back through the
    /// original handle.
    ///
    /// Same caveat as the bounds test: the assertion and the `Err` return
    /// under it test `slot.value` for the same thing, so with debug
    /// assertions on (test builds) the assertion is what fires, and it is
    /// what this proves. It fires BEFORE the write, which is the property
    /// that matters — the occupant is still there afterwards.
    #[test]
    fn a_free_list_entry_naming_an_occupied_slot_never_displaces_its_occupant() {
        let mut pool: Pool<String> = Pool::with_capacity(2);
        let live = pool.insert("occupant".to_string()).expect("has room");
        pool.free.push(live.index);

        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.insert("intruder".to_string())
        }));
        // As above: loud with debug assertions, a returned Err without
        // them, and only the applicable arm is compiled.
        #[cfg(debug_assertions)]
        {
            let message = panic_text(
                refused
                    .expect_err("a duplicated free-list entry must be refused")
                    .as_ref(),
            );
            assert!(
                message.contains("the free list held an occupied slot"),
                "unexpected payload: {message}"
            );
        }
        #[cfg(not(debug_assertions))]
        assert_eq!(
            refused.expect("with assertions off the entry is refused by returning"),
            Err("intruder".to_string()),
            "the value comes back to the caller"
        );

        // The occupant never moved: same handle, same value, same count —
        // and it is still the one that owns the slot at removal time.
        assert_eq!(pool.get(live).map(String::as_str), Some("occupant"));
        assert_eq!(pool.len(), 1, "the intruder was never counted");
        assert_eq!(pool.remove(live).as_deref(), Some("occupant"));
        assert!(pool.is_empty());
    }
}
