//! Reading a WAV file the engine did not write.
//!
//! This is the crate's whole untrusted-input surface, and it is one
//! function. Everything [`parse`] returns borrows the caller's bytes, so a
//! malformed file cannot cost an allocation, and a caller that has already
//! bounded the read has bounded the parse too.
//!
//! **A file's header is the least trustworthy thing about it.** RIFF is a
//! format of nested declared lengths, and every one of them here is
//! checked against the bytes actually present before it is used to slice
//! anything. The arithmetic that combines them is checked as well, because
//! a length near `u32::MAX` that wrapped a 32-bit address would land back
//! inside the file and look reasonable.
//!
//! The accepted set is deliberately narrow: RIFF/WAVE, PCM, 16-bit, mono
//! or stereo, a sample rate between 8 kHz and 192 kHz, exactly one `fmt `
//! chunk and exactly one `data` chunk. Anything else is refused by name
//! rather than approximated. Two consequences are worth stating plainly,
//! because they are choices and not oversights:
//!
//! - **A header that lies about itself is refused.** The `block_align` and
//!   `byte_rate` fields are redundant — both are functions of the channel
//!   count, the bit depth and the sample rate — and this reader requires
//!   them to agree with what they are derived from. A file whose redundant
//!   fields disagree is a file whose author and this reader do not mean the
//!   same thing by the rest of it.
//! - **Nothing may follow the RIFF chunk.** Files carrying appended
//!   metadata past the declared end are common in the wild and are refused
//!   here. Unknown *chunks* inside the RIFF chunk are skipped happily,
//!   before or after `data`; it is only bytes outside the container that
//!   have no meaning at all.

use core::fmt;

/// The bytes every RIFF file opens with: the container chunk's own
/// eight-byte header, and the four-byte form id its body starts with.
const RIFF_HEADER_BYTES: usize = 12;
/// A chunk's four-byte id followed by its four-byte declared size.
const CHUNK_HEADER_BYTES: usize = 8;

/// The container chunk's id.
const RIFF: [u8; 4] = *b"RIFF";
/// The form id that makes a RIFF file a WAV file.
const WAVE: [u8; 4] = *b"WAVE";
/// The chunk describing the sample format. Its id is four bytes, so the
/// trailing space is part of the name and not a typo.
const FMT: [u8; 4] = *b"fmt ";
/// The chunk holding the samples themselves.
const DATA: [u8; 4] = *b"data";

/// The smallest RIFF body that can describe anything: the form id alone.
const MIN_RIFF_BODY: u32 = 4;
/// `WAVE_FORMAT_PCM`, the only audio format this reader decodes.
const FORMAT_PCM: u16 = 1;
/// A PCM `fmt ` body: format, channels, rate, byte rate, block alignment,
/// bit depth.
const FMT_PCM_BYTES: usize = 16;
/// The same body plus the two-byte extension-size field, which this reader
/// accepts only when it declares no extension.
const FMT_EXTENDED_BYTES: usize = 18;
/// The only sample width this reader decodes.
const BITS_PER_SAMPLE: u16 = 16;
/// [`BITS_PER_SAMPLE`] in bytes, which is what frame arithmetic needs.
const BYTES_PER_SAMPLE: u16 = 2;
/// The lowest accepted sample rate: below this nothing in the wild is real
/// audio, and a zero here would divide by zero downstream.
const MIN_SAMPLE_RATE: u32 = 8_000;
/// The highest accepted sample rate.
const MAX_SAMPLE_RATE: u32 = 192_000;

/// The scale from a 16-bit sample to a normalised one.
///
/// 2^15 rather than 32767: it is a power of two, so the division only
/// changes an exponent and every sample converts exactly. The cost is that
/// full positive scale reaches 32767/32768 rather than 1.0, which is
/// inaudible and is the conventional trade. The gain is that decoding is
/// exact and reversible, which is what makes it testable by equality.
const FULL_SCALE: f32 = 32_768.0;

