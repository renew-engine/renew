//! The decoder, over inputs nobody chose.
//!
//! The unit suite asserts what happens to files somebody sat down and
//! wrote: this one asserts what must hold for *every* file in a shape.
//! Four properties. Each was probed by a named mutant before it was
//! committed, and the one place a probe came back green is written into
//! that property's own documentation rather than left for a reader to
//! discover: the round trip cannot reach filters one to four, because
//! this crate's encoder never writes them.
//!
//! **This is not the fuzz target and does not replace it.** The fuzzer
//! explores; these are closed statements about a generated population,
//! and they run on the stable toolchain as part of the ordinary test
//! suite. The fuzz workspace needs nightly and its own schedule, so
//! between merges this is what actually exercises the decoder on bytes
//! nobody wrote.

// A property body is not literally a `#[test]` fn -- the macro wraps it --
// so the lints that allow a test to panic do not see it. These are
// assertions about a decoder, and a failure here is a failed test.
#![allow(clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use renew_png::decode::decode;

/// A whole image: an extent and exactly the pixels it needs.
///
/// Capped at 24 by 24 because the cost here is real — every case encodes
/// a whole file and decodes it back, and the properties are about the
/// shape of the code rather than about scale. A 24-square image still
/// crosses several deflate blocks' worth of literals and back-references
/// on random data, which is where the interesting branches are.
fn image() -> impl Strategy<Value = (u32, u32, Vec<u8>)> {
    (1u32..=24, 1u32..=24).prop_flat_map(|(width, height)| {
        let count = width as usize * height as usize * 4;
        (
            Just(width),
            Just(height),
            prop::collection::vec(any::<u8>(), count),
        )
    })
}

/// A file this crate wrote, with the pixels that went into it.
fn file() -> impl Strategy<Value = (Vec<u8>, Vec<u8>)> {
    image().prop_map(|(width, height, pixels)| {
        let bytes =
            renew_png::encode(width, height, &pixels).expect("the encoder accepts a whole image");
        (bytes, pixels)
    })
}

proptest! {
    /// **What the encoder writes, the decoder returns — exactly.**
    ///
    /// The unit suite makes this claim about five hand-picked extents and
    /// one generated picture. It is a claim about every extent and every
    /// pixel, and random pixels are the hard case: a picture with runs
    /// and gradients compresses into back-references, while noise
    /// compresses into literals, and the two take different paths through
    /// the encoder's Huffman coding and the decoder's inflate.
    ///
    /// **It reaches filter zero only, and that is a property of the
    /// encoder rather than a choice made here.** This crate writes every
    /// row unfiltered, so no round trip through it can exercise the Sub,
    /// Up, Average or Paeth reconstruction — measured, not assumed:
    /// undoing Sub with the wrong neighbour leaves this property green
    /// and is caught instead by the decoder's own filter tests, which
    /// build their fixtures by hand, and by the recorded corpus, which
    /// carries a file another tool wrote.
    ///
    /// Probed by undoing filter zero as `value + 1`: the pixels come back
    /// changed and the property fails on the first case.
    #[test]
    fn what_the_encoder_writes_the_decoder_returns_exactly((width, height, pixels) in image()) {
        let bytes = renew_png::encode(width, height, &pixels)
            .expect("the encoder accepts a whole image");
        let image = decode(&bytes).expect("this crate reads what it wrote");

        prop_assert_eq!(image.width, width, "the width changed in the round trip");
        prop_assert_eq!(image.height, height, "the height changed in the round trip");
        prop_assert_eq!(image.pixels, pixels, "the pixels changed in the round trip");
    }

    /// **Every byte string gets an answer**, and an answer that says `Ok`
    /// describes an image that exists.
    ///
    /// A decoder is allowed to refuse anything; it is not allowed to
    /// panic, to read past the end, or to hand back an image whose
    /// buffer disagrees with the extent it reports. Random bytes almost
    /// never reach past the signature, so this is deliberately fed a
    /// mixture: raw noise, and noise behind a real signature, which is
    /// what gets the chunk walk itself under the property.
    ///
    /// Probed by deleting the chunk-overrun guard: a length field of
    /// noise then asks for a slice that is not there, and the property
    /// fails as a panic — `range end index 1118395930 out of range for
    /// slice of length 246` — which is the exact shape it exists to
    /// refuse.
    #[test]
    fn every_byte_string_gets_an_answer(
        noise in prop::collection::vec(any::<u8>(), 0..2048),
        signed in any::<bool>(),
    ) {
        let mut bytes = Vec::new();
        if signed {
            bytes.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        }
        bytes.extend_from_slice(&noise);

        if let Ok(image) = decode(&bytes) {
            prop_assert!(image.width > 0 && image.height > 0, "a zero extent decoded");
            prop_assert_eq!(
                image.pixels.len(),
                image.width as usize * image.height as usize * 4,
                "the buffer disagrees with the extent it was decoded at"
            );
        }
    }

    /// **No proper prefix of a file is a file.**
    ///
    /// A half-arrived download must not look like a short image. The
    /// terminator is the last chunk, so every prefix either cuts it or
    /// cuts something before it, and there is no length at which the
    /// decoder should be satisfied except the whole.
    ///
    /// Probed by deleting the missing-terminator refusal: a 55-byte
    /// prefix of a 67-byte file decoded without complaint, because every
    /// chunk it still held was whole.
    #[test]
    fn no_proper_prefix_of_a_file_decodes((bytes, _pixels) in file(), cut in any::<prop::sample::Index>()) {
        let keep = cut.index(bytes.len());
        prop_assert!(
            decode(&bytes[..keep]).is_err(),
            "{keep} bytes of a {}-byte file decoded without complaint",
            bytes.len()
        );
    }

    /// **One flipped bit anywhere in a file is refused.**
    ///
    /// This is the property that makes the chunk checksums load-bearing
    /// rather than decorative. Every byte after the signature lies in
    /// some chunk's length, type, payload or checksum: a flip in the
    /// type, payload or checksum breaks the CRC, and a flip in a length
    /// re-frames the chunk so the CRC breaks anyway or the chunk runs off
    /// the end. A flip inside the signature is not a PNG at all. There is
    /// nowhere left for a single bit to hide.
    ///
    /// Probed by deleting the checksum comparison in `decode`: the
    /// property fails at once, reporting that bit 1 of byte 71 was
    /// flipped and the file decoded anyway.
    #[test]
    fn one_flipped_bit_is_refused(
        (bytes, _pixels) in file(),
        at in any::<prop::sample::Index>(),
        bit in 0u32..8,
    ) {
        let mut damaged = bytes.clone();
        let index = at.index(damaged.len());
        damaged[index] ^= 1u8 << bit;

        prop_assert!(
            decode(&damaged).is_err(),
            "bit {bit} of byte {index} flipped and the file still decoded"
        );
    }
}
