//! Properties over written files, and the refusals a bent one earns.
//!
//! Three claims, different in kind.
//!
//! The first is that writing and reading are inverses over files nobody
//! thought to write by hand. On its own that is weak — a writer and a
//! reader making the *same* mistake are still inverses — which is why the
//! anchor beside this file asserts the bytes at offsets a person counted,
//! and reads two files a different implementation encoded.
//!
//! The second is about hostile input: the reader answers, one way or the
//! other, for every byte string it can be handed. It never panics and
//! never hangs. That is a small fuzzer with a fixed budget; a real one
//! with a corpus is what this reader needs before it can be called stable.
//!
//! The third is that each refusal is the *right* refusal. Those are
//! deterministic tests rather than properties, one per variant, because
//! "some error" and "the error naming what is wrong" are the difference
//! between a parser and a shrug.
//!
//! **The writer lives here rather than in the crate**, unlike the asset
//! pack's, which ships as public API. Nothing in this engine needs to
//! write a WAV file: the format is something it reads from elsewhere. A
//! shipped writer would be public surface with no consumer, so it is a
//! test fixture instead, with its two output modes split apart — `valid`
//! writes the canonical layout, `edge` writes the unusual-but-legal shapes
//! that the interesting paths in the reader are only reachable through.

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use renew_audio::wav::{self, WavError};

/// Offsets into a file [`valid`] produced. The layout is fixed — RIFF
/// header, `fmt ` chunk, `data` chunk, nothing else — which is what lets
/// the refusal tests below bend one field and leave the rest alone.
const OFF_RIFF_SIZE: usize = 4;
const OFF_FORMAT: usize = 20;
const OFF_CHANNELS: usize = 22;
const OFF_SAMPLE_RATE: usize = 24;
const OFF_BYTE_RATE: usize = 28;
const OFF_BLOCK_ALIGN: usize = 32;
const OFF_BITS: usize = 34;
const OFF_DATA_SIZE: usize = 40;
/// Where the samples start, which is also the size of the header a
/// canonical WAV file carries.
const OFF_DATA_BODY: usize = 44;

const FMT: [u8; 4] = *b"fmt ";
const DATA: [u8; 4] = *b"data";

// ---------------------------------------------------------------------
// The writer.
// ---------------------------------------------------------------------

/// A declared size, saturating rather than unwrapping.
///
/// Helper functions in a test file are not covered by the `#[test]`
/// exemption the lint wall grants, and every body this writer is handed is
/// a few dozen bytes.
fn declared(body: &[u8]) -> u32 {
    u32::try_from(body.len()).unwrap_or(u32::MAX)
}

/// One chunk: id, declared size, body, and the pad byte an odd body needs.
fn chunk(id: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = chunk_unpadded(id, body);
    if !body.len().is_multiple_of(2) {
        out.push(0);
    }
    out
}

/// The same chunk with its pad byte left off — a file no correct writer
/// emits, and the only way to reach one of the reader's refusals.
fn chunk_unpadded(id: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len() + 1);
    out.extend_from_slice(&id);
    out.extend_from_slice(&declared(body).to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Wrap chunks in a RIFF container whose declared size matches them.
fn riff(chunks: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + chunks.len());
    out.extend_from_slice(b"RIFF");
    // The declared size covers the form id and everything after it.
    out.extend_from_slice(&(4 + declared(chunks)).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(chunks);
    out
}

/// A PCM `fmt ` body, with the two redundant fields derived rather than
/// passed in — a writer that let a caller set them independently could not
/// be used to test that the reader requires them to agree.
fn fmt_body(channels: u16, sample_rate: u32, extended: bool) -> Vec<u8> {
    let block_align = channels.saturating_mul(2);
    let byte_rate = sample_rate.saturating_mul(u32::from(block_align));
    let mut body = Vec::with_capacity(18);
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&channels.to_le_bytes());
    body.extend_from_slice(&sample_rate.to_le_bytes());
    body.extend_from_slice(&byte_rate.to_le_bytes());
    body.extend_from_slice(&block_align.to_le_bytes());
    body.extend_from_slice(&16u16.to_le_bytes());
    if extended {
        // The extension-size field, saying there is no extension.
        body.extend_from_slice(&0u16.to_le_bytes());
    }
    body
}

/// **Valid mode**: the canonical layout, and nothing else in the file.
fn valid(channels: u16, sample_rate: u32, samples: &[u8]) -> Vec<u8> {
    let body = [
        chunk(FMT, &fmt_body(channels, sample_rate, false)),
        chunk(DATA, samples),
    ]
    .concat();
    riff(&body)
}