/// A validated WAV file, borrowing the bytes it was read from.
///
/// The fields are public because this is a description of somebody else's
/// bytes and every consumer needs all three. What [`parse`] guarantees
/// about a value it returned — and only about one it returned — is that
/// `channels` is 1 or 2, `sample_rate` is within
/// [`MIN_SAMPLE_RATE`]`..=`[`MAX_SAMPLE_RATE`], and `samples` is a whole
/// number of frames of 16-bit little-endian samples. A `Wav` assembled by
/// hand carries no such promise, and [`Wav::samples_f32`] is written to
/// answer rather than panic either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wav<'a> {
    /// 1 for mono, 2 for stereo.
    pub channels: u16,
    /// Frames per second.
    pub sample_rate: u32,
    /// The `data` chunk's body: interleaved 16-bit little-endian samples.
    pub samples: &'a [u8],
}

impl<'a> Wav<'a> {
    /// The samples decoded to `f32` in `[-1.0, 1.0]`, in file order.
    ///
    /// Interleaved exactly as they are in the file: for stereo, left then
    /// right then left again. Decoding is done here rather than at parse
    /// time so that reading a file stays proportional to its header, and
    /// so that a caller resampling or mixing can walk the samples once.
    #[must_use]
    pub fn samples_f32(&self) -> Samples<'a> {
        Samples { rest: self.samples }
    }
}

/// The samples of a [`Wav`], decoded one at a time.
///
/// A trailing odd byte — which [`parse`] cannot produce, but a hand-built
/// [`Wav`] can — is dropped rather than treated as half a sample. The
/// iterator's job is to answer, and half a sample is not an answer.
#[derive(Clone, Debug)]
pub struct Samples<'a> {
    rest: &'a [u8],
}

impl Iterator for Samples<'_> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let (sample, rest) = self.rest.split_first_chunk::<2>()?;
        self.rest = rest;
        Some(f32::from(i16::from_le_bytes(*sample)) / FULL_SCALE)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.rest.len() / usize::from(BYTES_PER_SAMPLE);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Samples<'_> {}

impl core::iter::FusedIterator for Samples<'_> {}

