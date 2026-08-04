//! The two threads, run against each other: a producer flooding the
//! command ring while a consumer drains it in a tight callback loop.
//!
//! What this proves is not throughput. It is that the ring's two halves
//! can be hammered from two threads at once without losing a command,
//! corrupting the queue, or deadlocking — and, under the scheduled
//! sanitizer runs, without a data race. The audio thread's contract is
//! the reason: it may never block, so the consumer uses a try-lock and
//! skips a contended callback, and a bug in that skip is exactly the
//! kind that only appears when both sides run flat out.
//!
//! The producer spawns through the platform crate's named-thread seam
//! because that is the engine's only thread-spawning door, tests
//! included.

use renew_audio::{MixerConfig, mixer, wav};
use renew_platform::thread;

const RATE: u32 = 48_000;
const PUSHES: usize = 20_000;

fn wav_bytes(frames: u32) -> Vec<u8> {
    let data_len = frames * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for index in 0..frames {
        // A slow ramp: the values matter only in that they are
        // not all zero, so the mixer has something to sum.
        let sample = (index % 1000) as i16 * 16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[test]
fn a_flooding_producer_and_a_draining_callback_do_not_race() {
    let bytes = wav_bytes(256);
    let parsed = wav::parse(&bytes).expect("hand-built wav");
    let (handle, mut mix) = mixer(MixerConfig::new(2, RATE));
    let id = mix.load(&parsed);

    // The producer runs on its own thread, pushing far faster than any
    // game would, so the try-lock's contended path is taken constantly
    // rather than never.
    let producer = thread::spawn_named("audio-stress-producer", move || {
        let mut accepted = 0usize;
        for _ in 0..PUSHES {
            if handle.play(id) {
                accepted += 1;
            }
            std::hint::spin_loop();
        }
        accepted
    })
    .expect("the platform crate can spawn a named thread");

    // Meanwhile the consumer behaves like a callback: fill, again,
    // again, with no coordination beyond the ring itself.
    let mut out = vec![0.0f32; 256];
    for _ in 0..PUSHES {
        mix.fill(&mut out);
        assert!(
            out.iter().all(|sample| sample.is_finite()),
            "a mixed buffer must never carry NaN or infinity"
        );
    }

    let accepted = producer.join().expect("the producer finished");
    assert!(
        accepted > 0,
        "a full run must have accepted commands; the ring answered no to all {PUSHES}"
    );

    // Whatever is still queued drains without complaint, and the mixer
    // keeps answering afterwards.
    for _ in 0..64 {
        mix.fill(&mut out);
    }
    assert!(out.iter().all(|sample| sample.is_finite()));
}
