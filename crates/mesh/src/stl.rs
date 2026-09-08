//! The STL reader: both dialects, and the guess between them made on
//! arithmetic rather than on a word.
//!
//! # The format, and the one hard thing about it
//!
//! An STL file is a list of triangles and nothing else — no names, no
//! materials, no shared vertices, no units. It exists in two encodings
//! that a file does not label:
//!
//! * **Binary.** Eighty bytes of anything, a `u32` triangle count, then
//!   fifty bytes per triangle: a normal and three corners as twelve
//!   little-endian `f32`s, then a `u16` nobody agrees about.
//! * **Text.** `solid`, then `facet normal` blocks, then `endsolid`.
//!
//! **A binary file's eighty-byte header is arbitrary, so it can begin
//! with the word `solid`** — and exporters do exactly that, because the
//! header is often a comment and "solid" is a natural thing to write in
//! one. A reader that dispatches on that word reads binary files as text
//! and fails on the first non-ASCII byte of a float. This is the classic
//! way to get STL wrong, and it is why [`read`] decides by size instead:
//! a binary file's length is fixed by its own count, and that is a
//! question with one answer.
//!
//! # What this reader will not do
//!
//! **It does not weld vertices.** STL stores three full corners per
//! triangle with no index buffer, so a cube arrives as thirty-six
//! distinct positions rather than eight. Welding them is a decision
//! about tolerance — how near is the same point? — and a decision about
//! what to do with the normals of the faces that meet there. Both belong
//! to whoever knows the model's scale, which is not this crate.
//!
//! **It does not correct winding.** See [`Mesh::winding_disagreements`],
//! which counts the disagreements so a caller can decide.
//!
//! [`Mesh::winding_disagreements`]: crate::Mesh::winding_disagreements

use crate::Mesh;
use crate::error::{MeshError, quoted};

/// Bytes before the triangle count in a binary file: eighty of header.
const HEADER: usize = 80;

/// One binary triangle: four `[f32; 3]` vectors and the attribute word.
const RECORD: usize = 4 * 3 * 4 + 2;

/// The least a binary file can be: the header and the count, describing
/// no triangles at all.
const LEAST_BINARY: usize = HEADER + 4;

/// Read an STL file, in whichever encoding it is written in.
///
/// # Errors
///
/// Every way the bytes can fail to be an STL file, by name — see
/// [`MeshError`]. A file that reads has at least one triangle, a
/// position count that is a multiple of three, and no coordinate that is
/// not a finite number.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    // Three questions, in the order that makes each refusal the useful
    // one. Whether the length arithmetic works out is decisive, so it
    // goes first. Whether the file reads as text is next, because a
    // text file has no reason to satisfy that arithmetic. What is left
    // is a file that is neither, and it is handed to the binary reader
    // rather than the text one: **the commonest way a real STL is
    // wrong is a binary file cut short**, and calling that "line 1:
    // expected `solid`" tells its holder nothing. The binary reader can
    // say which count was declared and how many bytes arrived, which is
    // the pair a truncated download is diagnosed from.
    if looks_binary(bytes) || !looks_text(bytes) {
        read_binary(bytes)
    } else {
        read_text(bytes)
    }
}

/// Whether these bytes open the way a text STL opens.
///
/// Deliberately shallow: valid UTF-8, and the first word is `solid`.
/// It is asked only after the binary arithmetic has said no, so its
/// job is to separate "text that will not parse" from "not text at
/// all", and a deeper look would be the parse itself.
fn looks_text(bytes: &[u8]) -> bool {
    core::str::from_utf8(bytes)
        .is_ok_and(|text| text.split_ascii_whitespace().next() == Some("solid"))
}

/// Whether these bytes are a binary STL, decided by arithmetic.
///
/// **The length is the evidence, not the leading word.** A binary file
/// is exactly `84 + 50n` bytes for the `n` its own header declares, and
/// a text file that happens to be that length to the byte, with a header
/// whose bytes 80..84 read as exactly its own triangle count, is not
/// something an exporter produces by accident.
///
/// The count is read before the length is checked against it, which is
/// the whole trick: a text file beginning `solid ` has whatever bytes its
/// object name puts at offset 80, and those bytes read as a `u32` almost
/// never describe the file's own size.
fn looks_binary(bytes: &[u8]) -> bool {
    let Some(count) = count_at(bytes, HEADER) else {
        return false;
    };
    // In `u64` so a hostile count cannot wrap the product on a 32-bit
    // target and land back on the file's own length.
    let declared = u64::from(count) * RECORD as u64 + LEAST_BINARY as u64;
    declared == bytes.len() as u64
}