/// Why a WAV file was refused.
///
/// Closed, with named fields and no catch-all. A parser of untrusted input
/// is judged by its refusals rather than by its successes, and one that can
/// say "malformed" without saying how has stopped being able to tell a
/// truncated download from an attack. Every variant carries the numbers a
/// person needs to find the problem in a file they did not build and
/// cannot read by eye.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavError {
    /// Shorter than the RIFF header, so there is nothing to believe.
    TooShort {
        /// How many bytes were handed in.
        len: usize,
    },
    /// The first four bytes are not `RIFF`.
    NotRiff {
        /// What was there instead.
        found: [u8; 4],
    },
    /// A RIFF file, but not a WAV one.
    NotWave {
        /// The form id found where `WAVE` was expected.
        found: [u8; 4],
    },
    /// The RIFF chunk declares a body too small to hold even the form id.
    RiffSizeTooSmall {
        /// The declared body size.
        declared: u32,
    },
    /// The RIFF chunk declares more bytes than the input holds.
    ///
    /// Refused rather than truncated to what is present: a file cut short
    /// in transit and a file that describes itself wrongly are both things
    /// a reader must not quietly complete.
    RiffSizeOverruns {
        /// The whole file size the header describes.
        declared: u64,
        /// The bytes actually present.
        present: usize,
    },
    /// Bytes follow the RIFF chunk's declared end.
    ///
    /// Checked as equality rather than "at least", so an appended payload
    /// is refused as firmly as a missing one is.
    TrailingBytes {
        /// Where the RIFF chunk ends.
        declared: u64,
        /// The bytes actually present.
        present: usize,
    },
    /// A chunk begins too close to the end to hold its own header.
    ChunkHeaderTruncated {
        /// Where the chunk begins.
        at: usize,
        /// How many bytes remain from there.
        remaining: usize,
    },
    /// A chunk declares a body longer than the bytes that follow it.
    ChunkOverruns {
        /// The chunk's id.
        id: [u8; 4],
        /// Where the chunk begins.
        at: usize,
        /// The body size it declares.
        declared: u32,
        /// The bytes that actually follow its header.
        available: usize,
    },
    /// An odd-sized chunk ends the file without its pad byte.
    MissingPadByte {
        /// The chunk's id.
        id: [u8; 4],
        /// Where the pad byte should have been.
        at: usize,
    },
    /// A second `fmt ` or `data` chunk.
    ///
    /// A file with two of either has two answers to a question with one,
    /// and picking the first or the last would be a coin toss dressed as a
    /// rule.
    DuplicateChunk {
        /// The repeated id.
        id: [u8; 4],
        /// Where the repeat begins.
        at: usize,
    },
    /// `data` arrives before any `fmt `, so its bytes have no format.
    ///
    /// The reader is single-pass by design: it validates each chunk where
    /// it finds it, which it cannot do for samples whose width and channel
    /// count have not been declared yet.
    DataBeforeFmt {
        /// Where the `data` chunk begins.
        at: usize,
    },
    /// A required chunk is absent.
    MissingChunk {
        /// Which one.
        id: [u8; 4],
    },
    /// The `fmt ` body is not a size this reader understands.
    FmtSize {
        /// The body size found.
        len: usize,
    },
    /// The audio format is not PCM.
    NotPcm {
        /// The format tag found.
        format: u16,
    },
    /// Neither mono nor stereo.
    ChannelCount {
        /// The channel count found.
        channels: u16,
    },
    /// A sample rate outside the accepted range.
    SampleRate {
        /// The rate found.
        rate: u32,
    },
    /// A sample width this reader does not decode.
    BitDepth {
        /// The bit depth found.
        bits: u16,
    },
    /// The declared block alignment is not the one the other fields imply.
    BlockAlign {
        /// What the header says.
        declared: u16,
        /// What its own channel count and bit depth give.
        derived: u16,
    },
    /// The declared byte rate is not the one the other fields imply.
    ByteRate {
        /// What the header says.
        declared: u32,
        /// What its own fields give.
        derived: u32,
    },
    /// An 18-byte `fmt ` body declaring an extension this reader has no
    /// implementation for.
    ExtensionSize {
        /// The declared extension size.
        declared: u16,
    },
    /// The `data` body is not a whole number of frames.
    DataNotWholeFrames {
        /// The body size.
        len: usize,
        /// The frame size the header implies.
        block_align: u16,
    },
}

