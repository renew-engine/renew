//! Properties over generated traces and generated garbage.
//!
//! Two claims, and they are different in kind. The first is that writing
//! and reading are inverses, over traces nobody thought to write by hand.
//! On its own that claim is weak — a writer and a reader that make the
//! *same* mistake are still inverses of each other — which is why the
//! golden file next to this one anchors it with text a person wrote and
//! the code did not. The second claim is about hostile input: the reader
//! answers, one way or the other, for every string it can be handed. It
//! is a small fuzzer with a fixed budget, standing in until real fuzzing
//! infrastructure exists.
//!
//! A generator is only as good as its alphabet, and that is not a
//! platitude here. An earlier version of this file drew header text from
//! characters that did not include `=`; every property below passed, and
//! so did a reader deliberately broken to split a header field at its
//! *last* `=` instead of its first — inverting the one rule the format
//! documents about that character. Nothing in a hundred and five tests
//! could tell the two readers apart, because no input ever contained the
//! character the rule is about. So values are generated with `=` in them
//! on purpose, keys are generated without one because the format forbids
//! it there, and the case that pins the rule is written out by hand
//! below rather than left to chance.

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use renew_trace::{
    FiniteF32, FiniteF64, Trace, TraceButton, TraceEvent, TraceHeader, TraceKey, parse, write,
};

/// A header key: no whitespace, no control characters, no `=` — a reader
/// splits a field at the first one, so a key carrying one would be read
/// back shorter than it was written.
fn key_text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_.:x-]{1,12}"
}

/// A header value, which *may* carry `=`: everything after the first one
/// belongs to the value, and generating them is what makes the split rule
/// observable to the properties below.
fn value_text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_.:=x-]{1,12}"
}

fn finite_f64() -> impl Strategy<Value = FiniteF64> {
    any::<u64>().prop_filter_map("finite", FiniteF64::from_bits)
}

fn finite_f32() -> impl Strategy<Value = FiniteF32> {
    any::<u32>().prop_filter_map("finite", FiniteF32::from_bits)
}

fn key() -> impl Strategy<Value = TraceKey> {
    proptest::sample::select(TraceKey::ALL)
}

fn button() -> impl Strategy<Value = TraceButton> {
    prop_oneof![
        proptest::sample::select(TraceButton::NAMED),
        any::<u16>().prop_map(TraceButton::Other),
    ]
}

fn event() -> impl Strategy<Value = TraceEvent> {
    prop_oneof![
        (key(), any::<bool>(), any::<bool>()).prop_map(|(code, pressed, repeat)| {
            TraceEvent::Key {
                code,
                pressed,
                repeat,
            }
        }),
        (finite_f64(), finite_f64()).prop_map(|(x, y)| TraceEvent::PointerMoved { x, y }),
        (button(), any::<bool>())
            .prop_map(|(button, pressed)| TraceEvent::PointerButton { button, pressed }),
        (finite_f32(), finite_f32()).prop_map(|(dx, dy)| TraceEvent::Wheel { dx, dy }),
        any::<bool>().prop_map(TraceEvent::Focused),
        (any::<u32>(), any::<u32>())
            .prop_map(|(width, height)| TraceEvent::Resized { width, height }),
        finite_f64().prop_map(|scale| TraceEvent::ScaleFactorChanged { scale }),
        Just(TraceEvent::RedrawRequested),
        Just(TraceEvent::CloseRequested),
    ]
}

/// A whole trace: a header with caller keys, and events on non-decreasing
/// ticks inside the run the header describes.
// A strategy, driven only from `#[test]` fns: the tests-only expect
// allowance covers those, not their helpers, and this extends it in the
// same spirit. Both refusals below are impossible by construction — the
// generated names are writable, and the ticks are reduced into range and
// sorted right above — so either one firing is a broken generator, which
// is exactly what should stop the run.
#[allow(clippy::expect_used)]
fn trace() -> impl Strategy<Value = Trace> {
    (
        value_text(),
        0_u64..1_000,
        any::<u64>(),
        any::<u32>(),
        proptest::collection::vec((key_text(), value_text()), 0..4),
        proptest::collection::vec((any::<u64>(), event()), 0..24),
    )
        .prop_map(|(sample, ticks, timestep_ns, budget, keys, events)| {
            let mut header = TraceHeader::new(&sample, ticks, timestep_ns, budget)
                .expect("the generated sample name is writable");
            for (key, value) in keys {
                // A repeated key is refused by design, so a generated
                // repeat is dropped rather than worked around.
                header = header.clone().with_key(&key, &value).unwrap_or(header);
            }
            let mut ordered: Vec<(u64, TraceEvent)> = events
                .into_iter()
                .map(|(tick, event)| (tick % (ticks + 1), event))
                .collect();
            ordered.sort_by_key(|(tick, _)| *tick);
            Trace::new(header, ordered).expect("ticks are in range and sorted")
        })
}

