//! Opening an audio device on whatever machine this is.
//!
//! Two environments, one contract. On a developer's machine a device
//! usually opens, and this reports what it negotiated and stops — the
//! sound a game makes is not something a test can assert. On a runner
//! with no sound card the seam's whole promise is that the absence is
//! *reported* rather than fatal, and there `RENEW_AUDIO_ABSENT=1` turns
//! that promise into an assertion.
//!
//! The variable exists because the assertion is only meaningful where
//! the absence is known. A test that hard-asserted "no device" would
//! pass on the runner and redden on every machine with speakers,
//! including the one whose owner is listening for flap and score; a
//! test that asserted nothing would be a green check measuring nothing.
//! The environment says which of the two situations it is, and it is
//! set in exactly one CI cell.

use renew_platform::audio::{AudioDevice, AudioError};

#[test]
fn a_machine_without_a_sound_card_is_reported_and_not_fatal() {
    let known_absent = std::env::var_os("RENEW_AUDIO_ABSENT").is_some_and(|value| value == "1");
    match AudioDevice::open() {
        Ok(device) => {
            let config = device.config();
            assert!(
                !known_absent,
                "this environment declared it has no audio device, and one opened at \
                 {} Hz on {} channel(s) — the declaration or the seam is wrong",
                config.sample_rate, config.channels
            );
            assert!(
                config.channels >= 1,
                "a device that opened must carry at least one channel"
            );
            assert!(
                config.sample_rate >= 8_000,
                "a device that opened must carry a plausible rate, got {}",
                config.sample_rate
            );
            eprintln!(
                "audio: {} Hz, {} channel(s)",
                config.sample_rate, config.channels
            );
        }
        Err(AudioError::Unavailable { message }) => {
            // The seam's designed answer for a machine that cannot
            // play. Reaching it is the point of the strict lane.
            eprintln!("audio: no device here ({message})");
        }
        Err(other) => {
            assert!(
                !known_absent,
                "a machine with no audio device must answer Unavailable so the caller can \
                 tell 'no sound card' from 'something broke'; this answered {other}"
            );
            eprintln!("audio: unavailable in an unexpected shape ({other})");
        }
    }
}
