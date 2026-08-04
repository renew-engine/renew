//! Audio output — the sound-card seam, behind the `audio-out` feature.
//! The operating system owns the audio thread and calls back on its own
//! schedule (that is how every desktop platform wants it); whatever
//! produces sound stays a plain library that fills a buffer of
//! interleaved `f32` samples. No audio-library type crosses this
//! boundary: consumers see only the vocabulary below.
//!
//! Bring-up is two phases on purpose. [`AudioDevice::open`] negotiates
//! and reports the channel count and sample rate; only then does the
//! caller build whatever produces samples at that shape and hand it to
//! [`AudioDevice::start`]. A single call taking the producer up front
//! would need that producer built before the shape it has to produce is
//! known.
//!
//! Machines with no sound card, no backend, or no permission fail
//! recoverably with [`AudioError::Unavailable`] — the same graceful-skip
//! seam the window loop offers headless callers, so a silent run is a
//! reported outcome instead of a crash.

use core::fmt;
use core::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Records from this seam are filed under the crate rather than the
/// module path: they are emitted from the operating system's audio
/// thread, where no engine frame is in scope, and a reader filtering a
/// log wants the doorway they came through.
const TARGET: &str = "renew-platform";

/// The stream shape a device agreed to: interleaved frames of
/// `channels` `f32` samples, `sample_rate` frames per second.
///
/// Plain data, and deliberately only these two numbers — everything
/// that produces samples is built against them and needs nothing else
/// from the audio stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioConfig {
    pub channels: u16,
    pub sample_rate: u32,
}

/// Why audio could not be opened or started.
#[derive(Debug)]
#[non_exhaustive]
pub enum AudioError {
    /// No device, no backend, no permission — the machine-not-request
    /// seam, mirroring the window seam's loop-unavailable case.
    /// Recoverable by design: callers report it and run silent.
    Unavailable { message: String },
    /// The device is there and offers no `f32` output configuration.
    /// A stated limitation, not a machine failure: this seam carries
    /// `f32` and nothing else.
    FormatUnsupported { message: String },
    /// Building or starting the stream failed.
    Stream { message: String },
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { message } => write!(f, "audio output unavailable: {message}"),
            Self::FormatUnsupported { message } => {
                write!(f, "audio output format unsupported: {message}")
            }
            Self::Stream { message } => write!(f, "audio stream failed: {message}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// A negotiated output device: which device, and the shape it agreed
/// to. Nothing is playing yet — no callback fires until
/// [`start`](Self::start).
pub struct AudioDevice {
    device: cpal::Device,
    config: AudioConfig,
}

impl AudioDevice {
    /// Take the system's default output device and negotiate a stream
    /// shape for it.
    ///
    /// **The default device, and only the default device.** There is no
    /// enumeration API here: choosing between devices is a
    /// user-interface question, and inventing the vocabulary for one
    /// before anything asks would be surface with no consumer.
    ///
    /// The shape is negotiated in one order, stated rather than left to
    /// whoever reads this next: the device's own default configuration
    /// when that is already `f32`; otherwise the first `f32`
    /// configuration range the device lists, taken at that range's
    /// maximum sample rate; otherwise a refusal by name. A range is a
    /// range, so somebody has to pick the rate, and whatever produces
    /// samples is built against the answer either way.
    ///
    /// Constructing the audio host is the audio library's own call, and
    /// its contract treats each platform's default host as always
    /// present: on a machine whose backend cannot initialise at all it
    /// ends the process there rather than handing back a value.
    /// Everything this seam can refuse, it refuses as a value.
    ///
    /// # Errors
    ///
    /// [`AudioError::Unavailable`] when the host has no default output
    /// device, or when the device answers neither question about its
    /// configurations; [`AudioError::FormatUnsupported`] when the
    /// device answers and offers no `f32` configuration.
    pub fn open() -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| AudioError::Unavailable {
                message: "the audio host reports no default output device".to_string(),
            })?;
        let default_config = device.default_output_config();
        let ranges = device.supported_output_configs();
        // A device that answers nothing about itself is gone, not
        // merely `f32`-less. Naming the format seam here would name the
        // wrong cause on exactly the machines that have no sound card —
        // the ones where the answer has to be legible.
        if let (Err(config_error), Err(range_error)) = (&default_config, &ranges) {
            // The device is deliberately NOT named here. A device that
            // answers nothing about itself usually cannot describe
            // itself either, and the sound library's `Display` for a
            // device is fallible — a formatting error, which `format!`
            // turns into a panic. Naming the endpoint would therefore
            // crash on precisely the machines this arm exists to report
            // on: the one whose speakers were unplugged between the
            // moment the default device was taken and the moment it was
            // asked what it supports.
            return Err(AudioError::Unavailable {
                message: format!(
                    "the default output device answered no configuration query: \
                     {config_error}; {range_error}"
                ),
            });
        }
        // `Result` iterates its `Ok` value or nothing, so a device that
        // refuses to enumerate still gets the default-configuration rung
        // of the ladder rather than an early exit.
        let chosen = choose_output_config(default_config.ok(), ranges.into_iter().flatten())
            .ok_or_else(|| AudioError::FormatUnsupported {
                message: format!("output device `{device}` offers no f32 output configuration"),
            })?;
        Ok(Self {
            config: AudioConfig {
                channels: chosen.channels(),
                sample_rate: chosen.sample_rate(),
            },
            device,
        })
    }

