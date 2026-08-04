//! The command ring: a fixed-capacity queue a game thread pushes into
//! while an audio thread drains it.
//!
//! A `Mutex` rather than a lock-free pair of atomics, deliberately. The
//! lock-free version needs interior mutability over a shared array,
//! which needs `unsafe` — a whole safety argument, and a standing
//! obligation to keep it true — bought for eight voices and a handful
//! of commands per second. The audio side never waits for the lock: it
//! tries, and a frame it cannot have is a frame of commands that land
//! on the next callback, which is a few milliseconds nobody can hear.

use std::sync::{Arc, Mutex, TryLockError};

use crate::mixer::SoundId;

/// How many commands may wait between two callbacks. Eight voices and
/// a handful of events per frame make sixty-four a bound the game
/// cannot realistically reach; it is fixed so that pushing allocates
/// nothing, and full is a defined answer rather than a growth.
pub(crate) const CAPACITY: usize = 64;

/// What a game thread asks the mixer to do. `Copy` and small: the ring
/// stores commands by value, so a push copies bytes and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    /// Begin playing this sound on some voice.
    Play(SoundId),
}

/// The queue itself: a fixed array with a head and a tail, both
/// monotonic counts rather than wrapped indices, so "how many are
/// waiting" is a subtraction and an empty queue is indistinguishable
/// from a full lap.
pub(crate) struct CommandRing {
    slots: [Option<Command>; CAPACITY],
    /// Total pushed since creation. Wrapping at `u64` is unreachable:
    /// a game pushing a command every microsecond takes half a million
    /// years to get there.
    pushed: u64,
    /// Total drained since creation; never exceeds `pushed`.
    drained: u64,
}

impl CommandRing {
    fn new() -> Self {
        Self {
            slots: [None; CAPACITY],
            pushed: 0,
            drained: 0,
        }
    }

    /// Waiting commands, at most `CAPACITY`.
    fn waiting(&self) -> u64 {
        self.pushed - self.drained
    }

    /// Append `command`, or report the ring full.
    ///
    /// The slot is written **before** the count advances. A panic
    /// between the two leaves a written slot nothing will read — the
    /// next push overwrites it — rather than a published slot holding
    /// whatever was there before.
    fn push(&mut self, command: Command) -> bool {
        if self.waiting() >= CAPACITY as u64 {
            return false;
        }
        // The remainder is below CAPACITY, which is a `usize` — the
        // conversion cannot fail on any target, and saying so with a
        // fallible conversion beats a cast the reader has to audit.
        let index = usize::try_from(self.pushed % CAPACITY as u64).unwrap_or(0);
        self.slots[index] = Some(command);
        self.pushed += 1;
        true
    }

    /// Take the oldest waiting command, if any.
    fn pop(&mut self) -> Option<Command> {
        if self.drained == self.pushed {
            return None;
        }
        // Bounded by CAPACITY exactly as the push side is.
        let index = usize::try_from(self.drained % CAPACITY as u64).unwrap_or(0);
        let command = self.slots[index].take();
        self.drained += 1;
        command
    }
}

/// The producer half, held by the game thread.
pub(crate) struct Producer {
    ring: Arc<Mutex<CommandRing>>,
}

/// The consumer half, held by the mixer and therefore by the audio
/// thread once the mixer moves into the fill closure.
pub(crate) struct Consumer {
    ring: Arc<Mutex<CommandRing>>,
}

/// A fresh ring, split into the two halves that share it.
pub(crate) fn channel() -> (Producer, Consumer) {
    let ring = Arc::new(Mutex::new(CommandRing::new()));
    (
        Producer {
            ring: Arc::clone(&ring),
        },
        Consumer { ring },
    )
}

impl Producer {
    /// Push `command`, reporting whether the ring had room.
    ///
    /// Blocks for as long as the audio thread holds the lock, which is
    /// the length of one drain of at most `CAPACITY` copies — bounded,
    /// and on the thread that can afford to wait.
    ///
    /// A poisoned ring is recovered rather than abandoned: a producer
    /// that panicked while holding the lock left the queue's invariants
    /// intact by construction (the slot is written before the count
    /// advances), and the alternative — refusing every later command —
    /// is a game that goes permanently silent because one push
    /// unwound. This is the job pool's rule, applied for its reason.
    pub(crate) fn push(&self, command: Command) -> bool {
        let mut ring = match self.ring.lock() {
            Ok(ring) => ring,
            Err(poisoned) => poisoned.into_inner(),
        };
        ring.push(command)
    }
}

