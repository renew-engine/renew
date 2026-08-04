//! The mixer: a fixed table of voices, filled into an interleaved
//! buffer by whoever owns the audio callback.
//!
//! Every decision here follows from one constraint: `fill` runs on a
//! thread with a deadline it cannot miss. So the expensive work —
//! decoding, resampling, laying samples out for the device — happens at
//! load time, and the callback only copies, adds, and clamps. Nothing
//! it does allocates, locks-and-waits, or panics.

use crate::ring::{self, Command, Consumer, Producer};
use crate::wav::Wav;

/// How many sounds one mixer holds. Sixteen is a sound bank for the
/// games this engine has; the seventeenth load is a sizing bug in the
/// caller, refused by name rather than silently replacing a sound.
pub const MAX_SOUNDS: usize = 16;

/// How many sounds may play at once. A ninth simultaneous sound steals
/// the oldest voice still playing — the standard behaviour for effects,
/// and audible as one sound cut short rather than as a missing sound.
pub const MAX_VOICES: usize = 8;

/// The most commands one callback drains. Equal to the ring's capacity,
/// so a callback empties whatever waits rather than leaving a backlog
/// that grows.
const DRAIN_BATCH: usize = ring::CAPACITY;

/// A sound loaded into a mixer. Opaque: it indexes that mixer's table
/// and means nothing to another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoundId(usize);

impl SoundId {
    /// The ring's tests need identifiers without a mixer to load into.
    #[cfg(test)]
    pub(crate) fn from_index_for_test(index: usize) -> Self {
        Self(index)
    }
}

/// What the mixer must know before it can convert anything: the shape
/// of the buffers it will be asked to fill.
///
/// Both fields come from the device, so a mixer is built after the
/// output is opened and before it starts — the one order that lets
/// loading do the conversion work.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct MixerConfig {
    /// Samples per frame in the buffers `fill` receives.
    pub channels: u16,
    /// Frames per second the device consumes.
    pub sample_rate: u32,
}

impl MixerConfig {
    /// A configuration for a device of `channels` and `sample_rate`.
    #[must_use]
    pub fn new(channels: u16, sample_rate: u32) -> Self {
        Self {
            channels,
            sample_rate,
        }
    }
}

/// One loaded sound: f32 samples already at the device's rate and
/// channel count, so playing one is an add and a cursor bump.
struct Sound {
    samples: Vec<f32>,
}

/// One playing sound.
#[derive(Clone, Copy)]
struct Voice {
    sound: usize,
    /// How many samples of `sound` have already been mixed.
    cursor: usize,
    /// When this voice started, in commands accepted. Steal takes the
    /// lowest — the oldest — and a counter costs nothing where a clock
    /// would be both banned and wrong.
    sequence: u64,
}

/// The half a game thread keeps: it can ask for sounds and nothing
/// else.
pub struct MixerHandle {
    commands: Producer,
}

impl MixerHandle {
    /// Begin playing `sound`.
    ///
    /// Returns whether the request was accepted; a full command ring
    /// drops it rather than blocking the caller. With sixty-four slots
    /// against eight voices that is unreachable in a real frame, but a
    /// dropped effect is the right answer when the alternative is a
    /// game thread waiting on an audio thread.
    #[must_use = "a dropped return hides a command the ring refused"]
    pub fn play(&self, sound: SoundId) -> bool {
        self.commands.push(Command::Play(sound))
    }
}

/// The half the audio callback owns.
pub struct Mixer {
    config: MixerConfig,
    sounds: Vec<Sound>,
    voices: [Option<Voice>; MAX_VOICES],
    commands: Consumer,
    /// Increments per accepted command; a voice keeps the value it was
    /// started with, so the lowest live sequence is the oldest voice.
    next_sequence: u64,
}

