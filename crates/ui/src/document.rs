//! The compiled document: a versioned, fixed-width binary blob a tree
//! is read from, and captured into.
//!
//! **The shape follows the asset pack.** An eight-byte magic, a
//! version refused outright when unknown, whole-file size accounting
//! checked in `u64` before any entry is touched, an explicit node
//! ceiling, and bounds established before every read. Nothing is
//! trusted because the header said so; every refusal names what it
//! saw.
//!
//! **A document is a tree by construction, in one canonical form.**
//! Record 0 is the root and carries the no-parent sentinel; every
//! later record's parent index is strictly less than its own; and the
//! records sit in depth-first order — each record's parent must still
//! be on the open ancestor path when it appears, so siblings' subtrees
//! never interleave. Size bits under a cleared flag must be zero. A
//! blob that violates any of these is refused, so a read document
//! cannot hold a cycle, an orphan, a forward reference, or a second
//! spelling of the same tree — [`capture`] of an instantiated
//! document reproduces its bytes exactly, for every document this
//! reader accepts — and instantiation never needs to check again.
//! Sibling order is document order.
//!
//! **Version 2 carries structure, base styles, and the compiled
//! state tables**: a shared pool of complete resolved patches and a
//! per-node index for every combination of the four interaction
//! states. The canonical form extends over them — the pool sits in
//! first-use order over the document walk, every entry is referenced,
//! and dead freight refuses — so capture stays the exact inverse of
//! reading. Version 1 carried structure and base styles alone; the
//! version field was the evolution path, and it was taken.

use renew_fixed::Fixed;

use crate::layout::{Align, Direction, Edges, Size, Style};
use crate::{NodeId, Ui, UiLimits};

/// The document's opening bytes: seven ASCII characters and a NUL.
pub const MAGIC: [u8; 8] = *b"RENEWUI\0";

/// The one blob layout this reader understands. Anything else is
/// refused outright — version negotiation is a writer's job. Version
/// two added the state-patch pool and the per-node combination
/// tables; version one is refused like any other stranger.
pub const VERSION: u32 = 2;

/// Header: `MAGIC` + `version` + `node_count` + `patch_count`.
pub const HEADER_BYTES: usize = 20;
const OFF_VERSION: usize = 8;
const OFF_COUNT: usize = 12;
const OFF_PATCH_COUNT: usize = 16;

/// One node record, fixed width.
///
/// | offset | field |
/// |---|---|
/// | 0 | parent index, `u32`; [`NO_PARENT`] on the root only |
/// | 4 | direction, justify, align_cross, size flags — one byte each |
/// | 8 | width bits, `i64` (meaningful when flag bit 0 is set) |
/// | 16 | height bits, `i64` (meaningful when flag bit 1 is set) |
/// | 24 | margin: left, right, top, bottom bits, `i64` each |
/// | 56 | padding: left, right, top, bottom bits, `i64` each |
/// | 88 | gap bits, `i64` |
/// | 96 | grow, `u32` |
/// | 100 | background RGBA, four bytes |
/// | 104 | state table: sixteen `u16` patch indices, one per state
///   combination, [`crate::NO_PATCH`] wearing the base |
pub const NODE_BYTES: usize = 136;
const OFF_PARENT: usize = 0;
const OFF_DIRECTION: usize = 4;
const OFF_JUSTIFY: usize = 5;
const OFF_ALIGN_CROSS: usize = 6;
const OFF_SIZE_FLAGS: usize = 7;
const OFF_WIDTH: usize = 8;
const OFF_HEIGHT: usize = 16;
const OFF_MARGIN: usize = 24;
const OFF_PADDING: usize = 56;
const OFF_GAP: usize = 88;
const OFF_GROW: usize = 96;
const OFF_BACKGROUND: usize = 100;
const OFF_TABLE: usize = 104;

/// One pooled state patch, fixed width: a flags byte (bit zero:
/// wearing it moves geometry), three zero padding bytes, then the
/// same style block a node record carries at the same offsets.
pub const PATCH_BYTES: usize = 104;
const OFF_PATCH_FLAGS: usize = 0;
/// The one meaningful patch flag: layout fields differ from base.
const PATCH_TOUCHES_LAYOUT: u8 = 1;

/// Width is `Px` when set; `Auto` otherwise.
const FLAG_WIDTH_PX: u8 = 1;
/// Height is `Px` when set; `Auto` otherwise.
const FLAG_HEIGHT_PX: u8 = 2;

/// The root's parent field: no node has this index.
pub const NO_PARENT: u32 = u32::MAX;

/// The most nodes one document may declare. A ceiling, not a target:
/// the reader refuses past it before multiplying anything by anything.
pub const MAX_NODES: u32 = 4096;

/// The most pooled patches one document may declare, on the same
/// reasoning — and far below the table entries' own sentinel.
pub const MAX_PATCHES: u32 = 4096;

