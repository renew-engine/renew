//! Two captures of one slot space, and the rule that decides which pairs
//! may be blended.
//!
//! A producer steps at a fixed timestep; frames arrive faster. A consumer
//! captures the producer's presentation state once per executed step and
//! reads back the last two blended by the ratified interpolation factor —
//! keyed by `(slot, generation)`, so a slot's value is only ever blended
//! with *itself*. A recycled slot's new tenant never inherits the old
//! tenant's motion; that pairing, and nothing else, is what this crate is
//! for.
//!
//! # Contract
//!
//! - **A slot blends only with the same slot at the same generation.**
//!   Any other pairing is reported as [`Fate::Newborn`] or [`Fate::Dying`]
//!   and drawn unblended, at the one tick that is known about it.
//! - **The consumer never blends.** [`Snapshots::frame`] hands back values
//!   already resolved, so there is no call site at which a consumer could
//!   blend across a recycled slot even by mistake.
//! - **`frame` yields in the order [`Capture::put`] was called**, dying
//!   slots first. This crate never sorts and has no opinion about draw
//!   order beyond that.
//! - **At `Alpha::ZERO` every living value is bit-exactly its earlier
//!   capture**, and [`Blend::blend`] is not called at all. The tick-exact
//!   case is this container's guarantee rather than each payload's, so a
//!   sloppy `blend` cannot move a picture that an oracle or a committed
//!   image stands on.
//! - **Nothing here allocates** after construction. The budget is fixed
//!   at construction and [`Capture::put`] refuses a slot past it by
//!   name.
//!
//! # Capture locals, never composed transforms
//!
//! Two composed world matrices blended component-wise interpolate along a
//! chord; blending the locals and composing afterwards does not. Capture
//! what a thing *is* — a position, a gap centre — and derive what it looks
//! like on the far side of the blend. A pipe's `(x, gap_y)` blends and the
//! two bars derived from it are right; the two bars blended directly are
//! not.
//!
//! # When not to use this
//!
//! **A key is needed exactly when a slot can be recycled.** A singleton
//! with permanent identity — a camera, a player whose lifetime is the
//! session — wants one `Option<T>` and one blend, where `None` *is* the
//! newborn rule expressed in the type system. Forcing it through a keyed
//! container adds a slot dimension of size one and buys nothing.
//!
//! # What this does not reach
//!
//! Only things captured through it interpolate. A renderer that packs its
//! own instances straight from current state is unaffected by this crate
//! existing, and saying otherwise would be a claim about code this crate
//! never sees.

// A snapshot pair is arithmetic over values the caller handed it; it does
// not print. Floats are its medium on purpose — this crate is
// presentation-side, and deliberately does NOT deny float arithmetic,
// which is what mechanically refuses every simulation crate an edge to
// it.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use renew_math::Alpha;

/// Which slot, and how many times it had been reused when the value was
/// captured.
///
/// Deliberately not `Default`: a zeroed key names slot 0 at generation 0,
/// which is a real key, and a defaulted one that accidentally works is
/// worse than one that will not compile.
///
/// The fields are plain integers rather than any storage crate's handle.
/// That is what keeps this crate nameable from a renderer without
/// dragging entity storage into the renderer's build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// The dense index the producer stores this thing at.
    pub slot: u32,
    /// How many tenants that slot has had. Widened at the fill site — a
    /// producer with a narrower counter passes `u64::from(..)`, which is
    /// lossless.
    pub generation: u64,
}

impl Key {
    /// The key for `slot` at `generation`.
    #[must_use]
    pub const fn new(slot: u32, generation: u64) -> Self {
        Self { slot, generation }
    }
}

/// A value that can stand between two of its own captures.
///
/// # Contract
///
/// - `blend` is a pure function of its three arguments: no clock, no
///   stored state, same inputs same answer.
/// - It is **never called at `Alpha::ZERO`** — [`Snapshots::frame`]
///   short-circuits there and returns the earlier capture bit for bit.
///   Implementors need not reproduce that case exactly, and should not
///   try: the container owns it.
pub trait Blend: Copy {
    /// This value `alpha` of the way from `from` to `to`.
    #[must_use]
    fn blend(from: Self, to: Self, alpha: Alpha) -> Self;
}

