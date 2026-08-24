//! A PNG decoder, to the encoder's dimensions.
//!
//! # Why this exists
//!
//! A crate that writes pictures and cannot read one leaves every consumer
//! that ships art to find its own way in. That is the gap: this crate has
//! encoded since its first commit so a person could look at what the
//! renderer drew, and the moment anything wants to *load* an image —
//! sprites, tiles, a font atlas, a heightfield — it has nowhere to go.
//!
//! Written rather than depended on for the reason the encoder gives: the
//! two halves of a format should not disagree about what the format is,
//! and the tables, the bit order and the chunk layout are already here.
//!
//! # What it reads
//!
//! Every non-interlaced 8- and 16-bit PNG: greyscale, greyscale with
//! alpha, truecolour, truecolour with alpha, and palette — with `tRNS`
//! honoured for the three types that can carry it. Output is always 8-bit
//! RGBA, because a consumer that wanted to branch on colour type would be
//! doing the decoder's job.
//!
//! **Not** interlaced images, and not bit depths below eight. Both are
//! refused by name rather than mis-read. Adam7 is a different image layout
//! rather than a different pixel format, and sub-byte depths want a
//! bit-unpacker that nothing has asked for; when something does, they are
//! additions here rather than a second decoder.
//!
//! # Refusals
//!
//! Every chunk's CRC is checked, every length is validated against the
//! bytes actually present, and every field is checked against the values
//! the specification allows. A file from disk or from the network is
//! hostile input, and this reads it as such: nothing is trusted because it
//! is written down.

use crate::inflate::{InflateError, inflate};

/// A decoded image: 8-bit RGBA, row-major, top row first.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Image {
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
    /// `width * height * 4` bytes, red, green, blue then alpha.
    pub pixels: Vec<u8>,
}

/// Why an image could not be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The first eight bytes are not PNG's signature. Usually a file that
    /// is not a PNG at all, which is worth saying differently from a PNG
    /// that is damaged.
    NotAPng,
    /// A chunk's declared length runs past the end of the file.
    ChunkOverruns {
        /// The four bytes of the chunk's type.
        id: [u8; 4],
        /// Where the chunk began.
        at: usize,
        /// The length it declared.
        declared: usize,
        /// How many bytes were actually left.
        available: usize,
    },
    /// A chunk's checksum does not match its contents.
    BadChecksum {
        /// The four bytes of the chunk's type.
        id: [u8; 4],
        /// What the file claimed.
        declared: u32,
        /// What the bytes actually hash to.
        found: u32,
    },
    /// No `IHDR`, or one that is not thirteen bytes.
    BadHeader,
    /// A width or a height of zero, which PNG does not allow.
    ZeroExtent {
        /// The width the header declared.
        width: u32,
        /// The height the header declared.
        height: u32,
    },
    /// A colour type the format does not define.
    BadColourType {
        /// The value the header carried.
        colour: u8,
    },
    /// A bit depth this decoder does not read. Eight and sixteen are
    /// read; one, two and four are legal PNG and refused here.
    UnsupportedDepth {
        /// The depth the header declared.
        depth: u8,
        /// The colour type it went with.
        colour: u8,
    },
    /// An interlaced image. Legal PNG, and a different image layout
    /// rather than a different pixel format.
    Interlaced,
    /// A compression or filter method the format does not define — both
    /// have exactly one legal value.
    BadMethod {
        /// The compression method byte.
        compression: u8,
        /// The filter method byte.
        filter: u8,
    },
    /// A palette image with no `PLTE`, or an index past the palette's end.
    BadPalette {
        /// How many entries the palette held.
        entries: usize,
        /// The index that was asked for.
        index: usize,
    },
    /// No `IDAT` chunks at all.
    NoImageData,
    /// The file ended without an `IEND` chunk.
    ///
    /// **Which is how a truncation is caught.** Image data is written
    /// before the terminator, so a file cut short after its last `IDAT`
    /// holds a complete-looking image and would otherwise decode without
    /// complaint — a half-downloaded sprite that looks like a whole one.
    MissingEnd,
    /// The zlib wrapper around the image data is malformed.
    BadZlibHeader {
        /// The two bytes that were read.
        header: [u8; 2],
    },
    /// The image data could not be decompressed.
    Deflate {
        /// Why not.
        cause: InflateError,
    },
    /// The decompressed data is not a whole number of filtered rows.
    BadImageLength {
        /// What the header's shape requires.
        expected: usize,
        /// What arrived.
        found: usize,
    },
    /// A scanline filter byte the format does not define.
    BadFilter {
        /// The value that was read.
        filter: u8,
        /// Which row carried it.
        row: usize,
    },
    /// The image is larger than this machine can address.
    TooLarge {
        /// The width the header declared.
        width: u32,
        /// The height the header declared.
        height: u32,
    },
}