/// The unusual-but-legal shapes. Every one of them must still parse to the
/// fields it was written with; between them they reach the reader's
/// padding, extension and skip paths, which the canonical layout never
/// touches. They are also the seed set a fuzzer should start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    /// A `data` chunk with no samples. Silence is a legal sound.
    EmptyData,
    /// The 18-byte `fmt ` body whose extension size is zero.
    ExtendedFmt,
    /// An unknown chunk before `fmt `, which is where authoring tools put
    /// their metadata.
    UnknownPrefix,
    /// An odd-sized unknown chunk between `fmt ` and `data`, so the walk
    /// has to step over a pad byte to find the samples at all.
    OddUnknownBetween,
}

/// Every edge shape, in one place, so a new one cannot be added without
/// the properties below picking it up.
const EDGES: [Edge; 4] = [
    Edge::EmptyData,
    Edge::ExtendedFmt,
    Edge::UnknownPrefix,
    Edge::OddUnknownBetween,
];

/// **Edge mode**: one of [`EDGES`], carrying the same fields.
fn edge(shape: Edge, channels: u16, sample_rate: u32, samples: &[u8]) -> Vec<u8> {
    let extended = shape == Edge::ExtendedFmt;
    let format = chunk(FMT, &fmt_body(channels, sample_rate, extended));
    let data = chunk(
        DATA,
        if shape == Edge::EmptyData {
            &[]
        } else {
            samples
        },
    );
    let body = match shape {
        Edge::EmptyData | Edge::ExtendedFmt => [format, data].concat(),
        Edge::UnknownPrefix => [chunk(*b"LIST", b"INFOhere"), format, data].concat(),
        Edge::OddUnknownBetween => [format, chunk(*b"cue ", b"odd"), data].concat(),
    };
    riff(&body)
}

// ---------------------------------------------------------------------
// Helpers the tests share.
// ---------------------------------------------------------------------

/// Drop any partial frame, so a sample count always divides by channels.
fn whole_frames(values: &[i16], channels: u16) -> Vec<i16> {
    let whole = values.len() - values.len() % usize::from(channels);
    values.get(..whole).unwrap_or_default().to_vec()
}

fn to_bytes(values: &[i16]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Overwrite a little-endian `u16` at `offset`.
fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

/// Overwrite a little-endian `u32` at `offset`.
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// The file every bending test starts from: stereo, 44.1 kHz, three
/// frames. Byte-for-byte the file the anchor test asserts by hand, which
/// is what makes the offsets above trustworthy.
fn canonical() -> Vec<u8> {
    valid(2, 44_100, &to_bytes(&[0, 1, -1, 32_767, -32_768, 4_660]))
}

proptest! {
    // Seeded: a property suite that picks a fresh seed each run reports a
    // different question every time and cannot be re-run against a
    // failure.
    #![proptest_config(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x5c31_9a7f_04d6_2b8e),
        cases: 256,
        ..ProptestConfig::default()
    })]

    /// Valid mode: what was written comes back, field for field.
    #[test]
    fn writing_then_reading_returns_the_fields_that_went_in(
        channels in prop_oneof![Just(1u16), Just(2u16)],
        rate in 8_000u32..=192_000,
        values in proptest::collection::vec(any::<i16>(), 0..24),
    ) {
        let values = whole_frames(&values, channels);
        let samples = to_bytes(&values);
        let file = valid(channels, rate, &samples);

        let wav = wav::parse(&file).expect("our own writer's output must read");
        prop_assert_eq!(wav.channels, channels);
        prop_assert_eq!(wav.sample_rate, rate);
        prop_assert_eq!(wav.samples, &samples[..]);

        // And the decoded samples are exactly the scaled integers. Bit
        // patterns rather than a tolerance: the scale is a power of two,
        // so every conversion is exact and an approximate comparison
        // would pass for an off-by-one.
        let decoded: Vec<f32> = wav.samples_f32().collect();
        prop_assert_eq!(decoded.len(), values.len());
        for (got, raw) in decoded.iter().zip(values.iter()) {
            prop_assert_eq!(got.to_bits(), (f32::from(*raw) / 32_768.0).to_bits());
        }
    }

    /// Edge mode: the unusual shapes read back the same fields. Padding
    /// walked, extension accepted, unknown chunks stepped over either
    /// side of `fmt `.
    #[test]
    fn every_edge_shape_reads_back_the_fields_that_went_in(
        shape in proptest::sample::select(EDGES.to_vec()),
        channels in prop_oneof![Just(1u16), Just(2u16)],
        rate in 8_000u32..=192_000,
        values in proptest::collection::vec(any::<i16>(), 0..24),
    ) {
        let values = whole_frames(&values, channels);
        let samples = to_bytes(&values);
        let file = edge(shape, channels, rate, &samples);

        let wav = wav::parse(&file).expect("every edge shape is a legal file");
        prop_assert_eq!(wav.channels, channels);
        prop_assert_eq!(wav.sample_rate, rate);
        let expected: &[u8] = if shape == Edge::EmptyData { &[] } else { &samples };
        prop_assert_eq!(wav.samples, expected);
    }

    /// **Any** byte string gets an answer rather than a panic.
    #[test]
    fn arbitrary_bytes_get_an_answer(raw in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = wav::parse(&raw);
    }

    /// And so does anything shaped like a WAV, which reaches far deeper
    /// into the reader than random bytes ever do: random input dies at
    /// the magic, so without this the chunk walk below it would be
    /// exercised by nothing.
    #[test]
    fn a_truncated_file_with_noise_appended_gets_an_answer(
        shape in proptest::sample::select(EDGES.to_vec()),
        channels in prop_oneof![Just(1u16), Just(2u16)],
        rate in 8_000u32..=192_000,
        values in proptest::collection::vec(any::<i16>(), 0..24),
        cut in 0usize..96,
        noise in proptest::collection::vec(any::<u8>(), 0..24),
    ) {
        let samples = to_bytes(&whole_frames(&values, channels));
        for mut file in [valid(channels, rate, &samples), edge(shape, channels, rate, &samples)] {
            // Truncate somewhere, then append something. Both are
            // refusals the format promises to make; neither is a panic.
            file.truncate(cut.min(file.len()));
            file.extend_from_slice(&noise);
            let _ = wav::parse(&file);
        }
    }
}

