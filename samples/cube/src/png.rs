//! A PNG encoder, in about a hundred lines and with no dependency.
//!
//! # Why this exists rather than a crate
//!
//! Every dependency is presumed rejected until it earns itself, and a
//! third-party one is the owner's decision. Encoding a PNG turns out not
//! to need a compressor at all: the format's data is a zlib stream, and a
//! zlib stream may consist of deflate **stored** blocks, which are the
//! bytes copied verbatim behind a five-byte header. So the whole encoder
//! is four chunks, two checksums, and some framing.
//!
//! **The cost, stated honestly:** stored blocks add five bytes per 65535,
//! so the file is about 0.008% larger than the raw pixels — but a real
//! compressor would make it *much smaller* than raw, and for a rendered
//! image of flat-coloured faces the difference would be large. This trades
//! file size for having no dependency, an unambiguous byte layout, and an
//! encoder a reader can check against the specification in one sitting.
//!
//! # What it does not do
//!
//! No compression, no filtering (every scanline carries filter byte 0),
//! no palettes, no interlacing, 8-bit RGBA only. It exists so a person can
//! look at what the renderer drew; it is not an image library.
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
//! So the output was also handed to an independent decoder — five sizes,
//! including the one that needs two stored blocks — and every one opened
//! as RGBA at the right dimensions with the pixels round-tripping exactly,
//! with the zlib stream separately accepted by a stock inflater. The
//! probe that produced those files is not in the tree: it needed an image
//! library to judge the result, and this repository has none.

/// The eight bytes every PNG starts with.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The largest payload one deflate stored block may carry.
const MAX_BLOCK: usize = 65535;

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
    chunk(&mut out, *b"IDAT", &zlib(&raw));
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

