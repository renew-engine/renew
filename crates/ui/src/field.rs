//! Text fields: a fixed pool, and the editing a caller would otherwise
//! write once per form.
//!
//! **A type decision rather than a tunable.** [`MAX_FIELDS`] nodes may
//! hold text and each holds [`MAX_FIELD_BYTES`], so the pool is present
//! in every tree at a size nobody will notice and no capacity has to be
//! declared at construction. An interface wanting a ninth simultaneous
//! field is a different kind of interface and should argue for itself
//! rather than arrive by a number being raised — the same reasoning the
//! networking crate's seat ceiling states.
//!
//! Two designs were tried and discarded first: a buffer per node, which
//! multiplies to tens of kilobytes for a tree with one field, and an
//! arena sized by a new capacity, which dragged fifty-three
//! construction sites behind it and told none of them anything.

use crate::NodeId;

/// How many nodes may hold text at once.
pub const MAX_FIELDS: usize = 8;

/// How many bytes one field holds.
///
/// Sixty-four is an address, a name, a seed, or a search — the things a
/// form asks for. It is not a paragraph, and this crate does not offer
/// one: no scrolling, no wrapping, no multi-line.
pub const MAX_FIELD_BYTES: usize = 64;

/// What a caller can ask a focused field to do, as a closed set.
///
/// **Closed rather than a key code**, because an open one would let a
/// caller send `F7` and force this crate to have an opinion about it.
/// Which key means which operation is the driver's answer, and it
/// differs per platform and per person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditOp {
    /// Remove the byte before the cursor.
    Backspace,
    /// Remove the byte at the cursor.
    Delete,
    /// Move the cursor one byte earlier.
    Left,
    /// Move the cursor one byte later.
    Right,
    /// Move the cursor to the start.
    Home,
    /// Move the cursor to the end.
    End,
}

impl EditOp {
    /// A small stable number, for the digest fold. Written out rather
    /// than derived from the discriminant, so reordering the variants
    /// cannot silently change a fingerprint.
    pub(crate) const fn code(self) -> u64 {
        match self {
            Self::Backspace => 1,
            Self::Delete => 2,
            Self::Left => 3,
            Self::Right => 4,
            Self::Home => 5,
            Self::End => 6,
        }
    }
}

/// One field's storage and cursor.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Field {
    /// Which node owns this slot, or `None` while it is free.
    pub(crate) owner: Option<NodeId>,
    bytes: [u8; MAX_FIELD_BYTES],
    len: u8,
    /// Where the next insertion lands, in bytes from the start.
    /// Always `<= len`.
    cursor: u8,
}

impl Field {
    pub(crate) const EMPTY: Self = Self {
        owner: None,
        bytes: [0; MAX_FIELD_BYTES],
        len: 0,
        cursor: 0,
    };

    pub(crate) fn text(&self) -> &[u8] {
        self.bytes.get(..usize::from(self.len)).unwrap_or_default()
    }

    pub(crate) const fn cursor(&self) -> u8 {
        self.cursor
    }

    /// Insert one scalar, encoded as UTF-8.
    ///
    /// **Refuses rather than truncates.** A field that took half a
    /// character would hold bytes that are not text, and every reader
    /// after it would have to cope. Returns whether anything changed.
    pub(crate) fn insert(&mut self, ch: char) -> bool {
        let mut encoded = [0u8; 4];
        let encoded = ch.encode_utf8(&mut encoded).as_bytes();
        let len = usize::from(self.len);
        let at = usize::from(self.cursor);
        let Some(after) = len.checked_add(encoded.len()) else {
            return false;
        };
        if after > MAX_FIELD_BYTES {
            return false;
        }
        // Shift the tail right, from the end, so the copy never reads a
        // byte it has already written.
        for index in (at..len).rev() {
            let Some(from) = self.bytes.get(index).copied() else {
                return false;
            };
            let Some(to) = index
                .checked_add(encoded.len())
                .and_then(|i| self.bytes.get_mut(i))
            else {
                return false;
            };
            *to = from;
        }
        for (offset, byte) in encoded.iter().enumerate() {
            let Some(slot) = at.checked_add(offset).and_then(|i| self.bytes.get_mut(i)) else {
                return false;
            };
            *slot = *byte;
        }
        self.len = u8::try_from(after).unwrap_or(self.len);
        self.cursor = u8::try_from(at.saturating_add(encoded.len())).unwrap_or(self.cursor);
        true
    }

    /// Apply an editing operation. Returns whether anything changed.
    pub(crate) fn edit(&mut self, op: EditOp) -> bool {
        match op {
            EditOp::Left => {
                if self.cursor == 0 {
                    return false;
                }
                self.cursor = self.step_back(self.cursor);
                true
            }
            EditOp::Right => {
                if self.cursor >= self.len {
                    return false;
                }
                self.cursor = self.step_forward(self.cursor);
                true
            }
            EditOp::Home => {
                let moved = self.cursor != 0;
                self.cursor = 0;
                moved
            }
            EditOp::End => {
                let moved = self.cursor != self.len;
                self.cursor = self.len;
                moved
            }
            EditOp::Backspace => {
                if self.cursor == 0 {
                    return false;
                }
                let from = self.step_back(self.cursor);
                self.remove(from, self.cursor);
                self.cursor = from;
                true
            }
            EditOp::Delete => {
                if self.cursor >= self.len {
                    return false;
                }
                let to = self.step_forward(self.cursor);
                self.remove(self.cursor, to);
                true
            }
        }
    }

    /// The start of the character ending at `at`.
    ///
    /// **Character, not byte.** Backspacing a byte out of a multi-byte
    /// scalar would leave the field holding something that is not text,
    /// which the accessor promises it never does.
    fn step_back(&self, at: u8) -> u8 {
        let mut index = at.saturating_sub(1);
        while index > 0 && self.is_continuation(index) {
            index = index.saturating_sub(1);
        }
        index
    }

    /// The end of the character starting at `at`.
    fn step_forward(&self, at: u8) -> u8 {
        let mut index = at.saturating_add(1);
        while index < self.len && self.is_continuation(index) {
            index = index.saturating_add(1);
        }
        index.min(self.len)
    }

    fn is_continuation(&self, index: u8) -> bool {
        self.bytes
            .get(usize::from(index))
            .is_some_and(|byte| byte & 0b1100_0000 == 0b1000_0000)
    }

    /// Remove `from..to`, closing the gap.
    fn remove(&mut self, from: u8, to: u8) {
        let width = usize::from(to.saturating_sub(from));
        let len = usize::from(self.len);
        for index in usize::from(to)..len {
            let Some(byte) = self.bytes.get(index).copied() else {
                break;
            };
            let Some(slot) = index.checked_sub(width).and_then(|i| self.bytes.get_mut(i)) else {
                break;
            };
            *slot = byte;
        }
        self.len = self.len.saturating_sub(u8::try_from(width).unwrap_or(0));
    }
}

/// What the pool costs, stated where it cannot drift.
///
/// A review found the first prose figure wrong — 512 was the arithmetic
/// of the bytes alone, and a `Field` also carries an owner and two
/// cursors, with `Option<NodeId>` paying a discriminant beside a `u32`
/// and a `u64`. The number now comes from the compiler rather than from
/// anyone's addition, and the assertion below fails the build if a field
/// grows past what the docs claim.
pub const POOL_BYTES: usize = core::mem::size_of::<Field>() * MAX_FIELDS;

const _: () = assert!(
    POOL_BYTES <= 1024,
    "the field pool is meant to be a rounding error in every tree; past a kilobyte that stops \
     being obviously true and the claim in the module doc needs re-arguing rather than editing"
);
