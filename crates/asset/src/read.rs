//! Reading a pack the engine did not write.
//!
//! This is the crate's whole untrusted-input surface, and it is one
//! function. Everything it returns borrows the caller's bytes, so a
//! malformed file cannot cost an allocation, and a caller that has
//! already bounded the read has bounded the parse too.
//!
//! **The header is the least trustworthy part of a hostile file.** Every
//! length in it is checked against the bytes actually present before it
//! is used to slice anything, all arithmetic is checked, and the four
//! regions must account for the file exactly — not "at least", so a
//! truncated download and an appended payload are both refused.

use crate::error::PackError;
use crate::hash::fnv1a64;
use crate::layout::{
    ENTRY_BYTES, FORMAT, HEADER_BYTES, MAGIC, MAX_NAME_BYTES, OFF_COUNT, OFF_DATA_LEN, OFF_FORMAT,
    OFF_NAMES_LEN,
};

/// One entry, borrowed from the pack's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryRef<'a> {
    /// The entry's name. Unique within a pack, and sorted.
    pub name: &'a str,
    /// The digest the writer recorded for `bytes`.
    ///
    /// Not verified on read: see [`Pack::mismatched`], which is a
    /// separate and deliberately explicit step.
    pub hash: u64,
    /// The payload.
    pub bytes: &'a [u8],
}

/// A validated pack, borrowing the bytes it was read from.
#[derive(Clone, Debug)]
pub struct Pack<'a> {
    entries: Vec<EntryRef<'a>>,
}

/// Read a little-endian `u32` at `offset`, or `None` past the end.
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    let array: [u8; 4] = slice.try_into().ok()?;
    Some(u32::from_le_bytes(array))
}

/// Read a little-endian `u64` at `offset`, or `None` past the end.
fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let slice = bytes.get(offset..end)?;
    let array: [u8; 8] = slice.try_into().ok()?;
    Some(u64::from_le_bytes(array))
}

/// Narrow a `u64` to a `usize`, naming the field if it will not fit.
fn fit(value: u64, field: &'static str) -> Result<usize, PackError> {
    usize::try_from(value).map_err(|_| PackError::TooLarge { field, value })
}

impl<'a> Pack<'a> {
    /// Validate `bytes` and borrow them as a pack.
    ///
    /// # Errors
    ///
    /// A [`PackError`] naming exactly what was wrong. Every field is
    /// checked before use; nothing is trusted because the header said so.
    pub fn read(bytes: &'a [u8]) -> Result<Self, PackError> {
        if bytes.len() < HEADER_BYTES {
            return Err(PackError::NoHeader { len: bytes.len() });
        }
        if bytes.get(..MAGIC.len()) != Some(&MAGIC[..]) {
            return Err(PackError::NotAPack);
        }
        let format = u32_at(bytes, OFF_FORMAT).ok_or(PackError::NoHeader { len: bytes.len() })?;
        if format != FORMAT {
            return Err(PackError::UnknownFormat { found: format });
        }

        let count = u32_at(bytes, OFF_COUNT).ok_or(PackError::NoHeader { len: bytes.len() })?;
        let names_len =
            u32_at(bytes, OFF_NAMES_LEN).ok_or(PackError::NoHeader { len: bytes.len() })?;
        let data_len =
            u32_at(bytes, OFF_DATA_LEN).ok_or(PackError::NoHeader { len: bytes.len() })?;

        // The regions must account for the file exactly. Computed in u64
        // so a hostile count cannot wrap the sum on a 32-bit target and
        // land back inside the file.
        let table_bytes =
            u64::from(count)
                .checked_mul(ENTRY_BYTES as u64)
                .ok_or(PackError::TooLarge {
                    field: "count",
                    value: u64::from(count),
                })?;
        let declared = (HEADER_BYTES as u64)
            .checked_add(table_bytes)
            .and_then(|sum| sum.checked_add(u64::from(names_len)))
            .and_then(|sum| sum.checked_add(u64::from(data_len)))
            .ok_or(PackError::TooLarge {
                field: "declared size",
                value: u64::MAX,
            })?;
        if declared != bytes.len() as u64 {
            return Err(PackError::SizeMismatch {
                declared,
                actual: bytes.len(),
            });
        }

        let table_bytes = fit(table_bytes, "count")?;
        let names_start = HEADER_BYTES
            .checked_add(table_bytes)
            .ok_or(PackError::TooLarge {
                field: "count",
                value: u64::from(count),
            })?;
        let names_end = names_start
            .checked_add(names_len as usize)
            .ok_or(PackError::TooLarge {
                field: "names_len",
                value: u64::from(names_len),
            })?;
        // Both slices are in range: `declared` equalled the length above.
        let names = bytes
            .get(names_start..names_end)
            .ok_or(PackError::NoHeader { len: bytes.len() })?;
        let data = bytes
            .get(names_end..)
            .ok_or(PackError::NoHeader { len: bytes.len() })?;

        let mut entries: Vec<EntryRef<'a>> = Vec::with_capacity(count as usize);
        for index in 0..count as usize {
            let entry = parse_entry(bytes, index, names, data)?;
            if let Some(previous) = entries.last() {
                match entry.name.cmp(previous.name) {
                    core::cmp::Ordering::Less => return Err(PackError::Unsorted { index }),
                    core::cmp::Ordering::Equal => return Err(PackError::DuplicateName { index }),
                    core::cmp::Ordering::Greater => {}
                }
            }
            entries.push(entry);
        }

        Ok(Self { entries })
    }

