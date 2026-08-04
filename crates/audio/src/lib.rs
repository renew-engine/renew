//! Sound as arithmetic: a fixed-voice mixer, the command ring a game
//! thread pushes into, and a reader that turns WAV bytes into samples.
//!
//! # Contract
//!
//! - **Nothing here touches a device.** The mixer fills an interleaved
//!   `&mut [f32]` and stops. Carrying that buffer to speakers is the
//!   platform crate's business, behind its own feature, so this crate
//!   is testable, fuzzable, and buildable with no audio hardware
//!   anywhere in sight.
//! - **The callback path allocates nothing and cannot panic.** Every
//!   expensive act — decoding, resampling, laying samples out for the
//!   device — happens when a sound is loaded. What runs on the audio
//!   thread copies, adds, and clamps, indexing only tables it owns.
//! - **The audio thread never waits.** Commands cross on a
//!   fixed-capacity ring; the mixer tries for the lock and, if the game
//!   thread holds it, mixes what it already has and takes the commands
//!   next callback. A few milliseconds of latency on an effect is
//!   inaudible; a missed buffer deadline is not.
//! - **Nothing a file claims is believed.** [`wav::parse`] validates
//!   every header field against the bytes actually present and refuses
//!   by name, so a malformed sound is a named error rather than a
//!   wrong-shaped read.
//! - **No clocks.** A voice advances by the samples it was asked for,
//!   and voice-stealing order comes from a sequence number the mixer
//!   assigns — so a slow frame changes nothing about what is heard,
//!   only when.

// Diagnostics go through sinks; the standard output macros are banned
// in this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod mixer;
mod ring;
pub mod wav;

pub use mixer::{MAX_SOUNDS, MAX_VOICES, Mixer, MixerConfig, MixerHandle, SoundId, mixer};