/// The little-endian `u32` at `offset`, or `None` past the end.
fn count_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Read the binary encoding.
fn read_binary(bytes: &[u8]) -> Result<Mesh, MeshError> {
    if bytes.len() < LEAST_BINARY {
        return Err(MeshError::TooShortForHeader {
            needs: LEAST_BINARY,
            len: bytes.len(),
        });
    }
    let count = count_at(bytes, HEADER).ok_or(MeshError::TooShortForHeader {
        needs: LEAST_BINARY,
        len: bytes.len(),
    })?;
    if count == 0 {
        return Err(MeshError::NoGeometry);
    }

    // Checked in `u64` for the same reason `looks_binary` is, then
    // narrowed once, so a 32-bit target refuses by name rather than
    // wrapping into a plausible size.
    let body = u64::from(count) * RECORD as u64;
    let declared = body + LEAST_BINARY as u64;
    if declared != bytes.len() as u64 {
        return Err(MeshError::CountMismatch {
            declared,
            actual: bytes.len(),
            count,
        });
    }
    let body_len = usize::try_from(body).map_err(|_| MeshError::TooLarge {
        field: "triangle count",
        value: u64::from(count),
    })?;

    // The length has already been shown to account for the file exactly,
    // so this reservation is bounded by the caller's own bytes: at most
    // one triangle per fifty of them.
    let triangles = usize::try_from(count).unwrap_or(usize::MAX);
    let mut positions = Vec::with_capacity(triangles * 3);
    let mut normals = Vec::with_capacity(triangles);

    let records =
        bytes
            .get(LEAST_BINARY..LEAST_BINARY + body_len)
            .ok_or(MeshError::CountMismatch {
                declared,
                actual: bytes.len(),
                count,
            })?;
    for (index, record) in records.as_chunks::<RECORD>().0.iter().enumerate() {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        let vector = |at: usize| {
            let mut out = [0.0f32; 3];
            for (lane, slot) in out.iter_mut().enumerate() {
                let start = at + lane * 4;
                // In range by construction: `chunks_exact` hands over
                // exactly `RECORD` bytes and every offset read here is
                // inside that.
                let four = &record[start..start + 4];
                *slot = f32::from_le_bytes([four[0], four[1], four[2], four[3]]);
            }
            out
        };

        let normal = vector(0);
        check_finite(normal, "normal", index)?;
        normals.push(normal);
        for corner in 0..3 {
            let position = vector(12 + corner * 12);
            check_finite(position, "position", index)?;
            positions.push(position);
        }
        // The attribute word at the end of the record is deliberately
        // not read. It has no agreed meaning: some tools write zero,
        // some write a colour in one of two incompatible packings, and
        // some write uninitialised memory. Reading it would mean
        // choosing one of those, and this crate has no standing to.
    }

    Ok(Mesh { positions, normals })
}

/// Every coordinate of `vector` is a number a bounding box can hold.
fn check_finite(vector: [f32; 3], field: &'static str, index: u32) -> Result<(), MeshError> {
    if vector.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(MeshError::NotFinite { field, index })
    }
}

