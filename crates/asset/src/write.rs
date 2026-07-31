//! Building a pack, deterministically.
//!
//! The one property this file exists to guarantee: **the same set of
//! named blobs produces byte-identical output, whatever order they were
//! added in, on any platform.** Directory iteration order is not stable
//! across filesystems, so a pack built from the same tree on two machines
//! would otherwise differ — and a content-addressed format whose bytes
//! depend on the order someone happened to walk a directory is not
//! content-addressed at all.
//!
//! Sorting by name is the whole mechanism. It is cheap, it is testable
//! directly, and it is what makes lookup a binary search on the reading
//! side.

use crate::error::BuildError;
use crate::hash::fnv1a64;
use crate::layout::{
    ENTRY_BYTES, FORMAT, HEADER_BYTES, MAGIC, MAX_NAME_BYTES, OFF_COUNT, OFF_DATA_LEN, OFF_FORMAT,
    OFF_NAMES_LEN,
};

/// Collects named blobs and writes them out as a pack.
#[derive(Debug, Default)]
pub struct PackBuilder {
    items: Vec<(String, Vec<u8>)>,
}

impl PackBuilder {
    /// An empty builder. An empty pack is legal and round-trips.
    #[must_use]
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// How many entries have been added.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether nothing has been added yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Add one named blob.
    ///
    /// # Errors
    ///
    /// [`BuildError::EmptyName`] or [`BuildError::NameTooLong`] for a name
    /// no reader would accept, and [`BuildError::DuplicateName`] when the
    /// name is already present.
    ///
    /// Duplicates are refused here rather than resolved. A pack holding
    /// two `mesh/hero` entries has no correct reading, and silently
    /// keeping the last one is the kind of decision that is discovered
    /// years later by someone whose asset did not change when they
    /// changed it.
    pub fn insert(&mut self, name: &str, bytes: &[u8]) -> Result<(), BuildError> {
        if name.is_empty() {
            return Err(BuildError::EmptyName);
        }
        if name.len() > MAX_NAME_BYTES {
            return Err(BuildError::NameTooLong {
                name: name.to_string(),
                len: name.len(),
            });
        }
        if self.items.iter().any(|(existing, _)| existing == name) {
            return Err(BuildError::DuplicateName {
                name: name.to_string(),
            });
        }
        self.items.push((name.to_string(), bytes.to_vec()));
        Ok(())
    }

    /// Serialise the pack.
    ///
    /// # Errors
    ///
    /// [`BuildError::TooLarge`] when the entry count or a region would
    /// exceed the widths the format reserves for them. Checked rather
    /// than truncated: a silently wrapped length is a reader pointed at
    /// arbitrary bytes.
    pub fn finish(mut self) -> Result<Vec<u8>, BuildError> {
        // The whole determinism guarantee, in one line. Sorting by name
        // makes insertion order irrelevant to the output bytes.
        self.items.sort_by(|left, right| left.0.cmp(&right.0));

        let count = u32::try_from(self.items.len()).map_err(|_| BuildError::TooLarge {
            field: "count",
            value: self.items.len() as u64,
        })?;

        let names_len: usize = self.items.iter().map(|(name, _)| name.len()).sum();
        let names_len = u32::try_from(names_len).map_err(|_| BuildError::TooLarge {
            field: "names_len",
            value: names_len as u64,
        })?;
        let data_len: usize = self.items.iter().map(|(_, bytes)| bytes.len()).sum();
        let data_len = u32::try_from(data_len).map_err(|_| BuildError::TooLarge {
            field: "data_len",
            value: data_len as u64,
        })?;

        let mut out = Vec::with_capacity(
            HEADER_BYTES + self.items.len() * ENTRY_BYTES + names_len as usize + data_len as usize,
        );
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&names_len.to_le_bytes());
        out.extend_from_slice(&data_len.to_le_bytes());
        debug_assert_eq!(out.len(), HEADER_BYTES);
        debug_assert_eq!(OFF_FORMAT + 4, OFF_COUNT);
        debug_assert_eq!(OFF_NAMES_LEN + 4, OFF_DATA_LEN);