impl fmt::Display for WavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { len } => write!(
                f,
                "too short to be a WAV: {len} bytes, and the RIFF header alone needs {RIFF_HEADER_BYTES}"
            ),
            Self::NotRiff { found } => write!(
                f,
                "not a RIFF file: it opens with `{}` rather than `RIFF`",
                found.escape_ascii()
            ),
            Self::NotWave { found } => write!(
                f,
                "a RIFF file, but its form is `{}` rather than `WAVE`",
                found.escape_ascii()
            ),
            Self::RiffSizeTooSmall { declared } => write!(
                f,
                "the RIFF chunk declares a {declared}-byte body, too small for the {MIN_RIFF_BODY}-byte form id"
            ),
            Self::RiffSizeOverruns { declared, present } => write!(
                f,
                "the RIFF chunk describes a {declared}-byte file but {present} bytes are present"
            ),
            Self::TrailingBytes { declared, present } => write!(
                f,
                "the RIFF chunk ends at {declared} and {present} bytes are present; nothing may follow it"
            ),
            Self::ChunkHeaderTruncated { at, remaining } => write!(
                f,
                "the chunk at {at} has {remaining} bytes left for its {CHUNK_HEADER_BYTES}-byte header"
            ),
            Self::ChunkOverruns {
                id,
                at,
                declared,
                available,
            } => write!(
                f,
                "chunk `{}` at {at} declares {declared} bytes of body but {available} follow its header",
                id.escape_ascii()
            ),
            Self::MissingPadByte { id, at } => write!(
                f,
                "chunk `{}` has an odd size and the file ends at {at} without its pad byte",
                id.escape_ascii()
            ),
            Self::DuplicateChunk { id, at } => write!(
                f,
                "a second `{}` chunk at {at}; a WAV holds exactly one",
                id.escape_ascii()
            ),
            Self::DataBeforeFmt { at } => write!(
                f,
                "the `data` chunk at {at} comes before any `fmt `, so nothing describes its samples"
            ),
            Self::MissingChunk { id } => {
                write!(f, "no `{}` chunk anywhere in the file", id.escape_ascii())
            }
            Self::FmtSize { len } => write!(
                f,
                "a `fmt ` body of {len} bytes; this reader reads {FMT_PCM_BYTES}, or {FMT_EXTENDED_BYTES} when it declares no extension"
            ),
            Self::NotPcm { format } => write!(
                f,
                "audio format {format} is not PCM ({FORMAT_PCM}), the only one this reader decodes"
            ),
            Self::ChannelCount { channels } => {
                write!(f, "{channels} channels; this reader takes mono or stereo")
            }
            Self::SampleRate { rate } => write!(
                f,
                "a sample rate of {rate} Hz, outside the accepted {MIN_SAMPLE_RATE}..={MAX_SAMPLE_RATE}"
            ),
            Self::BitDepth { bits } => write!(
                f,
                "{bits} bits per sample; this reader decodes {BITS_PER_SAMPLE}"
            ),
            Self::BlockAlign { declared, derived } => write!(
                f,
                "the header declares a block alignment of {declared} where its own channels and bit depth give {derived}"
            ),
            Self::ByteRate { declared, derived } => write!(
                f,
                "the header declares a byte rate of {declared} where its own fields give {derived}"
            ),
            Self::ExtensionSize { declared } => write!(
                f,
                "the `fmt ` chunk declares a {declared}-byte extension; this reader reads PCM with none"
            ),
            Self::DataNotWholeFrames { len, block_align } => write!(
                f,
                "{len} bytes of samples is not a whole number of {block_align}-byte frames"
            ),
        }
    }
}

impl core::error::Error for WavError {}

/// The `fmt ` fields, after every one of them has been checked.
///
/// Private: it exists so the chunk walk can carry a validated format
/// around without carrying the raw bytes it came from.
#[derive(Clone, Copy)]
struct Format {
    channels: u16,
    sample_rate: u32,
    block_align: u16,
}

