//! The format, asserted by hand, and by somebody else's encoder.
//!
//! The property suite beside this file proves the reader and the test
//! writer agree with each other. That claim is weak on its own — a writer
//! and a reader making the *same* mistake are still inverses — so this
//! file anchors the format twice, in two ways that cannot both drift the
//! same direction.
//!
//! First, a canonical file written out byte by byte, every field asserted
//! at an offset a person counted. Nothing here goes through the writer, so
//! a shared mistake has nowhere to hide.
//!
//! Second, and more important for this format than for one the engine
//! invented: files a **different implementation** encoded. WAV is defined
//! outside this repository, so first-party artefacts alone can be
//! collectively wrong about it — every byte in them chosen by the same
//! reading of the same specification. The two fixtures under
//! `tests/fixtures/` were encoded by the `wave` module of the `CPython` 3.9
//! standard library on 2026-08-04, from sample values chosen to pin both
//! ends of the 16-bit range. Their provenance is the point: nobody here
//! decided what byte goes where in them.

use renew_audio::wav::{self, Wav};

/// One canonical stereo file, 56 bytes, written out field by field.
///
/// Three frames at 44.1 kHz. The sample values are the ones worth pinning:
/// zero, either side of it by one, and both ends of the range.
const CANONICAL: [u8; 56] = [
    // RIFF container: id, the size of everything after this field, form.
    0x52, 0x49, 0x46, 0x46, // "RIFF"
    0x30, 0x00, 0x00, 0x00, // 48 = 56 - 8
    0x57, 0x41, 0x56, 0x45, // "WAVE"
    // The format chunk: a 16-byte PCM body.
    0x66, 0x6d, 0x74, 0x20, // "fmt "
    0x10, 0x00, 0x00, 0x00, // 16
    0x01, 0x00, // format 1: PCM
    0x02, 0x00, // 2 channels
    0x44, 0xac, 0x00, 0x00, // 44100 frames per second
    0x10, 0xb1, 0x02, 0x00, // 176400 bytes per second
    0x04, 0x00, // 4 bytes per frame
    0x10, 0x00, // 16 bits per sample
    // The samples: three stereo frames, left then right.
    0x64, 0x61, 0x74, 0x61, // "data"
    0x0c, 0x00, 0x00, 0x00, // 12
    0x00, 0x00, // 0
    0x01, 0x00, // 1
    0xff, 0xff, // -1
    0xff, 0x7f, // 32767
    0x00, 0x80, // -32768
    0x34, 0x12, // 4660
];

/// Mono at the lowest accepted rate, encoded by `CPython` 3.9's `wave`.
const THIRD_PARTY_MONO: &[u8] = include_bytes!("fixtures/mono_8000_stdlib.wav");

/// Stereo at 44.1 kHz, encoded by `CPython` 3.9's `wave`.
const THIRD_PARTY_STEREO: &[u8] = include_bytes!("fixtures/stereo_44100_stdlib.wav");

/// Assert that `samples` decodes to exactly `expected`, scaled.
///
/// Bit patterns rather than a tolerance: dividing a 16-bit integer by 2^15
/// only changes an exponent, so every value is exact, and an approximate
/// comparison would pass for the off-by-one these tests exist to catch.
fn assert_decodes_to(wav: &Wav<'_>, expected: &[i16]) {
    let decoded: Vec<f32> = wav.samples_f32().collect();
    assert_eq!(decoded.len(), expected.len(), "sample count");
    for (index, (got, raw)) in decoded.iter().zip(expected.iter()).enumerate() {
        let want = f32::from(*raw) / 32_768.0;
        assert_eq!(
            got.to_bits(),
            want.to_bits(),
            "sample {index}: {got} is not {want}"
        );
    }
}

