//! The on-disk shape, in one place.
//!
//! Every constant a reader and a writer must agree on lives here so they
//! cannot drift: the reader's bounds checks and the writer's offsets are
//! derived from the same numbers, and a test asserts the arithmetic
//! rather than trusting that two files were edited together.

/// Identifies the container before anything else is believed.
///
/// Eight bytes, NUL-terminated, so the file is recognisable in a hex dump
/// and cannot be confused with text. A reader that checks this first can
/// refuse a JPEG, a text file, or a truncated download without ever
/// looking at a length field.
pub const MAGIC: [u8; 8] = *b"RENEWPK\0";

/// The format version a reader understands.
///
/// **A reader refuses anything else rather than guessing.** Forward
/// compatibility is not attempted here: reading an unknown layout means
/// interpreting attacker-chosen bytes as offsets, and "probably still
/// works" is not a property a parser of untrusted input may assume.
pub const FORMAT: u32 = 1;

/// `MAGIC` + `format` + `count` + `names_len` + `data_len`.
pub const HEADER_BYTES: usize = 8 + 4 + 4 + 4 + 4;

/// One entry: `hash` + `name_off` + `name_len` + `data_off` + `data_len`.
///
/// Fixed width on purpose. The table can then be bounds-checked as a
/// whole before a single entry is read, and an entry can be located by
/// index without parsing the ones before it — neither of which is true
/// of a format with inline variable-length names.
pub const ENTRY_BYTES: usize = 8 + 4 + 4 + 8 + 8;

/// The longest name the format admits.
///
/// A bound exists so a malformed length cannot ask a reader to slice
/// four gigabytes, and so a writer refuses a name it could not later
/// read back. The value is arbitrary and generous; what matters is that
/// there is one.
pub const MAX_NAME_BYTES: usize = 1024;

/// Offsets within the header.
pub const OFF_FORMAT: usize = 8;
pub const OFF_COUNT: usize = 12;
pub const OFF_NAMES_LEN: usize = 16;
pub const OFF_DATA_LEN: usize = 20;

#[cfg(test)]
mod tests {
    use super::*;

    /// The header offsets are the running sum of the field widths.
    ///
    /// Written as arithmetic rather than as repeated literals: the
    /// constants above are the kind that get edited one at a time.
    #[test]
    fn the_header_offsets_follow_the_field_widths() {
        assert_eq!(OFF_FORMAT, MAGIC.len());
        assert_eq!(OFF_COUNT, OFF_FORMAT + 4);
        assert_eq!(OFF_NAMES_LEN, OFF_COUNT + 4);
        assert_eq!(OFF_DATA_LEN, OFF_NAMES_LEN + 4);
        assert_eq!(HEADER_BYTES, OFF_DATA_LEN + 4);
    }

    /// The magic is eight bytes and ends in NUL, so it cannot be mistaken
    /// for the start of a text file.
    #[test]
    fn the_magic_is_eight_bytes_and_not_text() {
        assert_eq!(MAGIC.len(), 8);
        assert_eq!(MAGIC[7], 0);
        assert!(MAGIC[..7].iter().all(u8::is_ascii_graphic));
    }
}