/// Read the text encoding.
///
/// The grammar, and all of it:
///
/// ```text
/// solid <name?>
///   facet normal <x> <y> <z>
///     outer loop
///       vertex <x> <y> <z>      (three times)
///     endloop
///   endfacet
/// endsolid <name?>
/// ```
///
/// **Read as a token stream rather than line by line**, because the
/// format's own producers disagree about line structure: some write a
/// whole facet on one line, some indent with tabs, some emit `\r\n` and
/// some do not. A line-oriented reader would refuse working files over a
/// disagreement the format never settled. Lines are still counted, but
/// for the refusals rather than for the grammar.
fn read_text(bytes: &[u8]) -> Result<Mesh, MeshError> {
    // Valid by construction: `looks_text` asked this question before
    // the dispatch chose this path, and the bytes have not moved. The
    // fallback is empty rather than a refusal because a refusal here
    // would be a variant no input could reach, which is the thing this
    // crate's error vocabulary has already been pruned of twice.
    let text = core::str::from_utf8(bytes).unwrap_or("");
    let mut words = Words::new(text);

    words.expect("solid")?;
    // Anything from here to the end of the line is the object's name,
    // which this crate does not keep: STL names nothing else, so a name
    // has nowhere to go and inventing a field for it would be a promise
    // to carry it somewhere.
    words.skip_line();

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    loop {
        let Some(word) = words.peek() else {
            // A file that ends without `endsolid` is a file that was cut
            // off, and saying which word was wanted is more use than
            // saying the file ended.
            return Err(MeshError::ExpectedKeyword {
                expected: "endsolid",
                found: String::new(),
                line: words.line,
            });
        };
        if word == "endsolid" {
            break;
        }

        let index = u32::try_from(normals.len()).unwrap_or(u32::MAX);
        words.expect("facet")?;
        words.expect("normal")?;
        let normal = words.vector()?;
        check_finite(normal, "normal", index)?;
        normals.push(normal);

        words.expect("outer")?;
        words.expect("loop")?;
        for _ in 0..3 {
            words.expect("vertex")?;
            let position = words.vector()?;
            check_finite(position, "position", index)?;
            positions.push(position);
        }
        words.expect("endloop")?;
        words.expect("endfacet")?;
    }

    if positions.is_empty() {
        return Err(MeshError::NoGeometry);
    }
    Ok(Mesh { positions, normals })
}

/// A cursor over whitespace-separated words, counting lines as it goes.
struct Words<'a> {
    rest: &'a str,
    line: u32,
}

impl<'a> Words<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            rest: text,
            line: 1,
        }
    }

    /// Consume whitespace, counting the newlines in it.
    fn skip_space(&mut self) {
        let mut at = 0;
        for byte in self.rest.bytes() {
            if !byte.is_ascii_whitespace() {
                break;
            }
            if byte == b'\n' {
                self.line = self.line.saturating_add(1);
            }
            at += 1;
        }
        // In range and on a boundary: every byte skipped is ASCII.
        self.rest = self.rest.get(at..).unwrap_or("");
    }

    /// The next word without consuming it.
    fn peek(&mut self) -> Option<&'a str> {
        self.skip_space();
        if self.rest.is_empty() {
            return None;
        }
        let end = self
            .rest
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(self.rest.len());
        self.rest.get(..end)
    }

    /// The next word, consumed.
    fn next_word(&mut self) -> Option<&'a str> {
        let word = self.peek()?;
        self.rest = self.rest.get(word.len()..).unwrap_or("");
        Some(word)
    }

    /// Consume the rest of the current line.
    fn skip_line(&mut self) {
        let end = self.rest.find('\n').map_or(self.rest.len(), |at| at + 1);
        if end > 0 && self.rest.as_bytes().get(end - 1) == Some(&b'\n') {
            self.line = self.line.saturating_add(1);
        }
        self.rest = self.rest.get(end..).unwrap_or("");
    }

    /// The next word must be `expected`.
    fn expect(&mut self, expected: &'static str) -> Result<(), MeshError> {
        let line = {
            self.skip_space();
            self.line
        };
        match self.next_word() {
            Some(word) if word == expected => Ok(()),
            other => Err(MeshError::ExpectedKeyword {
                expected,
                found: quoted(other.unwrap_or("")),
                line,
            }),
        }
    }

    /// Three numbers.
    fn vector(&mut self) -> Result<[f32; 3], MeshError> {
        let mut out = [0.0f32; 3];
        for slot in &mut out {
            self.skip_space();
            let line = self.line;
            let word = self.next_word().unwrap_or("");
            *slot = word.parse::<f32>().map_err(|_| MeshError::NotANumber {
                found: quoted(word),
                line,
            })?;
        }
        Ok(out)
    }
}