/// Validate `bytes` and borrow them as a WAV file.
///
/// # Errors
///
/// A [`WavError`] naming exactly what was wrong, for every input that is
/// not one of the narrow set this reader accepts: a file too short to hold
/// a RIFF header, one that is not RIFF or not WAVE, a RIFF size that does
/// not describe the bytes present in either direction, a chunk header or
/// body that runs past the end, an odd chunk missing its pad byte, a
/// repeated `fmt ` or `data`, a `data` chunk before the `fmt ` that
/// describes it, either of them missing, a `fmt ` body of an unread size
/// or declaring an extension, a format that is not PCM, a channel count
/// that is not mono or stereo, a rate outside
/// [`MIN_SAMPLE_RATE`]`..=`[`MAX_SAMPLE_RATE`], a bit depth that is not 16,
/// a `block_align` or `byte_rate` field disagreeing with what the header's
/// other fields derive, and a `data` body that is not a whole number of
/// frames.
///
/// Nothing is allocated on any path, including the refusing ones.
pub fn parse(bytes: &[u8]) -> Result<Wav<'_>, WavError> {
    let header: &[u8; RIFF_HEADER_BYTES] = bytes
        .first_chunk()
        .ok_or(WavError::TooShort { len: bytes.len() })?;
    // The container chunk's id, the size it declares for its own body, and
    // the form id that body opens with.
    let container = [header[0], header[1], header[2], header[3]];
    let riff_body = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let form = [header[8], header[9], header[10], header[11]];

    if container != RIFF {
        return Err(WavError::NotRiff { found: container });
    }
    if form != WAVE {
        return Err(WavError::NotWave { found: form });
    }
    if riff_body < MIN_RIFF_BODY {
        return Err(WavError::RiffSizeTooSmall {
            declared: riff_body,
        });
    }

    // The declared size counts the bytes after the size field, so the file
    // it describes is eight bytes longer. Computed in `u64` because a
    // hostile size near `u32::MAX` would wrap a 32-bit `usize` and land
    // back inside the file, where every later bound would agree with it.
    let declared = u64::from(riff_body) + CHUNK_HEADER_BYTES as u64;
    let present = bytes.len() as u64;
    if declared > present {
        return Err(WavError::RiffSizeOverruns {
            declared,
            present: bytes.len(),
        });
    }
    if declared < present {
        return Err(WavError::TrailingBytes {
            declared,
            present: bytes.len(),
        });
    }

    // Those two refusals together pin the RIFF chunk's end to the input's
    // end, so the walk below has one bound to check against rather than
    // two that could disagree.
    let mut format: Option<Format> = None;
    let mut samples: Option<&[u8]> = None;
    let mut cursor = RIFF_HEADER_BYTES;

    while cursor < bytes.len() {
        let at = cursor;
        // Sound because the loop condition proves `at` is inside.
        let remaining = bytes.len() - at;
        let chunk: &[u8; CHUNK_HEADER_BYTES] = bytes
            .get(at..)
            .and_then(<[u8]>::first_chunk)
            .ok_or(WavError::ChunkHeaderTruncated { at, remaining })?;
        let id = [chunk[0], chunk[1], chunk[2], chunk[3]];
        let declared = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);

        // Reading the header proved these eight bytes are present, so both
        // the offset and the subtraction below are inside the input.
        let body_at = at + CHUNK_HEADER_BYTES;
        let available = remaining - CHUNK_HEADER_BYTES;
        let overruns = || WavError::ChunkOverruns {
            id,
            at,
            declared,
            available,
        };
        // A size that cannot even be represented on this target certainly
        // overruns what is present, so it takes the same road out.
        let size = usize::try_from(declared).map_err(|_| overruns())?;
        let body = bytes
            .get(body_at..)
            .and_then(|rest| rest.get(..size))
            .ok_or_else(overruns)?;

        match id {
            FMT => {
                if format.is_some() {
                    return Err(WavError::DuplicateChunk { id, at });
                }
                format = Some(parse_format(body)?);
            }
            DATA => {
                if samples.is_some() {
                    return Err(WavError::DuplicateChunk { id, at });
                }
                if format.is_none() {
                    return Err(WavError::DataBeforeFmt { at });
                }
                samples = Some(body);
            }
            // Anything else is somebody else's business: a `LIST` of
            // authoring metadata, a `cue ` of markers, a `fact` a decoder
            // this one is not would want. Skipping them is what makes an
            // extensible container extensible, and their padding is walked
            // exactly like anybody's.
            _ => {}
        }

        // The slice above proved the body is present, so this sum is
        // inside the input.
        cursor = body_at + size;
        if !declared.is_multiple_of(2) {
            // RIFF pads an odd chunk to an even boundary, and the pad byte
            // belongs to the file rather than to the chunk. A file that
            // ends without it is a byte short, not merely untidy — and
            // reading on as if it were there would step into whatever
            // follows.
            if cursor == bytes.len() {
                return Err(WavError::MissingPadByte { id, at: cursor });
            }
            cursor += 1;
        }
    }

    let format = format.ok_or(WavError::MissingChunk { id: FMT })?;
    let samples = samples.ok_or(WavError::MissingChunk { id: DATA })?;

    // `block_align` was derived from a channel count already checked to be
    // 1 or 2, so it is 2 or 4 here and never 0.
    if samples.len() % usize::from(format.block_align) != 0 {
        return Err(WavError::DataNotWholeFrames {
            len: samples.len(),
            block_align: format.block_align,
        });
    }

    Ok(Wav {
        channels: format.channels,
        sample_rate: format.sample_rate,
        samples,
    })
}

