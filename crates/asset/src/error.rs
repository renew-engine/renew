//! Every way a pack can be refused, and why each is its own variant.
//!
//! A reader of untrusted input is judged by its refusals, not by its
//! successes. Each variant below names one specific thing that was wrong
//! and carries the numbers a person needs to see it — because the reader
//! of this message is usually someone holding a file they did not build
//! and cannot inspect by eye.
//!
//! There is deliberately no `Other` and no string-typed catch-all: a
//! parser that can say "malformed" without saying how has stopped being
//! able to distinguish a truncated download from an attack.

use core::fmt;

/// Why a pack was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackError {
    /// The first eight bytes are not the container's magic.
    NotAPack,
    /// A pack, but a version this build does not implement.
    UnknownFormat { found: u32 },
    /// The file is shorter than the fixed header.
    NoHeader { len: usize },
    /// The four regions do not account for exactly the bytes present.
    ///
    /// Checked as equality rather than "at least", so trailing bytes are
    /// refused as firmly as missing ones. Something appended to a pack is
    /// as much a sign of trouble as something cut off it.
    SizeMismatch { declared: u64, actual: usize },
    /// A count or length that cannot be represented on this target.
    TooLarge { field: &'static str, value: u64 },
    /// An entry's name lies outside the names region.
    NameOutOfRange { index: usize, offset: u32, len: u32 },
    /// An entry's payload lies outside the data region.
    DataOutOfRange { index: usize, offset: u64, len: u64 },
    /// An entry's name is not UTF-8.
    NameNotUtf8 { index: usize },
    /// An entry's name is empty, which no lookup could ever match.
    EmptyName { index: usize },
    /// A name longer than the format admits.
    NameTooLong { index: usize, len: usize },
    /// Entries are not in ascending name order.
    ///
    /// Order is part of the format, not a convenience: it is what makes
    /// lookup a binary search, and what makes a pack byte-identical
    /// whatever order its inputs arrived in.
    Unsorted { index: usize },
    /// Two entries share a name, so a lookup has no correct answer.
    DuplicateName { index: usize },
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAPack => write!(f, "not an asset pack: the magic does not match"),
            Self::UnknownFormat { found } => write!(
                f,
                "asset pack format {found} is not one this build reads (expects {})",
                crate::layout::FORMAT
            ),
            Self::NoHeader { len } => write!(
                f,
                "too short to be a pack: {len} bytes, header needs {}",
                crate::layout::HEADER_BYTES
            ),
            Self::SizeMismatch { declared, actual } => write!(
                f,
                "the header describes {declared} bytes but the file holds {actual}"
            ),
            Self::TooLarge { field, value } => {
                write!(f, "`{field}` is {value}, too large for this target")
            }
            Self::NameOutOfRange { index, offset, len } => write!(
                f,
                "entry {index} names bytes {offset}..+{len}, which is outside the names region"
            ),
            Self::DataOutOfRange { index, offset, len } => write!(
                f,
                "entry {index} claims data {offset}..+{len}, which is outside the data region"
            ),
            Self::NameNotUtf8 { index } => write!(f, "entry {index} has a name that is not UTF-8"),
            Self::EmptyName { index } => write!(f, "entry {index} has an empty name"),
            Self::NameTooLong { index, len } => write!(
                f,
                "entry {index} has a {len}-byte name; the limit is {}",
                crate::layout::MAX_NAME_BYTES
            ),
            Self::Unsorted { index } => write!(
                f,
                "entry {index} sorts before the one before it; a pack's entries are ordered"
            ),
            Self::DuplicateName { index } => write!(
                f,
                "entry {index} repeats the previous name; a pack cannot hold two of one name"
            ),
        }
    }
}

impl core::error::Error for PackError {}

/// Why a pack could not be built.
///
/// Separate from [`PackError`] because the two have different audiences.
/// A build failure is a mistake by whoever is packing, in their own
/// inputs, and it can name them. A read failure is a statement about a
/// file of unknown origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    /// Two inputs share a name.
    DuplicateName { name: String },
    /// A name no reader would accept.
    EmptyName,
    /// A name longer than the format admits.
    NameTooLong { name: String, len: usize },
    /// The pack would not fit the format's own width limits.
    TooLarge { field: &'static str, value: u64 },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName { name } => {
                write!(f, "two entries are named `{name}`")
            }
            Self::EmptyName => write!(f, "an entry has an empty name"),
            Self::NameTooLong { name, len } => write!(
                f,
                "`{}` is a {len}-byte name; the limit is {}",
                truncated(name),
                crate::layout::MAX_NAME_BYTES
            ),
            Self::TooLarge { field, value } => {
                write!(f, "`{field}` would be {value}, too large for the format")
            }
        }
    }
}