/// `raw` wrapped in a zlib stream of stored deflate blocks.
fn zlib(raw: &[u8]) -> Vec<u8> {
    // 0x78: deflate, 32 KiB window. 0x01: no preset dictionary, fastest
    // level. The pair must be a multiple of 31, and 0x7801 is 31 * 991.
    let mut out = vec![0x78, 0x01];

    // An empty stream still needs one (final, empty) block, or the
    // decoder reaches the checksum looking for a block header.
    let mut chunks = raw.chunks(MAX_BLOCK).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
    }
    while let Some(block) = chunks.next() {
        let final_block = u8::from(chunks.peek().is_none());
        out.push(final_block); // BFINAL in bit 0, BTYPE 00 (stored)
        let len = u16::try_from(block.len()).unwrap_or(u16::MAX);
        // LEN and NLEN are little-endian, unlike everything else in a
        // PNG, because they belong to deflate rather than to PNG.
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }

    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
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

    /// An empty stream still carries one block.
    ///
    /// `encode` cannot reach this — it refuses a zero dimension, so there
    /// is always at least a filter byte — but `zlib` is the piece that
    /// knows the framing, and a decoder handed a stream with no block at
    /// all reads the Adler-32 as a block header and fails somewhere far
    /// from the cause.
    #[test]
    fn an_empty_stream_still_carries_one_final_block() {
        let stream = zlib(&[]);
        assert_eq!(stream[..2], [0x78, 0x01], "the zlib header");
        assert_eq!(stream[2], 0x01, "one block, final");
        assert_eq!(&stream[3..5], &0u16.to_le_bytes(), "carrying nothing");
        assert_eq!(
            &stream[5..7],
            &(!0u16).to_le_bytes(),
            "NLEN still complements it"
        );
        assert_eq!(
            &stream[7..],
            &1u32.to_be_bytes(),
            "Adler-32 of nothing is one"
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

    /// A one-pixel image, checked byte by byte against the format.
    ///
    /// Hand-derived rather than recorded from the encoder: this is the
    /// test that would catch the encoder agreeing with itself.
    #[test]
    fn a_single_pixel_encodes_to_the_bytes_the_format_specifies() {
        let png = encode(1, 1, &[255, 0, 0, 255]).expect("one red pixel");

        assert_eq!(&png[..8], &SIGNATURE, "the signature");
        // 8 signature + 25 IHDR + IDAT(12 + 2 + 5 + 5 + 4) + 12 IEND
        assert_eq!(
            png.len(),
            8 + 25 + 12 + 16 + 12,
            "the whole file is 73 bytes"
        );

        // IHDR: length 13, then the type.
        assert_eq!(&png[8..12], &13u32.to_be_bytes());
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &1u32.to_be_bytes(), "width");
        assert_eq!(&png[20..24], &1u32.to_be_bytes(), "height");
        assert_eq!(
            &png[24..29],
            &[8, 6, 0, 0, 0],
            "depth 8, RGBA, no interlace"
        );

        // IDAT's payload: the zlib header, one final stored block of five
        // bytes (filter + RGBA), then the Adler-32 of those five.
        let idat = 33;
        assert_eq!(&png[idat..idat + 4], &16u32.to_be_bytes(), "IDAT length");
        assert_eq!(&png[idat + 4..idat + 8], b"IDAT");
        assert_eq!(&png[idat + 8..idat + 10], &[0x78, 0x01], "zlib header");
        assert_eq!(png[idat + 10], 0x01, "one block, and it is the final one");
        assert_eq!(&png[idat + 11..idat + 13], &5u16.to_le_bytes(), "LEN");
        assert_eq!(&png[idat + 13..idat + 15], &(!5u16).to_le_bytes(), "NLEN");
        assert_eq!(
            &png[idat + 15..idat + 20],
            &[0, 255, 0, 0, 255],
            "filter, then the pixel"
        );
        assert_eq!(
            &png[idat + 20..idat + 24],
            &adler32(&[0, 255, 0, 0, 255]).to_be_bytes()
        );

        // IEND: empty, and its CRC is a constant of the format.
        assert_eq!(&png[png.len() - 12..png.len() - 8], &0u32.to_be_bytes());
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        assert_eq!(&png[png.len() - 4..], &0xae42_6082u32.to_be_bytes());
    }

    /// The zlib header is a multiple of 31, which is the check a decoder
    /// performs first and rejects the file on.
    #[test]
    fn the_zlib_header_passes_its_own_check() {
        let png = encode(1, 1, &[0, 0, 0, 0]).expect("one pixel");
        let header = u16::from_be_bytes([png[41], png[42]]);
        assert_eq!(header % 31, 0, "0x{header:04x} must be a multiple of 31");
    }

    /// More than 65535 bytes of scanline means more than one block, and
    /// only the last is final.
    ///
    /// The single-pixel test above cannot reach this, and a decoder that
    /// stops at the first block would produce a truncated image rather
    /// than an error — the failure that looks like a rendering bug.
    #[test]
    fn a_large_image_is_split_into_blocks_and_only_the_last_is_final() {
        // 1 pixel wide so the row is 5 bytes with its filter: 13108 rows
        // is 65540 raw bytes, one block over the limit.
        let height = 13108;
        let pixels = vec![7u8; height * CHANNELS];
        let png = encode(1, u32::try_from(height).expect("fits"), &pixels).expect("encodes");

        // Walk the stored blocks from the start of the zlib data.
        let mut at = 8 + 25 + 8 + 2; // signature, IHDR, IDAT header, zlib
        let mut headers = Vec::new();
        loop {
            let final_block = png[at];
            let len = u16::from_le_bytes([png[at + 1], png[at + 2]]);
            let nlen = u16::from_le_bytes([png[at + 3], png[at + 4]]);
            assert_eq!(nlen, !len, "NLEN is LEN's complement");
            headers.push((final_block, len));
            at += 5 + usize::from(len);
            if final_block == 1 {
                break;
            }
        }

        assert_eq!(headers.len(), 2, "65540 bytes needs two blocks");
        assert_eq!(
            headers[0],
            (0, 65535),
            "the first block is full and not final"
        );
        assert_eq!(headers[1], (1, 5), "the remainder is final");
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