/// The largest image this decoder will expand into memory, in bytes.
///
/// **A ceiling rather than trust.** A header is four bytes of width and
/// four of height, so a file of sixty bytes can ask for sixty-four
/// gigabytes; without a limit the refusal is an allocation failure rather
/// than an answer. Two hundred and fifty-six megabytes is far past any
/// texture a game loads and far short of anything that hurts.
const CEILING: usize = 256 << 20;

/// Read a PNG.
///
/// # Errors
///
/// Every way a file can fail to be a readable PNG, by name — see
/// [`DecodeError`].
pub fn decode(bytes: &[u8]) -> Result<Image, DecodeError> {
    if bytes.len() < crate::SIGNATURE.len() || bytes[..crate::SIGNATURE.len()] != crate::SIGNATURE {
        return Err(DecodeError::NotAPng);
    }

    let mut header: Option<Header> = None;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut alpha: Vec<u8> = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    let mut ended = false;

    let mut at = crate::SIGNATURE.len();
    while at + 8 <= bytes.len() {
        let declared =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let id = [bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]];
        let body_at = at + 8;
        let available = bytes.len().saturating_sub(body_at);
        // The four checksum bytes must be there too.
        if declared.saturating_add(4) > available {
            return Err(DecodeError::ChunkOverruns {
                id,
                at,
                declared,
                available,
            });
        }
        let body = &bytes[body_at..body_at + declared];
        let stated = u32::from_be_bytes([
            bytes[body_at + declared],
            bytes[body_at + declared + 1],
            bytes[body_at + declared + 2],
            bytes[body_at + declared + 3],
        ]);
        let found = crate::checksum(id, body);
        if stated != found {
            return Err(DecodeError::BadChecksum {
                id,
                declared: stated,
                found,
            });
        }

        match &id {
            b"IHDR" => header = Some(read_header(body)?),
            b"PLTE" => {
                palette = body.as_chunks::<3>().0.to_vec();
            }
            b"tRNS" => alpha = body.to_vec(),
            b"IDAT" => data.extend_from_slice(body),
            b"IEND" => {
                ended = true;
                break;
            }
            _ => {}
        }
        at = body_at + declared + 4;
    }

    let header = header.ok_or(DecodeError::BadHeader)?;
    if !ended {
        return Err(DecodeError::MissingEnd);
    }
    if data.is_empty() {
        return Err(DecodeError::NoImageData);
    }
    let raw = decompress(&data, &header)?;
    let samples = unfilter(raw, &header)?;
    expand(&samples, &header, &palette, &alpha)
}

/// What `IHDR` said.
#[derive(Clone, Copy, Debug)]
struct Header {
    width: u32,
    height: u32,
    depth: u8,
    colour: u8,
}

impl Header {
    /// How many samples each pixel carries.
    const fn channels(self) -> usize {
        match self.colour {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            _ => 4,
        }
    }

    /// How many bytes each pixel occupies in the filtered stream.
    const fn stride(self) -> usize {
        self.channels() * (self.depth as usize / 8)
    }

    /// How many bytes each row occupies, without its filter byte.
    const fn row(self) -> usize {
        self.width as usize * self.stride()
    }
}