/// Build a mixer and the handle that feeds it.
///
/// The two halves share a fixed-capacity command ring. The mixer is
/// `Send` — it crosses to the audio thread once, when the caller seals
/// it into the fill callback — and the handle stays where it was made.
#[must_use]
pub fn mixer(config: MixerConfig) -> (MixerHandle, Mixer) {
    let (producer, consumer) = ring::channel();
    (
        MixerHandle { commands: producer },
        Mixer {
            config,
            sounds: Vec::with_capacity(MAX_SOUNDS),
            voices: [None; MAX_VOICES],
            commands: consumer,
            next_sequence: 0,
        },
    )
}

impl Mixer {
    /// Load `wav`, converting it once to the device's rate and channel
    /// count, and return the identifier that plays it.
    ///
    /// **Load time only.** This allocates and resamples, which is
    /// exactly what the callback must never do — so every sound a run
    /// will play is loaded before the mixer moves to the audio thread.
    ///
    /// # Panics
    ///
    /// Past [`MAX_SOUNDS`], by name: a caller loading a seventeenth
    /// sound has mis-sized its bank, and silently replacing one would
    /// make some later frame play the wrong effect for no visible
    /// reason.
    pub fn load(&mut self, wav: &Wav<'_>) -> SoundId {
        assert!(
            self.sounds.len() < MAX_SOUNDS,
            "sound capacity {MAX_SOUNDS} exceeded; size the bank for its game"
        );
        let samples = convert(wav, self.config);
        self.sounds.push(Sound { samples });
        SoundId(self.sounds.len() - 1)
    }

    /// Fill `out` with everything currently playing.
    ///
    /// `out` is interleaved at the configured channel count. The whole
    /// buffer is written every call — silence included — because a
    /// callback that returns without writing hands the device whatever
    /// was in the buffer before.
    ///
    /// Allocates nothing, never blocks, and cannot panic: every index
    /// is derived from the same fixed tables it walks.
    ///
    /// # Panics
    ///
    /// In dev builds, when `out` is not a whole number of frames. Every
    /// audio backend hands over whole frames, so this cannot happen
    /// through a device — but a voice advanced by a partial frame would
    /// land mid-frame and deliver that sound's channels swapped from
    /// then on, silently and permanently. A debug assertion is the
    /// cheapest place to catch a caller who fills by hand.
    pub fn fill(&mut self, out: &mut [f32]) {
        debug_assert!(
            out.len().is_multiple_of(self.config.channels as usize),
            "a fill buffer must be a whole number of frames: {} samples at {} channels",
            out.len(),
            self.config.channels
        );
        for sample in out.iter_mut() {
            *sample = 0.0;
        }
        self.start_pending_sounds();
        for slot in 0..self.voices.len() {
            let Some(voice) = self.voices[slot] else {
                continue;
            };
            let Some(sound) = self.sounds.get(voice.sound) else {
                // Unreachable: a voice only ever holds an index this
                // mixer handed out. Clearing rather than indexing keeps
                // the callback panic-free by construction anyway.
                self.voices[slot] = None;
                continue;
            };
            let remaining = sound.samples.len() - voice.cursor;
            let take = remaining.min(out.len());
            for (offset, sample) in out.iter_mut().take(take).enumerate() {
                *sample += sound.samples[voice.cursor + offset];
            }
            if take == remaining {
                // The sound ended inside this buffer; it contributed
                // its tail and the voice is free again.
                self.voices[slot] = None;
            } else {
                self.voices[slot] = Some(Voice {
                    cursor: voice.cursor + take,
                    ..voice
                });
            }
        }
        // Clamped once, at the end: several voices summing past full
        // scale is normal for effects, and clamping per-add would make
        // the result depend on the order the voices happen to sit in.
        for sample in out.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }

    /// Take whatever the game asked for since the last callback.
    ///
    /// The scratch buffer is a local: a fixed-size array of `Copy`
    /// commands lives on the stack, so this allocates nothing, and a
    /// field would only have added a swap to keep the borrow checker
    /// happy about draining into something the mixer owns.
    fn start_pending_sounds(&mut self) {
        let mut scratch = [None; DRAIN_BATCH];
        let taken = self.commands.drain(&mut scratch);
        for slot in scratch.iter().take(taken) {
            if let Some(Command::Play(sound)) = slot {
                self.start(*sound);
            }
        }
    }