impl Blend for f32 {
    fn blend(from: Self, to: Self, alpha: Alpha) -> Self {
        // `from + (to - from) * t` rather than `from * (1 - t) + to * t`:
        // fewer roundings, and the spelling the rest of the tree uses.
        //
        // It is NOT exact at t = 0 — `-0.0 + (x - -0.0) * 0.0` is `+0.0`,
        // so the sign of a zero would be lost. That is precisely why the
        // container short-circuits the boundary rather than trusting this
        // to reproduce it, and why the trait says so in its contract.
        // `alpha_zero_is_bit_exactly_the_earlier_capture` is the test that
        // would fail if the short-circuit were removed.
        from + (to - from) * alpha.get()
    }
}

/// What two captures say about one slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fate {
    /// The same key in both captures: the value is blended.
    Living,
    /// Present only in the newer capture — a newborn, or a recycled
    /// slot's new tenant. One known tick, so it stands at it.
    Newborn,
    /// Present only in the older capture. Its last known value.
    ///
    /// Reported, never acted on: a painter's-order 2D pass draws it once
    /// more underneath the living so nothing vanishes mid-blend, and a
    /// depth-tested pass drops it with a one-line filter. This crate does
    /// not know which it is talking to.
    Dying,
}

/// One slot's contribution to one frame.
///
/// Produced only by [`Snapshots::frame`], never built by callers — the
/// resolution is the container's job, and a hand-built one would be a
/// value nothing resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Drawn<T> {
    /// The slot and the generation the value belongs to. For
    /// [`Fate::Dying`] this is the departed tenant's generation, not
    /// whatever occupies the slot now.
    pub key: Key,
    /// The resolved value: blended for [`Fate::Living`], as captured
    /// otherwise.
    pub value: T,
    /// Which of the three cases produced it.
    pub fate: Fate,
}

/// One slot in one capture.
#[derive(Clone, Copy, Debug, Default)]
struct Entry<T> {
    /// A field rather than a sentinel generation, because generation 0 of
    /// slot 0 is a real key. It is also the only thing a reset writes,
    /// which is what makes the by-order reset below sound.
    present: bool,
    generation: u64,
    value: T,
}

/// One capture: slot-indexed entries plus the order they were written.
#[derive(Debug)]
struct Buffer<T> {
    entries: Vec<Entry<T>>,
    /// The slots written this capture, in the order they were written.
    order: Vec<u32>,
}

impl<T: Default + Copy> Buffer<T> {
    fn with_slots(slots: u32) -> Self {
        Self {
            entries: vec![Entry::default(); slots as usize],
            order: Vec::with_capacity(slots as usize),
        }
    }
}

/// Two captures of one slot space, and the blend between them.
///
/// See the crate documentation for the rule this exists to hold.
#[derive(Debug)]
pub struct Snapshots<T> {
    previous: Buffer<T>,
    current: Buffer<T>,
}

impl<T: Default + Copy> Snapshots<T> {
    /// A pair sized for `slots` slots, both captures empty.
    ///
    /// **The budget is the consumer's obligation, and it is fixed.** A
    /// producer whose live set is bounded by its own rules asserts that
    /// bound in a test; sizing by a storage crate's high-water slot count
    /// also works, and never shrinks, which is the safe direction.
    /// [`Capture::put`] refuses by name rather than silently dropping a
    /// value if the budget turns out to be wrong.
    ///
    /// There is deliberately no `resize`. Growing while keeping both
    /// captures is easy to write and nothing in the tree needs it — every
    /// producer here has a bounded live set — and an unused method is a
    /// method nobody has ever run. It is free to add when a producer
    /// arrives whose slot space really does grow.
    #[must_use]
    pub fn new(slots: u32) -> Self {
        Self {
            previous: Buffer::with_slots(slots),
            current: Buffer::with_slots(slots),
        }
    }
}

impl<T: Copy> Snapshots<T> {
    /// Retire the current capture to previous and open a new one. Call
    /// once per **executed** producer step, after the step — a frame that
    /// runs three catch-up steps captures three times, or the earlier
    /// capture is three ticks stale and the blend spans the wrong
    /// interval.
    ///
    /// The blend never asks how old a capture is: a host that stops
    /// stepping and resumes blends one interval from wherever it left
    /// off. A host that wants a pause to hold perfectly still freezes its
    /// own factor and stops capturing — freezing only one of the two
    /// sweeps a frozen pair, which looks worse than not blending at all.
    pub fn capture(&mut self) -> Capture<'_, T> {
        core::mem::swap(&mut self.previous, &mut self.current);
        // Reset by outgoing order rather than clearing every entry. The
        // invariant is "every slot outside `order` has `present == false`,
        // and its stale value is unreachable", which is inductive:
        // construction defaults them all, and each reset clears exactly
        // the set the previous reset let through. Clearing the whole array
        // instead would be O(capacity) per step against a slot space that
        // never shrinks.
        for &slot in &self.current.order {
            self.current.entries[slot as usize].present = false;
        }
        self.current.order.clear();
        Capture {
            buffer: &mut self.current,
        }
    }
}

