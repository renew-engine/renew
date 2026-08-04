//! The allocation contract, pinned: once its sounds are loaded, a
//! mixer fills buffers and starts voices without touching the heap.
//!
//! Own process on purpose (the counters are process-wide); single test
//! so no sibling allocates alongside. Measurement protocol: warmup
//! first, so lazy initialization is measured out, then retry windows —
//! the counters see every thread in the process, including the test
//! harness's own, so one-shot neighbour noise rides out while a
//! genuine per-callback allocation reproduces in every window and
//! still fails.
//!
//! This is the gate the whole design bends around. A mixer that
//! allocates on the audio thread does not merely run slowly: it takes
//! a lock inside the allocator on a thread with a hard deadline, and
//! the symptom is a click nobody can reproduce. Every expensive act
//! belongs to `load`, which runs before the stream starts, and this
//! test is what keeps it there.

use renew_audio::{MixerConfig, mixer, wav};
use renew_memory::CountingAllocator;
use renew_memory::counters::quiet_window;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const RATE: u32 = 48_000;

/// A mono PCM16 sound of `frames` samples at `rate`, built by hand so
/// the gate needs no fixture file.
fn wav_bytes(frames: u32, rate: u32, value: i16) -> Vec<u8> {
    let data_len = frames * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
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
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for index in 0..frames {
        let sample = if index % 2 == 0 { value } else { -value };
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
fn a_loaded_mixer_fills_buffers_without_touching_the_heap() {
    let short_bytes = wav_bytes(64, RATE, i16::MAX / 4);
    let long_bytes = wav_bytes(4096, RATE, i16::MAX / 8);
    let odd_rate_bytes = wav_bytes(128, 22_050, i16::MAX / 4);
    let short = wav::parse(&short_bytes).expect("hand-built wav");
    let long = wav::parse(&long_bytes).expect("hand-built wav");
    let odd_rate = wav::parse(&odd_rate_bytes).expect("hand-built wav");

    // Everything that allocates happens out here: parsing borrows, but
    // loading converts and resamples into owned buffers, and the
    // output buffer is the caller's.
    let (handle, mut mix) = mixer(MixerConfig::new(2, RATE));
    let short_id = mix.load(&short);
    let long_id = mix.load(&long);
    let resampled_id = mix.load(&odd_rate);
    let mut out = vec![0.0f32; 1024];

    // Warmup: the first callbacks may touch lazily initialized state.
    for _ in 0..4 {
        assert!(handle.play(short_id));
        mix.fill(&mut out);
    }

    let verdict = quiet_window(5, || {
        for round in 0..16 {
            // A frame's worth of events, then the callback that
            // consumes them — including the voice-stealing path, which
            // is where a naive implementation would reach for a Vec.
            assert!(handle.play(short_id));
            assert!(handle.play(long_id));
            assert!(handle.play(resampled_id));
            if round % 4 == 0 {
                for _ in 0..8 {
                    assert!(handle.play(short_id));
                }
            }
            mix.fill(&mut out);
        }
    });

    if let Err(activity) = verdict {
        panic!("the mixer was loud in every window (last: {activity})");
    }

    // A silent window would satisfy the counters perfectly, so the
    // measurement is only worth its name if something was mixed inside
    // it. One more play-and-fill on the same path, checked.
    assert!(handle.play(short_id));
    let mut probe = vec![0.0f32; 1024];
    mix.fill(&mut probe);
    assert!(
        probe.iter().any(|sample| *sample != 0.0),
        "the gate measured a path that produces no sound, which measures nothing"
    );
}
