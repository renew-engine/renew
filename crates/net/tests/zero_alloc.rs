//! Mechanical enforcement of the session's allocation contract: after
//! construction, a full pump — deliver, submit, advance, commit, drain
//! the outbox — performs no heap allocation through the global allocator.
//!
//! Shipped with the crate's first per-tick code rather than after it,
//! because a gate that arrives later measures whatever the code has grown
//! into rather than what it promised.
//!
//! **This one is a security control as much as an allocation-discipline
//! one.** Every byte the session absorbs can come from a hostile peer, so
//! a per-datagram allocation is an allocation an attacker drives. The
//! window therefore includes refused datagrams — a wrong session id every
//! pump — because "nothing allocates on the happy path" is not the claim
//! that matters.
//!
//! **The harness allocates nothing either, and that is not decoration.**
//! An earlier version collected outbound datagrams with `to_vec()`, which
//! mints a fresh allocation per datagram per pump; the window then
//! measured the test's own bookkeeping and failed. Everything here is
//! fixed-size arrays for that reason, and the counter is left to describe
//! the session alone.
//!
//! **One test in this file**, following the rule the sibling crates
//! learned from a red lane: the `#[global_allocator]` is process-wide and
//! cargo runs a file's tests concurrently, so two counting tests in one
//! binary race and the loser reports a delta it did not cause.
//!
//! **And the window retries, for the half of that hazard one test per
//! file does not close.** A process-wide counter also sees the harness
//! around the test, so a single window can report activity this code did
//! not cause — which is what happened: an identical build of this binary
//! counted four allocations on one CI run and zero on the next, from the
//! same commit's tree. Retrying separates the two cases, because
//! one-shot noise rides out while a real allocation on the measured path
//! reproduces in every attempt and still fails. Every other
//! allocation-counting gate in the tree already did this; this one was
//! the exception, which is why this one was the one that flaked.

use core::num::NonZeroU64;

use renew_memory::CountingAllocator;
use renew_memory::counters::quiet_window;
use renew_net::{Advance, MAX_DATAGRAM_BYTES, PeerId, Session, SessionParams, wire};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The most datagrams one peer emits in one pump: an `Inputs` and a
/// `Digest` for each remote. Two peers means one remote each.
const OUTBOX: usize = 4;

/// One full exchange between the pair: submit, advance, commit, drain,
/// deliver — plus one hostile datagram, so the refusal path is inside
/// whatever window encloses a call to this.
fn pump(
    peers: &mut [Box<Session>; 2],
    outbox: &mut [u8; MAX_DATAGRAM_BYTES],
    carried: &mut [[u8; MAX_DATAGRAM_BYTES]; OUTBOX],
    lengths: &mut [usize; OUTBOX],
    junk: &[u8],
    confirmed: &mut u64,
) {
    for from in 0..2usize {
        let mut held = 0usize;
        if let Some(source) = peers.get_mut(from) {
            if source.wants_local() {
                let tick = source.next_local_tick();
                let byte = u8::try_from(tick & 0xff).unwrap_or(0);
                let seat_byte = u8::try_from(from).unwrap_or(0);
                let _ = source.submit(&[byte, seat_byte]);
            }
            while let Advance::Step(step) = source.advance() {
                let tick = step.tick();
                let digest = step.digest_due().then_some(tick.wrapping_mul(31));
                let _ = source.commit(tick, digest);
                *confirmed = confirmed.saturating_add(1);
            }
            while let Some(out) = source.next_outbound(outbox) {
                let bytes = out.bytes();
                if let (Some(slot), Some(length)) = (carried.get_mut(held), lengths.get_mut(held))
                    && let Some(room) = slot.get_mut(..bytes.len())
                {
                    room.copy_from_slice(bytes);
                    *length = bytes.len();
                    held = held.saturating_add(1);
                }
            }
        }

        let Some(sender) = u8::try_from(from).ok().and_then(PeerId::new) else {
            return;
        };
        let target_index = from ^ 1;
        if let Some(target) = peers.get_mut(target_index) {
            for index in 0..held {
                if let (Some(slot), Some(length)) = (carried.get(index), lengths.get(index))
                    && let Some(bytes) = slot.get(..*length)
                {
                    let _ = target.deliver(sender, bytes);
                }
            }
            let _ = target.deliver(sender, junk);
        }
    }
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "allocation counting is invalid under instrumented allocators"
)]
#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn a_whole_pump_allocates_nothing() {
    let session_id = NonZeroU64::new(0x5e55_1013).expect("not zero");
    let seat = |index: u8| PeerId::new(index).expect("in range");
    let params = |local: u8| SessionParams {
        peer_count: 2,
        local: seat(local),
        input_bytes: 2,
        input_delay: 1,
        digest_period: 2,
        seed: 11,
        content: 1,
        rules: 2,
        session: session_id,
    };

    // Everything that may allocate happens out here: the two sessions,
    // boxed because each holds its whole input window inline, and the
    // buffers a real driver would own for the lifetime of the run.
    let mut peers = [
        Box::new(Session::new(
            params(0).validate().expect("valid parameters"),
        )),
        Box::new(Session::new(
            params(1).validate().expect("valid parameters"),
        )),
    ];
    let mut outbox = [0u8; MAX_DATAGRAM_BYTES];
    let mut carried = [[0u8; MAX_DATAGRAM_BYTES]; OUTBOX];
    let mut lengths = [0usize; OUTBOX];

    // A datagram from a session nobody here is in: refused at the header,
    // and delivered inside the measured window.
    let mut junk = [0u8; MAX_DATAGRAM_BYTES];
    let stranger_len = wire::write_bye(
        &mut junk,
        wire::Addressing {
            sender: seat(1),
            session: NonZeroU64::new(0xdead_beef).expect("not zero"),
        },
        &wire::ByeBody { tick: 0 },
    );

    let mut confirmed = 0u64;
    let stranger = junk.get(..stranger_len).unwrap_or_default().to_vec();

    // Warmup: far enough that every lazy initialisation has landed and
    // both sessions are playing, so the window measures the steady state
    // rather than the handshake.
    for _ in 0..8 {
        pump(
            &mut peers,
            &mut outbox,
            &mut carried,
            &mut lengths,
            &stranger,
            &mut confirmed,
        );
    }
    assert!(
        peers[0].is_playing() && peers[1].is_playing(),
        "the warmup must reach the playing phase, or the window measures joining"
    );
    let warmed = confirmed;

    quiet_window(5, || {
        for _ in 0..16 {
            pump(
                &mut peers,
                &mut outbox,
                &mut carried,
                &mut lengths,
                &stranger,
                &mut confirmed,
            );
        }
    })
    .expect("the steady state allocated: a per-datagram allocation is one a hostile peer drives");

    // Anti-vacuity, both halves: the window must have run ticks, and it
    // must have refused something. A gate over a window where nothing
    // happened is a gate that measures nothing and passes.
    assert!(
        confirmed > warmed,
        "the measured window confirmed no ticks, so it proved nothing"
    );
    assert!(
        peers[0].stats().datagrams_refused > 0,
        "no datagram was refused in the window, so the refusal path went unmeasured"
    );
}