    /// Put `sound` on a free voice, or on the oldest playing one.
    fn start(&mut self, sound: SoundId) {
        if self.sounds.get(sound.0).is_none() {
            // An identifier from another mixer. Ignoring it keeps the
            // callback total; the handle that produced it is a caller
            // bug the loud paths cannot reach from here.
            return;
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let voice = Voice {
            sound: sound.0,
            cursor: 0,
            sequence,
        };
        if let Some(free) = self.voices.iter().position(Option::is_none) {
            self.voices[free] = Some(voice);
            return;
        }
        // Every voice is busy: the oldest gives way. `min_by_key` keeps
        // the first of equal keys, and sequences are unique, so the
        // choice is exact.
        let oldest = self
            .voices
            .iter()
            .enumerate()
            .filter_map(|(slot, voice)| voice.map(|voice| (slot, voice.sequence)))
            .min_by_key(|(_, sequence)| *sequence)
            .map(|(slot, _)| slot);
        if let Some(slot) = oldest {
            self.voices[slot] = Some(voice);
        }
    }
}

/// Convert a parsed sound into f32 samples at `config`'s rate and
/// channel count.
///
/// Linear resampling, which is the honest choice for short effects: it
/// costs one multiply per sample and its artefacts sit above what a
/// flap or a coin can carry. A sound at the device's own rate skips it
/// entirely.
fn convert(wav: &Wav<'_>, config: MixerConfig) -> Vec<f32> {
    let source: Vec<f32> = wav.samples_f32().collect();
    let source_channels = wav.channels as usize;
    let target_channels = config.channels as usize;
    if source_channels == 0 || target_channels == 0 || source.is_empty() {
        return Vec::new();
    }
    let source_frames = source.len() / source_channels;
    // `Wav` is a public struct with public fields, and its own docs say
    // a hand-built one carries none of `parse`'s promises. So this
    // function answers for shapes a file could never contain: fewer
    // samples than one frame needs, and a rate of zero. Both would
    // otherwise reach arithmetic below that underflows or divides by
    // zero — a panic on a load path whose only documented refusal is
    // the sound-bank size.
    if source_frames == 0 || wav.sample_rate == 0 || config.sample_rate == 0 {
        return Vec::new();
    }
    let ratio = f64::from(config.sample_rate) / f64::from(wav.sample_rate);
    // At least one frame out for any non-empty input: a sound short
    // enough to round to nothing would otherwise vanish silently.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    let target_frames = ((source_frames as f64) * ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(target_frames * target_channels);
    for frame in 0..target_frames {
        // Where this output frame falls between two input frames.
        #[allow(clippy::cast_precision_loss)]
        let position = (frame as f64) / ratio;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let left = (position as usize).min(source_frames - 1);
        let right = (left + 1).min(source_frames - 1);
        // The fraction is in [0, 1); f32 carries it with room to
        // spare, and the loss the lint warns about is the point of
        // mixing in f32 at all.
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let blend = (position - left as f64) as f32;
        for channel in 0..target_channels {
            // Mono into stereo duplicates; stereo into mono takes the
            // left channel. Both at load, so the callback never asks
            // what shape a sound is.
            let source_channel = if source_channels == 1 {
                0
            } else {
                channel.min(source_channels - 1)
            };
            let a = source[left * source_channels + source_channel];
            let b = source[right * source_channels + source_channel];
            out.push(a + (b - a) * blend);
        }
    }
    out
}

#[cfg(test)]
impl Mixer {
    /// Voices currently playing — the steal test's only way to see the
    /// table's occupancy.
    pub(crate) fn live_voices(&self) -> usize {
        self.voices.iter().filter(|voice| voice.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SOUNDS, MAX_VOICES, MixerConfig, mixer};
    use crate::wav;

    /// A mono 8-bit-free PCM16 WAV of `frames` samples at `rate`, each
    /// sample the same value — enough for the mixer to have something
    /// with a known shape to play.
    fn wav_bytes(frames: usize, rate: u32, value: i16) -> Vec<u8> {
        let data_len = frames * 2;
        let mut bytes = Vec::with_capacity(44 + data_len);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&u32::try_from(36 + data_len).expect("small").to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&u32::try_from(data_len).expect("small").to_le_bytes());
        for _ in 0..frames {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    const RATE: u32 = 48_000;

    #[test]
    fn a_silent_mixer_writes_silence_over_whatever_was_there() {
        let (_handle, mut mix) = mixer(MixerConfig::new(2, RATE));
        let mut out = [1.0f32; 16];
        mix.fill(&mut out);
        assert!(
            out.iter().all(|sample| *sample == 0.0),
            "an idle callback must still write the whole buffer"
        );
    }

    #[test]
    fn a_played_sound_reaches_the_buffer_and_then_ends() {
        let bytes = wav_bytes(4, RATE, i16::MAX / 2);
        let parsed = wav::parse(&bytes).expect("a hand-built wav parses");
        let (handle, mut mix) = mixer(MixerConfig::new(1, RATE));
        let id = mix.load(&parsed);
        assert!(handle.play(id));

        let mut out = [0.0f32; 4];
        mix.fill(&mut out);
        assert!(out.iter().all(|sample| *sample > 0.4), "{out:?}");

        // The sound was four samples long: the next buffer is silent
        // again, and the voice freed itself without being told.
        let mut next = [0.0f32; 4];
        mix.fill(&mut next);
        assert!(next.iter().all(|sample| *sample == 0.0), "{next:?}");
    }

    #[test]
    fn a_sound_ending_mid_buffer_contributes_its_tail_and_stops() {
        let bytes = wav_bytes(2, RATE, i16::MAX / 2);
        let parsed = wav::parse(&bytes).expect("wav");
        let (handle, mut mix) = mixer(MixerConfig::new(1, RATE));
        let id = mix.load(&parsed);
        assert!(handle.play(id));

        let mut out = [0.0f32; 6];
        mix.fill(&mut out);
        assert!(out[0] > 0.4 && out[1] > 0.4, "the two samples played");
        assert!(
            out[2..].iter().all(|sample| *sample == 0.0),
            "and nothing past them: {out:?}"
        );
    }

    #[test]
    fn simultaneous_sounds_sum_and_the_sum_is_clamped() {
        let bytes = wav_bytes(4, RATE, i16::MAX);
        let parsed = wav::parse(&bytes).expect("wav");
        let (handle, mut mix) = mixer(MixerConfig::new(1, RATE));
        let id = mix.load(&parsed);
        for _ in 0..4 {
            assert!(handle.play(id));
        }
        let mut out = [0.0f32; 4];
        mix.fill(&mut out);
        assert!(
            out.iter()
                .all(|sample| (*sample - 1.0).abs() < f32::EPSILON),
            "four full-scale voices must saturate at one, not overflow: {out:?}"
        );
    }

    #[test]
    fn the_ninth_voice_steals_the_oldest_and_not_a_newer_one() {
        // Two sounds of very different lengths: the identity of the
        // stolen voice is then visible in what the buffer carries, not
        // just in how many voices are live — a count stays at eight
        // whichever voice is taken, which is why it cannot be the
        // assertion.
        let long = wav_bytes(4096, RATE, i16::MAX / 8);
        let short = wav_bytes(8, RATE, i16::MAX / 8);
        let long = wav::parse(&long).expect("wav");
        let short = wav::parse(&short).expect("wav");
        let (handle, mut mix) = mixer(MixerConfig::new(1, RATE));
        let long_id = mix.load(&long);
        let short_id = mix.load(&short);

        // The oldest voice is a long sound; the seven after it are the
        // short one, which ends inside the first buffer. So after one
        // fill, exactly one voice is still live — the long one — and
        // the ninth play must take it.
        assert!(handle.play(long_id));
        for _ in 1..MAX_VOICES {
            assert!(handle.play(short_id));
        }
        let mut out = [0.0f32; 8];
        mix.fill(&mut out);
        assert_eq!(
            mix.live_voices(),
            1,
            "the seven short sounds ended inside the buffer"
        );

        // Fill every slot again so the table is full, with the long
        // sound as the oldest survivor, then ask for one more.
        for _ in 0..(MAX_VOICES - 1) {
            assert!(handle.play(short_id));
        }
        assert!(handle.play(short_id));
        mix.fill(&mut out);
        assert_eq!(
            mix.live_voices(),
            0,
            "stealing the long voice leaves only short ones, which all ended"
        );
    }

    #[test]
    fn a_sound_at_another_rate_is_resampled_at_load() {
        // Half the device's rate: the same frames should cover twice
        // as many output samples.
        let bytes = wav_bytes(8, RATE / 2, i16::MAX / 2);
        let parsed = wav::parse(&bytes).expect("wav");
        let (handle, mut mix) = mixer(MixerConfig::new(1, RATE));
        let id = mix.load(&parsed);
        assert!(handle.play(id));
        let mut out = [0.0f32; 16];
        mix.fill(&mut out);
        assert!(
            out.iter().all(|sample| *sample > 0.4),
            "sixteen output samples from eight input frames: {out:?}"
        );
    }

    #[test]
    fn a_mono_sound_reaches_both_channels_of_a_stereo_device() {
        let bytes = wav_bytes(2, RATE, i16::MAX / 2);
        let parsed = wav::parse(&bytes).expect("wav");
        let (handle, mut mix) = mixer(MixerConfig::new(2, RATE));
        let id = mix.load(&parsed);
        assert!(handle.play(id));
        let mut out = [0.0f32; 4];
        mix.fill(&mut out);
        assert!(
            out.iter().all(|sample| *sample > 0.4),
            "both channels carry the mono source: {out:?}"
        );
    }

    #[test]
    fn the_sound_bank_refuses_the_load_past_its_size() {
        let bytes = wav_bytes(1, RATE, 1);
        let parsed = wav::parse(&bytes).expect("wav");
        let (_handle, mut mix) = mixer(MixerConfig::new(1, RATE));
        for _ in 0..MAX_SOUNDS {
            let _ = mix.load(&parsed);
        }
        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = mix.load(&parsed);
        }));
        assert!(
            refused.is_err(),
            "a seventeenth sound is a sizing bug, refused by name"
        );
    }