proptest! {
    // Fixed RNG seed: the suite explores the same inputs on every run and
    // every machine, so a property failure anywhere reproduces everywhere
    // — and the lines these cases reach are the same lines on every run,
    // which is what a coverage gate needs from a randomised suite. Fresh
    // exploration is a deliberate act (change the seed), never an ambient
    // one.
    #![proptest_config(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x0000_7ace),
        cases: 128,
        ..ProptestConfig::default()
    })]

    /// Reading what was written gives back what was written. Every trace
    /// that can be built can be written, and nothing is lost on the way
    /// back: not a bit pattern, not a caller key, not the order of two
    /// events on one tick.
    #[test]
    fn writing_then_reading_gives_back_the_same_trace(trace in trace()) {
        prop_assert_eq!(parse(&write(&trace)), Ok(trace));
    }

    /// And the other direction, for text the writer produced: writing
    /// what was read gives back the same bytes.
    #[test]
    fn reading_then_writing_gives_back_the_same_text(trace in trace()) {
        let text = write(&trace);
        let read = parse(&text).expect("the writer's own output reads back");
        prop_assert_eq!(write(&read), text);
    }

    /// The reader answers for every string. No panic, no hang, no
    /// unwinding out of a codec — an answer, even if the answer is a
    /// refusal naming a line.
    #[test]
    fn any_text_at_all_gets_an_answer(text in ".{0,400}") {
        match parse(&text) {
            Ok(trace) => prop_assert!(!write(&trace).is_empty()),
            Err(error) => prop_assert!(error.line() >= 1),
        }
    }

    /// The same, over text shaped like a trace: mutations of a real file
    /// reach far deeper into the reader than random characters do.
    ///
    /// The caller-key tail varies too, and with `=` in its alphabet. It
    /// is the half of the header the codec does not interpret, which
    /// makes it the half where a parsing mistake has nothing else to
    /// trip over — and leaving it fixed at `sample=s …` was how a
    /// deliberately broken split rule once passed the whole suite.
    #[test]
    fn text_shaped_like_a_trace_gets_an_answer(
        version in "[0-9]{0,3}",
        ticks in "[0-9a-z]{0,6}",
        tail in "( ?[a-z]{0,4}(=[a-z0-9=]{0,6})?){0,3}",
        line in "e [0-9]{0,4} (key|pointer|button|wheel|focus|resize|scale|redraw|close|gamepad)( [0-9a-fx:-]{0,18}){0,3}",
    ) {
        let text = format!(
            "renew-trace {version} sample=s ticks={ticks} timestep_ns=1 budget=1{tail}\n{line}\n"
        );
        match parse(&text) {
            Ok(trace) => prop_assert_eq!(parse(&write(&trace)), Ok(trace)),
            Err(error) => prop_assert!(error.line() >= 1),
        }
    }
}

/// The split rule, written out rather than generated: a header field is
/// split at its **first** `=`, so everything after that belongs to the
/// value.
///
/// This is the case a fuzzer with the wrong alphabet cannot reach and the
/// one a reader broken to split at the last `=` fails immediately. It is
/// a fixed input on purpose — the rule is one sentence in the format's
/// documentation, and one sentence deserves one assertion that says the
/// same thing.
#[test]
fn a_header_field_is_split_at_its_first_equals_sign() {
    let text = "renew-trace 0 sample=s ticks=1 timestep_ns=1 budget=1 k=v=w\n";
    let trace = parse(text).expect("a value may carry an equals sign");
    assert_eq!(
        trace.header().keys(),
        [("k".to_string(), "v=w".to_string())]
    );
    assert_eq!(trace.header().value("k"), Some("v=w"));
    // One key in, one key out, and the same bytes back.
    assert_eq!(write(&trace), text);

    // The positional field is a value in the same sense, which is why
    // the no-equals rule binds keys only and cannot bind both halves.
    let odd = "renew-trace 0 sample=a=b ticks=1 timestep_ns=1 budget=1\n";
    let trace = parse(odd).expect("the sample name is a value too");
    assert_eq!(trace.header().sample(), "a=b");
    assert_eq!(write(&trace), odd);
}