/// The edge set is four files, and all four are legal. This is the seed
/// set, so a shape that stopped parsing would be a silent hole in it.
#[test]
fn the_whole_edge_set_parses() {
    let samples = to_bytes(&[7i16, -7, 1_000, -1_000]);
    for shape in EDGES {
        let file = edge(shape, 2, 22_050, &samples);
        let wav = wav::parse(&file).unwrap_or_else(|error| panic!("{shape:?}: {error}"));
        assert_eq!(wav.channels, 2, "{shape:?}");
        assert_eq!(wav.sample_rate, 22_050, "{shape:?}");
    }
}

/// The writer and the hand-written anchor agree about the canonical
/// layout, which is what makes the offsets the bending tests use real.
#[test]
fn the_canonical_file_is_the_length_its_layout_implies() {
    let file = canonical();
    assert_eq!(file.len(), OFF_DATA_BODY + 12);
    assert_eq!(&file[0..4], b"RIFF");
    assert_eq!(&file[8..12], b"WAVE");
    assert_eq!(&file[12..16], &FMT);
    assert_eq!(&file[36..40], &DATA);
    assert!(wav::parse(&file).is_ok());
}

// ---------------------------------------------------------------------
// One test per refusal. Each bends exactly one thing.
// ---------------------------------------------------------------------

#[test]
fn a_file_too_short_for_a_riff_header_is_refused() {
    let file = canonical();
    let error = wav::parse(&file[..11]).expect_err("eleven bytes is not a header");
    assert_eq!(error, WavError::TooShort { len: 11 });
}

#[test]
fn a_file_that_does_not_open_with_riff_is_refused() {
    let mut file = canonical();
    file[0..4].copy_from_slice(b"RIFX");
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(error, WavError::NotRiff { found: *b"RIFX" });
}

#[test]
fn a_riff_file_that_is_not_wave_is_refused() {
    let mut file = canonical();
    file[8..12].copy_from_slice(b"AVI ");
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(error, WavError::NotWave { found: *b"AVI " });
}

#[test]
fn a_riff_size_too_small_for_the_form_id_is_refused() {
    let mut file = canonical();
    put_u32(&mut file, OFF_RIFF_SIZE, 3);
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(error, WavError::RiffSizeTooSmall { declared: 3 });
}

#[test]
fn a_riff_size_past_the_end_of_the_input_is_refused() {
    let mut file = canonical();
    put_u32(&mut file, OFF_RIFF_SIZE, 1_000);
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(
        error,
        WavError::RiffSizeOverruns {
            declared: 1_008,
            present: 56,
        }
    );
}