fn read_header(body: &[u8]) -> Result<Header, DecodeError> {
    let Some(body) = body.get(..13) else {
        return Err(DecodeError::BadHeader);
    };
    let width = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    let height = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
    let (depth, colour, compression, filter, interlace) =
        (body[8], body[9], body[10], body[11], body[12]);
    if width == 0 || height == 0 {
        return Err(DecodeError::ZeroExtent { width, height });
    }
    if !matches!(colour, 0 | 2 | 3 | 4 | 6) {
        return Err(DecodeError::BadColourType { colour });
    }
    if compression != 0 || filter != 0 {
        return Err(DecodeError::BadMethod {
            compression,
            filter,
        });
    }
    if interlace != 0 {
        return Err(DecodeError::Interlaced);
    }
    // Sixteen-bit palette entries do not exist; everything else may be
    // eight or sixteen here.
    if !matches!(depth, 8 | 16) || (colour == 3 && depth != 8) {
        return Err(DecodeError::UnsupportedDepth { depth, colour });
    }
    let header = Header {
        width,
        height,
        depth,
        colour,
    };
    // The filtered stream is one byte per row plus the row itself, and it
    // has to fit in a `usize` before anything is allocated for it.
    header
        .row()
        .checked_add(1)
        .and_then(|row| row.checked_mul(height as usize))
        .filter(|total| *total <= CEILING)
        .ok_or(DecodeError::TooLarge { width, height })?;
    Ok(header)
}

/// Strip the zlib wrapper and decompress.
fn decompress(data: &[u8], header: &Header) -> Result<Vec<u8>, DecodeError> {
    let Some(head) = data.get(..2) else {
        return Err(DecodeError::BadZlibHeader { header: [0, 0] });
    };
    let head = [head[0], head[1]];
    // The low nibble of the first byte is the compression method, which
    // PNG fixes at 8; bit 5 of the second is a preset dictionary, which
    // PNG forbids; and the pair read big-endian must divide by 31.
    let checked = (u32::from(head[0]) << 8 | u32::from(head[1])) % 31 == 0;
    if head[0] & 0x0f != 8 || head[1] & 0x20 != 0 || !checked {
        return Err(DecodeError::BadZlibHeader { header: head });
    }
    // The trailing Adler-32 is not read: the chunk CRCs already covered
    // these bytes on the way in, and a second checksum over the same
    // bytes says nothing new.
    let stream = data.get(2..).unwrap_or(&[]);
    let want = (header.row() + 1) * header.height as usize;
    inflate(stream, want.min(CEILING)).map_err(|cause| DecodeError::Deflate { cause })
}

/// Undo the per-row filters, leaving raw samples.
///
/// **In place, row by row, and each row needs the one above it** — which
/// is why this consumes the filtered buffer rather than borrowing it: the
/// reconstructed bytes of row `n - 1` are what row `n` is reconstructed
/// against, so the two cannot be separate buffers without copying every
/// row twice.
fn unfilter(mut raw: Vec<u8>, header: &Header) -> Result<Vec<u8>, DecodeError> {
    let row = header.row();
    let stride = header.stride();
    let height = header.height as usize;
    let expected = (row + 1) * height;
    if raw.len() != expected {
        return Err(DecodeError::BadImageLength {
            expected,
            found: raw.len(),
        });
    }

    let mut out = vec![0u8; row * height];
    for line in 0..height {
        let filter = raw[line * (row + 1)];
        let from = line * (row + 1) + 1;
        let source: Vec<u8> = raw[from..from + row].to_vec();
        for at in 0..row {
            let left = if at >= stride {
                out[line * row + at - stride]
            } else {
                0
            };
            let up = if line > 0 {
                out[(line - 1) * row + at]
            } else {
                0
            };
            let corner = if line > 0 && at >= stride {
                out[(line - 1) * row + at - stride]
            } else {
                0
            };
            let value = source[at];
            out[line * row + at] = match filter {
                0 => value,
                1 => value.wrapping_add(left),
                2 => value.wrapping_add(up),
                // The specification's `Average` is the floor of the two
                // neighbours' mean, computed without overflowing — which
                // is exactly `midpoint`.
                3 => value.wrapping_add(left.midpoint(up)),
                4 => value.wrapping_add(paeth(left, up, corner)),
                other => {
                    return Err(DecodeError::BadFilter {
                        filter: other,
                        row: line,
                    });
                }
            };
        }
    }
    raw.clear();
    Ok(out)
}