    /// The shape the device agreed to.
    #[must_use]
    pub fn config(&self) -> AudioConfig {
        self.config
    }

    /// Build the output stream and start it playing.
    ///
    /// `fill` runs on the operating system's audio thread, on that
    /// thread's schedule, and must write the whole slice on every call —
    /// interleaved frames at the negotiated channel count, silence
    /// included, because whatever the callback leaves behind is what the
    /// speakers get. **It must never panic, allocate, or block**: the
    /// callback has a hard deadline and missing it is audible. Anything
    /// it needs is captured before it is handed over, which is why
    /// [`open`](Self::open) reports the shape first.
    ///
    /// The buffer size is the platform's own default: v0 accepts the
    /// latency the host picks, and asking for a size is a builder for
    /// the day something measures the difference. Backend start-up is
    /// waited on without a deadline of this seam's own — a timeout there
    /// is a latency policy nobody has asked for.
    ///
    /// # Errors
    ///
    /// [`AudioError::Stream`] when the stream cannot be built at the
    /// negotiated shape, or cannot be started once built.
    pub fn start(
        self,
        mut fill: impl FnMut(&mut [f32]) + Send + 'static,
    ) -> Result<AudioOutput, AudioError> {
        let healthy = Arc::new(AtomicBool::new(true));
        let reported = Arc::clone(&healthy);
        let config = cpal::StreamConfig {
            channels: self.config.channels,
            sample_rate: self.config.sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let stream = self
            .device
            .build_output_stream::<f32, _, _>(
                config,
                // The timing information the audio library offers each
                // callback is dropped here deliberately: nothing in this
                // engine schedules against a stream clock, and passing
                // it on would put an audio-library type in the seam.
                move |out, _timing| fill(out),
                move |error| report(&reported, &error),
                None,
            )
            .map_err(|error| AudioError::Stream {
                message: format!("building the output stream failed: {error}"),
            })?;
        // Streams are built stopped; nothing is heard until this call.
        stream.play().map_err(|error| AudioError::Stream {
            message: format!("starting the output stream failed: {error}"),
        })?;
        Ok(AudioOutput {
            stream,
            healthy,
            unsendable: PhantomData,
        })
    }
}

/// A live output stream.
///
/// **The stream is the playing, not a handle to it**: dropping this
/// value stops the callbacks, so it has to be kept for as long as
/// anything should be heard. The audio library owns its thread's
/// lifecycle beyond that point, and no join is claimed here.
///
/// **Not `Send`, deliberately — stricter than the audio library
/// requires.** A live stream belongs to the thread that started it: its
/// drop is what stops the callbacks, and the backends underneath carry
/// thread affinities of their own. Nothing in this engine needs to move
/// one between threads, so this seam declines to answer which thread may
/// tear one down rather than answering it wrongly.
pub struct AudioOutput {
    /// Kept for its lifetime, never read: see the type's contract.
    #[expect(dead_code, reason = "dropping the stream is what stops playback")]
    stream: cpal::Stream,
    healthy: Arc<AtomicBool>,
    /// The `!Send` marker. A raw-pointer phantom is the standard way to
    /// say it: it holds nothing and points nowhere.
    unsendable: PhantomData<*const ()>,
}

impl AudioOutput {
    /// Whether the stream is still playing, as far as it has reported.
    ///
    /// False only once a **fatal** report has arrived. A route change or
    /// an underrun leaves this true, because the stream is still playing
    /// after both — latching on either would report a muted run over
    /// audible sound.
    ///
    /// This is news from another thread, so it is a snapshot: `true`
    /// means nothing fatal had been reported at the moment it was read.
    #[must_use]
    pub fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }
}

