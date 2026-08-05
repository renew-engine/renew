//! A PNG encoder, in about a hundred lines and with no dependency.
//!
//! # Why this exists rather than a crate
//!
//! Every dependency is presumed rejected until it earns itself, and a
//! third-party one is the owner's decision. A PNG turns out to be four
//! chunks, two checksums, and a deflate stream — and deflate's *fixed*
//! Huffman tables are published constants, so a compressor good enough
//! for flat-shaded pictures needs no tables of its own and no tuning.
//!
//! # What it does and does not do
//!
//! Fixed Huffman codes over a three-candidate back-reference search: the
//! pixel to the left, the pixel above, and the byte before. Those are what
//! a rendered picture is made of, and they take a 256×256 flat image from
//! 256 KiB to **about two kilobytes**. Data with no such structure comes
//! out slightly *larger* than raw — fixed Huffman spends nine bits on half
//! the byte values — which is the honest trade: this encoder is for
//! pictures of geometry, not for photographs.
//!
//! No dynamic Huffman, no filtering (every scanline carries filter byte
//! 0), no palettes, no interlacing, 8-bit RGBA only. It exists so a person
//! can look at what the renderer drew, and so those pictures can be
//! committed without weighing down the repository.
//!
//! Output is a pure function of the pixels — the same image encodes to the
//! same bytes on every platform and every run — so an encoded file is
//! comparable byte for byte.
//!
//! # How the format was checked
//!
//! The tests below assert the byte layout against the specification. That
//! catches a typo and would not catch a specification consistently
//! misread, since the same reading wrote both the encoder and the test.
//! So the output is also handed to an independent decoder — seven images
//! across the flat, banded, striped and incompressible cases — and every
//! one opened as RGBA at the right dimensions with the pixels
//! round-tripping exactly, with the stream separately accepted by a stock
//! inflater. **That is not a formality: it caught a real defect.** The
//! length-symbol arithmetic was off by one, so every match of eleven bytes
//! or more encoded as the wrong symbol. The file was still small, still
//! structurally a PNG, still had a valid header, and every test in this
//! file passed. Only a decoder refused it. The symbol tables are now
//! pinned to the published ones by a test, and the probe that writes those
//! files is not in the tree — judging the result needs an image library,
//! and this repository has none.

/// The eight bytes every PNG starts with.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Bytes per pixel: 8-bit RGBA.
const CHANNELS: usize = 4;

/// Encode `pixels` as an 8-bit RGBA PNG.
///
/// `pixels` is `width * height * 4` bytes, row-major from the top.
///
/// # Errors
///
/// `None` when the dimensions are zero, when they disagree with the
/// length of `pixels`, or when the encoded size would overflow the sizes
/// PNG can express. Every one of those is a caller mistake rather than a
/// condition to recover from, but an image writer that panicked on a bad
/// size would be a poor neighbour to a command line.
#[must_use]
pub fn encode(width: u32, height: u32, pixels: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return None;
    }
    // Checked throughout. The arithmetic here is exactly the arithmetic
    // that decides how much is copied later, so a wrap would produce a
    // plausible-looking header over the wrong number of bytes.
    let row = usize::try_from(width).ok()?.checked_mul(CHANNELS)?;
    let rows = usize::try_from(height).ok()?;
    if pixels.len() != row.checked_mul(rows)? {
        return None;
    }
    // Each scanline is prefixed with its filter byte.
    let raw_len = row.checked_add(1)?.checked_mul(rows)?;

    let mut raw = Vec::with_capacity(raw_len);
    for line in pixels.chunks_exact(row) {
        raw.push(0); // filter: None
        raw.extend_from_slice(line);
    }

    let mut out = Vec::from(SIGNATURE);
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // depth 8, RGBA, no filter or interlace
    chunk(&mut out, *b"IHDR", &header);
    chunk(&mut out, *b"IDAT", &zlib(&raw, row + 1));
    chunk(&mut out, *b"IEND", &[]);
    Some(out)
}