/// The Paeth predictor: whichever of the three neighbours is nearest to
/// their linear estimate.
const fn paeth(left: u8, up: u8, corner: u8) -> u8 {
    let estimate = left as i16 + up as i16 - corner as i16;
    let to_left = (estimate - left as i16).abs();
    let to_up = (estimate - up as i16).abs();
    let to_corner = (estimate - corner as i16).abs();
    if to_left <= to_up && to_left <= to_corner {
        left
    } else if to_up <= to_corner {
        up
    } else {
        corner
    }
}

/// Turn raw samples into 8-bit RGBA.
fn expand(
    samples: &[u8],
    header: &Header,
    palette: &[[u8; 3]],
    alpha: &[u8],
) -> Result<Image, DecodeError> {
    let count = header.width as usize * header.height as usize;
    let mut pixels = Vec::with_capacity(count * 4);
    let stride = header.stride();
    // Sixteen-bit samples are taken by their high byte. The low byte is
    // a precision this decoder's output cannot carry, and averaging the
    // pair would invent a value neither of them holds.
    let step = header.depth as usize / 8;
    for index in 0..count {
        let at = index * stride;
        let sample =
            |channel: usize| -> u8 { samples.get(at + channel * step).copied().unwrap_or(0) };
        let rgba = match header.colour {
            0 => {
                let grey = sample(0);
                [grey, grey, grey, grey_alpha(alpha, grey, header)]
            }
            2 => {
                let (r, g, b) = (sample(0), sample(1), sample(2));
                [r, g, b, colour_alpha(alpha, [r, g, b], header)]
            }
            3 => {
                let index = usize::from(sample(0));
                let entry = palette.get(index).copied().ok_or(DecodeError::BadPalette {
                    entries: palette.len(),
                    index,
                })?;
                let opacity = alpha.get(index).copied().unwrap_or(255);
                [entry[0], entry[1], entry[2], opacity]
            }
            4 => {
                let grey = sample(0);
                [grey, grey, grey, sample(1)]
            }
            _ => [sample(0), sample(1), sample(2), sample(3)],
        };
        pixels.extend_from_slice(&rgba);
    }
    Ok(Image {
        width: header.width,
        height: header.height,
        pixels,
    })
}

/// Which byte of a `tRNS` sample this decoder compares against.
///
/// **The specification writes every `tRNS` value as sixteen bits**, so an
/// 8-bit image's transparent shade sits in the *low* byte of the pair and
/// a 16-bit image's meaningful half is the high one — which is the half
/// this decoder keeps. Reading index zero for both is the obvious mistake
/// and it makes transparency work on exactly the images that do not need
/// it.
const fn trns_byte(header: &Header) -> usize {
    if header.depth == 16 { 0 } else { 1 }
}

/// `tRNS` for a greyscale image names one fully transparent shade.
fn grey_alpha(alpha: &[u8], grey: u8, header: &Header) -> u8 {
    match alpha.get(trns_byte(header)) {
        Some(clear) if *clear == grey => 0,
        _ => 255,
    }
}

/// `tRNS` for a truecolour image names one fully transparent colour.
fn colour_alpha(alpha: &[u8], rgb: [u8; 3], header: &Header) -> u8 {
    let pick = trns_byte(header);
    let named = [
        alpha.get(pick).copied(),
        alpha.get(2 + pick).copied(),
        alpha.get(4 + pick).copied(),
    ];
    match named {
        [Some(r), Some(g), Some(b)] if [r, g, b] == rgb => 0,
        _ => 255,
    }
}