/// Appending to a valid file is refused as firmly as cutting it short.
#[test]
fn bytes_after_the_riff_chunk_are_refused() {
    let mut file = canonical();
    file.push(0);
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(
        error,
        WavError::TrailingBytes {
            declared: 56,
            present: 57,
        }
    );
}

#[test]
fn a_chunk_header_cut_short_by_the_container_is_refused() {
    // A RIFF chunk holding four bytes: enough for an id, not for a size.
    let file = riff(b"fmt ");
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(
        error,
        WavError::ChunkHeaderTruncated {
            at: 12,
            remaining: 4,
        }
    );
}

#[test]
fn a_chunk_declaring_more_than_follows_it_is_refused() {
    let mut file = canonical();
    put_u32(&mut file, OFF_DATA_SIZE, 400);
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(
        error,
        WavError::ChunkOverruns {
            id: DATA,
            at: 36,
            declared: 400,
            available: 12,
        }
    );
}

#[test]
fn an_odd_chunk_ending_the_file_without_its_pad_byte_is_refused() {
    let body = [
        chunk(FMT, &fmt_body(1, 8_000, false)),
        chunk_unpadded(DATA, &[0]),
    ]
    .concat();
    let file = riff(&body);
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(
        error,
        WavError::MissingPadByte {
            id: DATA,
            at: file.len(),
        }
    );
}

#[test]
fn a_second_fmt_chunk_is_refused() {
    let format = chunk(FMT, &fmt_body(1, 8_000, false));
    let body = [format.clone(), format, chunk(DATA, &to_bytes(&[1i16, 2]))].concat();
    let error = wav::parse(&riff(&body)).expect_err("must refuse");
    assert_eq!(error, WavError::DuplicateChunk { id: FMT, at: 36 });
}

#[test]
fn a_second_data_chunk_is_refused() {
    let data = chunk(DATA, &to_bytes(&[1i16, 2]));
    let body = [chunk(FMT, &fmt_body(1, 8_000, false)), data.clone(), data].concat();
    let error = wav::parse(&riff(&body)).expect_err("must refuse");
    assert_eq!(error, WavError::DuplicateChunk { id: DATA, at: 48 });
}

/// Samples before the header that describes them have no described
/// format, and the reader is single-pass by design.
#[test]
fn a_data_chunk_before_the_fmt_chunk_is_refused() {
    let body = [
        chunk(DATA, &to_bytes(&[1i16, 2])),
        chunk(FMT, &fmt_body(1, 8_000, false)),
    ]
    .concat();
    let error = wav::parse(&riff(&body)).expect_err("must refuse");
    assert_eq!(error, WavError::DataBeforeFmt { at: 12 });
}

#[test]
fn a_file_with_no_fmt_chunk_is_refused() {
    let file = riff(&chunk(*b"LIST", b"INFOonly"));
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(error, WavError::MissingChunk { id: FMT });
}

#[test]
fn a_file_with_no_data_chunk_is_refused() {
    let file = riff(&chunk(FMT, &fmt_body(2, 44_100, false)));
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(error, WavError::MissingChunk { id: DATA });
}

/// The 40-byte extensible body, which is a real format and not this one.
#[test]
fn a_fmt_body_of_a_size_this_reader_does_not_read_is_refused() {
    let mut body = fmt_body(2, 44_100, true);
    body.resize(40, 0);
    let file = riff(&[chunk(FMT, &body), chunk(DATA, &[])].concat());
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(error, WavError::FmtSize { len: 40 });
}

#[test]
fn a_format_that_is_not_pcm_is_refused() {
    let mut file = canonical();
    // 3 is IEEE float: a real format, and not one this reader decodes.
    put_u16(&mut file, OFF_FORMAT, 3);
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(error, WavError::NotPcm { format: 3 });
}

#[test]
fn a_channel_count_that_is_neither_mono_nor_stereo_is_refused() {
    for channels in [0u16, 3, 8] {
        let mut file = canonical();
        put_u16(&mut file, OFF_CHANNELS, channels);
        let error = wav::parse(&file).expect_err("must refuse");
        assert_eq!(error, WavError::ChannelCount { channels });
    }
}

#[test]
fn a_sample_rate_outside_the_accepted_range_is_refused() {
    for rate in [0u32, 7_999, 192_001, u32::MAX] {
        let mut file = canonical();
        put_u32(&mut file, OFF_SAMPLE_RATE, rate);
        let error = wav::parse(&file).expect_err("must refuse");
        assert_eq!(error, WavError::SampleRate { rate });
    }
}