/// The negotiation ladder, over the two answers a device gives about its
/// output configurations: the device's default configuration when that
/// is already `f32`, else the first `f32` range it lists taken at that
/// range's maximum sample rate, else nothing.
///
/// Pure over the two answers rather than over a device, because a build
/// machine has no sound card: the fallback rung and the refusal are
/// provable only away from real hardware.
fn choose_output_config(
    default_config: Option<cpal::SupportedStreamConfig>,
    mut supported: impl Iterator<Item = cpal::SupportedStreamConfigRange>,
) -> Option<cpal::SupportedStreamConfig> {
    if let Some(config) = default_config
        && config.sample_format() == cpal::SampleFormat::F32
    {
        return Some(config);
    }
    supported
        .find(|range| range.sample_format() == cpal::SampleFormat::F32)
        .map(cpal::SupportedStreamConfigRange::with_max_sample_rate)
}

/// What one report from the stream's error callback means for the stream
/// that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    /// The audio route changed and the host rerouted the stream, which
    /// keeps playing.
    Rerouted,
    /// The host missed a buffer deadline and recovered by itself.
    Underrun,
    /// The stream is gone; whatever runs on continues silent.
    Fatal,
}

/// Classify one reported error kind.
///
/// This distinction is the whole reason the callback does more than
/// latch a flag. A route change is what plugging in headphones mid-game
/// looks like, and the audio library's own contract for it is that the
/// stream remains active with no rebuild required; an underrun is a
/// glitch the host recovers from. Treating either as death would report
/// a muted run while sound kept playing.
///
/// Every other kind — including kinds added to the library after this
/// was written, which is why the last arm is open — is taken as death.
/// A false alarm is legible; a stream that is silently gone is not.
fn classify(kind: cpal::ErrorKind) -> Report {
    match kind {
        cpal::ErrorKind::DeviceChanged => Report::Rerouted,
        cpal::ErrorKind::Xrun => Report::Underrun,
        _ => Report::Fatal,
    }
}