    /// How many entries the pack holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pack holds nothing. An empty pack is legal.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry, in name order.
    pub fn entries(&self) -> impl Iterator<Item = &EntryRef<'a>> {
        self.entries.iter()
    }

    /// Look one entry up by name.
    ///
    /// A binary search, which the sort order in the format exists to
    /// make possible.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&EntryRef<'a>> {
        let found = self
            .entries
            .binary_search_by(|entry| entry.name.cmp(name))
            .ok()?;
        self.entries.get(found)
    }

    /// The entries whose payload does not hash to the recorded digest.
    ///
    /// Separate from [`Pack::read`] on purpose. Reading is what every
    /// consumer does and must stay proportional to the table; verifying
    /// touches every byte, so it is a step a caller asks for — `inspect`
    /// does, a runtime load would not.
    #[must_use]
    pub fn mismatched(&self) -> Vec<&EntryRef<'a>> {
        self.entries
            .iter()
            .filter(|entry| fnv1a64(entry.bytes) != entry.hash)
            .collect()
    }
}

/// Validate one fixed-width table entry and borrow what it points at.
///
/// Split out of [`Pack::read`] so the header arithmetic and the per-entry
/// bounds checks can each be read on their own. Every field is checked
/// against the region it claims to live in; nothing is sliced on a length
/// the file supplied without first proving that length fits.
fn parse_entry<'a>(
    bytes: &'a [u8],
    index: usize,
    names: &'a [u8],
    data: &'a [u8],
) -> Result<EntryRef<'a>, PackError> {
    let short = || PackError::NoHeader { len: bytes.len() };
    let base = HEADER_BYTES + index * ENTRY_BYTES;
    let hash = u64_at(bytes, base).ok_or_else(short)?;
    let name_at = u32_at(bytes, base + 8).ok_or_else(short)?;
    let name_size = u32_at(bytes, base + 12).ok_or_else(short)?;
    let blob_at = u64_at(bytes, base + 16).ok_or_else(short)?;
    let blob_size = u64_at(bytes, base + 24).ok_or_else(short)?;

    if name_size == 0 {
        return Err(PackError::EmptyName { index });
    }
    if name_size as usize > MAX_NAME_BYTES {
        return Err(PackError::NameTooLong {
            index,
            len: name_size as usize,
        });
    }
    let out_of_names = || PackError::NameOutOfRange {
        index,
        offset: name_at,
        len: name_size,
    };
    let name_end = name_at.checked_add(name_size).ok_or_else(out_of_names)?;
    let raw = names
        .get(name_at as usize..name_end as usize)
        .ok_or_else(out_of_names)?;
    let name = core::str::from_utf8(raw).map_err(|_| PackError::NameNotUtf8 { index })?;

    let out_of_data = || PackError::DataOutOfRange {
        index,
        offset: blob_at,
        len: blob_size,
    };
    let blob_end = blob_at.checked_add(blob_size).ok_or_else(out_of_data)?;
    let from = fit(blob_at, "data_off")?;
    let to = fit(blob_end, "data_off + data_len")?;
    let payload = data.get(from..to).ok_or_else(out_of_data)?;

    Ok(EntryRef {
        name,
        hash,
        bytes: payload,
    })
}