impl core::error::Error for BuildError {}

/// The front of an over-long name, so a diagnostic about a 4 MiB string
/// does not print one.
fn truncated(name: &str) -> &str {
    const SHOWN: usize = 48;
    if name.len() <= SHOWN {
        return name;
    }
    // Back off to a character boundary so the message never panics on a
    // multi-byte name.
    let mut end = SHOWN;
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every refusal says what was wrong, with its numbers.
    #[test]
    fn every_refusal_carries_the_detail_a_reader_needs() {
        let cases: Vec<(PackError, &[&str])> = vec![
            (PackError::NotAPack, &["magic"]),
            (PackError::UnknownFormat { found: 9 }, &["9", "1"]),
            (PackError::NoHeader { len: 3 }, &["3", "24"]),
            (
                PackError::SizeMismatch {
                    declared: 100,
                    actual: 40,
                },
                &["100", "40"],
            ),
            (
                PackError::NameOutOfRange {
                    index: 2,
                    offset: 7,
                    len: 9,
                },
                &["2", "7", "9"],
            ),
            (
                PackError::DataOutOfRange {
                    index: 1,
                    offset: 5,
                    len: 6,
                },
                &["1", "5", "6"],
            ),
            (PackError::NameNotUtf8 { index: 4 }, &["4", "UTF-8"]),
            (PackError::EmptyName { index: 0 }, &["0", "empty"]),
            (PackError::NameTooLong { index: 3, len: 99 }, &["3", "99"]),
            (PackError::Unsorted { index: 6 }, &["6"]),
            (PackError::DuplicateName { index: 8 }, &["8"]),
        ];
        for (error, expected) in cases {
            let shown = error.to_string();
            for needle in expected {
                assert!(
                    shown.contains(needle),
                    "`{shown}` does not mention `{needle}`"
                );
            }
        }
    }

    /// The two variants no malformed file reaches: one is a width limit
    /// the format reserves against, the other is every way a caller can
    /// hand the builder something it cannot store.
    #[test]
    fn the_remaining_variants_say_what_they_are() {
        let shown = PackError::TooLarge {
            field: "count",
            value: 1 << 40,
        }
        .to_string();
        assert!(shown.contains("count"), "{shown}");
        assert!(shown.contains(&(1u64 << 40).to_string()), "{shown}");

        let cases: Vec<(BuildError, &str)> = vec![
            (
                BuildError::DuplicateName {
                    name: "mesh/hero".to_string(),
                },
                "mesh/hero",
            ),
            (BuildError::EmptyName, "empty"),
            (
                BuildError::NameTooLong {
                    name: "short".to_string(),
                    len: 7,
                },
                "short",
            ),
            (
                BuildError::TooLarge {
                    field: "names_len",
                    value: 99,
                },
                "names_len",
            ),
        ];
        for (error, needle) in cases {
            let shown = error.to_string();
            assert!(shown.contains(needle), "`{shown}` omits `{needle}`");
        }
    }

    /// A build failure quotes only the front of an enormous name.
    #[test]
    fn a_build_refusal_quotes_only_the_front_of_a_huge_name() {
        let name = "n".repeat(10_000);
        let shown = BuildError::NameTooLong {
            len: name.len(),
            name,
        }
        .to_string();
        assert!(shown.len() < 200, "message was {} bytes", shown.len());
        assert!(shown.contains("10000"));
    }

    /// Truncation lands on a character boundary rather than splitting one.
    ///
    /// The leading `x` is load-bearing. A string of two- or three-byte
    /// characters alone puts a boundary exactly at 48, so the back-off
    /// loop never runs and the test passes without exercising the thing
    /// it is named for — which is how it was written the first time, and
    /// the coverage report is what noticed. Offsetting by one byte makes
    /// 48 land mid-character.
    #[test]
    fn truncation_never_splits_a_character() {
        let name = format!("x{}", "\u{3042}".repeat(200));
        assert!(!name.is_char_boundary(48), "the fixture must straddle 48");
        let shown = truncated(&name);
        assert!(name.starts_with(shown));
        assert!(shown.len() < 48, "the back-off must have moved it");
        assert!(name.is_char_boundary(shown.len()));

        // And a short name comes back whole.
        assert_eq!(truncated("short"), "short");
    }
}