impl<T: Blend> Snapshots<T> {
    /// One frame: the [`Fate::Dying`] slots first, in the earlier
    /// capture's put order, then the newer capture's slots in its put
    /// order. Borrowing and allocation-free.
    ///
    /// At `Alpha::ZERO` every [`Fate::Living`] value is bit-exactly its
    /// earlier capture, and [`Blend::blend`] is not called.
    ///
    /// **The same rule is implemented a second time in the tree**, over a
    /// different payload: `UiPresenter::frame` in `renew-ui-render` blends
    /// widget rectangles and their inherited clips under exactly this
    /// generation guard. The two were not merged — the payloads have
    /// little in common and one is already green — so a correction to the
    /// rule has to be made in both, and each names the other so neither
    /// is corrected alone.
    pub fn frame(&self, alpha: Alpha) -> impl Iterator<Item = Drawn<T>> + '_ {
        // Alpha is a ratio of unsigned integers clamped below one, so it
        // is never negative and never a negative zero; comparing bits is
        // an exact "is this the tick boundary" with no float equality.
        let tick_exact = alpha.get().to_bits() == 0;
        let dying = self.previous.order.iter().filter_map(move |&slot| {
            let old = self.previous.entries[slot as usize];
            let now = self.current.entries[slot as usize];
            if now.present && now.generation == old.generation {
                return None;
            }
            Some(Drawn {
                key: Key::new(slot, old.generation),
                value: old.value,
                fate: Fate::Dying,
            })
        });
        let living = self.current.order.iter().map(move |&slot| {
            let now = self.current.entries[slot as usize];
            let old = self.previous.entries[slot as usize];
            let (value, fate) = if old.present && old.generation == now.generation {
                let value = if tick_exact {
                    old.value
                } else {
                    T::blend(old.value, now.value, alpha)
                };
                (value, Fate::Living)
            } else {
                (now.value, Fate::Newborn)
            };
            Drawn {
                key: Key::new(slot, now.generation),
                value,
                fate,
            }
        });
        dying.chain(living)
    }
}

/// The write half of one capture.
///
/// Dropping it without writing anything is not an error — it is the
/// truthful answer for an emptied world, where everything dies once and
/// then stops.
#[derive(Debug)]
pub struct Capture<'a, T> {
    buffer: &'a mut Buffer<T>,
}

impl<T: Copy> Capture<'_, T> {
    /// Record one slot's presentation state.
    ///
    /// **The order of these calls is the order [`Snapshots::frame`]
    /// yields.** This crate never sorts.
    ///
    /// # Panics
    ///
    /// When `key.slot` is past the budget, and when a slot is put twice
    /// in one capture — both by name. The first would otherwise be a bare
    /// slice index whose message names neither the budget nor the
    /// producer; the second would silently draw one thing twice and blend
    /// the survivor against whichever write happened to land last.
    pub fn put(&mut self, key: Key, value: T) {
        let slots = self.buffer.entries.len();
        assert!(
            (key.slot as usize) < slots,
            "slot {} is past this snapshot pair's budget of {slots} slots: size the pair \
             for the producer's slot space, or grow it with resize before the frame",
            key.slot
        );
        let entry = &mut self.buffer.entries[key.slot as usize];
        assert!(
            !entry.present,
            "slot {} was put twice in one capture: a slot holds one value per capture, \
             and two would draw it twice and blend the survivor against whichever landed last",
            key.slot
        );
        entry.present = true;
        entry.generation = key.generation;
        entry.value = value;
        self.buffer.order.push(key.slot);
    }
}

#[cfg(test)]
mod tests {
    use super::{Blend, Capture, Drawn, Fate, Key, Snapshots};
    use core::num::NonZeroU64;
    use renew_math::Alpha;

    /// Half a step past the boundary — the factor every blend claim below
    /// is made at, chosen because it is exact in binary and so an
    /// expectation can be written as a literal rather than a tolerance.
    fn half() -> Alpha {
        Alpha::new(1, NonZeroU64::new(2).expect("2 is not zero"))
    }

    fn quarter() -> Alpha {
        Alpha::new(1, NonZeroU64::new(4).expect("4 is not zero"))
    }

