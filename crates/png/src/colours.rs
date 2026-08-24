//! Every colour type read, and every refusal refused.
//!
//! **A decoder tested only against what this crate writes is tested
//! against one file shape.** The encoder emits 8-bit RGBA, unfiltered,
//! fixed Huffman — so a round trip proves the truecolour-with-alpha path
//! and nothing else. Greyscale, truecolour, palette and greyscale-with-
//! alpha were all unverified when they were written, which is the same
//! hole the scanline filters were in and was found the same way: a probe
//! that broke one of them stayed green.
//!
//! The fixtures here are built by hand, in a **stored** deflate block, so
//! what is under test is the pixel layout rather than the Huffman coding
//! — which the round trip and the real-file test already cover from both
//! directions.

#[cfg(test)]
mod tests {
    use crate::decode::{DecodeError, Image, decode};

    /// Wrap bytes as a PNG chunk: length, type, body, checksum.
    fn chunk(id: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(0).to_be_bytes());
        out.extend_from_slice(&id);
        out.extend_from_slice(body);
        out.extend_from_slice(&crate::checksum(id, body).to_be_bytes());
        out
    }

    /// A zlib stream holding `raw` in one stored block.
    fn stored(raw: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01, 0b0000_0001];
        let length = u16::try_from(raw.len()).expect("a small fixture");
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(raw);
        out.extend_from_slice(&crate::adler32(raw).to_be_bytes());
        out
    }

    /// How a fixture is shaped.
    #[derive(Clone)]
    struct Shape {
        width: u32,
        height: u32,
        depth: u8,
        colour: u8,
        /// Samples, already in the file's own layout, without filter
        /// bytes — those are added here, all zero.
        samples: Vec<u8>,
        palette: Vec<u8>,
        alpha: Vec<u8>,
    }

    /// A whole PNG from a shape.
    fn file(shape: &Shape) -> Vec<u8> {
        let channels = match shape.colour {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            _ => 4,
        };
        let stride = shape.width as usize * channels * usize::from(shape.depth) / 8;
        let mut raw = Vec::new();
        for line in 0..shape.height as usize {
            raw.push(0);
            raw.extend_from_slice(&shape.samples[line * stride..(line + 1) * stride]);
        }

        let mut header = Vec::new();
        header.extend_from_slice(&shape.width.to_be_bytes());
        header.extend_from_slice(&shape.height.to_be_bytes());
        header.extend_from_slice(&[shape.depth, shape.colour, 0, 0, 0]);

        let mut out = crate::SIGNATURE.to_vec();
        out.extend_from_slice(&chunk(*b"IHDR", &header));
        if !shape.palette.is_empty() {
            out.extend_from_slice(&chunk(*b"PLTE", &shape.palette));
        }
        if !shape.alpha.is_empty() {
            out.extend_from_slice(&chunk(*b"tRNS", &shape.alpha));
        }
        out.extend_from_slice(&chunk(*b"IDAT", &stored(&raw)));
        out.extend_from_slice(&chunk(*b"IEND", &[]));
        out
    }

    fn read(shape: &Shape) -> Image {
        decode(&file(shape)).unwrap_or_else(|error| {
            panic!(
                "colour {} at depth {} did not decode: {error:?}",
                shape.colour, shape.depth
            )
        })
    }

    /// **Greyscale becomes grey, on every channel, and opaque.**
    ///
    /// Probed by copying only the red channel: green and blue come out
    /// zero and the pixels compare unequal.
    #[test]
    fn greyscale_expands_to_grey() {
        let shape = Shape {
            width: 3,
            height: 2,
            depth: 8,
            colour: 0,
            samples: vec![0, 40, 255, 90, 130, 200],
            palette: Vec::new(),
            alpha: Vec::new(),
        };
        let image = read(&shape);
        let want: Vec<u8> = shape
            .samples
            .iter()
            .flat_map(|grey| [*grey, *grey, *grey, 255])
            .collect();
        assert_eq!(image.pixels, want, "greyscale did not expand evenly");
    }

    /// A greyscale image's `tRNS` names one shade as fully clear — and
    /// **the value sits in the low byte of a sixteen-bit pair**, which is
    /// the mistake that makes transparency work only on the images that
    /// do not need it.
    ///
    /// Probed by reading index zero instead: nothing comes out clear.
    #[test]
    fn a_greyscale_transparent_shade_is_read_from_the_low_byte() {
        let shape = Shape {
            width: 3,
            height: 1,
            depth: 8,
            colour: 0,
            samples: vec![10, 20, 30],
            palette: Vec::new(),
            // Sixteen bits, big-endian: the value 20.
            alpha: vec![0, 20],
        };
        let image = read(&shape);
        let alphas: Vec<u8> = image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3])
            .collect();
        assert_eq!(
            alphas,
            vec![255, 0, 255],
            "the named shade is not the one that came out clear"
        );
    }

    /// Truecolour keeps its channels and comes out opaque.
    #[test]
    fn truecolour_keeps_its_channels() {
        let shape = Shape {
            width: 2,
            height: 1,
            depth: 8,
            colour: 2,
            samples: vec![10, 20, 30, 200, 100, 50],
            palette: Vec::new(),
            alpha: Vec::new(),
        };
        assert_eq!(
            read(&shape).pixels,
            vec![10, 20, 30, 255, 200, 100, 50, 255],
            "truecolour changed its channels"
        );
    }

    /// A truecolour `tRNS` names one colour as clear, read from the low
    /// byte of each pair for the same reason greyscale's is.
    #[test]
    fn a_truecolour_transparent_colour_is_named_exactly() {
        let shape = Shape {
            width: 2,
            height: 1,
            depth: 8,
            colour: 2,
            samples: vec![10, 20, 30, 200, 100, 50],
            palette: Vec::new(),
            alpha: vec![0, 200, 0, 100, 0, 50],
        };
        let image = read(&shape);
        assert_eq!(
            image.pixels[3], 255,
            "a colour that was not named came out clear"
        );
        assert_eq!(
            image.pixels[7], 0,
            "the named colour did not come out clear"
        );
    }

    /// **A palette image is its palette**, and its `tRNS` is one alpha per
    /// entry rather than a value to compare against.
    ///
    /// Probed by indexing the palette with the pixel's position instead of
    /// its value: every pixel comes out the first entry.
    #[test]
    fn a_palette_image_reads_its_entries() {
        let shape = Shape {
            width: 4,
            height: 1,
            depth: 8,
            colour: 3,
            samples: vec![2, 0, 1, 2],
            palette: vec![255, 0, 0, 0, 255, 0, 0, 0, 255],
            alpha: vec![255, 128],
        };
        assert_eq!(
            read(&shape).pixels,
            vec![
                0, 0, 255, 255, // entry 2, no tRNS entry, so opaque
                255, 0, 0, 255, // entry 0, alpha 255
                0, 255, 0, 128, // entry 1, alpha 128
                0, 0, 255, 255,
            ],
            "the palette was not read entry by entry"
        );
    }

    /// An index past the palette's end is a refusal, not a wrap: a file
    /// that names an entry it did not supply is corrupt, and reading
    /// entry zero instead would hand back a picture that looks plausible.
    #[test]
    fn a_palette_index_past_the_end_is_refused() {
        let shape = Shape {
            width: 2,
            height: 1,
            depth: 8,
            colour: 3,
            samples: vec![0, 7],
            palette: vec![255, 0, 0],
            alpha: Vec::new(),
        };
        assert_eq!(
            decode(&file(&shape)),
            Err(DecodeError::BadPalette {
                entries: 1,
                index: 7
            })
        );
    }

    /// Greyscale with alpha carries its own opacity per pixel.
    #[test]
    fn greyscale_with_alpha_keeps_both() {
        let shape = Shape {
            width: 2,
            height: 1,
            depth: 8,
            colour: 4,
            samples: vec![90, 10, 200, 255],
            palette: Vec::new(),
            alpha: Vec::new(),
        };
        assert_eq!(
            read(&shape).pixels,
            vec![90, 90, 90, 10, 200, 200, 200, 255],
            "greyscale with alpha lost one of its halves"
        );
    }

    /// **Sixteen-bit samples are taken by their high byte.** The low byte
    /// is a precision this decoder's output cannot carry, and averaging
    /// the pair would invent a value neither of them holds.
    ///
    /// Probed by taking the low byte instead: every channel comes out the
    /// second of its pair.
    #[test]
    fn sixteen_bit_samples_keep_their_high_byte() {
        let shape = Shape {
            width: 2,
            height: 1,
            depth: 16,
            colour: 2,
            // Big-endian pairs: (0x1234, 0x5678, 0x9abc) then all 0xff00.
            samples: vec![
                0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xff, 0x00, 0xff, 0x00, 0xff, 0x00,
            ],
            palette: Vec::new(),
            alpha: Vec::new(),
        };
        assert_eq!(
            read(&shape).pixels,
            vec![0x12, 0x56, 0x9a, 255, 0xff, 0xff, 0xff, 255],
            "a sixteen-bit sample did not come back as its high byte"
        );
    }

    /// Every header field the format constrains is checked, and each is
    /// refused by its own name rather than by a shared "bad file".
    #[test]
    fn every_impossible_header_field_is_named() {
        let good = Shape {
            width: 2,
            height: 1,
            depth: 8,
            colour: 6,
            samples: vec![1, 2, 3, 4, 5, 6, 7, 8],
            palette: Vec::new(),
            alpha: Vec::new(),
        };
        // The header body begins after the signature and the chunk's
        // length and type; its checksum follows the thirteen bytes.
        let at = crate::SIGNATURE.len() + 8;
        let bend = |byte: usize, value: u8| -> Vec<u8> {
            let mut out = file(&good);
            out[at + byte] = value;
            let body = out[at..at + 13].to_vec();
            let fixed = crate::checksum(*b"IHDR", &body);
            out[at + 13..at + 17].copy_from_slice(&fixed.to_be_bytes());
            out
        };

        assert_eq!(
            decode(&bend(9, 9)),
            Err(DecodeError::BadColourType { colour: 9 }),
            "a colour type the format does not define was accepted"
        );
        assert_eq!(
            decode(&bend(8, 4)),
            Err(DecodeError::UnsupportedDepth {
                depth: 4,
                colour: 6
            }),
            "a sub-byte depth was accepted"
        );
        assert_eq!(
            decode(&bend(12, 1)),
            Err(DecodeError::Interlaced),
            "an interlaced image was accepted"
        );
        assert_eq!(
            decode(&bend(10, 1)),
            Err(DecodeError::BadMethod {
                compression: 1,
                filter: 0
            }),
            "a compression method the format does not define was accepted"
        );
        assert_eq!(
            decode(&bend(11, 1)),
            Err(DecodeError::BadMethod {
                compression: 0,
                filter: 1
            }),
            "a filter method the format does not define was accepted"
        );
    }

    /// **A chunk this decoder has no use for is stepped over**, not
    /// refused: the format is built on that, and a file carrying a text
    /// note or a gamma value is an ordinary file rather than a damaged
    /// one.
    #[test]
    fn a_chunk_it_does_not_know_is_stepped_over() {
        let shape = Shape {
            width: 2,
            height: 1,
            depth: 8,
            colour: 6,
            samples: vec![1, 2, 3, 4, 5, 6, 7, 8],
            palette: Vec::new(),
            alpha: Vec::new(),
        };
        let plain = file(&shape);
        // A `tEXt` chunk between the header and the data.
        let at = crate::SIGNATURE.len() + 25;
        let mut out = plain[..at].to_vec();
        out.extend_from_slice(&chunk(*b"tEXt", b"Comment\0made up"));
        out.extend_from_slice(&plain[at..]);
        assert_eq!(
            decode(&out),
            decode(&plain),
            "an unknown chunk changed what the file decoded to"
        );
    }

    /// A header chunk shorter than the thirteen bytes the format fixes is
    /// refused rather than read out of whatever follows it.
    #[test]
    fn a_header_shorter_than_the_format_allows_is_refused() {
        let mut out = crate::SIGNATURE.to_vec();
        out.extend_from_slice(&chunk(*b"IHDR", &[0, 0, 0, 2, 0, 0, 0, 1]));
        out.extend_from_slice(&chunk(*b"IEND", &[]));
        assert_eq!(decode(&out), Err(DecodeError::BadHeader));
    }

    /// Image data too short to hold even a zlib header is refused there
    /// rather than at the first byte of a stream that does not exist.
    #[test]
    fn image_data_too_short_for_a_zlib_header_is_refused() {
        let mut header = Vec::new();
        header.extend_from_slice(&2u32.to_be_bytes());
        header.extend_from_slice(&1u32.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut out = crate::SIGNATURE.to_vec();
        out.extend_from_slice(&chunk(*b"IHDR", &header));
        out.extend_from_slice(&chunk(*b"IDAT", &[0x78]));
        out.extend_from_slice(&chunk(*b"IEND", &[]));
        assert_eq!(
            decode(&out),
            Err(DecodeError::BadZlibHeader { header: [0, 0] })
        );
    }

    /// A file with no image data in it is refused rather than decoded to
    /// an empty picture.
    #[test]
    fn a_file_with_no_image_data_is_refused() {
        let mut header = Vec::new();
        header.extend_from_slice(&2u32.to_be_bytes());
        header.extend_from_slice(&1u32.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut out = crate::SIGNATURE.to_vec();
        out.extend_from_slice(&chunk(*b"IHDR", &header));
        out.extend_from_slice(&chunk(*b"IEND", &[]));
        assert_eq!(decode(&out), Err(DecodeError::NoImageData));
    }

    /// A file with no header is refused, however well-formed the rest of
    /// it is.
    #[test]
    fn a_file_with_no_header_is_refused() {
        let mut out = crate::SIGNATURE.to_vec();
        out.extend_from_slice(&chunk(*b"IDAT", &stored(&[0, 1, 2, 3])));
        out.extend_from_slice(&chunk(*b"IEND", &[]));
        assert_eq!(decode(&out), Err(DecodeError::BadHeader));
    }

    /// The zlib wrapper's own checks are made rather than skipped over:
    /// a method that is not deflate, a preset dictionary PNG forbids, and
    /// the header pair's divisibility.
    #[test]
    fn a_malformed_zlib_header_is_refused() {
        let good = Shape {
            width: 2,
            height: 1,
            depth: 8,
            colour: 6,
            samples: vec![1, 2, 3, 4, 5, 6, 7, 8],
            palette: Vec::new(),
            alpha: Vec::new(),
        };
        // The IDAT body starts after the signature, IHDR (25 bytes) and
        // its own length and type.
        let at = crate::SIGNATURE.len() + 25 + 8;
        let bend = |byte: usize, value: u8| -> Vec<u8> {
            let mut out = file(&good);
            out[at + byte] = value;
            // The chunk's own checksum is not fixed up: this asks about
            // the zlib header, so the CRC must not be what refuses it.
            let length =
                u32::from_be_bytes([out[at - 8], out[at - 7], out[at - 6], out[at - 5]]) as usize;
            let body = out[at..at + length].to_vec();
            let fixed = crate::checksum(*b"IDAT", &body);
            out[at + length..at + length + 4].copy_from_slice(&fixed.to_be_bytes());
            out
        };
        for (byte, value) in [(0, 0x79u8), (1, 0x21), (1, 0x02)] {
            let mut header = [0x78, 0x01];
            header[byte] = value;
            assert_eq!(
                decode(&bend(byte, value)),
                Err(DecodeError::BadZlibHeader { header }),
                "byte {byte} set to {value:#04x} was accepted as a zlib header"
            );
        }
    }

    /// A stream that decompresses to the wrong number of bytes is refused
    /// rather than padded or cropped — a row short is a picture with a
    /// stripe of nothing at the bottom.
    #[test]
    fn image_data_of_the_wrong_length_is_refused() {
        let mut shape = Shape {
            width: 2,
            height: 2,
            depth: 8,
            colour: 6,
            samples: vec![0; 2 * 2 * 4],
            palette: Vec::new(),
            alpha: Vec::new(),
        };
        // Claim three rows and supply two.
        let short = file(&shape);
        shape.height = 3;
        let mut out = short;
        let at = crate::SIGNATURE.len() + 8;
        out[at + 4..at + 8].copy_from_slice(&3u32.to_be_bytes());
        let body = out[at..at + 13].to_vec();
        let fixed = crate::checksum(*b"IHDR", &body);
        out[at + 13..at + 17].copy_from_slice(&fixed.to_be_bytes());
        // Named exactly: two rows of nine bytes supplied against three
        // required. A looser pattern would pass on a decoder that had
        // refused for some other reason entirely.
        assert_eq!(
            decode(&out),
            Err(DecodeError::BadImageLength {
                expected: 27,
                found: 18
            }),
            "a stream two rows long was accepted for a three-row image"
        );
    }

    /// A scanline filter the format does not define is refused, and the
    /// error names the row — because a file with one bad row is a file
    /// somebody will want to find the row in.
    #[test]
    fn a_filter_the_format_does_not_define_is_refused() {
        let shape = Shape {
            width: 2,
            height: 2,
            depth: 8,
            colour: 6,
            samples: vec![7; 2 * 2 * 4],
            palette: Vec::new(),
            alpha: Vec::new(),
        };
        let mut out = file(&shape);
        // The second row's filter byte, inside the stored block: past the
        // signature, IHDR, the IDAT header, the zlib header, the stored
        // block header, one filter byte and one row.
        let stride = 2 * 4;
        let at = crate::SIGNATURE.len() + 25 + 8 + 2 + 5 + (1 + stride);
        out[at] = 9;
        let length = u32::from_be_bytes([
            out[crate::SIGNATURE.len() + 25],
            out[crate::SIGNATURE.len() + 26],
            out[crate::SIGNATURE.len() + 27],
            out[crate::SIGNATURE.len() + 28],
        ]) as usize;
        let body_at = crate::SIGNATURE.len() + 25 + 8;
        let body = out[body_at..body_at + length].to_vec();
        let fixed = crate::checksum(*b"IDAT", &body);
        out[body_at + length..body_at + length + 4].copy_from_slice(&fixed.to_be_bytes());
        assert_eq!(
            decode(&out),
            Err(DecodeError::BadFilter { filter: 9, row: 1 }),
            "a filter byte of nine was accepted"
        );
    }
}