#[cfg(test)]
mod tests {
    use super::{DecodeError, decode};

    /// A picture with runs, gradients and sharp edges in it, so the
    /// stream exercises literals and back-references rather than one row
    /// of one colour.
    fn picture(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let banded = u8::try_from((x / 3 + y / 5) * 37 % 256).unwrap_or(0);
                let edge = if (x + y) % 11 == 0 { 255 } else { 0 };
                pixels.extend_from_slice(&[banded, edge, u8::try_from(y % 256).unwrap_or(0), 255]);
            }
        }
        pixels
    }

    /// **What this crate writes, it reads.**
    ///
    /// The assertion that binds the two halves together: a format one
    /// half of a crate cannot read back is a format the two halves
    /// disagree about, and that disagreement is invisible until somebody
    /// hands a file to the other one.
    ///
    /// Probed by taking every filtered row's bytes one to the left in
    /// `unfilter`: the round trip fails on the first row and names it.
    #[test]
    fn what_this_crate_writes_it_reads() {
        for (width, height) in [(1, 1), (16, 9), (37, 5), (5, 37), (64, 64)] {
            let pixels = picture(width, height);
            let file = crate::encode(width, height, &pixels).expect("the encoder accepts this");
            let image = decode(&file)
                .unwrap_or_else(|error| panic!("{width}x{height} did not decode: {error:?}"));
            assert_eq!(image.width, width, "{width}x{height} changed width");
            assert_eq!(image.height, height, "{width}x{height} changed height");
            assert_eq!(
                image.pixels, pixels,
                "{width}x{height} came back with different pixels"
            );
        }
    }

    /// **Files written by something else.**
    ///
    /// The encoder here writes one fixed-Huffman block with no filtering,
    /// so a decoder tested only against this crate's own output is never
    /// asked about dynamic Huffman — which every other encoder writes.
    /// These are the repository's own brand images, made by a design
    /// tool, and they are in the tree already.
    ///
    /// **Embedded rather than read.** This crate never touches the
    /// filesystem — that is its contract and its clippy configuration
    /// enforces it — so the fixtures are compiled in. A hundred and fifty
    /// kilobytes of test binary is a cheap price for the only fixtures in
    /// the tree that no part of this crate produced.
    ///
    /// Probed by refusing dynamic blocks in `inflate`: both files fail
    /// and the message names the kind.
    #[test]
    fn files_written_by_another_tool_read() {
        let files: [(&str, &[u8]); 2] = [
            (
                "renew-icon-512.png",
                include_bytes!("../../../assets/brand/renew-icon-512.png"),
            ),
            (
                "renew-banner.png",
                include_bytes!("../../../assets/brand/renew-banner.png"),
            ),
        ];
        for (name, bytes) in files {
            let image =
                decode(bytes).unwrap_or_else(|error| panic!("{name} did not decode: {error:?}"));
            assert!(
                image.width > 0 && image.height > 0,
                "{name} decoded to nothing"
            );
            assert_eq!(
                image.pixels.len(),
                image.width as usize * image.height as usize * 4,
                "{name} decoded to the wrong number of bytes"
            );
            // A brand image is not one flat colour, and a decoder that
            // produced a solid field would pass every assertion above it.
            let first = image.pixels.get(..4).unwrap_or(&[]);
            assert!(
                image
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel != first),
                "{name} decoded to a single flat colour"
            );
        }
    }

    /// Something that is not a PNG is said to be not a PNG, which is
    /// worth distinguishing from a PNG that is damaged.
    #[test]
    fn a_file_that_is_not_a_png_is_named_as_such() {
        assert_eq!(decode(b"not a png at all"), Err(DecodeError::NotAPng));
        assert_eq!(decode(&[]), Err(DecodeError::NotAPng));
    }

    /// **A damaged chunk is refused rather than read.** Every chunk
    /// carries a CRC and this checks it, so a file that changed in
    /// transit is an error with a name instead of an image with a stripe
    /// through it.
    #[test]
    fn a_chunk_whose_checksum_fails_is_refused() {
        let mut file = crate::encode(8, 8, &picture(8, 8)).expect("the encoder accepts this");
        // The last byte of `IHDR`'s body: the interlace flag, which is
        // covered by the checksum four bytes later.
        let at = 8 + 8 + 12;
        file[at] ^= 0x01;
        assert!(
            matches!(decode(&file), Err(DecodeError::BadChecksum { .. })),
            "a chunk with a broken checksum was read anyway"
        );
    }

    /// A truncated file is refused by name rather than decoded to
    /// whatever arrived — a half-downloaded sprite must not look like a
    /// short one.
    #[test]
    fn a_truncated_file_is_refused() {
        let file = crate::encode(16, 16, &picture(16, 16)).expect("the encoder accepts this");
        for keep in [12, 30, file.len() / 2, file.len() - 5] {
            assert!(
                decode(&file[..keep]).is_err(),
                "{keep} bytes of a file decoded without complaint"
            );
        }
    }

    /// The header's own fields are checked against what the format
    /// allows, so a file claiming an impossible shape is an error rather
    /// than an allocation.
    #[test]
    fn an_impossible_header_is_refused() {
        let mut file = crate::encode(8, 8, &picture(8, 8)).expect("the encoder accepts this");
        // Width to zero, and the checksum rewritten so the refusal comes
        // from the field rather than from the CRC that would also catch it.
        let body_at = 8 + 8;
        file[body_at..body_at + 4].copy_from_slice(&0u32.to_be_bytes());
        let body = file[body_at..body_at + 13].to_vec();
        let fixed = crate::checksum(*b"IHDR", &body);
        file[body_at + 13..body_at + 17].copy_from_slice(&fixed.to_be_bytes());
        assert_eq!(
            decode(&file),
            Err(DecodeError::ZeroExtent {
                width: 0,
                height: 8
            }),
            "a zero-width image was accepted"
        );
    }
}