/// Why a byte string is not a document. Every variant names what was
/// seen, because "invalid" teaches a reader nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentError {
    /// Shorter than the fixed header.
    NoHeader {
        /// How many bytes there were.
        len: usize,
    },
    /// The magic does not open the bytes.
    NotADocument,
    /// A version this reader does not understand.
    UnknownVersion {
        /// What the header declared.
        found: u32,
    },
    /// A document must hold at least its root.
    Empty,
    /// More nodes than [`MAX_NODES`].
    TooManyNodes {
        /// What the header declared.
        count: u32,
    },
    /// The declared layout does not account for the bytes exactly.
    SizeMismatch {
        /// Header plus the declared node table, in bytes.
        declared: u64,
        /// What was handed over.
        actual: usize,
    },
    /// A parent index that breaks the forward-only rule: the root must
    /// carry [`NO_PARENT`], and every other record must name an
    /// earlier one.
    BadParent {
        /// Which record.
        index: u32,
        /// The parent field it carried.
        parent: u32,
    },
    /// A record whose parent is earlier but no longer on the open
    /// ancestor path: the records are not in depth-first order, and
    /// the format admits exactly one spelling of a tree.
    NotPreorder {
        /// Which record.
        index: u32,
        /// The parent it named.
        parent: u32,
    },
    /// Size bits carried under a cleared flag: dead payload the
    /// canonical form requires to be zero.
    UnsetSizeBits {
        /// Which record.
        index: u32,
        /// Which size.
        field: &'static str,
    },
    /// An enum byte outside its range.
    BadStyleByte {
        /// Which record.
        index: u32,
        /// Which field.
        field: &'static str,
        /// The byte it carried.
        value: u8,
    },
    /// More pooled patches than [`MAX_PATCHES`].
    TooManyPatches {
        /// What the header declared.
        count: u32,
    },
    /// A byte inside a pooled patch outside its range — an enum, the
    /// flags byte, or padding that must be zero.
    BadPatch {
        /// Which patch.
        index: u32,
        /// Which field.
        field: &'static str,
        /// The byte it carried.
        value: u8,
    },
    /// Size bits carried under a cleared flag inside a pooled patch.
    UnsetPatchBits {
        /// Which patch.
        index: u32,
        /// Which size.
        field: &'static str,
    },
    /// A state-table entry pointing past the declared pool.
    PatchOutOfPool {
        /// Which record's table.
        index: u32,
        /// The entry it carried.
        entry: u16,
    },
    /// A pooled patch whose layout flag lies about the reference: the
    /// flag must equal whether the patch's style moves geometry
    /// relative to the referencing node's base — the runtime trusts
    /// it in the frame loop, so the reader proves it at the door.
    WrongPatchFlag {
        /// Which record's table made the reference.
        index: u32,
        /// The pooled patch whose flag disagrees.
        entry: u16,
    },
    /// The pool is not in canonical first-use order, or holds entries
    /// no table references: scanning every table entry in document
    /// order, each patch index must first appear exactly when the
    /// count of already-seen patches equals it, and the scan must end
    /// having seen them all.
    PoolNotCanonical {
        /// How many patches had appeared when the rule broke.
        expected: u32,
        /// What appeared instead — or the declared count, when the
        /// scan ended short.
        found: u32,
    },
}

impl core::fmt::Display for DocumentError {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoHeader { len } => {
                write!(out, "{len} bytes cannot hold a {HEADER_BYTES}-byte header")
            }
            Self::NotADocument => write!(out, "the magic bytes are absent"),
            Self::UnknownVersion { found } => {
                write!(out, "version {found} is not the understood {VERSION}")
            }
            Self::Empty => write!(out, "a document must hold at least its root"),
            Self::TooManyNodes { count } => {
                write!(out, "{count} nodes exceed the ceiling of {MAX_NODES}")
            }
            Self::SizeMismatch { declared, actual } => {
                write!(
                    out,
                    "the header declares {declared} bytes, {actual} arrived"
                )
            }
            Self::BadParent { index, parent } => {
                write!(
                    out,
                    "record {index} names parent {parent}, which is not earlier"
                )
            }
            Self::NotPreorder { index, parent } => {
                write!(
                    out,
                    "record {index} names parent {parent}, which is closed: records \
                     sit in depth-first order"
                )
            }
            Self::UnsetSizeBits { index, field } => {
                write!(out, "record {index} carries {field} under a cleared flag")
            }
            Self::TooManyPatches { count } => {
                write!(out, "{count} patches exceed the ceiling of {MAX_PATCHES}")
            }
            Self::BadPatch {
                index,
                field,
                value,
            } => {
                write!(out, "patch {index}: {value} is not a {field}")
            }
            Self::UnsetPatchBits { index, field } => {
                write!(out, "patch {index} carries {field} under a cleared flag")
            }
            Self::PatchOutOfPool { index, entry } => {
                write!(
                    out,
                    "record {index} names patch {entry}, which is outside the pool"
                )
            }
            Self::WrongPatchFlag { index, entry } => {
                write!(
                    out,
                    "patch {entry}'s layout flag disagrees with record {index}'s base"
                )
            }
            Self::PoolNotCanonical { expected, found } => {
                write!(
                    out,
                    "the pool is not in first-use order: after {expected} patches, \
                     {found} appeared"
                )
            }
            Self::BadStyleByte {
                index,
                field,
                value,
            } => {
                write!(out, "record {index}: {value} is not a {field}")
            }
        }
    }
}

impl core::error::Error for DocumentError {}

/// A validated document, borrowing the bytes it was read from. Every
/// record has passed the structural and range checks; [`Self::tree`]
/// instantiates without re-checking.
#[derive(Debug)]
pub struct Document<'a> {
    nodes: &'a [u8],
    count: u32,
    pool: &'a [u8],
    patch_count: u32,
}

/// Read a little-endian `u32` at `offset`, or `None` past the end.
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    let array: [u8; 4] = slice.try_into().ok()?;
    Some(u32::from_le_bytes(array))
}

/// Read a little-endian `i64` at `offset` inside one node record.
/// The record's bounds were established before any field is read, so
/// this cannot run past the slice; the zero default is unreachable
/// and draws nothing if that invariant ever bent.
fn i64_in(record: &[u8], offset: usize) -> i64 {
    offset
        .checked_add(8)
        .and_then(|end| record.get(offset..end))
        .and_then(|slice| <[u8; 8]>::try_from(slice).ok())
        .map_or(0, i64::from_le_bytes)
}

/// One `Fixed` from record bytes.
fn fixed_in(record: &[u8], offset: usize) -> Fixed {
    Fixed::from_bits(i64_in(record, offset))
}

/// Read a little-endian `u16` at `offset`, the same posture as its
/// wider siblings: total where the caller proved bounds, fail-closed
/// where it did not — the sentinel wears the base and references
/// nothing, so it cannot slip past the pool checks.
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    offset
        .checked_add(2)
        .and_then(|end| bytes.get(offset..end))
        .and_then(|slice| <[u8; 2]>::try_from(slice).ok())
        .map_or(crate::NO_PATCH, u16::from_le_bytes)
}

/// Decode an alignment byte, naming the field on refusal.
fn align_of(index: u32, field: &'static str, value: u8) -> Result<Align, DocumentError> {
    match value {
        0 => Ok(Align::Start),
        1 => Ok(Align::Center),
        2 => Ok(Align::End),
        _ => Err(DocumentError::BadStyleByte {
            index,
            field,
            value,
        }),
    }
}