/// And both ends of the range are inside it.
#[test]
fn the_ends_of_the_accepted_rate_range_are_accepted() {
    for rate in [8_000u32, 192_000] {
        let file = valid(1, rate, &to_bytes(&[1i16, -1]));
        let wav = wav::parse(&file).unwrap_or_else(|error| panic!("{rate}: {error}"));
        assert_eq!(wav.sample_rate, rate);
    }
}

#[test]
fn a_bit_depth_this_reader_does_not_decode_is_refused() {
    for bits in [0u16, 8, 24, 32] {
        let mut file = canonical();
        put_u16(&mut file, OFF_BITS, bits);
        let error = wav::parse(&file).expect_err("must refuse");
        assert_eq!(error, WavError::BitDepth { bits });
    }
}

/// A header that disagrees with itself is refused rather than believed in
/// part. Both redundant fields get their own test, because a reader that
/// checked only one would pass a suite that only bent the other.
#[test]
fn a_block_alignment_contradicting_the_header_is_refused() {
    let mut file = canonical();
    put_u16(&mut file, OFF_BLOCK_ALIGN, 8);
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(
        error,
        WavError::BlockAlign {
            declared: 8,
            derived: 4,
        }
    );
}

#[test]
fn a_byte_rate_contradicting_the_header_is_refused() {
    let mut file = canonical();
    put_u32(&mut file, OFF_BYTE_RATE, 1);
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(
        error,
        WavError::ByteRate {
            declared: 1,
            derived: 176_400,
        }
    );
}

#[test]
fn a_fmt_chunk_declaring_an_extension_is_refused() {
    let samples = to_bytes(&[1i16, -1]);
    let mut file = edge(Edge::ExtendedFmt, 1, 8_000, &samples);
    // The extension-size field is the last two bytes of the 18-byte body,
    // which starts at 20.
    put_u16(&mut file, 20 + 16, 22);
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(error, WavError::ExtensionSize { declared: 22 });
}

#[test]
fn samples_that_are_not_a_whole_number_of_frames_are_refused() {
    // Stereo frames are four bytes, so two bytes is half of one.
    let file = valid(2, 44_100, &[0, 0]);
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(
        error,
        WavError::DataNotWholeFrames {
            len: 2,
            block_align: 4,
        }
    );
}

/// An `fmt ` body of an unread size is caught before its fields are, so a
/// truncated header cannot be read as a shorter one that happens to fit.
#[test]
fn a_fmt_body_shorter_than_pcm_needs_is_refused() {
    let short = fmt_body(2, 44_100, false);
    let file = riff(&[chunk(FMT, &short[..14]), chunk(DATA, &[])].concat());
    let error = wav::parse(&file).expect_err("must refuse");
    assert_eq!(error, WavError::FmtSize { len: 14 });
}

// ---------------------------------------------------------------------
// Truncation, cut by cut.
// ---------------------------------------------------------------------

/// Every prefix of a valid file is refused, and by the variant that
/// explains it: below the header there is nothing to read, and above it
/// the container's own size field is the first thing that no longer
/// describes the bytes present.
#[test]
fn every_truncation_of_a_valid_file_is_refused_by_name() {
    let whole = canonical();
    for cut in 0..whole.len() {
        let error = wav::parse(&whole[..cut]).expect_err("a short file must be refused");
        let expected = if cut < 12 {
            WavError::TooShort { len: cut }
        } else {
            WavError::RiffSizeOverruns {
                declared: 56,
                present: cut,
            }
        };
        assert_eq!(error, expected, "cut at {cut}");
    }
    assert!(wav::parse(&whole).is_ok(), "the whole file still reads");
}

/// The same cuts with the container's size field repaired to match, which
/// is what a truncating attacker would do and what the test above cannot
/// reach past. Each one is still refused, and each refusal names the chunk
/// boundary the cut fell in.
#[test]
fn every_repaired_truncation_is_refused_by_name() {
    let whole = canonical();
    for cut in 0..whole.len() {
        let mut file = whole[..cut].to_vec();
        if file.len() >= 12 {
            let body = declared(&file[8..]);
            put_u32(&mut file, OFF_RIFF_SIZE, body);
        }
        let error = wav::parse(&file).expect_err("a short file must be refused");
        assert!(
            matches!(
                error,
                WavError::TooShort { .. }
                    | WavError::ChunkHeaderTruncated { .. }
                    | WavError::ChunkOverruns { .. }
                    | WavError::MissingChunk { .. }
            ),
            "cut at {cut}: {error}"
        );
    }
}
