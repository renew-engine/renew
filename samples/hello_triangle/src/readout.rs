//! The on-screen frame-time readout: the window's title bar.
//!
//! The engine renders no text, and a text renderer is a long way off, so
//! the honest first version of "on screen" is the
//! title — where samples have always shown a frame counter, and where it
//! is visible in the task switcher as well as on the window itself.
//!
//! Two properties shape everything here, and an innocent-looking edit
//! loses either of them.
//!
//! - **Nothing on this path allocates.** The steady frame path is proven
//!   heap-silent by `tests/zero_alloc.rs`, and a `format!` per frame
//!   would be the easiest way to break that. The text is written into a
//!   fixed-capacity buffer the readout owns and handed out borrowed —
//!   the same shape the diagnostics crate asks its sinks for.
//! - **The interval comes from the frame loop's own timestamps.** The
//!   caller already reads the clock once per iteration and hands that
//!   instant over. Reading a wall clock here to decide whether to
//!   relabel a window would put a second time source in a sample that
//!   deliberately has one, and it would make the readout untestable
//!   without waiting real seconds.

use core::fmt;
use core::fmt::Write as _;

use renew_frame::{Nanos, Timestamp};

/// How often the title is relabelled: four times a second.
///
/// Two costs pull in opposite directions and this sits between them. Any
/// faster and the digits are unreadable — a number that changes sixty
/// times a second is a blur, and the value anyone actually quotes is the
/// one that stood still long enough to read. Any slower and the readout
/// stops answering the question it exists for, which is what the machine
/// is doing *now*. A quarter second also makes the cost of the OS call
/// disappear: one relabel per fifteen frames at 60 Hz, against one per
/// frame for the naive version.
pub const INTERVAL: Nanos = Nanos::from_nanos(250_000_000);

/// Capacity of the title buffer.
///
/// The longest reading this can produce is 68 bytes: a 24-byte prefix,
/// the millisecond count for a `u64::MAX` frame (14 digits and two
/// decimals), and the rate for a one-nanosecond frame (ten digits and
/// one decimal) — and those two extremes cannot occur together. Double
/// that, so a longer sample title does not quietly push the readout into
/// refusing to draw at all.
const CAPACITY: usize = 128;

/// A fixed-capacity text buffer: format into it, borrow the result, reuse
/// it next time, never touch the heap.
///
/// The capacity is a type parameter rather than a constant so that the
/// refusal below is reachable from a test holding a deliberately tiny
/// buffer. A branch nothing can execute is a branch nobody has checked.
struct Text<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Text<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }

    /// Discard what was written; the next format starts from the front.
    fn clear(&mut self) {
        self.length = 0;
    }

    /// What has been written so far.
    fn as_str(&self) -> &str {
        // Every byte here arrived as a whole `&str` that fitted, so the
        // slice is valid text by construction — but reading bytes back
        // as text is a checked conversion in safe code, and this crate
        // has no unsafe. An empty title is the harmless answer if the
        // construction argument above were ever broken.
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

impl<const N: usize> fmt::Write for Text<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let bytes = text.as_bytes();
        let end = self.length.saturating_add(bytes.len());
        if end > N {
            // All or nothing per piece: a piece accepted halfway could
            // split a multi-byte character and leave the buffer holding
            // something that is not text.
            return Err(fmt::Error);
        }
        self.bytes[self.length..end].copy_from_slice(bytes);
        self.length = end;
        Ok(())
    }
}

/// A frame cost in milliseconds, to two decimals.
struct Millis(Nanos);

impl fmt::Display for Millis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Integer arithmetic, like every other duration in this tree: a
        // hundredth of a millisecond is 10 000 ns, and adding half of
        // one before dividing rounds to nearest instead of always down,
        // so a 16 666 667 ns frame reads 16.67 ms and not 16.66.
        let hundredths = self.0.get().saturating_add(5_000) / 10_000;
        write!(f, "{}.{:02}", hundredths / 100, hundredths % 100)
    }
}

/// The frame rate a mean frame cost implies, to one decimal.
struct Rate(Nanos);

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nanos = self.0.get();
        if nanos == 0 {
            // No rate divides out of no elapsed time. A frame too fast
            // for the clock to resolve is a real reading; a made-up rate
            // for it would be the only lie on the window.
            return f.write_str("--");
        }
        // Ten billion over the frame cost is tenths of a frame per
        // second, rounded to nearest by the same half-divisor trick —
        // without it a 16 666 667 ns frame reads 59.9 fps, because 60 Hz
        // is not representable in whole nanoseconds.
        let tenths = 10_000_000_000_u64.saturating_add(nanos / 2) / nanos;
        write!(f, "{}.{}", tenths / 10, tenths % 10)
    }
}