impl<'a> Document<'a> {
    /// Validate `bytes` and borrow them as a document.
    ///
    /// # Errors
    ///
    /// A [`DocumentError`] naming exactly what was wrong. Every field
    /// is checked before use, sizes are accounted in `u64` so a
    /// hostile count cannot wrap back inside the file, and the
    /// forward-parent rule is proven here so instantiation never
    /// re-checks it.
    pub fn read(bytes: &'a [u8]) -> Result<Self, DocumentError> {
        if bytes.len() < HEADER_BYTES {
            return Err(DocumentError::NoHeader { len: bytes.len() });
        }
        if bytes.get(..MAGIC.len()) != Some(&MAGIC[..]) {
            return Err(DocumentError::NotADocument);
        }
        // Total past the length check above; the zero defaults cannot
        // be reached, and would refuse if they were — version zero is
        // unknown and count zero is empty. Fail closed, not sideways.
        let version = u32_at(bytes, OFF_VERSION).unwrap_or(0);
        if version != VERSION {
            return Err(DocumentError::UnknownVersion { found: version });
        }
        let count = u32_at(bytes, OFF_COUNT).unwrap_or(0);
        if count == 0 {
            return Err(DocumentError::Empty);
        }
        if count > MAX_NODES {
            return Err(DocumentError::TooManyNodes { count });
        }
        // The zero-patch default here is fail-closed too: a header too
        // short to carry the count already refused above.
        let patch_count = u32_at(bytes, OFF_PATCH_COUNT).unwrap_or(0);
        if patch_count > MAX_PATCHES {
            return Err(DocumentError::TooManyPatches { count: patch_count });
        }
        // The ceilings above keep these multiplications small, but the
        // accounting stays in u64 anyway: the check must not depend on
        // the ceilings staying small forever.
        let table_bytes = u64::from(count) * (NODE_BYTES as u64);
        let pool_bytes = u64::from(patch_count) * (PATCH_BYTES as u64);
        let declared = (HEADER_BYTES as u64) + table_bytes + pool_bytes;
        if declared != bytes.len() as u64 {
            return Err(DocumentError::SizeMismatch {
                declared,
                actual: bytes.len(),
            });
        }
        // Both region bounds hold by the equality just proven.
        let pool_start = HEADER_BYTES + count as usize * NODE_BYTES;
        let nodes = bytes.get(HEADER_BYTES..pool_start).unwrap_or_default();
        let pool = bytes.get(pool_start..).unwrap_or_default();

        check_records(nodes, count)?;
        check_patches(pool, patch_count)?;
        check_tables(nodes, pool, count, patch_count)?;

        Ok(Self {
            nodes,
            count,
            pool,
            patch_count,
        })
    }

    /// How many nodes the document holds, root included.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.count
    }

    /// A document is never empty: [`Self::read`] refuses a zero count.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The style of record `index`, decoded. `None` past the end.
    #[must_use]
    pub fn style(&self, index: u32) -> Option<Style> {
        if index >= self.count {
            return None;
        }
        Some(style_in(record_of(self.nodes, index)))
    }

    /// How many pooled patches the document carries.
    #[must_use]
    pub fn patch_count(&self) -> u32 {
        self.patch_count
    }

    /// The pooled patch at `index`, decoded; `None` past the pool.
    #[must_use]
    pub fn patch(&self, index: u32) -> Option<crate::StatePatch> {
        if index >= self.patch_count {
            return None;
        }
        let record = patch_of(self.pool, index);
        let flags = record.get(OFF_PATCH_FLAGS).copied().unwrap_or(0);
        Some(crate::StatePatch {
            style: style_in(record),
            touches_layout: flags & PATCH_TOUCHES_LAYOUT != 0,
        })
    }

    /// The state table of record `index`; `None` past the end.
    #[must_use]
    pub fn state_table(&self, index: u32) -> Option<[u16; crate::STATE_COMBINATIONS]> {
        if index >= self.count {
            return None;
        }
        let record = record_of(self.nodes, index);
        let mut table = [crate::NO_PATCH; crate::STATE_COMBINATIONS];
        for (slot, entry) in table.iter_mut().enumerate() {
            *entry = u16_at(record, OFF_TABLE + slot * 2);
        }
        Some(table)
    }

    /// The parent index of record `index`; `None` for the root or past
    /// the end.
    #[must_use]
    pub fn parent(&self, index: u32) -> Option<u32> {
        if index == 0 || index >= self.count {
            return None;
        }
        u32_at(record_of(self.nodes, index), OFF_PARENT)
    }

    /// Instantiate the document as a tree sized exactly to it.
    ///
    /// The forward-parent rule was proven at read, so every insert
    /// lands under an already-built node and the arena is sized to
    /// hold every record; the assert is the contract, not a check.
    ///
    /// # Panics
    ///
    /// When the instantiated tree does not hold every record — a
    /// contract violation [`Self::read`]'s proof makes unreachable,
    /// asserted rather than assumed.
    #[must_use]
    pub fn tree(&self) -> Ui {
        let mut ui = Ui::new(UiLimits { nodes: self.count });
        let mut ids: Vec<NodeId> = Vec::with_capacity(self.count as usize);
        ids.push(ui.root());
        if let Some(style) = self.style(0) {
            ui.set_style(ui.root(), style);
        }
        for index in 1..self.count {
            let parent = self
                .parent(index)
                .and_then(|at| ids.get(at as usize).copied());
            debug_assert!(parent.is_some(), "read's proof builds parents first");
            let parent = parent.unwrap_or_else(|| ui.root());
            // The refusal arm is unreachable past read's proof — the
            // arena is sized to the count and the parent is already
            // built — and the fallback keeps the walk total so the
            // assert below stays the loud version of this sentence.
            let node = ui.insert(parent).unwrap_or(parent);
            if let Some(style) = self.style(index) {
                ui.set_style(node, style);
            }
            ids.push(node);
        }
        assert_eq!(
            ui.live(),
            self.count,
            "a validated document must instantiate every record"
        );
        if self.patch_count > 0 {
            let pool: Vec<crate::StatePatch> = (0..self.patch_count)
                .filter_map(|at| self.patch(at))
                .collect();
            // Validation proved the pool addressable and every table
            // entry inside it; a refusal here would be that proof
            // broken, said loudly.
            assert!(ui.set_patch_pool(pool), "a validated pool must load");
            for index in 0..self.count {
                if let Some(table) = self.state_table(index)
                    && table.iter().any(|&entry| entry != crate::NO_PATCH)
                {
                    let node = ids[index as usize];
                    assert!(
                        ui.set_state_table(node, table),
                        "a validated table must land"
                    );
                }
            }
        }
        ui
    }
}