        let mut name_off: u32 = 0;
        let mut data_off: u64 = 0;
        for (name, bytes) in &self.items {
            // Each length was bounded above by the region totals, so
            // these conversions cannot fail; they are written as checked
            // arithmetic anyway because an unchecked cast here is a
            // reader pointed somewhere it should not be.
            let this_name = u32::try_from(name.len()).map_err(|_| BuildError::TooLarge {
                field: "name_len",
                value: name.len() as u64,
            })?;
            let this_data = u64::try_from(bytes.len()).map_err(|_| BuildError::TooLarge {
                field: "data_len",
                value: bytes.len() as u64,
            })?;
            out.extend_from_slice(&fnv1a64(bytes).to_le_bytes());
            out.extend_from_slice(&name_off.to_le_bytes());
            out.extend_from_slice(&this_name.to_le_bytes());
            out.extend_from_slice(&data_off.to_le_bytes());
            out.extend_from_slice(&this_data.to_le_bytes());
            name_off = name_off.saturating_add(this_name);
            data_off = data_off.saturating_add(this_data);
        }

        for (name, _) in &self.items {
            out.extend_from_slice(name.as_bytes());
        }
        for (_, bytes) in &self.items {
            out.extend_from_slice(bytes);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the whole file exists for: insertion order does not
    /// reach the output.
    #[test]
    fn insertion_order_does_not_change_the_bytes() {
        let mut forward = PackBuilder::new();
        forward.insert("b/two", b"second").expect("insert");
        forward.insert("a/one", b"first").expect("insert");
        forward.insert("c/three", b"third").expect("insert");

        let mut backward = PackBuilder::new();
        backward.insert("c/three", b"third").expect("insert");
        backward.insert("b/two", b"second").expect("insert");
        backward.insert("a/one", b"first").expect("insert");

        assert_eq!(
            forward.finish().expect("pack"),
            backward.finish().expect("pack"),
            "the same entries in a different order produced different bytes"
        );
    }

    /// And the same builder twice, which catches anything that leaks in
    /// from outside the inputs — a clock, an address, a hash seed.
    #[test]
    fn the_same_inputs_pack_identically_twice() {
        let build = || {
            let mut b = PackBuilder::new();
            b.insert("x", b"one").expect("insert");
            b.insert("y", b"two").expect("insert");
            b.finish().expect("pack")
        };
        assert_eq!(build(), build());
    }

    /// The accessor pair, which a caller uses to decide whether to write
    /// anything at all.
    #[test]
    fn the_builder_reports_what_it_holds() {
        let mut builder = PackBuilder::new();
        assert!(builder.is_empty());
        assert_eq!(builder.len(), 0);
        builder.insert("one", b"x").expect("insert");
        assert!(!builder.is_empty());
        assert_eq!(builder.len(), 1);
    }

    #[test]
    fn an_empty_pack_is_just_a_header() {
        let bytes = PackBuilder::new().finish().expect("pack");
        assert_eq!(bytes.len(), HEADER_BYTES);
        assert!(bytes.starts_with(&MAGIC));
    }

    #[test]
    fn a_duplicate_name_is_refused_rather_than_resolved() {
        let mut b = PackBuilder::new();
        b.insert("same", b"first").expect("insert");
        let error = b.insert("same", b"second").expect_err("must refuse");
        assert_eq!(
            error,
            BuildError::DuplicateName {
                name: "same".to_string()
            }
        );
    }

    #[test]
    fn an_empty_name_is_refused() {
        let mut b = PackBuilder::new();
        assert_eq!(
            b.insert("", b"x").expect_err("must refuse"),
            BuildError::EmptyName
        );
    }

    #[test]
    fn an_over_long_name_is_refused() {
        let mut b = PackBuilder::new();
        let name = "n".repeat(MAX_NAME_BYTES + 1);
        let error = b.insert(&name, b"x").expect_err("must refuse");
        assert!(matches!(error, BuildError::NameTooLong { .. }));
    }

    /// A zero-length blob is a real entry, not an absence.
    #[test]
    fn an_entry_may_hold_no_bytes() {
        let mut b = PackBuilder::new();
        b.insert("empty", b"").expect("insert");
        let bytes = b.finish().expect("pack");
        assert_eq!(bytes.len(), HEADER_BYTES + ENTRY_BYTES + "empty".len());
    }
}