    // An identifier from another mixer indexes a table it was never
    // part of. Ignoring it keeps the callback total — the alternative
    // is a panic on the one thread that must never have one.
    #[test]
    fn an_identifier_from_another_mixer_plays_nothing() {
        let bytes = wav_bytes(4, RATE, i16::MAX / 2);
        let parsed = wav::parse(&bytes).expect("wav");
        let (_first_handle, mut first) = mixer(MixerConfig::new(1, RATE));
        let foreign = first.load(&parsed);
        // A second mixer with nothing loaded: the identifier is valid
        // in the first and means nothing here.
        let (second_handle, mut second) = mixer(MixerConfig::new(1, RATE));
        assert!(second_handle.play(foreign));
        let mut out = [0.0f32; 4];
        second.fill(&mut out);
        assert!(
            out.iter().all(|sample| *sample == 0.0),
            "an unknown sound must be silence, not a crash: {out:?}"
        );
    }

    #[test]
    fn a_sound_with_no_samples_at_all_loads_and_plays_silence() {
        let bytes = wav_bytes(0, RATE, 0);
        let parsed = wav::parse(&bytes).expect("an empty data chunk is a legal wav");
        let (handle, mut mix) = mixer(MixerConfig::new(1, RATE));
        let id = mix.load(&parsed);
        assert!(handle.play(id));
        let mut out = [0.0f32; 4];
        mix.fill(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0), "{out:?}");
    }

    /// A stereo PCM16 sound, so the per-channel path is exercised with
    /// two channels that carry different values.
    fn stereo_wav_bytes(frames: usize, rate: u32, left: i16, right: i16) -> Vec<u8> {
        let data_len = frames * 4;
        let mut bytes = Vec::with_capacity(44 + data_len);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&u32::try_from(36 + data_len).expect("small").to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&u32::try_from(data_len).expect("small").to_le_bytes());
        for _ in 0..frames {
            bytes.extend_from_slice(&left.to_le_bytes());
            bytes.extend_from_slice(&right.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_stereo_sound_keeps_its_channels_apart() {
        let bytes = stereo_wav_bytes(4, RATE, i16::MAX / 2, i16::MAX / 8);
        let parsed = wav::parse(&bytes).expect("wav");
        let (handle, mut mix) = mixer(MixerConfig::new(2, RATE));
        let id = mix.load(&parsed);
        assert!(handle.play(id));
        let mut out = [0.0f32; 8];
        mix.fill(&mut out);
        // Even indices are the left channel, odd the right: the louder
        // side must stay the louder side.
        for frame in 0..4 {
            assert!(
                out[frame * 2] > out[frame * 2 + 1] * 2.0,
                "frame {frame} lost its channel separation: {out:?}"
            );
        }
    }

    #[test]
    fn a_stereo_sound_folds_down_for_a_mono_device() {
        let bytes = stereo_wav_bytes(4, RATE, i16::MAX / 2, 0);
        let parsed = wav::parse(&bytes).expect("wav");
        let (handle, mut mix) = mixer(MixerConfig::new(1, RATE));
        let id = mix.load(&parsed);
        assert!(handle.play(id));
        let mut out = [0.0f32; 4];
        mix.fill(&mut out);
        assert!(
            out.iter().all(|sample| *sample > 0.4),
            "a mono device takes the left channel: {out:?}"
        );
    }

    // `Wav` is public with public fields, so `load` receives shapes
    // `parse` would never hand it. Each one below panicked before the
    // guard: a partial frame underflowed a frame count, and a zero rate
    // made the resampling ratio infinite and its frame count saturate.
    #[test]
    fn a_hand_built_sound_with_less_than_one_frame_loads_as_silence() {
        let partial = crate::wav::Wav {
            channels: 2,
            sample_rate: 44_100,
            samples: &[0, 0],
        };
        let (handle, mut mix) = mixer(MixerConfig::new(2, RATE));
        let id = mix.load(&partial);
        assert!(handle.play(id));
        let mut out = [0.0f32; 8];
        mix.fill(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0), "{out:?}");
    }

    #[test]
    fn a_hand_built_sound_with_no_rate_loads_as_silence() {
        let rateless = crate::wav::Wav {
            channels: 1,
            sample_rate: 0,
            samples: &[0, 0, 1, 0],
        };
        let (handle, mut mix) = mixer(MixerConfig::new(1, RATE));
        let id = mix.load(&rateless);
        assert!(handle.play(id));
        let mut out = [0.0f32; 4];
        mix.fill(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0), "{out:?}");
    }

    #[test]
    fn a_device_claiming_no_rate_at_all_mixes_silence_rather_than_hanging() {
        let bytes = wav_bytes(4, RATE, i16::MAX / 2);
        let parsed = wav::parse(&bytes).expect("wav");
        let (handle, mut mix) = mixer(MixerConfig::new(1, 0));
        let id = mix.load(&parsed);
        assert!(handle.play(id));
        let mut out = [0.0f32; 4];
        mix.fill(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0), "{out:?}");
    }

    #[test]
    fn a_dropped_handle_leaves_the_mixer_playing_what_it_has() {
        let bytes = wav_bytes(4, RATE, i16::MAX / 2);
        let parsed = wav::parse(&bytes).expect("wav");
        let (handle, mut mix) = mixer(MixerConfig::new(1, RATE));
        let id = mix.load(&parsed);
        assert!(handle.play(id));
        drop(handle);
        let mut out = [0.0f32; 4];
        mix.fill(&mut out);
        assert!(out.iter().all(|sample| *sample > 0.4), "{out:?}");
    }
}
