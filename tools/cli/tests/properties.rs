//! Property-based tests for the hand-rolled JSON reader.
//!
//! **This is the third parser of external data in the workspace and was
//! the only one whose robustness rested on examples.** The pack format
//! and the trace codec each carry a generated-input suite with an
//! "any input at all gets an answer" property; this reader had sixteen
//! unit tests and a set of hostile documents written by hand. Sixteen
//! good examples are sixteen good examples — they cannot say anything
//! about the inputs nobody thought of, which is the whole reason the
//! reader has a depth bound in the first place.
//!
//! What it consumes is genuinely external: `cargo metadata` output and
//! coverage reports produced by another program, neither of which this
//! workspace writes.

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use renew_cli::json::{Value, parse};

/// Strings that exercise the escape writer: quotes, backslashes, the
/// named control escapes, sub-`0x20` bytes that become `\u00xx`, and
/// ordinary text. Generated from parts rather than `any::<String>()` so
/// the interesting characters actually appear instead of turning up once
/// in a thousand cases.
fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        proptest::sample::select(vec![
            "a",
            "Z",
            "0",
            " ",
            "\"",
            "\\",
            "\n",
            "\r",
            "\t",
            "\u{1}",
            "\u{1f}",
            "/",
            "{",
            "}",
            "[",
            "]",
            ":",
            ",",
            "\u{e9}",
            "\u{1f600}",
        ]),
        0..8,
    )
    .prop_map(|parts| parts.concat())
}

/// A whole document. Recursive, because nesting is where a reader that
/// tracks depth wrongly goes wrong, and shallow-only generation would
/// never reach the bound-adjacent cases.
///
/// **Non-finite floats are excluded deliberately**: JSON cannot spell
/// them, the reader refuses them on input, and so a `Float` holding one
/// is unreachable from any document. Generating them would test the
/// behaviour of a value the parser cannot produce.
fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Number),
        any::<f64>()
            .prop_filter("JSON has no non-finite numbers", |f| f.is_finite())
            .prop_map(Value::Float),
        text().prop_map(Value::String),
    ];
    leaf.prop_recursive(6, 48, 6, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            proptest::collection::vec((text(), inner), 0..6).prop_map(Value::Object),
        ]
    })
}

proptest! {
    // Fixed seed, matching every other property suite in the tree: the
    // same inputs are explored on every run and every machine, so a
    // failure reproduces from the message alone.
    #![proptest_config(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x0d5e_7a11),
        ..ProptestConfig::default()
    })]

    /// **Any** text at all gets an answer rather than a panic. The
    /// weakest property here and the one that would have caught the most.
    #[test]
    fn any_text_at_all_gets_an_answer(raw in ".{0,400}") {
        let _ = parse(&raw);
    }

    /// And so does anything shaped like a document, which reaches far
    /// deeper than random text ever does: random input dies at the first
    /// byte, so without this the reader below that point would be
    /// exercised by nothing.
    #[test]
    fn text_shaped_like_json_gets_an_answer(
        document in value(),
        cut in 0usize..400,
        noise in ".{0,16}",
    ) {
        let mut rendered = document.render();
        rendered.truncate(
            (0..=cut.min(rendered.len()))
                .rev()
                .find(|at| rendered.is_char_boundary(*at))
                .unwrap_or(0),
        );
        rendered.push_str(&noise);
        let _ = parse(&rendered);
    }

    /// Writing then reading gives back what was written — for every value
    /// the reader itself can produce.
    ///
    /// This is the property that found something. An integral float
    /// rendered as `2`, which the reader took back as `Number(2)`,
    /// because the integer branch claims any lexeme without `.` or `e`.
    /// The value survived the trip; its type did not.
    #[test]
    fn rendering_then_parsing_returns_the_same_value(document in value()) {
        let rendered = document.render();
        let read = parse(&rendered);
        prop_assert!(read.is_ok(), "own output rejected: {rendered:?} -> {read:?}");
        prop_assert_eq!(read.expect("checked above"), document, "via {}", rendered);
    }

    /// Reading then writing gives back the same text, which is the claim
    /// the `--json` contract actually needs: a document this tool passes
    /// through is byte-identical, not merely equivalent.
    #[test]
    fn parsing_then_rendering_returns_the_same_text(document in value()) {
        let once = document.render();
        let read = parse(&once).expect("own output parses");
        prop_assert_eq!(read.render(), once);
    }
}

/// Nesting past the documented bound is refused, and refusing is not the
/// same as surviving: the point of the bound is that neither answer is a
/// stack overflow, so both sides of it are checked here.
///
/// A fixed input rather than a generated one — the bound is a specific
/// number in the reader's documentation, and the cases that matter are
/// the two either side of it plus one far past.
#[test]
fn nesting_is_bounded_rather_than_fatal() {
    for (depth, expected_ok) in [(120usize, true), (128, true), (129, false), (4096, false)] {
        let document = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let outcome = parse(&document);
        assert_eq!(
            outcome.is_ok(),
            expected_ok,
            "depth {depth} should {} — got {outcome:?}",
            if expected_ok { "parse" } else { "be refused" }
        );
    }
}

/// The integral-float case, written out rather than left to generation.
///
/// The property above covers it, but a generated failure names a seed
/// while this names the rule: a float that happens to be whole still
/// reads back as a float, because `2` and `2.0` are different values to
/// this reader even though JSON does not distinguish them.
#[test]
fn a_whole_float_stays_a_float_across_a_round_trip() {
    for text in ["2.0", "-0.0", "1e2", "0.0"] {
        let once = parse(text).expect("parses");
        let again = parse(&once.render()).expect("re-parses");
        assert_eq!(once, again, "{text} did not survive a round trip");
        assert!(
            matches!(once, Value::Float(_)),
            "{text} should read as a float, got {once:?}"
        );
    }
}
