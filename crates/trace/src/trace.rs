//! The trace itself: a header, and the events in the order they happened.

use crate::error::{TraceError, TraceErrorKind};
use crate::event::TraceEvent;
use crate::grammar::{ASSIGN, FIRST_EVENT_LINE, HEADER_LINE, RESERVED_KEYS, SAMPLE};

/// What a trace says about itself before it says anything happened.
///
/// Four fields are the codec's own and are positional, so a reader knows
/// what it is holding before it interprets any of it. Everything after
/// them is the caller's: the codec preserves those keys verbatim, in
/// order, and validates nothing but their uniqueness. It does not know
/// what a sample is, what a seed does, or what a window extent means, and
/// a codec that guessed would be wrong in a different way for every
/// caller. The caller checks that `sample` names the thing it is about to
/// run, applies its own keys, and reports its own mismatches.
///
/// `timestep_ns` and `budget` pin the schedule the events were recorded
/// against, because the same event at tick 40 is a different moment at
/// 30 Hz than at 60 Hz, and the budget decides how a stall was clamped.
/// The codec stores them and never reads them.
///
/// A header carries no version of its own. There is exactly one format
/// version today, so a parsed header and a built one make the same claim,
/// and storing a number that can only hold one value would be machinery
/// pretending to be a decision. The day a reader accepts an older version
/// as well as its own, whether rewriting preserves the older claim is a
/// question for whoever adds it — and it will be a visible addition rather
/// than a field that quietly upgraded every file it touched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceHeader {
    sample: String,
    ticks: u64,
    timestep_ns: u64,
    budget: u32,
    keys: Vec<(String, String)>,
}

impl TraceHeader {
    /// A header for a run of `ticks` ticks of `sample`, at this timestep
    /// and step budget.
    ///
    /// `ticks` is a count, and it bounds the events: a tick equal to it is
    /// legal and means *after the final step*.
    ///
    /// # Errors
    ///
    /// When `sample` could not be written back out as itself — it is
    /// empty, carries whitespace, or carries a control character. The
    /// refusal is reported against line 1, the line the header occupies.
    pub fn new(
        sample: &str,
        ticks: u64,
        timestep_ns: u64,
        budget: u32,
    ) -> Result<Self, TraceError> {
        check_field(SAMPLE, sample, HEADER_LINE)?;
        Ok(Self {
            sample: sample.to_string(),
            ticks,
            timestep_ns,
            budget,
            keys: Vec::new(),
        })
    }

    /// The same header with one more caller-owned key.
    ///
    /// # Errors
    ///
    /// When the key repeats one the header already has — including one of
    /// the four the codec owns — or when either half could not be written
    /// back out as itself. A key may not contain `=`, because a reader
    /// splits at the first one; a value may contain as many as it likes.
    pub fn with_key(mut self, key: &str, value: &str) -> Result<Self, TraceError> {
        check_field(key, value, HEADER_LINE)?;
        if RESERVED_KEYS.contains(&key) || self.keys.iter().any(|(known, _)| known == key) {
            return Err(TraceError::new(
                HEADER_LINE,
                TraceErrorKind::DuplicateHeaderKey {
                    key: key.to_string(),
                },
            ));
        }
        self.keys.push((key.to_string(), value.to_string()));
        Ok(self)
    }

    /// What was running. Uninterpreted: matching it against a sample is
    /// the caller's check, not the codec's.
    #[must_use]
    pub fn sample(&self) -> &str {
        &self.sample
    }

    /// How many ticks the run lasted, and so the largest legal event tick.
    #[must_use]
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }

    #[must_use]
    pub const fn timestep_ns(&self) -> u64 {
        self.timestep_ns
    }

    #[must_use]
    pub const fn budget(&self) -> u32 {
        self.budget
    }

    /// The caller-owned keys, in the order they were written.
    #[must_use]
    pub fn keys(&self) -> &[(String, String)] {
        &self.keys
    }

    /// The value of one caller-owned key.
    #[must_use]
    pub fn value(&self, key: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(known, _)| known == key)
            .map(|(_, value)| value.as_str())
    }
}

/// A recorded run: what it was, and what happened during it.
///
/// The events are ordered and never sorted. Two keys can go down on the
/// same tick, and which of them the application saw first is part of what
/// was recorded — sorting or deduplicating within a tick would silently
/// change the input while looking like tidying.
///
/// What a trace reproduces is the *simulation*: the state a run reaches,
/// and the exact interleaving of events with steps. It does not reproduce
/// how many frames carried those steps, or how many were dropped, which
/// are facts about the schedule that carried the input rather than about
/// the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trace {
    header: TraceHeader,
    events: Vec<(u64, TraceEvent)>,
}