/// Validate a `fmt ` chunk's body.
///
/// Split out of [`parse`] so that the chunk walk and the format rules can
/// each be read on their own. Every field is checked before any of them is
/// believed, and the two redundant fields are required to agree with what
/// they are redundant with.
fn parse_format(body: &[u8]) -> Result<Format, WavError> {
    // A PCM body is 16 bytes, or 18 when it carries the extension-size
    // field. `first_chunk` yields the fixed layout the rest of this
    // function reads by offset; the length test is what refuses the
    // 40-byte extensible body and everything else.
    let sized = body.len() == FMT_PCM_BYTES || body.len() == FMT_EXTENDED_BYTES;
    let head: &[u8; FMT_PCM_BYTES] = body
        .first_chunk()
        .filter(|_| sized)
        .ok_or(WavError::FmtSize { len: body.len() })?;

    let format = u16::from_le_bytes([head[0], head[1]]);
    let channels = u16::from_le_bytes([head[2], head[3]]);
    let sample_rate = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
    let byte_rate = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);
    let block_align = u16::from_le_bytes([head[12], head[13]]);
    let bits = u16::from_le_bytes([head[14], head[15]]);

    if format != FORMAT_PCM {
        return Err(WavError::NotPcm { format });
    }
    if channels != 1 && channels != 2 {
        return Err(WavError::ChannelCount { channels });
    }
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
        return Err(WavError::SampleRate { rate: sample_rate });
    }
    if bits != BITS_PER_SAMPLE {
        return Err(WavError::BitDepth { bits });
    }

    // Both products are bounded by the checks just above — at most 2
    // channels of 2 bytes, and at most 192000 frames of 4 bytes — so
    // neither can overflow its width.
    let derived_align = channels * BYTES_PER_SAMPLE;
    if block_align != derived_align {
        return Err(WavError::BlockAlign {
            declared: block_align,
            derived: derived_align,
        });
    }
    let derived_rate = sample_rate * u32::from(derived_align);
    if byte_rate != derived_rate {
        return Err(WavError::ByteRate {
            declared: byte_rate,
            derived: derived_rate,
        });
    }

    // `None` is the 16-byte body, which has no extension field at all;
    // `Some(0)` is the 18-byte body saying it has no extension. Anything
    // else describes a format this reader has no implementation for, and
    // ignoring the field would mean decoding those bytes as if it did.
    match u16_at(body, FMT_PCM_BYTES) {
        None | Some(0) => {}
        Some(declared) => return Err(WavError::ExtensionSize { declared }),
    }

    Ok(Format {
        channels,
        sample_rate,
        block_align,
    })
}

