//! Deflate, decompressed — the half of RFC 1951 this crate could only
//! write.
//!
//! # Why this exists rather than a crate
//!
//! The same argument the encoder makes, from the other side. Every
//! dependency is presumed rejected until it earns itself, and the encoder
//! already carries the format's tables, its bit order and its
//! back-reference arithmetic; a decompressor is those same published
//! constants read in the other direction. Taking a crate for the second
//! half of something already written by hand would leave the two halves
//! disagreeing about what the format is.
//!
//! # What it does and does not do
//!
//! All three block kinds: stored, fixed Huffman, and **dynamic Huffman** —
//! which the encoder never writes and every other encoder does, so a
//! decoder without it can read this crate's own output and nothing else.
//! No preset dictionaries, which zlib streams in the wild do not use and
//! which PNG forbids outright.
//!
//! Decoding is symbol at a time against canonical code tables rather than
//! through a lookup window. That is slower per byte and it is the version
//! whose correctness can be read off the specification; a window is an
//! optimisation to make once something measures it.
//!
//! # Refusals
//!
//! Every way a stream can be malformed is a named error, never a panic and
//! never a silent truncation. This runs on bytes from disk and from the
//! network, so it treats the input as hostile: no index is taken that was
//! not bounds-checked, and every table is validated as canonical before it
//! is used to decode anything.

/// Why a stream could not be decompressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InflateError {
    /// The stream ended in the middle of something.
    Truncated {
        /// How many bytes had been consumed.
        at: usize,
    },
    /// A block header named a kind that does not exist. Kind 3 is
    /// reserved and its presence means the stream is not deflate.
    BadBlockKind {
        /// The two bits that were read.
        kind: u32,
    },
    /// A stored block's length and its complement disagree, which is the
    /// format's own check that the stream is aligned where it thinks.
    StoredLengthMismatch {
        /// The length as written.
        length: u16,
        /// Its complement as written.
        complement: u16,
    },
    /// A code-length table that no canonical Huffman code can be built
    /// from — over-subscribed, or incomplete where the format requires
    /// completeness.
    BadCodeLengths,
    /// A symbol that decodes to no code in its table.
    BadSymbol,
    /// A back-reference reaching further back than the output produced so
    /// far, which would read from before the beginning.
    BadDistance {
        /// How far back it asked to go.
        distance: usize,
        /// How much output existed.
        produced: usize,
    },
    /// The decompressed stream is longer than the caller allowed.
    TooLarge {
        /// The ceiling that was passed.
        limit: usize,
    },
}

/// Bits, least-significant first within each byte.
///
/// **Deflate's own order, and it is not the obvious one.** Huffman codes
/// are packed most-significant-bit first *within a code* while everything
/// else is packed least-significant first, which is why the code reader
/// below shifts a bit in at the bottom and the length reader accumulates
/// upward. Getting this backwards produces a stream that decodes for a
/// few symbols and then dissolves, which is the hardest kind of wrong to
/// read off a hex dump.
struct Bits<'a> {
    bytes: &'a [u8],
    /// Which byte the next bit comes from.
    at: usize,
    /// Which bit of that byte, from 0 (least significant) to 7.
    bit: u32,
}

impl<'a> Bits<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            bit: 0,
        }
    }

    /// One bit, or nothing if the stream has ended.
    fn one(&mut self) -> Option<u32> {
        let byte = *self.bytes.get(self.at)?;
        let value = (u32::from(byte) >> self.bit) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.at += 1;
        }
        Some(value)
    }

    /// `count` bits as a little-endian integer.
    fn take(&mut self, count: u32) -> Option<u32> {
        let mut out = 0;
        for step in 0..count {
            out |= self.one()? << step;
        }
        Some(out)
    }

    /// Skip to the next byte boundary, as a stored block's header
    /// requires.
    const fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.at += 1;
        }
    }
}

/// The longest code deflate allows.
const MAX_BITS: usize = 15;

