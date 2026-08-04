//! Writes the three sounds the game plays.
//!
//! The bytes are committed beside this file, and so is the generator
//! that made them: a sound whose producer has been deleted is a magic
//! number nobody can change. Wanting a louder flap, a fourth effect, or
//! a re-encode after the reader's contract moves should mean editing a
//! formula here and running one command, not reverse-engineering a
//! waveform.
//!
//! Deterministic by construction: no clock, no randomness, no floating
//! point that depends on anything but the constants below. Running this
//! twice writes byte-identical files, which is what lets the committed
//! bytes be reviewed as the output of the source beside them.
//!
//! Run it from the repository root:
//!
//! ```text
//! cargo run -p renew-sample-glide --example make_sounds
//! ```

// Every sample index and frame count here is at most a few tens of
// thousands, so the conversions to f32 below are exact — the lint's
// warning is about magnitudes this generator never reaches, and the
// alternative (f64 throughout, or checked conversions per sample) buys
// nothing a reader can hear.
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use std::io::Write;

/// The rate every sound is written at. The mixer resamples to whatever
/// the device wants, so this is chosen for size rather than fidelity:
/// these are blips, and 22.05 kHz carries them with nothing to spare
/// and nothing wasted.
const RATE: u32 = 22_050;

/// Where the bytes land, relative to the repository root.
const DIRECTORY: &str = "samples/glide/sounds";

fn main() -> std::io::Result<()> {
    std::fs::create_dir_all(DIRECTORY)?;
    write("flap.wav", &flap())?;
    write("score.wav", &score())?;
    write("death.wav", &death())?;
    Ok(())
}

/// The flap: a short downward chirp, the sound of a wing pushing air.
/// Brief enough to fire every few frames without becoming a drone.
fn flap() -> Vec<i16> {
    let frames = RATE as usize * 9 / 100;
    (0..frames)
        .map(|frame| {
            let t = frame as f32 / RATE as f32;
            let progress = frame as f32 / frames as f32;
            // 520 Hz falling to 180: the pitch drop is what makes it
            // read as a push rather than a beep.
            let hz = 520.0 - 340.0 * progress;
            let envelope = (1.0 - progress).powi(2);
            sample(triangle(hz * t) * envelope * 0.55)
        })
        .collect()
}

/// The score: two rising tones, the universal "you got one".
fn score() -> Vec<i16> {
    let frames = RATE as usize * 18 / 100;
    let half = frames / 2;
    (0..frames)
        .map(|frame| {
            let t = frame as f32 / RATE as f32;
            let progress = frame as f32 / frames as f32;
            // A perfect fourth up at the halfway point; the second
            // tone's envelope restarts so the two are distinct rather
            // than one sound bending.
            let (hz, local) = if frame < half {
                (660.0, frame as f32 / half as f32)
            } else {
                (880.0, (frame - half) as f32 / half as f32)
            };
            let envelope = (1.0 - local).powi(2) * (1.0 - progress * 0.3);
            sample(triangle(hz * t) * envelope * 0.5)
        })
        .collect()
}

/// The death: a longer descending buzz. Square-ish and detuned, so it
/// lands as a failure rather than as another note in the melody.
fn death() -> Vec<i16> {
    let frames = RATE as usize * 45 / 100;
    (0..frames)
        .map(|frame| {
            let t = frame as f32 / RATE as f32;
            let progress = frame as f32 / frames as f32;
            let hz = 300.0 - 220.0 * progress;
            // The second voice sits a few Hz off the first; the beating
            // between them is the roughness that reads as "wrong".
            let body = square(hz * t) * 0.6 + square((hz + 7.0) * t) * 0.4;
            let envelope = (1.0 - progress).powi(1);
            sample(body * envelope * 0.45)
        })
        .collect()
}

/// A triangle wave at `cycles` complete cycles. Softer than a square
/// and cheaper than anything band-limited, which is the right trade for
/// sounds this short.
fn triangle(cycles: f32) -> f32 {
    let phase = cycles.fract();
    if phase < 0.5 {
        4.0 * phase - 1.0
    } else {
        3.0 - 4.0 * phase
    }
}

/// A square wave at `cycles` complete cycles.
fn square(cycles: f32) -> f32 {
    if cycles.fract() < 0.5 { 1.0 } else { -1.0 }
}

/// One f32 in `[-1, 1]` as a 16-bit sample, clamped rather than wrapped
/// — a wrapped overload is a click, and a clamped one is a limit.
fn sample(value: f32) -> i16 {
    let clamped = value.clamp(-1.0, 1.0);
    (clamped * f32::from(i16::MAX)) as i16
}

/// Write `samples` as a mono PCM16 WAV.
///
/// Hand-built rather than through a library: the whole point of this
/// file is that the bytes have one visible source, and a dependency
/// that emitted a `LIST` chunk or a padding byte would be a fourth
/// party to the format the reader accepts.
fn write(name: &str, samples: &[i16]) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    bytes.extend_from_slice(&2u16.to_le_bytes()); // block align
    bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    let path = format!("{DIRECTORY}/{name}");
    let mut file = std::fs::File::create(&path)?;
    file.write_all(&bytes)?;
    println!("{path}: {} bytes", bytes.len());
    Ok(())
}