/// Write one reading behind `prefix`, into `out`.
///
/// Pure: the same numbers produce the same bytes on every machine and in
/// every run. `None` means the text did not fit, which is the only way a
/// fixed buffer fails; the caller then leaves the title as it was rather
/// than showing half a number.
fn compose<'a, const N: usize>(out: &'a mut Text<N>, prefix: &str, mean: Nanos) -> Option<&'a str> {
    out.clear();
    if write!(out, "{prefix} — {} ms ({} fps)", Millis(mean), Rate(mean)).is_err() {
        return None;
    }
    Some(out.as_str())
}

/// The window-title readout: what the sample shows, and when.
pub struct Readout {
    prefix: &'static str,
    text: Text<CAPACITY>,
    /// The current interval's samples, folded in as they arrive.
    ///
    /// A boxcar mean over the interval rather than over the whole run:
    /// a live readout exists to say what the machine is doing now, and a
    /// run average stops moving after the first few seconds — exactly
    /// when a hitch starts being interesting. It is also exact integer
    /// arithmetic, where a decaying average would need a divisor whose
    /// truncation error accumulates.
    sum: u64,
    count: u64,
    /// When the next relabel falls due, on the frame loop's timeline.
    due: Timestamp,
    interval: Nanos,
}

impl Readout {
    /// A readout that relabels `prefix` every `interval`, starting with
    /// the very first frame.
    ///
    /// The first frame on purpose: a window whose title says nothing for
    /// a quarter of a second looks broken, and a run shorter than one
    /// interval would otherwise never show a number at all.
    #[must_use]
    pub const fn new(prefix: &'static str, interval: Nanos) -> Self {
        Self {
            prefix,
            text: Text::new(),
            sum: 0,
            count: 0,
            // Before any instant the loop can hand over, so the first
            // frame is already due.
            due: Timestamp::from_nanos(0),
            interval,
        }
    }

    /// Fold one frame's measured CPU cost in, and say what the title
    /// should read if the interval has elapsed.
    ///
    /// `now` is the frame loop's own instant — the one the caller has
    /// already read — so the readout introduces no second time source.
    /// `None` is the answer for most frames and means "leave the title
    /// alone".
    pub fn record(&mut self, cpu_frame: Nanos, now: Timestamp) -> Option<&str> {
        self.sum = self.sum.saturating_add(cpu_frame.get());
        self.count = self.count.saturating_add(1);
        if now < self.due {
            return None;
        }
        // The divisor is at least one: this frame's sample was folded in
        // two lines above.
        let mean = Nanos::from_nanos(self.sum / self.count);
        self.sum = 0;
        self.count = 0;
        self.due = now.saturating_add(self.interval);
        compose(&mut self.text, self.prefix, mean)
    }
}

#[cfg(test)]
mod tests {
    use super::{CAPACITY, INTERVAL, Millis, Rate, Readout, Text, compose};
    use renew_frame::{Nanos, Timestamp};

    fn reading(nanos: u64) -> String {
        let mut text = Text::<CAPACITY>::new();
        compose(&mut text, "renew", Nanos::from_nanos(nanos))
            .expect("the reading fits the buffer")
            .to_string()
    }

    #[test]
    fn a_sixty_hertz_frame_reads_as_the_numbers_people_quote() {
        // Both halves round to nearest rather than down: 60 Hz is
        // 16 666 667 ns, which truncation would report as 16.66 ms and
        // 59.9 fps — two numbers that look like a bug and are not.
        assert_eq!(reading(16_666_667), "renew — 16.67 ms (60.0 fps)");
        assert_eq!(reading(33_333_333), "renew — 33.33 ms (30.0 fps)");
        assert_eq!(reading(1_000_000), "renew — 1.00 ms (1000.0 fps)");
    }

    /// A frame too fast for the clock to resolve. Nothing divides by
    /// zero, and the rate says it has no answer rather than inventing
    /// one.
    #[test]
    fn a_zero_frame_time_reports_no_rate_at_all() {
        assert_eq!(reading(0), "renew — 0.00 ms (-- fps)");
    }

    /// The other end: a frame so slow that every quantity is near its
    /// ceiling. Saturating arithmetic keeps it a reading rather than an
    /// overflow.
    #[test]
    fn a_very_slow_frame_still_produces_one_finite_reading() {
        assert_eq!(reading(u64::MAX), "renew — 18446744073709.55 ms (0.0 fps)");
        // One whole second, and the digit that separates a hitch from a
        // freeze.
        assert_eq!(reading(1_000_000_000), "renew — 1000.00 ms (1.0 fps)");
        assert_eq!(reading(60_000_000_000), "renew — 60000.00 ms (0.0 fps)");
    }

    /// The fastest frame the clock can express, which is where the rate
    /// is largest and the buffer is under the most pressure.
    #[test]
    fn a_one_nanosecond_frame_produces_the_widest_rate() {
        assert_eq!(reading(1), "renew — 0.00 ms (1000000000.0 fps)");
    }