/// The bytes are exactly what the format says, at offsets counted by hand.
#[test]
fn the_canonical_file_is_laid_out_where_the_format_says() {
    // RIFF header: 12 bytes, and the declared size counts everything after
    // its own four bytes.
    assert_eq!(CANONICAL.len(), 56);
    assert_eq!(&CANONICAL[0..4], b"RIFF");
    assert_eq!(&CANONICAL[4..8], &48u32.to_le_bytes(), "riff size");
    assert_eq!(48 + 8, CANONICAL.len(), "the declared size covers the file");
    assert_eq!(&CANONICAL[8..12], b"WAVE");

    // The format chunk: an 8-byte header then a 16-byte body.
    assert_eq!(&CANONICAL[12..16], b"fmt ");
    assert_eq!(&CANONICAL[16..20], &16u32.to_le_bytes(), "fmt body size");
    assert_eq!(&CANONICAL[20..22], &1u16.to_le_bytes(), "format tag");
    assert_eq!(&CANONICAL[22..24], &2u16.to_le_bytes(), "channels");
    assert_eq!(&CANONICAL[24..28], &44_100u32.to_le_bytes(), "sample rate");
    assert_eq!(&CANONICAL[28..32], &176_400u32.to_le_bytes(), "byte rate");
    assert_eq!(&CANONICAL[32..34], &4u16.to_le_bytes(), "block align");
    assert_eq!(&CANONICAL[34..36], &16u16.to_le_bytes(), "bit depth");

    // The two redundant fields, derived here the way the reader derives
    // them: bytes per frame is channels times sample width, and bytes per
    // second is that times the frame rate.
    // Read from the file rather than restated: comparing two
    // constants to each other proves arithmetic, not bytes.
    assert_eq!(
        u16::from_le_bytes([CANONICAL[32], CANONICAL[33]]),
        2 * 2,
        "block align is channels times sample bytes"
    );
    assert_eq!(
        u32::from_le_bytes([CANONICAL[28], CANONICAL[29], CANONICAL[30], CANONICAL[31]]),
        44_100 * 4,
        "byte rate is rate times block"
    );

    // The data chunk: an 8-byte header then three 4-byte frames.
    assert_eq!(&CANONICAL[36..40], b"data");
    assert_eq!(&CANONICAL[40..44], &12u32.to_le_bytes(), "data body size");
    assert_eq!(&CANONICAL[44..46], &0i16.to_le_bytes(), "frame 0 left");
    assert_eq!(&CANONICAL[46..48], &1i16.to_le_bytes(), "frame 0 right");
    assert_eq!(&CANONICAL[48..50], &(-1i16).to_le_bytes(), "frame 1 left");
    assert_eq!(
        &CANONICAL[50..52],
        &32_767i16.to_le_bytes(),
        "frame 1 right"
    );
    assert_eq!(
        &CANONICAL[52..54],
        &(-32_768i16).to_le_bytes(),
        "frame 2 left"
    );
    assert_eq!(&CANONICAL[54..56], &4_660i16.to_le_bytes(), "frame 2 right");
}

/// And the reader reads exactly those bytes as those fields.
#[test]
fn the_canonical_file_parses_to_the_fields_written_at_those_offsets() {
    let wav = wav::parse(&CANONICAL).expect("the canonical file is a WAV");
    assert_eq!(wav.channels, 2);
    assert_eq!(wav.sample_rate, 44_100);
    assert_eq!(wav.samples, &CANONICAL[44..56], "the data chunk's body");
    assert_eq!(wav.samples.len(), 12, "three frames of four bytes");
    assert_decodes_to(&wav, &[0, 1, -1, 32_767, -32_768, 4_660]);
}

/// A file this repository did not encode, read as the encoder meant it.
#[test]
fn a_third_party_mono_file_parses_to_its_encoder_s_fields() {
    // 44 bytes of canonical header plus eight frames of two.
    assert_eq!(THIRD_PARTY_MONO.len(), 60);
    assert_eq!(&THIRD_PARTY_MONO[0..4], b"RIFF");
    assert_eq!(&THIRD_PARTY_MONO[8..12], b"WAVE");

    let wav = wav::parse(THIRD_PARTY_MONO).expect("a stdlib-encoded WAV must read");
    assert_eq!(wav.channels, 1);
    assert_eq!(wav.sample_rate, 8_000);
    assert_eq!(wav.samples.len(), 16);
    assert_decodes_to(&wav, &[0, 1, -1, 256, -256, 32_767, -32_768, 12_345]);
}

/// The same, in stereo and at a rate the mixer will actually see.
#[test]
fn a_third_party_stereo_file_parses_to_its_encoder_s_fields() {
    // 44 bytes of canonical header plus six frames of four.
    assert_eq!(THIRD_PARTY_STEREO.len(), 68);
    assert_eq!(&THIRD_PARTY_STEREO[0..4], b"RIFF");
    assert_eq!(&THIRD_PARTY_STEREO[8..12], b"WAVE");

    let wav = wav::parse(THIRD_PARTY_STEREO).expect("a stdlib-encoded WAV must read");
    assert_eq!(wav.channels, 2);
    assert_eq!(wav.sample_rate, 44_100);
    assert_eq!(wav.samples.len(), 24);
    assert_decodes_to(
        &wav,
        &[
            0, 1, 100, -100, 32_767, -32_768, -1, 2, 16_384, -16_384, 7, -7,
        ],
    );
}

/// The third-party fixtures agree with this reader about the redundant
/// fields too — which is the assertion that would catch a derivation this
/// repository got backwards and then wrote its own fixtures to match.
#[test]
fn the_third_party_headers_carry_the_derived_fields_this_reader_requires() {
    for (name, bytes, channels, rate) in [
        ("mono", THIRD_PARTY_MONO, 1u16, 8_000u32),
        ("stereo", THIRD_PARTY_STEREO, 2, 44_100),
    ] {
        let block_align = u16::from_le_bytes([bytes[32], bytes[33]]);
        let byte_rate = u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]);
        assert_eq!(block_align, channels * 2, "{name} block align");
        assert_eq!(byte_rate, rate * u32::from(block_align), "{name} byte rate");
    }
}