#[cfg(test)]
mod filters {
    use super::decode;

    /// The five scanline filters, applied **forwards**.
    ///
    /// **Written here rather than reused from the decoder**, which is the
    /// whole point: the decoder implements the inverse, so a test that
    /// called the decoder's own arithmetic to build its fixture would
    /// agree with any mistake in it. These are transcribed from the
    /// specification, and the two meet in the middle.
    fn apply(filter: u8, row: &[u8], above: &[u8], bpp: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(row.len());
        for (at, raw) in row.iter().enumerate() {
            let left = if at >= bpp { row[at - bpp] } else { 0 };
            let up = above.get(at).copied().unwrap_or(0);
            let corner = if at >= bpp {
                above.get(at - bpp).copied().unwrap_or(0)
            } else {
                0
            };
            let predicted = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => left.midpoint(up),
                _ => {
                    let estimate = i16::from(left) + i16::from(up) - i16::from(corner);
                    let (dl, du, dc) = (
                        (estimate - i16::from(left)).abs(),
                        (estimate - i16::from(up)).abs(),
                        (estimate - i16::from(corner)).abs(),
                    );
                    if dl <= du && dl <= dc {
                        left
                    } else if du <= dc {
                        up
                    } else {
                        corner
                    }
                }
            };
            out.push(raw.wrapping_sub(predicted));
        }
        out
    }

    /// Wrap bytes as a PNG chunk: length, type, body, checksum.
    fn chunk(id: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(0).to_be_bytes());
        out.extend_from_slice(&id);
        out.extend_from_slice(body);
        out.extend_from_slice(&crate::checksum(id, body).to_be_bytes());
        out
    }

    /// A whole PNG, hand-built, whose rows each use a different filter.
    ///
    /// The image data goes in a **stored** deflate block, so the fixture
    /// needs no compressor of its own and the thing under test is the
    /// filtering rather than the Huffman coding — which the other tests
    /// already cover from both directions.
    fn built(width: u32, pixels: &[u8], filters: &[u8]) -> Vec<u8> {
        const BPP: usize = 4;
        let stride = width as usize * BPP;
        let height = u32::try_from(filters.len()).unwrap_or(0);

        let mut raw = Vec::new();
        let mut above = vec![0u8; stride];
        for (line, filter) in filters.iter().enumerate() {
            let row = &pixels[line * stride..(line + 1) * stride];
            raw.push(*filter);
            raw.extend_from_slice(&apply(*filter, row, &above, BPP));
            above = row.to_vec();
        }

        let mut zlib = vec![0x78, 0x01, 0b0000_0001];
        let length = u16::try_from(raw.len()).expect("a small fixture");
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(&raw);
        zlib.extend_from_slice(&crate::adler32(&raw).to_be_bytes());

        let mut header = Vec::new();
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);

        let mut file = crate::SIGNATURE.to_vec();
        file.extend_from_slice(&chunk(*b"IHDR", &header));
        file.extend_from_slice(&chunk(*b"IDAT", &zlib));
        file.extend_from_slice(&chunk(*b"IEND", &[]));
        file
    }

    /// **The Paeth predictor's third answer.**
    ///
    /// Paeth takes whichever of the three neighbours is nearest their
    /// linear estimate, and the up-left corner winning is the rarest of
    /// the three — the generated fixture above never produced it, so that
    /// branch was unexercised while every filter looked covered. These
    /// values make it win outright: left 10, up 200, corner 100 gives an
    /// estimate of 110, which is a hundred from the left, ninety from the
    /// up, and ten from the corner.
    ///
    /// Probed by returning `up` where the corner is chosen: the second
    /// row comes back wrong.
    #[test]
    fn the_paeth_predictor_can_choose_the_corner() {
        let width = 2u32;
        let mut pixels = vec![100u8, 100, 100, 255, 200, 200, 200, 255];
        pixels.extend_from_slice(&[10, 10, 10, 255, 77, 88, 99, 255]);
        let file = built(width, &pixels, &[0, 4]);
        let image = decode(&file).expect("the fixture decodes");
        assert_eq!(
            image.pixels, pixels,
            "the corner-predicted row came back wrong"
        );
    }

    /// **Every scanline filter, undone.**
    ///
    /// The encoder in this crate writes filter zero and nothing else, and
    /// the repository's own brand images turned out not to use all five
    /// either — so breaking the Paeth predictor outright left both of the
    /// other decoder tests green. Four of the five filters were unverified
    /// and looked verified, which is the failure this fixture exists for.
    ///
    /// Probed by breaking each predictor in turn: Paeth to always take
    /// the left neighbour, the average to take their sum, `Sub` to add
    /// nothing, and `Up` to read the wrong row. Each names its row.
    #[test]
    fn every_scanline_filter_is_undone() {
        let width = 5u32;
        // Values chosen to make the predictors disagree: neighbours that
        // differ in both directions, and wrapping past 255 and below 0.
        let pixels: Vec<u8> = (0..width * 5 * 4)
            .map(|at| u8::try_from((at * 37 + at / 7 * 91) % 256).unwrap_or(0))
            .collect();
        for filters in [
            vec![0, 1, 2, 3, 4],
            vec![4, 3, 2, 1, 0],
            vec![4, 4, 4, 4, 4],
            vec![3, 3, 3, 3, 3],
            vec![1, 2, 1, 2, 1],
        ] {
            let file = built(width, &pixels, &filters);
            let image = decode(&file)
                .unwrap_or_else(|error| panic!("{filters:?} did not decode: {error:?}"));
            assert_eq!(image.width, width);
            assert_eq!(image.height, 5);
            for (row, ((got, want), filter)) in image
                .pixels
                .chunks(width as usize * 4)
                .zip(pixels.chunks(width as usize * 4))
                .zip(&filters)
                .enumerate()
            {
                assert_eq!(
                    got, want,
                    "row {row} under filter {filter} came back different"
                );
            }
        }
    }
}
