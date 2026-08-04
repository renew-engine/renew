//! The three committed sounds, put through the reader that will read
//! them.
//!
//! `Audio::open` parses these on its way to a device, which means every
//! check they get today happens on a machine with speakers — not on any
//! lane. So a regenerated sound, or a tightening of the reader's
//! accepted set, could ship a game that starts silently and reports
//! only that "a committed sound was refused". This test is the lane's
//! copy of that parse, with none of the device in the way.
//!
//! It also pins what the generator claims about its own output, so the
//! record beside the bytes cannot drift from the bytes.

use renew_audio::wav;

/// Every sound the game ships, and what the generator says it wrote.
const SOUNDS: &[(&str, &[u8])] = &[
    ("flap", include_bytes!("../sounds/flap.wav")),
    ("score", include_bytes!("../sounds/score.wav")),
    ("death", include_bytes!("../sounds/death.wav")),
];

/// The rate the generator writes at.
const RATE: u32 = 22_050;

#[test]
fn every_committed_sound_parses_as_what_the_generator_says_it_is() {
    for (name, bytes) in SOUNDS {
        let sound = match wav::parse(bytes) {
            Ok(sound) => sound,
            Err(error) => panic!("the committed sound `{name}` was refused: {error}"),
        };
        assert_eq!(sound.channels, 1, "`{name}` should be mono");
        assert_eq!(sound.sample_rate, RATE, "`{name}` should be at {RATE} Hz");
        assert!(
            !sound.samples.is_empty(),
            "`{name}` carries no samples at all, which is a silent effect"
        );
        // Whole frames, which the reader already requires — asserted
        // here too because the mixer's frame arithmetic depends on it
        // and this is the file that would break first.
        assert_eq!(
            sound.samples.len() % 2,
            0,
            "`{name}` is not a whole number of 16-bit samples"
        );
        // Long enough to be heard: a few milliseconds of anything is a
        // click, and every one of these is meant to be a sound.
        let frames = sound.samples.len() / 2;
        assert!(
            frames > RATE as usize / 50,
            "`{name}` is {frames} frames, under 20 ms — too short to hear as anything"
        );
    }
}

#[test]
fn the_sounds_decode_to_something_audible() {
    for (name, bytes) in SOUNDS {
        let sound = wav::parse(bytes).expect("a committed sound parses");
        let peak = sound
            .samples_f32()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(
            peak > 0.1,
            "`{name}` peaks at {peak}, which is silence with extra steps"
        );
        assert!(
            peak <= 1.0,
            "`{name}` peaks at {peak}, past full scale — the generator's clamp failed"
        );
    }
}