impl Trace {
    /// A trace of these events, delivered at these ticks.
    ///
    /// Tick *k* means the event is delivered before the step whose tick is
    /// *k*. Ticks are 0-based. A tick equal to the header's `ticks` is
    /// legal and means *after the final step* — that is the common case,
    /// not an edge case: a run that free-runs its frames far faster than
    /// its steps will usually see the terminating event during a frame
    /// that ran no step at all.
    ///
    /// # Errors
    ///
    /// When a tick is earlier than the one before it, or past the end of
    /// the run the header describes. Equal ticks are allowed, and their
    /// order is preserved exactly as given. The refusal names the line the
    /// offending event would occupy in the written text.
    pub fn new(header: TraceHeader, events: Vec<(u64, TraceEvent)>) -> Result<Self, TraceError> {
        // A first event cannot go backwards from tick zero, so no
        // "is there a previous one" branch is needed here.
        let mut previous = 0;
        for (index, (tick, _)) in events.iter().enumerate() {
            let line = index + FIRST_EVENT_LINE;
            if *tick < previous {
                return Err(TraceError::new(
                    line,
                    TraceErrorKind::TickWentBackwards {
                        tick: *tick,
                        previous,
                    },
                ));
            }
            if *tick > header.ticks {
                return Err(TraceError::new(
                    line,
                    TraceErrorKind::TickBeyondHeader {
                        tick: *tick,
                        ticks: header.ticks,
                    },
                ));
            }
            previous = *tick;
        }
        Ok(Self { header, events })
    }

    #[must_use]
    pub const fn header(&self) -> &TraceHeader {
        &self.header
    }

    /// The events, each with the tick it is delivered before, in the order
    /// they were recorded.
    #[must_use]
    pub fn events(&self) -> &[(u64, TraceEvent)] {
        &self.events
    }
}