impl Consumer {
    /// Drain up to `out.len()` commands into `out`, returning how many
    /// were taken.
    ///
    /// **Never blocks.** On contention the drain is skipped entirely
    /// and the commands wait for the next callback; the audio thread
    /// missing a deadline is audible, and a few milliseconds of extra
    /// latency on a sound effect is not.
    ///
    /// Poisoning is recovered, exactly as the producer recovers it —
    /// and for a sharper reason on this side: a poisoned lock stays
    /// poisoned forever, so treating it as "skip this callback" would
    /// be a mixer that goes permanently deaf while every other signal
    /// says it is healthy.
    pub(crate) fn drain(&self, out: &mut [Option<Command>]) -> usize {
        let mut ring = match self.ring.try_lock() {
            Ok(ring) => ring,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return 0,
        };
        let mut taken = 0;
        while taken < out.len() {
            match ring.pop() {
                Some(command) => {
                    out[taken] = Some(command);
                    taken += 1;
                }
                None => break,
            }
        }
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::{CAPACITY, Command, channel};
    use crate::mixer::SoundId;

    fn sound(index: usize) -> Command {
        Command::Play(SoundId::from_index_for_test(index))
    }

    #[test]
    fn commands_come_out_in_the_order_they_went_in() {
        let (producer, consumer) = channel();
        for index in 0..4 {
            assert!(producer.push(sound(index)));
        }
        let mut out = [None; 8];
        assert_eq!(consumer.drain(&mut out), 4);
        for (index, slot) in out.iter().take(4).enumerate() {
            assert_eq!(*slot, Some(sound(index)), "position {index}");
        }
    }

    #[test]
    fn a_full_ring_refuses_rather_than_growing() {
        let (producer, consumer) = channel();
        for index in 0..CAPACITY {
            assert!(producer.push(sound(index)), "push {index} inside capacity");
        }
        assert!(
            !producer.push(sound(CAPACITY)),
            "the ring must refuse the command past its capacity"
        );
        // Draining makes room again: the refusal is about the moment,
        // not about the ring being spent.
        let mut out = [None; CAPACITY];
        assert_eq!(consumer.drain(&mut out), CAPACITY);
        assert!(producer.push(sound(0)), "a drained ring accepts again");
    }

    #[test]
    fn draining_an_empty_ring_takes_nothing_and_says_so() {
        let (_producer, consumer) = channel();
        let mut out = [None; 4];
        assert_eq!(consumer.drain(&mut out), 0);
        assert!(out.iter().all(Option::is_none));
    }

    #[test]
    fn a_short_output_slice_leaves_the_rest_waiting() {
        let (producer, consumer) = channel();
        for index in 0..5 {
            assert!(producer.push(sound(index)));
        }
        let mut two = [None; 2];
        assert_eq!(consumer.drain(&mut two), 2);
        assert_eq!(two[0], Some(sound(0)));
        assert_eq!(two[1], Some(sound(1)));
        let mut rest = [None; 8];
        assert_eq!(consumer.drain(&mut rest), 3, "the rest survived the cut");
        assert_eq!(rest[0], Some(sound(2)));
    }

    // The ring's counts are laps, not indices: pushing and draining
    // past the capacity several times over must keep order, which a
    // naive wrapped-index pair gets wrong exactly here.
    #[test]
    fn order_survives_many_laps_of_the_array() {
        let (producer, consumer) = channel();
        let mut expected = 0usize;
        for lap in 0..5 {
            for index in 0..CAPACITY {
                assert!(producer.push(sound(lap * CAPACITY + index)));
            }
            let mut out = [None; CAPACITY];
            assert_eq!(consumer.drain(&mut out), CAPACITY);
            for slot in &out {
                assert_eq!(*slot, Some(sound(expected)));
                expected += 1;
            }
        }
    }

    // A producer that panics mid-push poisons the lock. The engine's
    // rule is to recover it: the queue's invariants hold (the slot is
    // written before the count advances), and the alternative is a
    // game that goes silent forever because one push unwound.
    #[test]
    fn a_poisoned_ring_keeps_working_on_both_sides() {
        let (producer, consumer) = channel();
        assert!(producer.push(sound(1)));
        let poisoner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = producer.ring.lock();
            panic!("a producer unwinding while it holds the lock");
        }));
        assert!(poisoner.is_err(), "the test's own panic must be caught");

        assert!(
            producer.push(sound(2)),
            "the producer recovers a poisoned ring"
        );
        let mut out = [None; 4];
        assert_eq!(
            consumer.drain(&mut out),
            2,
            "the consumer recovers it too, and finds both commands"
        );
        assert_eq!(out[0], Some(sound(1)));
        assert_eq!(out[1], Some(sound(2)));
    }
}