/// One PNG chunk: length, type, data, CRC.
///
/// The length counts the data alone, and the CRC covers the type **and**
/// the data but not the length — a pairing that is easy to state and easy
/// to get subtly wrong, which is why both are asserted by a hand-checked
/// byte layout in the tests.
fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);

    let mut crc = Crc::new();
    crc.eat(&kind);
    crc.eat(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// `raw` wrapped in a zlib stream, compressed with fixed Huffman codes
/// over a small back-reference search.
///
/// **Compressed rather than stored, because these files are committed.**
/// A stored stream is simpler and was what this encoder wrote first, but
/// an uncompressed render is a quarter of a megabyte, every regeneration
/// adds another permanent copy to the repository's history, and the
/// pictures this writes are flat-coloured faces — the most compressible
/// thing there is.
fn zlib(raw: &[u8], stride: usize) -> Vec<u8> {
    // 0x78: deflate, 32 KiB window. 0x01: no preset dictionary. The pair
    // must be a multiple of 31, and 0x7801 is 31 * 991.
    let mut bits = BitWriter::new(vec![0x78, 0x01]);

    // One block for the whole image, final, with the fixed code tables.
    // BFINAL = 1 then BTYPE = 01, each written low bit first.
    bits.raw(1, 1);
    bits.raw(1, 2);

    let mut at = 0;
    while at < raw.len() {
        let (length, distance) = longest_match(raw, at, stride);
        if length >= MIN_MATCH {
            emit_length(&mut bits, length);
            emit_distance(&mut bits, distance);
            at += length;
        } else {
            emit_literal(&mut bits, raw[at]);
            at += 1;
        }
    }
    emit_literal_code(&mut bits, END_OF_BLOCK);

    let mut out = bits.finish();
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// The shortest back-reference deflate can express.
const MIN_MATCH: usize = 3;

/// The longest one.
const MAX_MATCH: usize = 258;

/// The literal/length symbol that ends a block.
const END_OF_BLOCK: u16 = 256;

/// The best back-reference at `at`, or a length below [`MIN_MATCH`] when
/// there is none worth making.
///
/// **Three candidate distances rather than a search**, and the choice is
/// what the pictures look like rather than a general compromise: `4` is
/// the pixel before this one, which covers a horizontal run of one
/// colour; `stride` is the pixel directly above, which covers a flat area
/// and the long vertical edges of a voxel face; `1` covers a byte run,
/// which is what a grey or a fully transparent region is.
///
/// A real compressor searches a hash chain over every distance and does
/// better on photographs. On flat-shaded geometry these three catch
/// almost all of it, and the whole matcher is a dozen lines that can be
/// read against the format rather than trusted.
fn longest_match(raw: &[u8], at: usize, stride: usize) -> (usize, usize) {
    let mut best = (0usize, 0usize);
    for distance in [4usize, stride, 1] {
        if distance == 0 || distance > at {
            continue;
        }
        let limit = MAX_MATCH.min(raw.len() - at);
        let mut length = 0;
        while length < limit && raw[at - distance + length] == raw[at + length] {
            length += 1;
        }
        if length > best.0 {
            best = (length, distance);
        }
    }
    best
}

/// Writes bits low-order first, which is how deflate packs everything
/// except a Huffman code.
struct BitWriter {
    out: Vec<u8>,
    bits: u32,
    count: u32,
}

impl BitWriter {
    const fn new(out: Vec<u8>) -> Self {
        Self {
            out,
            bits: 0,
            count: 0,
        }
    }

    /// `width` bits of `value`, least significant first.
    fn raw(&mut self, value: u32, width: u32) {
        self.bits |= value << self.count;
        self.count += width;
        while self.count >= 8 {
            self.out.push(u8::try_from(self.bits & 0xff).unwrap_or(0));
            self.bits >>= 8;
            self.count -= 8;
        }
    }

    /// A Huffman code, whose bits go **most** significant first.
    ///
    /// The one reversal in the format, and the classic way to produce a
    /// stream that decodes as garbage rather than failing outright.
    fn code(&mut self, value: u32, width: u32) {
        for index in (0..width).rev() {
            self.raw((value >> index) & 1, 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.out.push(u8::try_from(self.bits & 0xff).unwrap_or(0));
        }
        self.out
    }
}

/// One literal byte, in the fixed literal/length table.
fn emit_literal(bits: &mut BitWriter, byte: u8) {
    emit_literal_code(bits, u16::from(byte));
}

/// A symbol from the fixed literal/length table, which uses three code
/// lengths across four ranges.
fn emit_literal_code(bits: &mut BitWriter, symbol: u16) {
    let value = u32::from(symbol);
    match symbol {
        0..=143 => bits.code(0x30 + value, 8),
        144..=255 => bits.code(0x190 + value - 144, 9),
        256..=279 => bits.code(value - 256, 7),
        _ => bits.code(0xc0 + value - 280, 8),
    }
}

/// A match length, as its symbol plus any extra bits.
///
/// The symbol and its extra-bit count are computed rather than tabulated:
/// lengths 11 to 257 fall into groups of four whose size doubles, which
/// is exactly what the bit width of `length - 3` says.
fn emit_length(bits: &mut BitWriter, length: usize) {
    let (symbol, extra) = length_symbol(length);
    emit_literal_code(bits, symbol);
    if extra > 0 {
        let value = length - MIN_MATCH;
        bits.raw(
            u32::try_from(value).unwrap_or(0) & ((1 << extra) - 1),
            extra,
        );
    }
}

/// The literal/length symbol for a match length, and how many extra bits
/// follow it.
///
/// Computed rather than tabulated: lengths eleven to 257 fall into groups
/// of four whose size doubles, which is what the bit width of
/// `length - 3` says. Pinned to the published table by a test, because
/// the compactness is exactly what makes an off-by-one here invisible.
fn length_symbol(length: usize) -> (u16, u32) {
    if length == MAX_MATCH {
        return (285, 0);
    }
    if length <= 10 {
        return (u16::try_from(254 + length).unwrap_or(285), 0);
    }
    let value = length - MIN_MATCH;
    let extra = bit_width(value) - 2;
    let symbol = 261 + 4 * extra + ((value >> extra) & 3);
    (
        u16::try_from(symbol).unwrap_or(285),
        u32::try_from(extra).unwrap_or(0),
    )
}

/// A match distance, as its five-bit fixed code plus any extra bits.
fn emit_distance(bits: &mut BitWriter, distance: usize) {
    let (symbol, extra) = distance_symbol(distance);
    bits.code(u32::from(symbol), 5);
    if extra > 0 {
        let value = distance - 1;
        bits.raw(
            u32::try_from(value).unwrap_or(0) & ((1 << extra) - 1),
            extra,
        );
    }
}

/// The distance symbol for a match distance, and how many extra bits
/// follow it. Same shape and same reasoning as [`length_symbol`].
fn distance_symbol(distance: usize) -> (u16, u32) {
    let value = distance - 1;
    if distance <= 4 {
        return (u16::try_from(value).unwrap_or(0), 0);
    }
    let extra = bit_width(value) - 1;
    let symbol = 2 * extra + 2 + ((value >> extra) & 1);
    (
        u16::try_from(symbol).unwrap_or(0),
        u32::try_from(extra).unwrap_or(0),
    )
}

/// The position of `value`'s highest set bit, counting from zero.
///
/// `value` is never zero at either call site: a length reaching here is
/// at least 11, so `length - 3` is at least 8, and a distance reaching
/// there is at least 5, so `distance - 1` is at least 4.
fn bit_width(value: usize) -> usize {
    usize::BITS as usize - 1 - value.leading_zeros() as usize
}

/// CRC-32 as PNG uses it, computed a bit at a time so that no lookup
/// table has to be written out and trusted.
struct Crc(u32);

impl Crc {
    const fn new() -> Self {
        Self(0xffff_ffff)
    }

    fn eat(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u32::from(*byte);
            for _ in 0..8 {
                // The reflected polynomial. Branchless would be no
                // clearer here, and this runs once per output byte.
                self.0 = if self.0 & 1 == 1 {
                    (self.0 >> 1) ^ 0xedb8_8320
                } else {
                    self.0 >> 1
                };
            }
        }
    }

    const fn finish(&self) -> u32 {
        self.0 ^ 0xffff_ffff
    }
}

/// Adler-32, the checksum a zlib stream ends with.
///
/// Over the *uncompressed* bytes — which is the mistake worth naming,
/// since every other checksum in the file covers what is written.
fn adler32(bytes: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut low, mut high) = (1u32, 0u32);
    for byte in bytes {
        low = (low + u32::from(*byte)) % MOD;
        high = (high + low) % MOD;
    }
    (high << 16) | low
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The length and distance symbol tables, against the published ones.
    ///
    /// **These are computed rather than tabulated in the encoder**, which
    /// is compact and was wrong: the length exponent was off by one, so
    /// every match of eleven bytes or more encoded as the wrong symbol.
    /// Nothing in this file noticed -- the output was still small, still
    /// structurally a PNG, and still had a valid header. Only a decoder
    /// refused it. These cases pin the arithmetic to the format's own
    /// table so the next mistake fails here instead.
    #[test]
    fn length_and_distance_symbols_match_the_published_tables() {
        // (length, symbol, extra bits) from RFC 1951's table.
        for (length, symbol, extra) in [
            (3usize, 257u16, 0u32),
            (10, 264, 0),
            (11, 265, 1),
            (13, 266, 1),
            (19, 269, 2),
            (23, 270, 2),
            (35, 273, 3),
            (131, 281, 5),
            (257, 284, 5),
            (258, 285, 0),
        ] {
            assert_eq!(
                length_symbol(length),
                (symbol, extra),
                "length {length} should be symbol {symbol} with {extra} extra bits"
            );
        }

        for (distance, symbol, extra) in [
            (1usize, 0u16, 0u32),
            (4, 3, 0),
            (5, 4, 1),
            (7, 5, 1),
            (9, 6, 2),
            (1025, 20, 9),
            (24577, 29, 13),
        ] {
            assert_eq!(
                distance_symbol(distance),
                (symbol, extra),
                "distance {distance} should be symbol {symbol} with {extra} extra bits"
            );
        }
    }

    /// The stream a stock inflater accepts, checked structurally here and
    /// end to end elsewhere.
    ///
    /// **This is the test that cannot be complete.** A deflate bitstream
    /// is packed low-bit-first except for Huffman codes, which are
    /// high-bit-first, and a file that gets that backwards decodes into
    /// plausible garbage rather than failing. Asserting the bytes against
    /// my own reading of the format would check the same reading twice,
    /// so what actually establishes validity is an independent decoder --
    /// see the note at the top of this file. These assertions catch the
    /// things that are checkable from inside.
    #[test]
    fn the_stream_starts_with_a_valid_zlib_header_and_a_final_fixed_block() {
        let png = encode(1, 1, &[255, 0, 0, 255]).expect("one red pixel");
        let idat = png
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("an IDAT chunk");
        let stream = &png[idat + 4..];

        let header = u16::from_be_bytes([stream[0], stream[1]]);
        assert_eq!(header, 0x7801, "deflate, 32 KiB window, no dictionary");
        assert_eq!(
            header % 31,
            0,
            "the header must be a multiple of 31, which is what a decoder checks first"
        );

        // The first byte of the compressed data carries BFINAL then
        // BTYPE, low bit first: 1 then 01 makes the low three bits 0b011.
        assert_eq!(stream[2] & 0b111, 0b011, "one final block, fixed Huffman");
    }

    /// A flat image compresses, and by a lot.
    ///
    /// The reason this encoder gained a compressor at all: the pictures
    /// are committed to the repository, so an uncompressed quarter of a
    /// megabyte per render would accumulate in its history forever.
    #[test]
    fn a_flat_image_compresses_far_below_its_raw_size() {
        let pixels = vec![90u8; 256 * 256 * 4];
        let png = encode(256, 256, &pixels).expect("a flat square");
        let raw = 256 * 256 * 4;
        // Bound first, so no call hides in a region that runs only when
        // the assertion fails -- a line no passing run would cover.
        let size = png.len();
        assert!(
            size * 100 < raw,
            "a single-colour image should shrink by two orders of magnitude, got {size} against {raw}"
        );
    }

    /// Matching runs both across a row and against the row above.
    ///
    /// Vertical coherence is what a rendered wall is made of, and a
    /// matcher that only looked backwards along the row would miss it.
    #[test]
    fn vertical_repetition_compresses_as_well_as_horizontal() {
        // Every row identical, but no two adjacent pixels alike -- so
        // only a match against the row above can help.
        let row: Vec<u8> = (0..64u32)
            .flat_map(|x| [u8::try_from(x).unwrap_or(0), 200, 30, 255])
            .collect();
        let pixels: Vec<u8> = std::iter::repeat_n(row.clone(), 64).flatten().collect();
        let png = encode(64, 64, &pixels).expect("a striped square");
        let (size, width) = (png.len(), row.len());
        assert!(
            size < width * 8,
            "rows repeat exactly, so all but the first should nearly vanish, got {size}"
        );
    }

    /// Nothing compressible still encodes correctly.
    ///
    /// The path where every position falls through to a literal, which is
    /// the arm the flat cases never reach.
    #[test]
    fn incompressible_pixels_still_encode() {
        // A counter across all four channels: no repeat at distance 1, 4
        // or a row.
        let pixels: Vec<u8> = (0..32u32 * 32 * 4)
            .map(|index| u8::try_from((index * 37 + index / 7) % 256).unwrap_or(0))
            .collect();
        let png = encode(32, 32, &pixels).expect("noise");
        assert!(
            png.len() > 32 * 32,
            "noise cannot shrink much; a tiny file here means data was dropped"
        );
    }

    /// The published check value for CRC-32, and the empty case.
    #[test]
    fn crc_matches_its_known_answers() {
        let mut empty = Crc::new();
        empty.eat(b"");
        assert_eq!(empty.finish(), 0x0000_0000);

        let mut check = Crc::new();
        check.eat(b"123456789");
        assert_eq!(
            check.finish(),
            0xcbf4_3926,
            "the published CRC-32 check value"
        );
    }

    /// The published check value for Adler-32, and the empty case, which
    /// is 1 rather than 0 and is the classic thing to get wrong.
    #[test]
    fn adler_matches_its_known_answers() {
        assert_eq!(adler32(b""), 1, "an empty Adler-32 is one, not zero");
        assert_eq!(adler32(b"123456789"), 0x091e_01de);
    }

    /// Malformed requests are refused rather than encoded.
    #[test]
    fn impossible_images_are_refused() {
        assert!(encode(0, 1, &[]).is_none(), "a zero width has no image");
        assert!(encode(1, 0, &[]).is_none(), "a zero height has no image");
        assert!(
            encode(2, 2, &[0; 4]).is_none(),
            "four bytes is one pixel, not four"
        );
        assert!(
            encode(u32::MAX, u32::MAX, &[0; 4]).is_none(),
            "a size that cannot be allocated is refused, not wrapped"
        );
    }

    /// The same pixels encode to the same bytes, which is what lets an
    /// image be compared rather than merely looked at.
    #[test]
    fn encoding_is_a_pure_function_of_the_pixels() {
        let pixels: Vec<u8> = (0..64 * 64 * 4)
            .map(|index: u32| u8::try_from(index % 251).unwrap_or(0))
            .collect();
        assert_eq!(encode(64, 64, &pixels), encode(64, 64, &pixels));
    }
}