    /// One tick's worth of puts, so the tests read as ticks rather than
    /// as borrow bookkeeping.
    fn tick<T: Copy, const N: usize>(pair: &mut Snapshots<T>, values: [(u32, u64, T); N]) {
        let mut capture: Capture<'_, T> = pair.capture();
        for (slot, generation, value) in values {
            capture.put(Key::new(slot, generation), value);
        }
    }

    fn drawn<T: Blend>(pair: &Snapshots<T>, alpha: Alpha) -> Vec<Drawn<T>> {
        pair.frame(alpha).collect()
    }

    #[test]
    fn a_capture_yields_what_was_put() {
        let mut pair = Snapshots::<f32>::new(8);
        tick(&mut pair, [(0, 0, 1.0), (3, 0, 2.0), (5, 0, 3.0)]);
        let frame = drawn(&pair, half());
        assert_eq!(frame.len(), 3, "three slots in, three out");
        assert!(
            frame.iter().all(|d| d.fate == Fate::Newborn),
            "with one capture behind them every slot is a newborn"
        );
        assert_eq!(
            frame.iter().map(|d| d.value).collect::<Vec<_>>(),
            [1.0, 2.0, 3.0],
            "a newborn stands at its one known tick, unblended"
        );
    }

    #[test]
    fn the_order_out_is_the_order_put_in() {
        let mut pair = Snapshots::<f32>::new(8);
        // Deliberately not ascending: the crate must not sort, and a
        // sorted expectation would pass against an implementation that
        // did.
        tick(
            &mut pair,
            [(5, 0, 5.0), (1, 0, 1.0), (7, 0, 7.0), (0, 0, 0.0)],
        );
        assert_eq!(
            drawn(&pair, half())
                .iter()
                .map(|d| d.key.slot)
                .collect::<Vec<_>>(),
            [5, 1, 7, 0]
        );
    }

    /// **The deliverable.** A slot whose tenant was replaced must not
    /// blend the newcomer out of the corpse's last position.
    ///
    /// Read this beside `a_survivor_at_the_same_slot_and_generation_does_blend`:
    /// alone, this test passes against an implementation that never
    /// blends anything at all.
    #[test]
    fn a_recycled_slot_never_inherits_the_dead_tenants_motion() {
        let mut pair = Snapshots::<f32>::new(8);
        tick(&mut pair, [(3, 0, -16.0)]);
        tick(&mut pair, [(3, 1, 320.0)]);
        let frame = drawn(&pair, half());
        assert_eq!(
            frame.len(),
            2,
            "the corpse draws once more beside the newborn"
        );
        let dying = frame
            .iter()
            .find(|d| d.fate == Fate::Dying)
            .expect("the previous tenant is dying");
        let newborn = frame
            .iter()
            .find(|d| d.fate == Fate::Newborn)
            .expect("the new tenant is a newborn");
        assert_eq!(
            dying.key.generation, 0,
            "the dying entry names the tenant that left"
        );
        assert_eq!(dying.value.to_bits(), (-16.0f32).to_bits());
        assert_eq!(newborn.key.generation, 1);
        assert_eq!(
            newborn.value.to_bits(),
            320.0f32.to_bits(),
            "the newborn stands exactly where it was captured — 152.0 here would be it \
             blended out of the corpse, which is the whole defect"
        );
    }

    /// The negative control for the test above: at the *same* generation
    /// the value genuinely does move.
    #[test]
    fn a_survivor_at_the_same_slot_and_generation_does_blend() {
        let mut pair = Snapshots::<f32>::new(8);
        tick(&mut pair, [(3, 0, 0.0)]);
        tick(&mut pair, [(3, 0, 100.0)]);
        let frame = drawn(&pair, quarter());
        assert_eq!(frame.len(), 1, "nothing died");
        assert_eq!(frame[0].fate, Fate::Living);
        assert_eq!(
            frame[0].value.to_bits(),
            25.0f32.to_bits(),
            "a quarter of the way from 0 to 100"
        );
    }

    /// The by-order reset's invariant: a slot cleared two captures ago
    /// must not still read as present and suppress a dying entry.
    #[test]
    fn a_slot_live_two_ticks_ago_and_absent_since_is_not_read_as_present() {
        let mut pair = Snapshots::<f32>::new(8);
        tick(&mut pair, [(2, 0, 10.0)]);
        tick(&mut pair, []);
        tick(&mut pair, [(2, 1, 99.0)]);
        let frame = drawn(&pair, half());
        assert_eq!(
            frame.len(),
            1,
            "the tenant that left two captures ago has already had its last draw"
        );
        assert_eq!(frame[0].fate, Fate::Newborn);
        assert_eq!(frame[0].value.to_bits(), 99.0f32.to_bits());
    }