/// The byte record of node `index`. Bounds hold by the size accounting
/// in `read`; an out-of-range index yields an empty slice whose reads
/// all default, which no validated path can reach.
fn record_of(nodes: &[u8], index: u32) -> &[u8] {
    let start = (index as usize) * NODE_BYTES;
    nodes.get(start..start + NODE_BYTES).unwrap_or_default()
}

/// The byte record of pooled patch `index`, same posture.
fn patch_of(pool: &[u8], index: u32) -> &[u8] {
    let start = (index as usize) * PATCH_BYTES;
    pool.get(start..start + PATCH_BYTES).unwrap_or_default()
}

/// Every node record's structural and style rules: the forward-parent
/// rule, the open-ancestor-path depth-first order — a record's parent
/// must still be on the path, or subtrees interleave and the blob is a
/// second spelling of a tree the canonical form already has one for —
/// and the enum, flag, and dead-bit checks.
fn check_records(nodes: &[u8], count: u32) -> Result<(), DocumentError> {
    let mut path: Vec<u32> = Vec::with_capacity(16);
    for index in 0..count {
        let record = record_of(nodes, index);
        // The default is the one value that cannot pass either check
        // below: unreachable, and fail-closed if ever bent.
        let parent = u32_at(record, OFF_PARENT).unwrap_or(index);
        let legal = if index == 0 {
            parent == NO_PARENT
        } else {
            parent < index
        };
        if !legal {
            return Err(DocumentError::BadParent { index, parent });
        }
        if index == 0 {
            path.push(0);
        } else {
            while path.last().is_some_and(|&open| open != parent) {
                path.pop();
            }
            if path.is_empty() {
                return Err(DocumentError::NotPreorder { index, parent });
            }
            path.push(index);
        }
        let direction = record.get(OFF_DIRECTION).copied().unwrap_or(u8::MAX);
        if direction > 1 {
            return Err(DocumentError::BadStyleByte {
                index,
                field: "direction",
                value: direction,
            });
        }
        align_of(
            index,
            "justify",
            record.get(OFF_JUSTIFY).copied().unwrap_or(u8::MAX),
        )?;
        align_of(
            index,
            "align_cross",
            record.get(OFF_ALIGN_CROSS).copied().unwrap_or(u8::MAX),
        )?;
        let flags = record.get(OFF_SIZE_FLAGS).copied().unwrap_or(u8::MAX);
        if flags & !(FLAG_WIDTH_PX | FLAG_HEIGHT_PX) != 0 {
            return Err(DocumentError::BadStyleByte {
                index,
                field: "size flags",
                value: flags,
            });
        }
        if flags & FLAG_WIDTH_PX == 0 && i64_in(record, OFF_WIDTH) != 0 {
            return Err(DocumentError::UnsetSizeBits {
                index,
                field: "width bits",
            });
        }
        if flags & FLAG_HEIGHT_PX == 0 && i64_in(record, OFF_HEIGHT) != 0 {
            return Err(DocumentError::UnsetSizeBits {
                index,
                field: "height bits",
            });
        }
    }
    Ok(())
}

/// Every pooled patch, held to the same style rules as a node, plus
/// its own flags byte and zero padding.
fn check_patches(pool: &[u8], patch_count: u32) -> Result<(), DocumentError> {
    for index in 0..patch_count {
        let record = patch_of(pool, index);
        let flags = record.get(OFF_PATCH_FLAGS).copied().unwrap_or(u8::MAX);
        if flags & !PATCH_TOUCHES_LAYOUT != 0 {
            return Err(DocumentError::BadPatch {
                index,
                field: "patch flags",
                value: flags,
            });
        }
        for at in 1..4 {
            let byte = record.get(at).copied().unwrap_or(u8::MAX);
            if byte != 0 {
                return Err(DocumentError::BadPatch {
                    index,
                    field: "padding",
                    value: byte,
                });
            }
        }
        let direction = record.get(OFF_DIRECTION).copied().unwrap_or(u8::MAX);
        if direction > 1 {
            return Err(DocumentError::BadPatch {
                index,
                field: "direction",
                value: direction,
            });
        }
        for (field, offset) in [("justify", OFF_JUSTIFY), ("align_cross", OFF_ALIGN_CROSS)] {
            let value = record.get(offset).copied().unwrap_or(u8::MAX);
            if value > 2 {
                return Err(DocumentError::BadPatch {
                    index,
                    field,
                    value,
                });
            }
        }
        let size_flags = record.get(OFF_SIZE_FLAGS).copied().unwrap_or(u8::MAX);
        if size_flags & !(FLAG_WIDTH_PX | FLAG_HEIGHT_PX) != 0 {
            return Err(DocumentError::BadPatch {
                index,
                field: "size flags",
                value: size_flags,
            });
        }
        if size_flags & FLAG_WIDTH_PX == 0 && i64_in(record, OFF_WIDTH) != 0 {
            return Err(DocumentError::UnsetPatchBits {
                index,
                field: "width bits",
            });
        }
        if size_flags & FLAG_HEIGHT_PX == 0 && i64_in(record, OFF_HEIGHT) != 0 {
            return Err(DocumentError::UnsetPatchBits {
                index,
                field: "height bits",
            });
        }
    }
    Ok(())
}

/// The tables, in one scan that proves four things at once: every
/// entry lands inside the pool, the pool sits in first-use order, no
/// pooled patch goes unreferenced, and every referenced patch's
/// layout flag tells the truth about the referencing node's base —
/// the runtime trusts that flag in the frame loop, so the reader
/// proves it at the door rather than letting a crafted blob skip
/// re-solves it needed. A canonical blob carries no dead freight and
/// admits exactly one spelling of its pool.
fn check_tables(
    nodes: &[u8],
    pool: &[u8],
    count: u32,
    patch_count: u32,
) -> Result<(), DocumentError> {
    let mut first_uses: u32 = 0;
    for index in 0..count {
        let record = record_of(nodes, index);
        for slot in 0..crate::STATE_COMBINATIONS {
            let entry = u16_at(record, OFF_TABLE + slot * 2);
            if entry == crate::NO_PATCH {
                continue;
            }
            if u32::from(entry) >= patch_count {
                return Err(DocumentError::PatchOutOfPool { index, entry });
            }
            if u32::from(entry) == first_uses {
                first_uses += 1;
            } else if u32::from(entry) > first_uses {
                return Err(DocumentError::PoolNotCanonical {
                    expected: first_uses,
                    found: u32::from(entry),
                });
            }
            let patch = patch_of(pool, u32::from(entry));
            let declared =
                patch.get(OFF_PATCH_FLAGS).copied().unwrap_or(0) & PATCH_TOUCHES_LAYOUT != 0;
            let truth = crate::state::moves_geometry(&style_in(record), &style_in(patch));
            if declared != truth {
                return Err(DocumentError::WrongPatchFlag { index, entry });
            }
        }
    }
    if first_uses != patch_count {
        return Err(DocumentError::PoolNotCanonical {
            expected: first_uses,
            found: patch_count,
        });
    }
    Ok(())
}