/// Read a little-endian `u16` at `offset`, or `None` past the end.
fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let slice = bytes.get(offset..end)?;
    let array: [u8; 2] = slice.try_into().ok()?;
    Some(u16::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every refusal says what was wrong and carries the numbers that
    /// locate it. A message naming no offset, id or value would be a
    /// "malformed" with extra words.
    #[test]
    fn every_variant_displays_its_context() {
        let cases: Vec<(WavError, &[&str])> = vec![
            (WavError::TooShort { len: 3 }, &["3", "12"]),
            (WavError::NotRiff { found: *b"RIFX" }, &["RIFX"]),
            (WavError::NotWave { found: *b"AVI " }, &["AVI"]),
            (WavError::RiffSizeTooSmall { declared: 2 }, &["2", "4"]),
            (
                WavError::RiffSizeOverruns {
                    declared: 900,
                    present: 40,
                },
                &["900", "40"],
            ),
            (
                WavError::TrailingBytes {
                    declared: 56,
                    present: 57,
                },
                &["56", "57"],
            ),
            (
                WavError::ChunkHeaderTruncated {
                    at: 44,
                    remaining: 5,
                },
                &["44", "5"],
            ),
            (
                WavError::ChunkOverruns {
                    id: *b"data",
                    at: 36,
                    declared: 400,
                    available: 12,
                },
                &["data", "36", "400", "12"],
            ),
            (
                WavError::MissingPadByte {
                    id: *b"junk",
                    at: 45,
                },
                &["junk", "45"],
            ),
            (
                WavError::DuplicateChunk {
                    id: *b"fmt ",
                    at: 36,
                },
                &["fmt", "36"],
            ),
            (WavError::DataBeforeFmt { at: 12 }, &["data", "12"]),
            (WavError::MissingChunk { id: *b"data" }, &["data"]),
            (WavError::FmtSize { len: 40 }, &["40", "16", "18"]),
            (WavError::NotPcm { format: 3 }, &["3", "1"]),
            (WavError::ChannelCount { channels: 6 }, &["6"]),
            (
                WavError::SampleRate { rate: 4000 },
                &["4000", "8000", "192000"],
            ),
            (WavError::BitDepth { bits: 24 }, &["24", "16"]),
            (
                WavError::BlockAlign {
                    declared: 7,
                    derived: 4,
                },
                &["7", "4"],
            ),
            (
                WavError::ByteRate {
                    declared: 9,
                    derived: 176_400,
                },
                &["9", "176400"],
            ),
            (WavError::ExtensionSize { declared: 22 }, &["22"]),
            (
                WavError::DataNotWholeFrames {
                    len: 13,
                    block_align: 4,
                },
                &["13", "4"],
            ),
        ];
        for (error, expected) in cases {
            let shown = error.to_string();
            for needle in expected {
                assert!(
                    shown.contains(needle),
                    "`{shown}` does not mention `{needle}`"
                );
            }
        }
    }

    /// A chunk id of unprintable bytes is escaped rather than written
    /// through, so a refusal about a hostile file cannot itself put
    /// control characters on a terminal.
    #[test]
    fn a_hostile_chunk_id_is_escaped_in_its_message() {
        let shown = WavError::MissingChunk {
            id: [0x00, 0x1b, 0xff, b'a'],
        }
        .to_string();
        assert!(shown.contains("\\x00"), "{shown}");
        assert!(shown.contains("\\x1b"), "{shown}");
        assert!(shown.contains("\\xff"), "{shown}");
        assert!(!shown.contains('\u{1b}'), "an escape reached the message");
    }

    /// Decoding is the exact scaling the constant promises, at both ends
    /// of the range and at zero.
    #[test]
    fn samples_decode_to_exactly_the_scaled_value() {
        let bytes = [0x00, 0x00, 0x01, 0x00, 0xff, 0xff, 0xff, 0x7f, 0x00, 0x80];
        let wav = Wav {
            channels: 1,
            sample_rate: 8_000,
            samples: &bytes,
        };
        let decoded: Vec<f32> = wav.samples_f32().collect();
        let expected: [f32; 5] = [
            0.0,
            1.0 / 32_768.0,
            -1.0 / 32_768.0,
            32_767.0 / 32_768.0,
            -1.0,
        ];
        // Compared as bit patterns: every value here is a power-of-two
        // rescaling of an integer that fits an `f32` mantissa, so each is
        // exact, and a tolerance would hide the off-by-one this exists to
        // catch.
        for (got, want) in decoded.iter().zip(expected.iter()) {
            assert_eq!(got.to_bits(), want.to_bits(), "{got} is not {want}");
        }
        assert_eq!(decoded.len(), expected.len());
    }

    /// The iterator reports its own length, and an odd trailing byte is
    /// dropped rather than read as half a sample.
    #[test]
    fn the_sample_iterator_counts_whole_samples_only() {
        let bytes = [1u8, 0, 2, 0, 3];
        let wav = Wav {
            channels: 1,
            sample_rate: 8_000,
            samples: &bytes,
        };
        let mut samples = wav.samples_f32();
        assert_eq!(samples.len(), 2);
        assert!(samples.next().is_some());
        assert_eq!(samples.len(), 1);
        assert!(samples.next().is_some());
        assert_eq!(samples.len(), 0);
        assert!(samples.next().is_none());
        // Fused: once it is done it stays done.
        assert!(samples.next().is_none());
    }
}