    #[test]
    fn a_departed_entity_draws_once_more_and_then_stops() {
        let mut pair = Snapshots::<f32>::new(8);
        tick(&mut pair, [(1, 0, 42.0)]);
        tick(&mut pair, []);
        let first = drawn(&pair, half());
        assert_eq!(first.len(), 1, "it draws once more at its last known place");
        assert_eq!(first[0].fate, Fate::Dying);
        assert_eq!(first[0].value.to_bits(), 42.0f32.to_bits());
        tick(&mut pair, []);
        assert!(
            drawn(&pair, half()).is_empty(),
            "and then it is gone, rather than lingering forever"
        );
    }

    /// A payload that refuses to be blended, so the short-circuit is
    /// proved by construction rather than by comparing numbers that
    /// happen to agree at zero.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    struct NeverBlends(f32);

    impl Blend for NeverBlends {
        fn blend(_from: Self, _to: Self, _alpha: Alpha) -> Self {
            panic!("the container must not consult blend at the tick boundary");
        }
    }

    /// The premise of the test below: this payload really does refuse.
    /// Without this, a `NeverBlends` that had quietly stopped panicking
    /// would make the short-circuit test pass for no reason.
    #[test]
    #[should_panic(expected = "must not consult blend")]
    fn the_refusing_payload_really_refuses() {
        let _ = NeverBlends::blend(NeverBlends(1.0), NeverBlends(2.0), half());
    }

    #[test]
    fn blend_is_never_consulted_at_zero() {
        let mut pair = Snapshots::<NeverBlends>::new(4);
        tick(&mut pair, [(0, 0, NeverBlends(1.0))]);
        tick(&mut pair, [(0, 0, NeverBlends(2.0))]);
        let frame = drawn(&pair, Alpha::ZERO);
        assert_eq!(
            frame[0].value,
            NeverBlends(1.0),
            "the earlier capture, untouched"
        );
    }

    #[test]
    fn alpha_zero_is_bit_exactly_the_earlier_capture() {
        let mut pair = Snapshots::<f32>::new(4);
        // Negative zero is the trap: `from + (to - from) * 0.0` turns
        // -0.0 into +0.0, so an implementation that blended anyway would
        // pass a `==` comparison and fail this one.
        tick(&mut pair, [(0, 0, -0.0), (1, 0, 5.0)]);
        tick(&mut pair, [(0, 0, 7.0), (1, 0, 9.0)]);
        let frame = drawn(&pair, Alpha::ZERO);
        assert_eq!(
            frame[0].value.to_bits(),
            (-0.0f32).to_bits(),
            "sign of zero survives"
        );
        assert_eq!(frame[1].value.to_bits(), 5.0f32.to_bits());
    }

    #[test]
    #[should_panic(expected = "past this snapshot pair's budget")]
    fn a_slot_past_the_budget_refuses_by_name() {
        let mut pair = Snapshots::<f32>::new(4);
        tick(&mut pair, [(4, 0, 1.0)]);
    }

    #[test]
    #[should_panic(expected = "was put twice in one capture")]
    fn putting_a_slot_twice_refuses_by_name() {
        let mut pair = Snapshots::<f32>::new(4);
        tick(&mut pair, [(1, 0, 1.0), (1, 0, 2.0)]);
    }

    #[test]
    fn presentation_is_a_pure_function() {
        let script = [
            [(0u32, 0u64, 1.0f32), (1, 0, 2.0)],
            [(0, 0, 3.0), (1, 1, 4.0)],
            [(0, 0, 5.0), (1, 1, 6.0)],
        ];
        let run = || {
            let mut pair = Snapshots::<f32>::new(8);
            for step in script {
                tick(&mut pair, step);
            }
            drawn(&pair, half())
                .iter()
                .map(|d| (d.key, d.value.to_bits(), d.fate))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run(), "same captures, same frame, bit for bit");
    }

    #[test]
    fn an_emptied_producer_is_a_capture_with_no_puts() {
        let mut pair = Snapshots::<f32>::new(4);
        tick(&mut pair, [(0, 0, 1.0), (2, 0, 2.0)]);
        tick(&mut pair, []);
        let frame = drawn(&pair, half());
        assert_eq!(frame.len(), 2, "everything dies once");
        assert!(frame.iter().all(|d| d.fate == Fate::Dying));
    }
}