/// Decode the style block a node record and a pooled patch share —
/// the same fields at the same offsets, past their different first
/// words. The enum bytes were validated by read; the defaults here
/// are the unreachable arms of that proof, not a second decoder.
fn style_in(record: &[u8]) -> Style {
    let flags = record.get(OFF_SIZE_FLAGS).copied().unwrap_or(0);
    let size = |flag: u8, offset: usize| {
        if flags & flag == 0 {
            Size::Auto
        } else {
            Size::Px(fixed_in(record, offset))
        }
    };
    let edges = |offset: usize| Edges {
        left: fixed_in(record, offset),
        right: fixed_in(record, offset + 8),
        top: fixed_in(record, offset + 16),
        bottom: fixed_in(record, offset + 24),
    };
    let direction = if record.get(OFF_DIRECTION).copied().unwrap_or(0) == 1 {
        Direction::Column
    } else {
        Direction::Row
    };
    let realign = |value: u8| align_of(0, "validated", value).unwrap_or(Align::Start);
    let mut background = [0u8; 4];
    if let Some(bytes) = record.get(OFF_BACKGROUND..OFF_BACKGROUND + 4) {
        background.copy_from_slice(bytes);
    }
    Style {
        direction,
        width: size(FLAG_WIDTH_PX, OFF_WIDTH),
        height: size(FLAG_HEIGHT_PX, OFF_HEIGHT),
        margin: edges(OFF_MARGIN),
        padding: edges(OFF_PADDING),
        gap: fixed_in(record, OFF_GAP),
        grow: u32_at(record, OFF_GROW).unwrap_or(0),
        justify: realign(record.get(OFF_JUSTIFY).copied().unwrap_or(0)),
        align_cross: realign(record.get(OFF_ALIGN_CROSS).copied().unwrap_or(0)),
        background,
    }
}

/// Encode a style into a record's shared style block, leaving the
/// record's first word — parent, or patch flags — to the caller.
fn encode_style(record: &mut [u8], style: &Style) {
    record[OFF_DIRECTION] = match style.direction {
        Direction::Row => 0,
        Direction::Column => 1,
    };
    let align_byte = |align: Align| match align {
        Align::Start => 0,
        Align::Center => 1,
        Align::End => 2,
    };
    record[OFF_JUSTIFY] = align_byte(style.justify);
    record[OFF_ALIGN_CROSS] = align_byte(style.align_cross);
    let mut flags = 0;
    if let Size::Px(width) = style.width {
        flags |= FLAG_WIDTH_PX;
        record[OFF_WIDTH..OFF_WIDTH + 8].copy_from_slice(&width.to_bits().to_le_bytes());
    }
    if let Size::Px(height) = style.height {
        flags |= FLAG_HEIGHT_PX;
        record[OFF_HEIGHT..OFF_HEIGHT + 8].copy_from_slice(&height.to_bits().to_le_bytes());
    }
    record[OFF_SIZE_FLAGS] = flags;
    for (offset, value) in [(OFF_MARGIN, style.margin), (OFF_PADDING, style.padding)] {
        for (nth, bits) in [value.left, value.right, value.top, value.bottom]
            .into_iter()
            .enumerate()
        {
            let at = offset + nth * 8;
            record[at..at + 8].copy_from_slice(&bits.to_bits().to_le_bytes());
        }
    }
    record[OFF_GAP..OFF_GAP + 8].copy_from_slice(&style.gap.to_bits().to_le_bytes());
    record[OFF_GROW..OFF_GROW + 4].copy_from_slice(&style.grow.to_le_bytes());
    record[OFF_BACKGROUND..OFF_BACKGROUND + 4].copy_from_slice(&style.background);
}