    #[test]
    fn the_pieces_format_independently_of_the_line_they_sit_on() {
        assert_eq!(Millis(Nanos::from_nanos(16_666_667)).to_string(), "16.67");
        assert_eq!(Millis(Nanos::ZERO).to_string(), "0.00");
        assert_eq!(Rate(Nanos::from_nanos(16_666_667)).to_string(), "60.0");
        assert_eq!(Rate(Nanos::ZERO).to_string(), "--");
    }

    /// A buffer too small refuses rather than showing half a number, and
    /// stays usable afterwards.
    #[test]
    fn a_reading_that_does_not_fit_is_refused_whole() {
        let mut tiny = Text::<8>::new();
        assert_eq!(
            compose(&mut tiny, "renew", Nanos::from_nanos(16_666_667)),
            None
        );
        // Exactly enough room, then one reading too wide for it, then the
        // first one again: a refusal costs the frame it happened on and
        // nothing after it.
        let mut exact = Text::<21>::new();
        assert_eq!(
            compose(&mut exact, "", Nanos::ZERO),
            Some(" — 0.00 ms (-- fps)")
        );
        assert_eq!(compose(&mut exact, "", Nanos::from_nanos(16_666_667)), None);
        assert_eq!(
            compose(&mut exact, "", Nanos::ZERO),
            Some(" — 0.00 ms (-- fps)")
        );
    }

    #[test]
    fn the_first_frame_is_labelled_and_the_rest_of_the_interval_is_not() {
        let mut readout = Readout::new("renew", INTERVAL);
        let first = readout.record(Nanos::from_nanos(16_666_667), Timestamp::from_nanos(0));
        assert_eq!(first, Some("renew — 16.67 ms (60.0 fps)"));
        // Everything inside the interval is folded in silently.
        for frame in 1..15u64 {
            let now = Timestamp::from_nanos(frame * 16_666_667);
            assert_eq!(readout.record(Nanos::from_nanos(16_666_667), now), None);
        }
    }

    /// The reading is the mean over the interval that just ended, not
    /// over the run and not the last frame alone.
    #[test]
    fn the_reading_averages_the_interval_that_just_ended() {
        let interval = Nanos::from_nanos(100);
        let mut readout = Readout::new("renew", interval);
        // The first frame is due immediately and reports itself.
        assert!(
            readout
                .record(Nanos::from_nanos(4_000_000), Timestamp::from_nanos(0))
                .is_some()
        );
        // Two frames inside the interval, averaging 3 ms, and a third at
        // the deadline that carries the reading.
        assert_eq!(
            readout.record(Nanos::from_nanos(2_000_000), Timestamp::from_nanos(40)),
            None
        );
        assert_eq!(
            readout.record(Nanos::from_nanos(4_000_000), Timestamp::from_nanos(80)),
            None
        );
        assert_eq!(
            readout.record(Nanos::from_nanos(3_000_000), Timestamp::from_nanos(100)),
            Some("renew — 3.00 ms (333.3 fps)"),
            "the mean of the interval, not the run and not the last frame"
        );
        // The next interval starts empty: the slow frame above does not
        // follow the readout around.
        assert_eq!(
            readout.record(Nanos::from_nanos(16_666_667), Timestamp::from_nanos(200)),
            Some("renew — 16.67 ms (60.0 fps)")
        );
    }

    /// The number in the source, asserted rather than described, so an
    /// edit to it makes a liar of the test instead of the comment above
    /// it.
    #[test]
    fn the_relabel_interval_is_a_quarter_of_a_second() {
        assert_eq!(INTERVAL.get(), 250_000_000);
    }

    /// Saturating arithmetic all the way down: an interval whose samples
    /// sum past the end of a `u64` reports the ceiling divided by the
    /// count, never a wrapped number and never a panic.
    #[test]
    fn an_overflowing_accumulator_saturates_instead_of_wrapping() {
        let mut readout = Readout::new("renew", Nanos::from_nanos(1_000));
        // The first frame is due immediately; the interval it opens then
        // banks four saturated frame costs.
        assert!(
            readout
                .record(Nanos::from_nanos(u64::MAX), Timestamp::from_nanos(0))
                .is_some()
        );
        for frame in 1..4u64 {
            assert_eq!(
                readout.record(Nanos::from_nanos(u64::MAX), Timestamp::from_nanos(frame)),
                None
            );
        }
        let expected = reading(u64::MAX / 4);
        assert_eq!(
            readout.record(Nanos::from_nanos(u64::MAX), Timestamp::from_nanos(1_000)),
            Some(expected.as_str()),
            "four saturated samples average to the ceiling over four"
        );
    }
}