/// Handle one report from the stream: say what happened, and latch the
/// health flag when — and only when — the stream is actually gone.
///
/// This runs on the audio library's thread, **inside** the loop that
/// services buffers — the backends report an underrun and then recover
/// on the next iteration, so a slow report here is a report delivered
/// between a missed period and its repair. The diagnostic call is still
/// the right thing: the engine's diag emit path defers formatting to
/// whichever sink is installed and allocates nothing itself. What that
/// buys depends on the sink, and a sink that formats into a `String`
/// and takes a lock would be doing it here, on this thread. Said out
/// loud because the earlier note claimed this ran outside the deadline,
/// which the backends' own loops disprove.
fn report(healthy: &AtomicBool, error: &cpal::Error) {
    match classify(error.kind()) {
        Report::Rerouted => {
            renew_diag::info!(target: TARGET, "audio route changed, the stream plays on: {error}");
        }
        Report::Underrun => {
            renew_diag::warn!(target: TARGET, "audio buffer underrun, the host recovers: {error}");
        }
        Report::Fatal => {
            renew_diag::error!(target: TARGET, "audio stream lost, playback is over: {error}");
            // Relaxed: the flag carries nothing but itself, and its
            // reader wants the news, not an ordering against other
            // writes.
            healthy.store(false, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::{
        SampleFormat, SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange,
    };

    /// One entry of a hand-built supported-configuration list. Buffer
    /// sizes play no part in the ladder, so every fixture reports the
    /// least informative answer a backend can give.
    fn range(format: SampleFormat, channels: u16, rates: (u32, u32)) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            channels,
            rates.0,
            rates.1,
            SupportedBufferSize::Unknown,
            format,
        )
    }

    /// A hand-built default configuration.
    fn config(format: SampleFormat, channels: u16, rate: u32) -> SupportedStreamConfig {
        SupportedStreamConfig::new(channels, rate, SupportedBufferSize::Unknown, format)
    }

    #[test]
    fn an_f32_default_is_taken_before_anything_is_enumerated() {
        let default = config(SampleFormat::F32, 2, 48_000);
        let chosen = choose_output_config(
            Some(default),
            // A list that would win on rate if it were ever consulted.
            [range(SampleFormat::F32, 8, (44_100, 192_000))].into_iter(),
        );
        assert_eq!(
            chosen,
            Some(default),
            "the device's own default is the first rung"
        );
    }

    #[test]
    fn a_non_f32_default_falls_back_to_the_first_f32_range_at_its_top_rate() {
        let chosen = choose_output_config(
            Some(config(SampleFormat::I16, 2, 48_000)),
            [
                // Not f32: skipped however attractive it looks.
                range(SampleFormat::I16, 2, (8_000, 192_000)),
                range(SampleFormat::F32, 2, (44_100, 96_000)),
                // A later f32 range never displaces the first.
                range(SampleFormat::F32, 6, (48_000, 192_000)),
            ]
            .into_iter(),
        );
        let chosen = chosen.expect("an f32 range is listed");
        assert_eq!(chosen.sample_format(), SampleFormat::F32);
        assert_eq!(chosen.channels(), 2, "the first f32 range wins");
        assert_eq!(
            chosen.sample_rate(),
            96_000,
            "a range is taken at its maximum rate"
        );
    }

    #[test]
    fn a_device_with_no_default_configuration_still_reaches_the_list() {
        let chosen = choose_output_config(
            None,
            [range(SampleFormat::F32, 1, (22_050, 44_100))].into_iter(),
        );
        assert_eq!(
            chosen,
            Some(config(SampleFormat::F32, 1, 44_100)),
            "no default answer is not a refusal"
        );
    }

    #[test]
    fn a_device_offering_no_f32_anywhere_is_refused() {
        assert_eq!(
            choose_output_config(
                Some(config(SampleFormat::I16, 2, 48_000)),
                [
                    range(SampleFormat::I16, 2, (44_100, 48_000)),
                    range(SampleFormat::U16, 2, (44_100, 48_000)),
                    range(SampleFormat::I24, 2, (44_100, 192_000)),
                ]
                .into_iter(),
            ),
            None,
            "v0 carries f32 only, and says so rather than converting"
        );
        assert_eq!(
            choose_output_config(None, std::iter::empty()),
            None,
            "a device that offers nothing at all is refused too"
        );
    }

    #[test]
    fn a_route_change_and_an_underrun_are_not_deaths_and_everything_else_is() {
        assert_eq!(classify(cpal::ErrorKind::DeviceChanged), Report::Rerouted);
        assert_eq!(classify(cpal::ErrorKind::Xrun), Report::Underrun);
        for kind in [
            cpal::ErrorKind::DeviceBusy,
            cpal::ErrorKind::DeviceNotAvailable,
            cpal::ErrorKind::HostUnavailable,
            cpal::ErrorKind::InvalidInput,
            cpal::ErrorKind::PermissionDenied,
            cpal::ErrorKind::RealtimeDenied,
            cpal::ErrorKind::ResourceExhausted,
            cpal::ErrorKind::StreamInvalidated,
            cpal::ErrorKind::UnsupportedConfig,
            cpal::ErrorKind::UnsupportedOperation,
            cpal::ErrorKind::BackendError,
            cpal::ErrorKind::Other,
        ] {
            assert_eq!(classify(kind), Report::Fatal, "{kind:?}");
        }
    }

    #[test]
    fn only_a_fatal_report_latches_the_health_flag() {
        let healthy = AtomicBool::new(true);
        // The survivable reports, delivered through exactly the function
        // the stream calls — including its diagnostics, which reach no
        // sink here and must stay a silent no-op.
        report(&healthy, &cpal::Error::new(cpal::ErrorKind::DeviceChanged));
        assert!(
            healthy.load(Ordering::Relaxed),
            "a rerouted stream is still playing"
        );
        report(&healthy, &cpal::Error::new(cpal::ErrorKind::Xrun));
        assert!(
            healthy.load(Ordering::Relaxed),
            "a recovered underrun is still playing"
        );
        report(
            &healthy,
            &cpal::Error::with_message(cpal::ErrorKind::DeviceNotAvailable, "unplugged"),
        );
        assert!(!healthy.load(Ordering::Relaxed), "a lost stream is not");
        // The latch is one-way: a survivable report afterwards must not
        // resurrect a stream that is gone.
        report(&healthy, &cpal::Error::new(cpal::ErrorKind::Xrun));
        assert!(!healthy.load(Ordering::Relaxed), "the latch holds");
    }

    #[test]
    fn every_variant_displays_its_context() {
        let cases = [
            (
                AudioError::Unavailable {
                    message: "no default output device".to_string(),
                }
                .to_string(),
                "no default output device",
            ),
            (
                AudioError::FormatUnsupported {
                    message: "offers no f32 output configuration".to_string(),
                }
                .to_string(),
                "f32",
            ),
            (
                AudioError::Stream {
                    message: "backend refused the shape".to_string(),
                }
                .to_string(),
                "backend refused the shape",
            ),
        ];
        for (text, needle) in cases {
            assert!(text.contains(needle), "`{text}` missing `{needle}`");
        }
    }
}