/// Serialize a live tree into document bytes: the inverse of
/// [`Document::read`] + [`Document::tree`], used by tests as the
/// round-trip oracle and by tooling as the writer's backend. Walks
/// depth-first — parents before children, siblings in tree order —
/// which is the one record order the reader accepts, so capture of an
/// instantiated document reproduces its bytes exactly, for every
/// document the reader accepts.
///
/// # Panics
///
/// When the tree exceeds [`MAX_NODES`], or its tables reference more
/// than [`MAX_PATCHES`] distinct patches — a document the reader
/// would refuse must not be minted silently, and the writer is the
/// place that says so.
#[must_use]
pub fn capture(ui: &Ui) -> Vec<u8> {
    assert!(
        ui.live() <= MAX_NODES,
        "a {}-node tree exceeds the document ceiling of {MAX_NODES}",
        ui.live()
    );
    // Depth-first in sibling order: children push reversed so the
    // stack pops them forward — the presenter's walk, and the order
    // the forward-parent rule wants.
    let mut order: Vec<(NodeId, u32)> = Vec::with_capacity(ui.live() as usize);
    let mut stack: Vec<(NodeId, u32)> = vec![(ui.root(), NO_PARENT)];
    while let Some((node, parent)) = stack.pop() {
        let own = u32::try_from(order.len()).unwrap_or(NO_PARENT);
        order.push((node, parent));
        let before = stack.len();
        for child in ui.children(node) {
            stack.push((child, own));
        }
        stack[before..].reverse();
    }

    // The pool, renumbered into first-use order over that walk: the
    // canonical form admits one spelling, and this is where it is
    // spelled. Unreferenced entries in the live pool simply never
    // earn a number — capture carries no dead freight.
    let mut renumber: Vec<u16> = vec![crate::NO_PATCH; ui.patches.len()];
    let mut used: Vec<u16> = Vec::new();
    for (node, _) in &order {
        let table = ui.states[node.index() as usize].table;
        for entry in table {
            if entry != crate::NO_PATCH
                && usize::from(entry) < renumber.len()
                && renumber[usize::from(entry)] == crate::NO_PATCH
            {
                renumber[usize::from(entry)] = u16::try_from(used.len()).unwrap_or(crate::NO_PATCH);
                used.push(entry);
            }
        }
    }
    assert!(
        used.len() <= MAX_PATCHES as usize,
        "the referenced pool exceeds the ceiling"
    );

    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&ui.live().to_le_bytes());
    out.extend_from_slice(&u32::try_from(used.len()).unwrap_or(0).to_le_bytes());

    for (node, parent) in order {
        let index = node.index() as usize;
        // The base, not whatever patch happens to be worn: a document
        // is authored state, and dress is derived at runtime.
        let style = ui.states[index].base;
        let mut record = [0u8; NODE_BYTES];
        record[OFF_PARENT..OFF_PARENT + 4].copy_from_slice(&parent.to_le_bytes());
        encode_style(&mut record, &style);
        for (slot, entry) in ui.states[index].table.into_iter().enumerate() {
            let renumbered = if entry != crate::NO_PATCH && usize::from(entry) < renumber.len() {
                renumber[usize::from(entry)]
            } else {
                crate::NO_PATCH
            };
            let at = OFF_TABLE + slot * 2;
            record[at..at + 2].copy_from_slice(&renumbered.to_le_bytes());
        }
        out.extend_from_slice(&record);
    }

    for entry in used {
        let patch = &ui.patches[usize::from(entry)];
        let mut record = [0u8; PATCH_BYTES];
        record[OFF_PATCH_FLAGS] = u8::from(patch.touches_layout) * PATCH_TOUCHES_LAYOUT;
        encode_style(&mut record, &patch.style);
        out.extend_from_slice(&record);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small real tree: a column root with a padded row of two
    /// mixed-size leaves — every field the format carries, non-default.
    fn menu_shaped() -> Ui {
        let mut ui = Ui::new(UiLimits { nodes: 8 });
        let root = ui.root();
        ui.set_style(
            root,
            Style {
                direction: Direction::Column,
                justify: Align::Center,
                align_cross: Align::End,
                gap: Fixed::from_int(8),
                background: [10, 20, 30, 255],
                ..Style::default()
            },
        );
        let row = ui.insert(root).unwrap_or(root);
        ui.set_style(
            row,
            Style {
                margin: Edges::all(Fixed::from_int(2)),
                padding: Edges {
                    left: Fixed::from_int(12),
                    right: Fixed::from_int(12),
                    top: Fixed::from_int(5),
                    bottom: Fixed::from_int(5),
                },
                grow: 3,
                background: [40, 44, 52, 230],
                ..Style::default()
            },
        );
        let wide = ui.insert(row).unwrap_or(root);
        ui.set_style(
            wide,
            Style {
                width: Size::Px(Fixed::from_int(64)),
                height: Size::Px(Fixed::from_int(16)),
                ..Style::default()
            },
        );
        ui.insert(row).unwrap_or(root);
        ui
    }

    /// capture -> read -> tree -> capture reproduces the bytes: the
    /// round trip is the identity, which holds structure, order, and
    /// every style field at once.
    #[test]
    fn the_round_trip_is_the_identity() {
        let bytes = capture(&menu_shaped());
        let document = Document::read(&bytes).expect("a captured tree reads back");
        assert_eq!(document.len(), 4);
        assert!(!document.is_empty());
        let again = capture(&document.tree());
        assert_eq!(
            bytes, again,
            "capture of the instantiated document must reproduce it"
        );
    }

    /// The instantiated tree solves like the original: same rects for
    /// the document-order walk.
    #[test]
    fn the_instantiated_tree_solves_identically() {
        let mut original = menu_shaped();
        let bytes = capture(&original);
        let mut copy = Document::read(&bytes).expect("reads").tree();
        original.solve(Fixed::from_int(320), Fixed::from_int(240));
        copy.solve(Fixed::from_int(320), Fixed::from_int(240));
        let walk = |ui: &Ui| {
            let mut rects = Vec::new();
            let mut stack = vec![ui.root()];
            while let Some(node) = stack.pop() {
                rects.push(ui.rect(node));
                let before = stack.len();
                for child in ui.children(node) {
                    stack.push(child);
                }
                stack[before..].reverse();
            }
            rects
        };
        assert_eq!(walk(&original), walk(&copy));
    }

    /// Each header-level refusal, by name: the bytes are legal until
    /// the one edit, and the error names what the edit planted.
    #[test]
    fn every_header_refusal_names_what_it_saw() {
        let good = capture(&menu_shaped());

        assert_eq!(
            Document::read(&good[..HEADER_BYTES - 1]).err(),
            Some(DocumentError::NoHeader {
                len: HEADER_BYTES - 1
            }),
            "one byte short of a header"
        );

        let mut bad = good.clone();
        bad[0] = b'X';
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::NotADocument),
            "a broken magic"
        );

        let mut bad = good.clone();
        bad[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::UnknownVersion { found: 1 }),
            "yesterday's version is refused like any stranger"
        );

        let mut bad = good[..HEADER_BYTES].to_vec();
        bad[12..16].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::Empty),
            "a rootless document"
        );

        let mut bad = good[..HEADER_BYTES].to_vec();
        bad[12..16].copy_from_slice(&(MAX_NODES + 1).to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::TooManyNodes {
                count: MAX_NODES + 1
            }),
            "past the ceiling, before any multiplication matters"
        );

        let mut bad = good.clone();
        bad.push(0);
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::SizeMismatch {
                declared: good.len() as u64,
                actual: good.len() + 1
            }),
            "an unaccounted byte"
        );
    }

    /// Each record-level refusal, by name — the structural rules and
    /// the style bytes, against the same one-edit-from-legal bytes.
    #[test]
    fn every_record_refusal_names_what_it_saw() {
        let good = capture(&menu_shaped());

        let mut bad = good.clone();
        bad[HEADER_BYTES..HEADER_BYTES + 4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::BadParent {
                index: 0,
                parent: 0
            }),
            "a root that names a parent"
        );

        let second = HEADER_BYTES + NODE_BYTES;
        let mut bad = good.clone();
        bad[second..second + 4].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::BadParent {
                index: 1,
                parent: 1
            }),
            "a record that names itself"
        );

        let mut bad = good.clone();
        bad[second..second + 4].copy_from_slice(&NO_PARENT.to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::BadParent {
                index: 1,
                parent: NO_PARENT
            }),
            "a second root"
        );

        // menu_shaped's records are [root, row(0), wide(1), leaf(1)].
        // Reparenting `wide` onto the root closes the row's subtree,
        // so `leaf` then names a parent no longer on the open path:
        // the same tree, spelled out of depth-first order.
        let third = HEADER_BYTES + 2 * NODE_BYTES;
        let mut bad = good.clone();
        bad[third..third + 4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::NotPreorder {
                index: 3,
                parent: 1
            }),
            "interleaved subtrees"
        );

        let mut bad = good.clone();
        bad[HEADER_BYTES + OFF_WIDTH] = 1;
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::UnsetSizeBits {
                index: 0,
                field: "width bits"
            }),
            "width bits under a cleared flag"
        );

        let mut bad = good.clone();
        bad[HEADER_BYTES + OFF_HEIGHT + 7] = 0x80;
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::UnsetSizeBits {
                index: 0,
                field: "height bits"
            }),
            "height bits under a cleared flag"
        );

        for (offset, field, planted) in [
            (OFF_DIRECTION, "direction", 2u8),
            (OFF_JUSTIFY, "justify", 3),
            (OFF_ALIGN_CROSS, "align_cross", 9),
            (OFF_SIZE_FLAGS, "size flags", 4),
        ] {
            let mut bad = good.clone();
            bad[HEADER_BYTES + offset] = planted;
            assert_eq!(
                Document::read(&bad).err(),
                Some(DocumentError::BadStyleByte {
                    index: 0,
                    field,
                    value: planted
                }),
                "an out-of-range byte for {field}"
            );
        }
    }

    /// The canonical identity holds for hand-built bytes too, not
    /// only for capture's own output: whatever the reader accepts,
    /// capture reproduces.
    #[test]
    fn a_hand_built_document_captures_back_to_itself() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        // An empty table wears the base for every combination: all
        // sentinel bytes, which are 0xFF throughout.
        let mut root = [0u8; NODE_BYTES];
        root[OFF_TABLE..].fill(0xFF);
        root[..4].copy_from_slice(&NO_PARENT.to_le_bytes());
        root[OFF_DIRECTION] = 1;
        root[OFF_BACKGROUND..OFF_BACKGROUND + 4].copy_from_slice(&[9, 8, 7, 255]);
        bytes.extend_from_slice(&root);
        let mut child = [0u8; NODE_BYTES];
        child[OFF_TABLE..].fill(0xFF);
        child[..4].copy_from_slice(&0u32.to_le_bytes());
        child[OFF_SIZE_FLAGS] = FLAG_WIDTH_PX;
        child[OFF_WIDTH..OFF_WIDTH + 8]
            .copy_from_slice(&Fixed::from_int(31).to_bits().to_le_bytes());
        child[OFF_GROW..OFF_GROW + 4].copy_from_slice(&2u32.to_le_bytes());
        // The child wears a hover patch: table entry for the hover
        // bit alone names pool index zero, hand-encoded.
        child[OFF_TABLE + 2..OFF_TABLE + 4].copy_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&child);
        // The pooled patch: colour-only, so the flags byte stays
        // clear, and the style block mirrors the child's geometry.
        let mut patch = [0u8; PATCH_BYTES];
        patch[OFF_SIZE_FLAGS] = FLAG_WIDTH_PX;
        patch[OFF_WIDTH..OFF_WIDTH + 8]
            .copy_from_slice(&Fixed::from_int(31).to_bits().to_le_bytes());
        patch[OFF_GROW..OFF_GROW + 4].copy_from_slice(&2u32.to_le_bytes());
        patch[OFF_BACKGROUND..OFF_BACKGROUND + 4].copy_from_slice(&[1, 2, 3, 255]);
        bytes.extend_from_slice(&patch);

        let document = Document::read(&bytes).expect("hand-built canonical bytes read");
        assert_eq!(document.patch_count(), 1);
        assert_eq!(
            capture(&document.tree()),
            bytes,
            "the reader accepted it, so capture must reproduce it"
        );
    }

    /// The writer refuses to mint what the reader would refuse: a
    /// tree past the ceiling is the caller's contract violation, said
    /// loudly at the writer rather than discovered at a later load.
    #[test]
    #[should_panic(expected = "exceeds the document ceiling")]
    fn capture_refuses_a_tree_past_the_ceiling() {
        let mut ui = Ui::new(UiLimits {
            nodes: MAX_NODES + 1,
        });
        let root = ui.root();
        for _ in 0..MAX_NODES {
            let _ = ui.insert(root);
        }
        let _ = capture(&ui);
    }

    /// A dressed tree — pool loaded, tables set — survives the whole
    /// circle: capture, read, instantiate, capture again, with the
    /// pool renumbered into first-use order and unreferenced entries
    /// shed along the way.
    #[test]
    fn a_dressed_tree_round_trips_canonically() {
        let mut ui = menu_shaped();
        // Full resolved styles over the wide leaf's base — the shape
        // the compiler mints and the reader verifies flags against.
        let wide_base = Style {
            width: Size::Px(Fixed::from_int(64)),
            height: Size::Px(Fixed::from_int(16)),
            ..Style::default()
        };
        let hover = crate::StatePatch {
            style: Style {
                background: [90, 90, 90, 255],
                ..wide_base
            },
            touches_layout: false,
        };
        let grown = crate::StatePatch {
            style: Style {
                width: Size::Px(Fixed::from_int(70)),
                ..wide_base
            },
            touches_layout: true,
        };
        // Three entries: the middle one never referenced, so capture
        // must shed it and renumber the third.
        assert!(ui.set_patch_pool(vec![grown, hover, hover]));
        let row = ui.children(ui.root()).next().expect("the row");
        let wide = ui.children(row).next().expect("the wide leaf");
        let mut table = [crate::NO_PATCH; crate::STATE_COMBINATIONS];
        table[usize::from(crate::STATE_HOVER)] = 2;
        table[usize::from(crate::STATE_PRESSED)] = 0;
        assert!(ui.set_state_table(wide, table));

        let bytes = capture(&ui);
        let document = Document::read(&bytes).expect("a dressed capture reads");
        assert_eq!(document.patch_count(), 2, "the dead entry was shed");
        let hover_slot = document
            .state_table(2)
            .expect("the wide leaf is record two")[usize::from(crate::STATE_HOVER)];
        assert_eq!(
            document
                .patch(u32::from(hover_slot))
                .expect("in pool")
                .style
                .background,
            [90, 90, 90, 255],
            "the hover patch survived renumbering"
        );
        assert_eq!(
            capture(&document.tree()),
            bytes,
            "the dressed round trip is the identity"
        );
        assert!(
            document.patch(document.patch_count()).is_none(),
            "the pool declines past its end"
        );
        assert!(
            document.state_table(document.len()).is_none(),
            "the tables decline past the records"
        );
    }

    /// Each patch-section refusal, by name, one edit from legal.
    #[test]
    fn every_patch_refusal_names_what_it_saw() {
        let mut ui = menu_shaped();
        let row = ui.children(ui.root()).next().expect("the row");
        let row_base = ui.base_style(row).expect("live");
        assert!(ui.set_patch_pool(vec![crate::StatePatch {
            style: Style {
                background: [90, 90, 90, 255],
                ..row_base
            },
            touches_layout: false,
        }]));
        let mut table = [crate::NO_PATCH; crate::STATE_COMBINATIONS];
        table[usize::from(crate::STATE_HOVER)] = 0;
        assert!(ui.set_state_table(row, table));
        let good = capture(&ui);
        let patch_start = good.len() - PATCH_BYTES;

        let mut bad = good.clone();
        bad[16..20].copy_from_slice(&(MAX_PATCHES + 1).to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::TooManyPatches {
                count: MAX_PATCHES + 1
            }),
            "past the patch ceiling"
        );

        let mut bad = good.clone();
        bad[patch_start + OFF_PATCH_FLAGS] = 2;
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::BadPatch {
                index: 0,
                field: "patch flags",
                value: 2
            }),
            "an unknown patch flag"
        );

        let mut bad = good.clone();
        bad[patch_start + 2] = 9;
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::BadPatch {
                index: 0,
                field: "padding",
                value: 9
            }),
            "padding must be zero"
        );

        let mut bad = good.clone();
        bad[patch_start + OFF_DIRECTION] = 5;
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::BadPatch {
                index: 0,
                field: "direction",
                value: 5
            }),
            "a patch enum out of range"
        );

        let mut bad = good.clone();
        bad[patch_start + OFF_WIDTH] = 1;
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::UnsetPatchBits {
                index: 0,
                field: "width bits"
            }),
            "dead width bits in a patch"
        );

        // The row is record one; its hover entry points at the pool's
        // only patch. Pointing it past the pool refuses by name.
        let row_table = HEADER_BYTES + NODE_BYTES + OFF_TABLE;
        let hover_at = row_table + usize::from(crate::STATE_HOVER) * 2;
        let mut bad = good.clone();
        bad[hover_at..hover_at + 2].copy_from_slice(&7u16.to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::PatchOutOfPool { index: 1, entry: 7 }),
            "a table entry outside the pool"
        );

        // Lying about geometry refuses: the patch is colour-only, so
        // raising its layout flag disagrees with the base it dresses.
        let mut bad = good.clone();
        bad[patch_start + OFF_PATCH_FLAGS] = 1;
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::WrongPatchFlag { index: 1, entry: 0 }),
            "a layout flag that lies"
        );

        // Dropping the only reference leaves the pool unreferenced:
        // canonical blobs carry no dead freight.
        let mut bad = good.clone();
        bad[hover_at..hover_at + 2].copy_from_slice(&crate::NO_PATCH.to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::PoolNotCanonical {
                expected: 0,
                found: 1
            }),
            "an unreferenced pool entry"
        );
    }

    /// Bounds before read: every truncation of a valid document is an
    /// error, never a panic and never an accept.
    #[test]
    fn every_truncation_is_refused() {
        let good = capture(&menu_shaped());
        for len in 0..good.len() {
            let refused = Document::read(&good[..len]).is_err();
            assert!(refused, "a {len}-byte prefix must refuse");
        }
    }

    /// The accessors answer within bounds and decline past them.
    #[test]
    fn accessors_stay_inside_the_document() {
        let bytes = capture(&menu_shaped());
        let document = Document::read(&bytes).expect("reads");
        assert!(document.style(0).is_some());
        assert!(document.style(document.len()).is_none());
        assert_eq!(document.parent(0), None, "the root has no parent");
        assert_eq!(document.parent(1), Some(0));
        assert_eq!(document.parent(document.len()), None);
        let root_style = document.style(0).expect("in range");
        assert_eq!(root_style.background, [10, 20, 30, 255]);
        assert_eq!(root_style.justify, Align::Center);
        assert_eq!(root_style.align_cross, Align::End);
    }

    /// Every refusal prints the numbers a reader needs.
    #[test]
    fn refusals_display_their_evidence() {
        let cases = [
            (DocumentError::NoHeader { len: 3 }, "3"),
            (DocumentError::NotADocument, "magic"),
            (DocumentError::UnknownVersion { found: 9 }, "9"),
            (DocumentError::Empty, "root"),
            (DocumentError::TooManyNodes { count: 5000 }, "5000"),
            (
                DocumentError::SizeMismatch {
                    declared: 120,
                    actual: 121,
                },
                "120",
            ),
            (
                DocumentError::BadParent {
                    index: 7,
                    parent: 9,
                },
                "9",
            ),
            (
                DocumentError::BadStyleByte {
                    index: 1,
                    field: "justify",
                    value: 8,
                },
                "justify",
            ),
            (
                DocumentError::NotPreorder {
                    index: 3,
                    parent: 1,
                },
                "depth-first",
            ),
            (
                DocumentError::UnsetSizeBits {
                    index: 2,
                    field: "width bits",
                },
                "width bits",
            ),
            (DocumentError::TooManyPatches { count: 5000 }, "5000"),
            (
                DocumentError::BadPatch {
                    index: 1,
                    field: "padding",
                    value: 9,
                },
                "padding",
            ),
            (
                DocumentError::UnsetPatchBits {
                    index: 0,
                    field: "height bits",
                },
                "height bits",
            ),
            (
                DocumentError::PatchOutOfPool { index: 4, entry: 9 },
                "outside the pool",
            ),
            (
                DocumentError::PoolNotCanonical {
                    expected: 2,
                    found: 5,
                },
                "first-use",
            ),
            (
                DocumentError::WrongPatchFlag { index: 3, entry: 1 },
                "layout flag",
            ),
        ];
        for (error, needle) in cases {
            let text = error.to_string();
            assert!(text.contains(needle), "{text:?} must mention {needle:?}");
        }
    }
}