/// Whether a header key and value survive being written and read back.
///
/// Three rules bind both halves — they are the three ways text stops
/// being one field of one line: nothing at all, something that would
/// split into two fields, and something that would be invisible in the
/// places people actually read these files, a diff and a log.
///
/// A fourth binds the **key only**. A reader splits a field at its first
/// `=`, so a key containing one is read back as a shorter key and a
/// longer value: `a=b=c` written from the key `a=b` reads as the key `a`
/// with the value `b=c`, which is silent corruption, and a key of
/// `ticks=9` would smuggle a reserved name past the uniqueness guard. A
/// *value* may contain as many as it likes — everything after the first
/// `=` is the value, and `extent=640=480` round-trips exactly. The
/// positional `sample` field is a value in this sense too, which is why
/// the rule cannot be applied to both halves.
pub(crate) fn check_field(key: &str, value: &str, line: usize) -> Result<(), TraceError> {
    let refuse = |reason| {
        Err(TraceError::new(
            line,
            TraceErrorKind::UnwritableText {
                text: format!("{key}{ASSIGN}{value}"),
                reason,
            },
        ))
    };
    for text in [key, value] {
        if text.is_empty() {
            return refuse("a header key and a header value are each at least one character");
        }
        if text.chars().any(char::is_whitespace) {
            return refuse("whitespace would split it into two fields");
        }
        if text.chars().any(char::is_control) {
            return refuse("a control character is invisible in a diff and in a log");
        }
    }
    if key.contains(ASSIGN) {
        return refuse("an equals sign would split the key from its value");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Trace, TraceHeader};
    use crate::error::TraceErrorKind;
    use crate::event::TraceEvent;

    fn header() -> TraceHeader {
        TraceHeader::new("input_echo", 30, 16_666_667, 5).unwrap()
    }

    #[test]
    fn a_header_reports_the_four_fields_the_codec_owns() {
        let header = header();
        assert_eq!(header.sample(), "input_echo");
        assert_eq!(header.ticks(), 30);
        assert_eq!(header.timestep_ns(), 16_666_667);
        assert_eq!(header.budget(), 5);
        assert!(header.keys().is_empty());
    }

    #[test]
    fn caller_keys_keep_their_order_and_are_readable_by_name() {
        let header = header()
            .with_key("seed", "3")
            .unwrap()
            .with_key("extent", "640x480")
            .unwrap();
        assert_eq!(
            header.keys(),
            [
                ("seed".to_string(), "3".to_string()),
                ("extent".to_string(), "640x480".to_string()),
            ]
        );
        assert_eq!(header.value("seed"), Some("3"));
        assert_eq!(header.value("extent"), Some("640x480"));
        assert_eq!(header.value("absent"), None);
    }

    #[test]
    fn a_repeated_caller_key_is_refused() {
        let error = header()
            .with_key("seed", "3")
            .unwrap()
            .with_key("seed", "4")
            .unwrap_err();
        assert_eq!(error.line(), 1);
        assert_eq!(
            error.kind(),
            &TraceErrorKind::DuplicateHeaderKey {
                key: "seed".to_string()
            }
        );
    }

    /// The codec's own four are keys too, so a caller cannot shadow one
    /// and leave two answers to the same question in one file.
    #[test]
    fn a_caller_key_may_not_shadow_one_the_codec_owns() {
        for reserved in ["sample", "ticks", "timestep_ns", "budget"] {
            let error = header().with_key(reserved, "x").unwrap_err();
            assert_eq!(
                error.kind(),
                &TraceErrorKind::DuplicateHeaderKey {
                    key: reserved.to_string()
                }
            );
        }
    }

    #[test]
    fn header_text_that_could_not_be_written_back_is_refused() {
        let empty = TraceHeader::new("", 1, 1, 1).unwrap_err();
        assert_eq!(
            empty.kind(),
            &TraceErrorKind::UnwritableText {
                text: "sample=".to_string(),
                reason: "a header key and a header value are each at least one character",
            }
        );
        let spaced = TraceHeader::new("two words", 1, 1, 1).unwrap_err();
        assert_eq!(
            spaced.kind(),
            &TraceErrorKind::UnwritableText {
                text: "sample=two words".to_string(),
                reason: "whitespace would split it into two fields",
            }
        );
        let controlled = header().with_key("seed", "3\u{7}").unwrap_err();
        assert_eq!(
            controlled.kind(),
            &TraceErrorKind::UnwritableText {
                text: "seed=3\u{7}".to_string(),
                reason: "a control character is invisible in a diff and in a log",
            }
        );
        let blank_key = header().with_key("", "3").unwrap_err();
        assert_eq!(
            blank_key.kind(),
            &TraceErrorKind::UnwritableText {
                text: "=3".to_string(),
                reason: "a header key and a header value are each at least one character",
            }
        );
    }

    /// A key carrying an equals sign is the one asymmetry between the two
    /// halves of a header field, and it is not a nicety: a reader splits
    /// at the first `=`, so `a=b=c` would come back as the key `a` with
    /// the value `b=c` — the writer emitting a file that reads as
    /// something else, silently. Worse, `ticks=9` as a key would walk
    /// straight past the uniqueness guard and land in a file with two
    /// answers to one question.
    #[test]
    fn a_caller_key_carrying_an_equals_sign_is_refused() {
        let error = header().with_key("a=b", "c").unwrap_err();
        assert_eq!(error.line(), 1);
        assert_eq!(
            error.kind(),
            &TraceErrorKind::UnwritableText {
                text: "a=b=c".to_string(),
                reason: "an equals sign would split the key from its value",
            }
        );
        assert!(header().with_key("ticks=9", "x").is_err());
        assert!(header().with_key("=", "x").is_err());
    }

    /// The value half is deliberately unrestricted, because everything
    /// after the first `=` is the value and reads back as itself.
    #[test]
    fn a_value_carrying_an_equals_sign_is_kept() {
        let header = header().with_key("extent", "640=480").unwrap();
        assert_eq!(header.value("extent"), Some("640=480"));
        // The positional field is a value in the same sense.
        let sample = TraceHeader::new("odd=name", 1, 1, 1).unwrap();
        assert_eq!(sample.sample(), "odd=name");
    }

    #[test]
    fn a_trace_keeps_its_events_in_the_order_it_was_given_them() {
        let events = vec![
            (4, TraceEvent::Focused(true)),
            (4, TraceEvent::TextEntered { ch: 0x61 }),
            (4, TraceEvent::RedrawRequested),
            (30, TraceEvent::CloseRequested),
        ];
        let trace = Trace::new(header(), events.clone()).unwrap();
        assert_eq!(trace.events(), events.as_slice());
        assert_eq!(trace.header().sample(), "input_echo");
    }

    /// The trailing bucket, which is the common case rather than an edge
    /// one: an event delivered after the final step carries the run's own
    /// tick count.
    #[test]
    fn a_tick_equal_to_the_run_length_is_legal() {
        let trace = Trace::new(header(), vec![(30, TraceEvent::CloseRequested)]).unwrap();
        assert_eq!(trace.events()[0].0, trace.header().ticks());
    }

    #[test]
    fn a_tick_past_the_end_of_the_run_is_refused_against_its_own_line() {
        let error = Trace::new(
            header(),
            vec![
                (0, TraceEvent::RedrawRequested),
                (31, TraceEvent::CloseRequested),
            ],
        )
        .unwrap_err();
        // Header, then two events: the offender is the third line.
        assert_eq!(error.line(), 3);
        assert_eq!(
            error.kind(),
            &TraceErrorKind::TickBeyondHeader {
                tick: 31,
                ticks: 30
            }
        );
    }

    #[test]
    fn a_tick_earlier_than_the_one_before_it_is_refused() {
        let error = Trace::new(
            header(),
            vec![
                (7, TraceEvent::RedrawRequested),
                (3, TraceEvent::CloseRequested),
            ],
        )
        .unwrap_err();
        assert_eq!(error.line(), 3);
        assert_eq!(
            error.kind(),
            &TraceErrorKind::TickWentBackwards {
                tick: 3,
                previous: 7
            }
        );
    }

    #[test]
    fn a_trace_with_no_events_is_a_trace() {
        let trace = Trace::new(header(), Vec::new()).unwrap();
        assert!(trace.events().is_empty());
    }
}