/// A canonical Huffman table, as counts per length and symbols in order.
///
/// **Stored as the specification describes it** rather than as a decoded
/// tree: the counts say how many codes have each length, the symbols are
/// listed in code order, and decoding walks lengths adding one bit at a
/// time. Thirty-two bytes and a vector, no allocation per symbol, and the
/// canonical property is checked when it is built rather than assumed at
/// every use.
struct Huffman {
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
}

impl Huffman {
    /// Build a table from a symbol-length list.
    ///
    /// Returns `None` when the lengths are over-subscribed — more codes of
    /// some length than that length can hold, which no canonical code can
    /// satisfy and which a corrupt stream produces readily.
    ///
    /// An *under*-subscribed table is allowed here and refused at use: the
    /// format permits one legitimate case of it (a distance table with a
    /// single code, in a stream with exactly one back-reference), and
    /// rejecting it at construction would refuse valid files.
    fn new(lengths: &[u8]) -> Option<Self> {
        let mut counts = [0u16; MAX_BITS + 1];
        for length in lengths {
            let at = usize::from(*length);
            if at > MAX_BITS {
                return None;
            }
            counts[at] = counts[at].checked_add(1)?;
        }
        // Length zero means "this symbol is not in the table".
        counts[0] = 0;
        let mut left = 1i32;
        for count in counts.iter().skip(1) {
            left = left.checked_mul(2)?;
            left -= i32::from(*count);
            if left < 0 {
                return None;
            }
        }
        // Where each length's symbols begin in the symbol list.
        let mut offsets = [0u16; MAX_BITS + 2];
        for length in 1..=MAX_BITS {
            offsets[length + 1] = offsets[length].checked_add(counts[length])?;
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, length) in lengths.iter().enumerate() {
            if *length == 0 {
                continue;
            }
            let at = usize::from(*length);
            let slot = usize::from(offsets[at]);
            *symbols.get_mut(slot)? = u16::try_from(symbol).ok()?;
            offsets[at] = offsets[at].checked_add(1)?;
        }
        Some(Self { counts, symbols })
    }

