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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use renew_audio::{MixerConfig, mixer, wav};
use renew_platform::thread;

const RATE: u32 = 48_000;

/// How many pushes the producer keeps going until it has had *accepted*.
/// Far past the ring's capacity, so reaching it is only possible if the
/// consumer made room many times over.
const ACCEPTED_TARGET: usize = 2_000;

/// The producer's attempt ceiling, so a ring that never drains fails with
/// a message instead of hanging until the harness kills it.
const ATTEMPT_CEILING: usize = 20_000_000;

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

    let done = Arc::new(AtomicBool::new(false));

    // The producer runs on its own thread, pushing far faster than any
    // game would, so the try-lock's contended path is taken constantly
    // rather than never. It stops on a count of *accepted* pushes, not
    // of attempts: the target is what makes the meeting a fact of the
    // test's construction rather than a measurement taken afterwards.
    // Attempts are ceilinged only so a ring that stopped draining
    // reports that instead of hanging.
    let producer = {
        let done = Arc::clone(&done);
        thread::spawn_named("audio-stress-producer", move || {
            let mut accepted = 0usize;
            let mut attempts = 0usize;
            while accepted < ACCEPTED_TARGET && attempts < ATTEMPT_CEILING {
                attempts += 1;
                if handle.play(id) {
                    accepted += 1;
                }
                std::hint::spin_loop();
            }
            done.store(true, Ordering::Release);
            (accepted, attempts)
        })
        .expect("the platform crate can spawn a named thread")
    };

    // Meanwhile the consumer behaves like a callback: fill, again,
    // again, with no coordination beyond the ring itself — until the
    // producer says it is finished. Driven by the producer's progress
    // rather than by an iteration count, because an iteration count is
    // a guess about relative thread speed, and the two threads run at
    // whatever speed the machine and its instrumentation allow.
    let mut out = vec![0.0f32; 256];
    let mut fills = 0usize;
    while !done.load(Ordering::Acquire) {
        mix.fill(&mut out);
        fills += 1;
        assert!(
            out.iter().all(|sample| sample.is_finite()),
            "a mixed buffer must never carry NaN or infinity"
        );
    }

    let (accepted, attempts) = producer.join().expect("the producer finished");
    // Not `accepted > 0` — the ring starts empty, so the first sixty-four
    // pushes are accepted whatever the consumer does, and that assertion
    // would hold even if `fill` never drained anything at all. What
    // actually proves the two sides met is acceptance far past the
    // ring's own capacity: every push beyond it needed a drain to have
    // made room, and the target is thirty times the capacity.
    //
    // This is an equality rather than a threshold. The producer stops
    // *because* it reached the target, so the only way to arrive here
    // short of it is the ceiling — a ring that stopped making room.
    assert_eq!(
        accepted, ACCEPTED_TARGET,
        "the producer gave up after {attempts} attempts with {accepted} accepted, so the \
         ring stopped making room while the consumer was still draining it"
    );
    assert!(
        fills > 0,
        "the consumer never ran a single callback, so nothing was drained concurrently"
    );

    // Whatever is still queued drains without complaint, and the mixer
    // keeps answering afterwards.
    for _ in 0..64 {
        mix.fill(&mut out);
    }
    assert!(out.iter().all(|sample| sample.is_finite()));
}
