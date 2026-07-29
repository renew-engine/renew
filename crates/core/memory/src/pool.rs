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

enum Slot<T> {
    Vacant { generation: u32 },
    Occupied { generation: u32, value: T },
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
            slots.push(Slot::Vacant { generation: 0 });
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
        let Some(index) = self.free.pop() else {
            return Err(value);
        };
        // Free-list entries always point at existing, vacant slots; both
        // fallback arms below are defensive (and asserted) rather than
        // reachable, and neither can panic in release.
        let Some(slot) = self.slots.get_mut(index as usize) else {
            debug_assert!(false, "free list pointed outside the slot array");
            return Err(value);
        };
        let generation = match slot {
            Slot::Vacant { generation } => *generation,
            Slot::Occupied { .. } => {
                debug_assert!(false, "free list pointed at an occupied slot");
                return Err(value);
            }
        };
        *slot = Slot::Occupied { generation, value };
        self.live += 1;
        Ok(Handle { index, generation })
    }

    /// Remove and return the value, or `None` for stale/invalid handles.
    pub fn remove(&mut self, handle: Handle) -> Option<T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        match slot {
            Slot::Occupied { generation, .. } if *generation == handle.generation => {
                let next_generation = generation.wrapping_add(1);
                let previous = core::mem::replace(
                    slot,
                    Slot::Vacant {
                        generation: next_generation,
                    },
                );
                self.free.push(handle.index);
                self.live -= 1;
                match previous {
                    Slot::Occupied { value, .. } => Some(value),
                    Slot::Vacant { .. } => None,
                }
            }
            _ => None,
        }
    }

    /// Borrow the value, or `None` for stale/invalid handles.
    #[must_use]
    pub fn get(&self, handle: Handle) -> Option<&T> {
        match self.slots.get(handle.index as usize)? {
            Slot::Occupied { generation, value } if *generation == handle.generation => Some(value),
            _ => None,
        }
    }

    /// Mutably borrow the value, or `None` for stale/invalid handles.
    #[must_use]
    pub fn get_mut(&mut self, handle: Handle) -> Option<&mut T> {
        match self.slots.get_mut(handle.index as usize)? {
            Slot::Occupied { generation, value } if *generation == handle.generation => Some(value),
            _ => None,
        }
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
}
