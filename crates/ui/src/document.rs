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
//! **A document is a tree by construction.** Node records sit in
//! document order; record 0 is the root and carries the no-parent
//! sentinel, and every later record's parent index is strictly less
//! than its own. A blob that violates the rule is refused, so a read
//! document cannot hold a cycle, an orphan, or a forward reference,
//! and instantiation never needs to check again. Sibling order is
//! document order.
//!
//! **Version 1 carries structure and base styles only.** The state
//! variant tables the compiler will one day emit are not speculated
//! here as dead sections; the version field is the evolution path,
//! and an unknown version is refused, never skipped over.

use renew_fixed::Fixed;

use crate::layout::{Align, Direction, Edges, Size, Style};
use crate::{NodeId, Ui, UiLimits};

/// The document's opening bytes: seven ASCII characters and a NUL.
pub const MAGIC: [u8; 8] = *b"RENEWUI\0";

/// The one blob layout this reader understands. Anything else is
/// refused outright — version negotiation is a writer's job.
pub const VERSION: u32 = 1;

/// Header: `MAGIC` + `version` + `node_count`.
pub const HEADER_BYTES: usize = 16;
const OFF_VERSION: usize = 8;
const OFF_COUNT: usize = 12;

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
pub const NODE_BYTES: usize = 104;
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

/// Width is `Px` when set; `Auto` otherwise.
const FLAG_WIDTH_PX: u8 = 1;
/// Height is `Px` when set; `Auto` otherwise.
const FLAG_HEIGHT_PX: u8 = 2;

/// The root's parent field: no node has this index.
pub const NO_PARENT: u32 = u32::MAX;

/// The most nodes one document may declare. A ceiling, not a target:
/// the reader refuses past it before multiplying anything by anything.
pub const MAX_NODES: u32 = 4096;

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
    /// An enum byte outside its range.
    BadStyleByte {
        /// Which record.
        index: u32,
        /// Which field.
        field: &'static str,
        /// The byte it carried.
        value: u8,
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
    record
        .get(offset..offset + 8)
        .and_then(|slice| <[u8; 8]>::try_from(slice).ok())
        .map_or(0, i64::from_le_bytes)
}

/// One `Fixed` from record bytes.
fn fixed_in(record: &[u8], offset: usize) -> Fixed {
    Fixed::from_bits(i64_in(record, offset))
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
        let version =
            u32_at(bytes, OFF_VERSION).ok_or(DocumentError::NoHeader { len: bytes.len() })?;
        if version != VERSION {
            return Err(DocumentError::UnknownVersion { found: version });
        }
        let count = u32_at(bytes, OFF_COUNT).ok_or(DocumentError::NoHeader { len: bytes.len() })?;
        if count == 0 {
            return Err(DocumentError::Empty);
        }
        if count > MAX_NODES {
            return Err(DocumentError::TooManyNodes { count });
        }
        // The ceiling above keeps this multiplication small, but the
        // accounting stays in u64 anyway: the check must not depend on
        // the ceiling staying small forever.
        let declared = (HEADER_BYTES as u64) + u64::from(count) * (NODE_BYTES as u64);
        if declared != bytes.len() as u64 {
            return Err(DocumentError::SizeMismatch {
                declared,
                actual: bytes.len(),
            });
        }
        let nodes = bytes.get(HEADER_BYTES..).unwrap_or_default();

        for index in 0..count {
            let record = record_of(nodes, index);
            let parent = u32_at(record, OFF_PARENT).unwrap_or(NO_PARENT);
            let legal = if index == 0 {
                parent == NO_PARENT
            } else {
                parent < index
            };
            if !legal {
                return Err(DocumentError::BadParent { index, parent });
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
        }
        Ok(Self { nodes, count })
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
        let record = record_of(self.nodes, index);
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
        // The enum bytes were validated by read; the defaults here are
        // the unreachable arms of that proof, not a second decoder.
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
        Some(Style {
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
        })
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
                .and_then(|at| ids.get(at as usize).copied())
                .unwrap_or_else(|| ui.root());
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

/// Serialize a live tree into document bytes: the inverse of
/// [`Document::read`] + [`Document::tree`], used by tests as the
/// round-trip oracle and by tooling as the writer's backend. Walks in
/// document order — parents before children, siblings in tree order —
/// so capture of an instantiated document reproduces its bytes.
#[must_use]
pub fn capture(ui: &Ui) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&ui.live().to_le_bytes());

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

    for (node, parent) in order {
        let style = ui.style(node).unwrap_or_default();
        let mut record = [0u8; NODE_BYTES];
        record[OFF_PARENT..OFF_PARENT + 4].copy_from_slice(&parent.to_le_bytes());
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
        let mut edges = |offset: usize, value: Edges| {
            for (nth, bits) in [value.left, value.right, value.top, value.bottom]
                .into_iter()
                .enumerate()
            {
                let at = offset + nth * 8;
                record[at..at + 8].copy_from_slice(&bits.to_bits().to_le_bytes());
            }
        };
        edges(OFF_MARGIN, style.margin);
        edges(OFF_PADDING, style.padding);
        record[OFF_GAP..OFF_GAP + 8].copy_from_slice(&style.gap.to_bits().to_le_bytes());
        record[OFF_GROW..OFF_GROW + 4].copy_from_slice(&style.grow.to_le_bytes());
        record[OFF_BACKGROUND..OFF_BACKGROUND + 4].copy_from_slice(&style.background);
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

    /// Each refusal, by name: the bytes are legal until the one edit,
    /// and the error names what the edit planted.
    #[test]
    fn every_refusal_names_what_it_saw() {
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
        bad[8..12].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(
            Document::read(&bad).err(),
            Some(DocumentError::UnknownVersion { found: 2 }),
            "a future version is refused, not skipped"
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
        ];
        for (error, needle) in cases {
            let text = error.to_string();
            assert!(text.contains(needle), "{text:?} must mention {needle:?}");
        }
    }
}