    /// The next symbol in the stream.
    ///
    /// Codes are packed most-significant bit first, so this shifts each
    /// bit in at the bottom of the accumulator and compares against the
    /// running first-code of each length.
    fn decode(&self, bits: &mut Bits<'_>) -> Result<u16, InflateError> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for length in 1..=MAX_BITS {
            let bit = bits.one().ok_or(InflateError::Truncated { at: bits.at })?;
            code |= i32::try_from(bit).unwrap_or(0);
            let count = i32::from(self.counts[length]);
            if code - first < count {
                let at = usize::try_from(index + (code - first)).unwrap_or(usize::MAX);
                return self.symbols.get(at).copied().ok_or(InflateError::BadSymbol);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(InflateError::BadSymbol)
    }
}

/// The extra bits and base value of each length symbol, 257 through 285.
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
/// How many extra bits each length symbol reads.
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// The base distance of each distance symbol, 0 through 29.
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12_289, 16_385, 24_577,
];
/// How many extra bits each distance symbol reads.
const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order dynamic blocks write their code-length code lengths in.
///
/// Not sorted, and not arbitrary: the lengths most often zero are last, so
/// a block can stop writing early. Reading them in symbol order instead
/// produces a table that is subtly wrong rather than obviously so.
const LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// The fixed tables, which every stream may use without writing them.
fn fixed() -> Option<(Huffman, Huffman)> {
    let mut lengths = [0u8; 288];
    for (symbol, slot) in lengths.iter_mut().enumerate() {
        // Eight bits for 0..=143 and for 280..=287, which is why those
        // two ranges share the fallthrough rather than being written
        // out: the published table gives them the same length.
        *slot = match symbol {
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let literals = Huffman::new(&lengths)?;
    // Every distance code is five bits in the fixed table, including the
    // two that are never valid — the specification says so, and a table
    // built from thirty rather than thirty-two is over-subscribed.
    let distances = Huffman::new(&[5u8; 32])?;
    Some((literals, distances))
}

/// Decompress a raw deflate stream.
///
/// `limit` caps the output, so a stream that claims to expand to more than
/// a caller is willing to hold is refused rather than allocated. A
/// decompressor without one is a denial of service with a nice interface:
/// a few hundred bytes of deflate can name gigabytes of output.
///
/// # Errors
///
/// Every way the stream can be malformed, by name — see [`InflateError`].
pub fn inflate(bytes: &[u8], limit: usize) -> Result<Vec<u8>, InflateError> {
    let mut bits = Bits::new(bytes);
    let mut out = Vec::new();
    loop {
        let last = bits.one().ok_or(InflateError::Truncated { at: bits.at })? == 1;
        let kind = bits
            .take(2)
            .ok_or(InflateError::Truncated { at: bits.at })?;
        match kind {
            0 => stored(&mut bits, &mut out, limit)?,
            1 => {
                let (literals, distances) = fixed().ok_or(InflateError::BadCodeLengths)?;
                block(&mut bits, &literals, &distances, &mut out, limit)?;
            }
            2 => {
                let (literals, distances) = dynamic(&mut bits)?;
                block(&mut bits, &literals, &distances, &mut out, limit)?;
            }
            other => return Err(InflateError::BadBlockKind { kind: other }),
        }
        if last {
            return Ok(out);
        }
    }
}

/// A stored block: a length, its complement, and that many raw bytes.
fn stored(bits: &mut Bits<'_>, out: &mut Vec<u8>, limit: usize) -> Result<(), InflateError> {
    bits.align();
    let length = bits
        .take(16)
        .ok_or(InflateError::Truncated { at: bits.at })?;
    let complement = bits
        .take(16)
        .ok_or(InflateError::Truncated { at: bits.at })?;
    let (length, complement) = (
        u16::try_from(length).unwrap_or(0),
        u16::try_from(complement).unwrap_or(0),
    );
    if length != !complement {
        return Err(InflateError::StoredLengthMismatch { length, complement });
    }
    let want = usize::from(length);
    if out.len().saturating_add(want) > limit {
        return Err(InflateError::TooLarge { limit });
    }
    let from = bits.at;
    let slice = bytes_at(bits.bytes, from, want).ok_or(InflateError::Truncated { at: from })?;
    out.extend_from_slice(slice);
    bits.at = from + want;
    Ok(())
}

/// A window into a slice, or nothing if it does not fit.
fn bytes_at(bytes: &[u8], from: usize, want: usize) -> Option<&[u8]> {
    bytes.get(from..from.checked_add(want)?)
}

/// Read a dynamic block's two tables.
fn dynamic(bits: &mut Bits<'_>) -> Result<(Huffman, Huffman), InflateError> {
    let short =
        |bits: &mut Bits<'_>, count: u32| bits.take(count).ok_or(InflateError::Truncated { at: 0 });
    let literals = short(bits, 5)? as usize + 257;
    let distances = short(bits, 5)? as usize + 1;
    let codes = short(bits, 4)? as usize + 4;
    // **No bounds check on the three counts, because the bit widths are
    // the bounds.** `literals` is five bits plus 257, so at most 288;
    // `distances` five bits plus one, so at most 32; `codes` four bits
    // plus four, so at most 19 — which are exactly the ceilings the
    // format states. A guard against them would be a condition that
    // cannot fail, which reads as a check and is not one.

    // The table that describes the other two tables.
    let mut code_lengths = [0u8; LENGTH_ORDER.len()];
    for at in LENGTH_ORDER.iter().take(codes) {
        let length = short(bits, 3)?;
        *code_lengths
            .get_mut(*at)
            .ok_or(InflateError::BadCodeLengths)? = u8::try_from(length).unwrap_or(0);
    }
    let table = Huffman::new(&code_lengths).ok_or(InflateError::BadCodeLengths)?;

    // The two real tables, run-length coded through it.
    let want = literals + distances;
    let mut lengths = Vec::with_capacity(want);
    while lengths.len() < want {
        let symbol = table.decode(bits)?;
        match symbol {
            0..=15 => lengths.push(u8::try_from(symbol).unwrap_or(0)),
            16 => {
                // Repeat the previous length three to six times. There
                // must *be* a previous length; a stream that opens with
                // this is corrupt.
                let last = *lengths.last().ok_or(InflateError::BadCodeLengths)?;
                let times = short(bits, 2)? as usize + 3;
                for _ in 0..times {
                    lengths.push(last);
                }
            }
            // Both write a run of zeros; they differ only in how many
            // bits the count takes and where it starts, which is the
            // format's way of spending fewer bits on short runs.
            17 | 18 => {
                let (width, base) = if symbol == 17 { (3, 3) } else { (7, 11) };
                let times = short(bits, width)? as usize + base;
                lengths.extend(core::iter::repeat_n(0u8, times));
            }
            _ => return Err(InflateError::BadCodeLengths),
        }
    }
    // A run that overshoots would silently steal from the distance table.
    if lengths.len() != want {
        return Err(InflateError::BadCodeLengths);
    }
    let (literal_lengths, distance_lengths) = lengths.split_at(literals);
    Ok((
        Huffman::new(literal_lengths).ok_or(InflateError::BadCodeLengths)?,
        Huffman::new(distance_lengths).ok_or(InflateError::BadCodeLengths)?,
    ))
}

/// Decode one Huffman-coded block into `out`.
fn block(
    bits: &mut Bits<'_>,
    literals: &Huffman,
    distances: &Huffman,
    out: &mut Vec<u8>,
    limit: usize,
) -> Result<(), InflateError> {
    loop {
        let symbol = literals.decode(bits)?;
        match symbol {
            0..=255 => {
                if out.len() >= limit {
                    return Err(InflateError::TooLarge { limit });
                }
                out.push(u8::try_from(symbol).unwrap_or(0));
            }
            256 => return Ok(()),
            257..=285 => {
                let at = usize::from(symbol) - 257;
                let base = *LENGTH_BASE.get(at).ok_or(InflateError::BadSymbol)?;
                let extra = *LENGTH_EXTRA.get(at).ok_or(InflateError::BadSymbol)?;
                let more = bits
                    .take(u32::from(extra))
                    .ok_or(InflateError::Truncated { at: bits.at })?;
                let length = usize::from(base) + more as usize;

                let symbol = distances.decode(bits)?;
                let at = usize::from(symbol);
                let base = *DISTANCE_BASE.get(at).ok_or(InflateError::BadSymbol)?;
                let extra = *DISTANCE_EXTRA.get(at).ok_or(InflateError::BadSymbol)?;
                let more = bits
                    .take(u32::from(extra))
                    .ok_or(InflateError::Truncated { at: bits.at })?;
                let distance = usize::from(base) + more as usize;

                if distance > out.len() {
                    return Err(InflateError::BadDistance {
                        distance,
                        produced: out.len(),
                    });
                }
                if out.len().saturating_add(length) > limit {
                    return Err(InflateError::TooLarge { limit });
                }
                // **Byte at a time, and that is required rather than
                // lazy.** A match may overlap its own output — a run of
                // one byte is written as distance 1, length 200 — so a
                // block copy of the whole span would read bytes that have
                // not been produced yet.
                let from = out.len() - distance;
                for step in 0..length {
                    let byte = *out.get(from + step).ok_or(InflateError::BadDistance {
                        distance,
                        produced: out.len(),
                    })?;
                    out.push(byte);
                }
            }
            _ => return Err(InflateError::BadSymbol),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InflateError, inflate};

    /// A stored block: the header, a length and its complement, then the
    /// bytes. Written by hand so the test does not depend on the encoder
    /// to prove the simplest block kind works.
    fn stored_block(body: &[u8]) -> Vec<u8> {
        let length = u16::try_from(body.len()).expect("a short body");
        // BFINAL = 1, BTYPE = 00, then five bits of padding to the byte.
        let mut out = vec![0b0000_0001];
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    /// **The simplest block kind, end to end.**
    ///
    /// Probed by flipping a bit of the complement: the length check fails
    /// and names both halves.
    #[test]
    fn a_stored_block_comes_back_as_itself() {
        let body = b"the quick brown fox";
        assert_eq!(
            inflate(&stored_block(body), 1 << 20),
            Ok(body.to_vec()),
            "a stored block did not survive the round trip"
        );
    }

    /// A stored block whose length and complement disagree is refused by
    /// name — that pair is the format's own check that the reader is
    /// aligned where it thinks it is.
    #[test]
    fn a_stored_block_with_a_bad_complement_is_refused() {
        let mut bytes = stored_block(b"hello");
        bytes[3] ^= 0x01;
        // Named exactly rather than matched loosely: the pair it reports
        // is the evidence that it read the header where it thought it
        // did, and a pattern that ignores them would pass on a decoder
        // that had lost its place entirely.
        assert_eq!(
            inflate(&bytes, 1 << 20),
            Err(InflateError::StoredLengthMismatch {
                length: 5,
                complement: !5u16 ^ 1
            }),
            "a corrupt stored header was accepted"
        );
    }

    /// **The encoder's own output reads back.** This is the assertion
    /// that matters most: the two halves of the format live in one crate
    /// and must agree about it, and a decoder that could not read what
    /// this crate writes would be describing a different format.
    ///
    /// The encoder writes a two-byte zlib header before the deflate
    /// stream and a four-byte checksum after it, so both are stepped
    /// over here — the PNG reader above does the same thing for real.
    ///
    /// Probed by taking the stream from one byte later: the first symbol
    /// decodes to nothing and it is refused.
    #[test]
    fn what_the_encoder_writes_reads_back() {
        // A picture with runs in it, so the stream exercises
        // back-references rather than only literals.
        let (width, height) = (16u32, 8u32);
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let shade = u8::try_from((x / 4 + y / 4) * 40 % 255).unwrap_or(0);
                pixels.extend_from_slice(&[shade, 20, 200, 255]);
            }
        }
        let file = crate::encode(width, height, &pixels).expect("the encoder accepts this");

        // The `IDAT` body, found by walking the chunks.
        let mut at = 8;
        let mut data = Vec::new();
        while at + 8 <= file.len() {
            let length =
                u32::from_be_bytes([file[at], file[at + 1], file[at + 2], file[at + 3]]) as usize;
            let kind = &file[at + 4..at + 8];
            if kind == b"IDAT" {
                data.extend_from_slice(&file[at + 8..at + 8 + length]);
            }
            at += 12 + length;
        }
        assert!(!data.is_empty(), "the encoder wrote no image data");

        let raw = inflate(&data[2..data.len() - 4], 1 << 20).expect("the stream decompresses");
        // One filter byte per row, then the row's pixels.
        let stride = width as usize * 4;
        assert_eq!(
            raw.len(),
            (stride + 1) * height as usize,
            "the decompressed stream is not a whole number of filtered rows"
        );
        for (row, chunk) in raw.chunks(stride + 1).enumerate() {
            assert_eq!(
                chunk[0], 0,
                "row {row} claims a filter this encoder never writes"
            );
            let from = row * stride;
            assert_eq!(
                &chunk[1..],
                &pixels[from..from + stride],
                "row {row} came back different"
            );
        }
    }

    /// A stream that ends mid-symbol is refused rather than returning
    /// what it managed — a decoder that hands back a partial image lets a
    /// truncated download look like a small picture.
    #[test]
    fn a_truncated_stream_is_refused() {
        let file = crate::encode(8, 8, &[200u8; 8 * 8 * 4]).expect("the encoder accepts this");
        let mut at = 8;
        let mut data = Vec::new();
        while at + 8 <= file.len() {
            let length =
                u32::from_be_bytes([file[at], file[at + 1], file[at + 2], file[at + 3]]) as usize;
            if &file[at + 4..at + 8] == b"IDAT" {
                data.extend_from_slice(&file[at + 8..at + 8 + length]);
            }
            at += 12 + length;
        }
        let stream = &data[2..data.len() - 4];
        let cut = &stream[..stream.len() / 2];
        assert!(
            inflate(cut, 1 << 20).is_err(),
            "half a stream decompressed without complaint"
        );
    }

    /// The ceiling is a refusal, not a truncation: a stream that expands
    /// past what a caller will hold is an answer they can act on.
    #[test]
    fn a_stream_past_the_limit_is_refused() {
        let body = [7u8; 600];
        assert_eq!(
            inflate(&stored_block(&body), 100),
            Err(InflateError::TooLarge { limit: 100 }),
            "a stream past the ceiling was allowed through"
        );
    }
}

#[cfg(test)]
mod refusals {
    use super::{Huffman, InflateError, inflate};

    /// A deflate stream's own bytes, built bit by bit, least significant
    /// first — which is how the format packs everything except the
    /// Huffman codes themselves.
    #[derive(Default)]
    struct Writer {
        bytes: Vec<u8>,
        bit: u32,
    }

    impl Writer {
        /// `count` bits of `value`, low bit first.
        fn low(&mut self, value: u32, count: u32) {
            for step in 0..count {
                self.one((value >> step) & 1);
            }
        }

        /// `count` bits of `value`, **high bit first** — the order a
        /// Huffman code is written in.
        fn high(&mut self, value: u32, count: u32) {
            for step in (0..count).rev() {
                self.one((value >> step) & 1);
            }
        }

        fn one(&mut self, value: u32) {
            if self.bit == 0 {
                self.bytes.push(0);
            }
            if value & 1 == 1
                && let Some(last) = self.bytes.last_mut()
            {
                *last |= 1 << self.bit;
            }
            self.bit = (self.bit + 1) % 8;
        }
    }

    /// A final block using the fixed tables, ready for symbols.
    fn fixed_block() -> Writer {
        let mut out = Writer::default();
        out.low(1, 1);
        out.low(1, 2);
        out
    }

    /// **Block kind three does not exist**, and its presence means the
    /// stream is not deflate at all rather than that it is damaged.
    #[test]
    fn a_block_kind_that_does_not_exist_is_refused() {
        let mut out = Writer::default();
        out.low(1, 1);
        out.low(3, 2);
        assert_eq!(
            inflate(&out.bytes, 1 << 20),
            Err(InflateError::BadBlockKind { kind: 3 })
        );
    }

    /// **A back-reference cannot reach further back than the output.**
    /// A stream that opens with one is asking to read from before the
    /// beginning, and a decoder that answered with zeros would hand back
    /// a picture built out of a bug.
    #[test]
    fn a_back_reference_before_the_beginning_is_refused() {
        let mut out = fixed_block();
        // Length symbol 257 — seven bits in the fixed table, code
        // 0000001 — then distance symbol 0, five bits of zero.
        out.high(0b000_0001, 7);
        out.high(0, 5);
        assert_eq!(
            inflate(&out.bytes, 1 << 20),
            Err(InflateError::BadDistance {
                distance: 1,
                produced: 0
            })
        );
    }

    /// The fixed table names two length symbols the format never assigns
    /// a meaning to. They decode, and then they are refused.
    #[test]
    fn a_length_symbol_the_format_never_defined_is_refused() {
        let mut out = fixed_block();
        // Symbol 286: eight bits in the fixed table, code 11000110.
        out.high(0b1100_0110, 8);
        assert_eq!(inflate(&out.bytes, 1 << 20), Err(InflateError::BadSymbol));
    }

    /// The ceiling holds inside a Huffman block as well as a stored one —
    /// a literal past it is refused rather than pushed.
    #[test]
    fn a_literal_past_the_ceiling_is_refused() {
        let mut out = fixed_block();
        // Three literals: 'A', 'B', 'C', each eight bits in the fixed
        // table at code 0x30 + value.
        for byte in *b"ABC" {
            out.high(0b0011_0000 + u32::from(byte), 8);
        }
        assert_eq!(
            inflate(&out.bytes, 2),
            Err(InflateError::TooLarge { limit: 2 })
        );
    }

    /// A back-reference that would run past the ceiling is refused before
    /// it is copied, rather than after.
    #[test]
    fn a_match_past_the_ceiling_is_refused() {
        let mut out = fixed_block();
        out.high(0b0011_0000 + u32::from(b'A'), 8);
        // Length symbol 257 is three bytes; distance symbol 0 is one
        // back, which is the 'A' just written.
        out.high(0b000_0001, 7);
        out.high(0, 5);
        assert_eq!(
            inflate(&out.bytes, 2),
            Err(InflateError::TooLarge { limit: 2 })
        );
    }

    /// **A code-length table no canonical code can satisfy is refused at
    /// construction**, not at first use — an over-subscribed table would
    /// otherwise decode a few symbols and then wander.
    #[test]
    fn a_table_no_canonical_code_can_satisfy_is_refused() {
        // Three one-bit codes, where one bit holds two.
        assert!(Huffman::new(&[1, 1, 1]).is_none(), "over-subscribed");
        // A length past the fifteen the format allows.
        assert!(Huffman::new(&[16]).is_none(), "a code of sixteen bits");
        // And a legal table is built.
        assert!(Huffman::new(&[1, 1]).is_some(), "two one-bit codes");
    }

    /// A run that overshoots the tables it is filling would silently steal
    /// from the distance table, so the lengths are counted afterwards.
    #[test]
    fn a_code_length_run_that_overshoots_is_refused() {
        let mut out = Writer::default();
        out.low(1, 1);
        out.low(2, 2);
        // HLIT = 0 (257 literals), HDIST = 0 (1 distance), HCLEN = 0 (4).
        out.low(0, 5);
        out.low(0, 5);
        out.low(0, 4);
        // Four code lengths in the format's own order: 16, 17, 18, 0.
        // Give 18 and 0 one bit each so both are usable.
        for length in [0u32, 1, 1, 0] {
            out.low(length, 3);
        }
        // Symbol 18 writes a run of 11..138 zeros; enough of them
        // overshoot 258.
        for _ in 0..3 {
            out.high(1, 1);
            out.low(127, 7);
        }
        assert_eq!(
            inflate(&out.bytes, 1 << 20),
            Err(InflateError::BadCodeLengths)
        );
    }

    /// **A table with a hole in it refuses the bits that fall into the
    /// hole.** The format allows a table that does not use every code —
    /// one back-reference in a whole stream leaves a distance table with a
    /// single entry — so an under-subscribed table is built rather than
    /// rejected, and the refusal has to come at the moment a code walks
    /// off the end of it.
    #[test]
    fn a_code_that_names_no_symbol_is_refused() {
        let mut out = Writer::default();
        out.low(1, 1);
        out.low(2, 2);
        out.low(0, 5);
        out.low(0, 5);
        out.low(0, 4);
        // One usable code-length symbol, `18`, which writes runs of
        // zeros: every literal length comes out zero, so the literal
        // table has no codes at all and the first bit read against it
        // falls through all fifteen lengths.
        for length in [0u32, 0, 1, 0] {
            out.low(length, 3);
        }
        // 257 + 1 lengths of zero, in runs of at most 138.
        for times in [138u32, 120] {
            out.high(0, 1);
            out.low(times - 11, 7);
        }
        out.bytes.extend_from_slice(&[0u8; 8]);
        assert_eq!(inflate(&out.bytes, 1 << 20), Err(InflateError::BadSymbol));
    }

    /// A repeat-previous symbol with nothing before it is a stream that
    /// opens by copying what it has not written.
    #[test]
    fn a_repeat_with_nothing_to_repeat_is_refused() {
        let mut out = Writer::default();
        out.low(1, 1);
        out.low(2, 2);
        out.low(0, 5);
        out.low(0, 5);
        out.low(0, 4);
        // Lengths for 16, 17, 18, 0: give 16 and 0 one bit each.
        for length in [1u32, 0, 0, 1] {
            out.low(length, 3);
        }
        // Symbol 16 first: repeat the previous length, of which there is
        // none. Padded afterwards so the refusal comes from the missing
        // previous length rather than from the stream running out.
        //
        // Code 1, not 0: among the two one-bit codes the canonical
        // assignment gives symbol 0 the zero and symbol 16 the one, and
        // asking for the wrong one reads a run of zero lengths instead.
        out.high(1, 1);
        out.low(0, 2);
        out.bytes.extend_from_slice(&[0u8; 8]);
        assert_eq!(
            inflate(&out.bytes, 1 << 20),
            Err(InflateError::BadCodeLengths)
        );
    }
}
