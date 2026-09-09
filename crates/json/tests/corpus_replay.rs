//! The recorded fuzz corpus, replayed on the stable toolchain — the same
//! claim the other parsers' gates make: every committed input answers,
//! `Ok` or a named refusal, in a merge-gating run.
//!
//! The fuzz workspace needs nightly and runs on its own schedule. This
//! test is what puts the corpus in front of every merge: if a change
//! makes a recorded input panic or read past its end, it fails here
//! rather than on a nightly job nobody is watching.
//!
//! **It cannot catch a hang.** The harness has no per-test deadline and
//! no workflow here sets one, so an input that loops forever wedges the
//! job rather than failing it. Said plainly because the obvious reading
//! of "every input answers" is that non-answers are caught, and one kind
//! is not.

// The tripwire ban on filesystem access protects engine code; replaying
// committed corpus artifacts is this test's whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes, never a path,
// but this harness's whole subject is a directory of committed files.
#![allow(clippy::disallowed_types)]
// A missing or unreadable corpus is a broken checkout, not a condition
// this harness recovers from -- and these live in helpers rather than in
// the test bodies, where the lints would allow them, because reading the
// corpus by content is what stops the tests naming files the corpus
// procedure is required to rename.
#![allow(clippy::panic, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use renew_json::{Json, JsonErrorKind, Kind, Value};

/// The committed corpus never shrinks below this many **distinct**
/// inputs.
///
/// Distinct by content, not by directory entry: counting entries lets a
/// corpus be padded back to strength with copies of one file, which is a
/// mutation this floor is supposed to refuse and would otherwise admit.
const LOW_WATER: usize = 30;

/// How many distinct outcomes the seeds must still reach between them.
///
/// **This is the assertion that stops the corpus rotting into
/// uselessness.** A corpus can keep its file count while every input
/// decays to the same early refusal, and a count-only gate would stay
/// green through it. The committed seeds reach twenty-six — every
/// refusal the parse can make, plus `Ok`; the floor sits four below so
/// the fuzzer's own minimisation has room, and no lower, because slack
/// here is exactly how many of the reader's refusals may go unseeded
/// without anyone noticing.
const DISTINCT_OUTCOMES: usize = 22;

/// Refusals a seed must still provoke, each guarding something a count
/// cannot.
///
/// **A distinct-count floor is not enough, and the reason is
/// arithmetic.** Deleting one guard does not necessarily remove an
/// answer from the corpus — the seed that reached it falls through to
/// whatever the next check says, which other seeds already produce, so
/// the total drops by one and clears any floor loose enough to let the
/// fuzzer minimise. These five are the guards where that would matter
/// most:
///
/// * `DepthLimit` is the one that turns a deeply nested document from a
///   stack overflow into an answer. Losing it is the difference between
///   a refusal and a crash.
/// * `NumberTooLong` is the bound on how much work one token can cost.
/// * `LoneHighSurrogate` is what stops half a character being silently
///   substituted inside a name a caller is about to compare.
/// * `NotUtf8` is the check every offset in every other refusal rests
///   on, and the only seed here that is not text.
/// * `LeadingZero` is the place the standard library's own number
///   grammar is wider than the format's, so it is the refusal most
///   easily lost by delegating.
const REQUIRED_OUTCOMES: [&str; 5] = [
    "DepthLimit",
    "NumberTooLong",
    "LoneHighSurrogate",
    "NotUtf8",
    "LeadingZero",
];

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/json_parse")
}

/// Every committed input, as bytes.
///
/// Read by content and never by name. `cargo fuzz cmin` renames what it
/// keeps to a content hash, so a test that names a file forbids the
/// minimisation the corpus's own procedure requires.
fn corpus() -> Vec<Vec<u8>> {
    let dir = corpus_dir();
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|error| {
        panic!(
            "the committed corpus at {} must exist: {error}",
            dir.display()
        )
    });
    entries
        .map(|entry| {
            let entry = entry.expect("corpus entries are readable");
            std::fs::read(entry.path()).expect("corpus files are readable")
        })
        .collect()
}

/// The answer a byte string gets, as a name — the variant alone, without
/// its fields, so two truncations at different offsets count as one
/// answer.
///
/// **Matched on the enum, and with no wildcard**, which this test can do
/// where the image decoder's twin cannot: `JsonErrorKind` is closed, so
/// a refusal added later stops this file compiling until somebody
/// decides whether a seed should reach it. That is the whole benefit of
/// the enum being closed, collected here.
///
/// The last four arms are the refusals a *caller* provokes rather than
/// the parse — they cannot come out of `parse` at all, and they are
/// written out anyway because leaving them to a wildcard is exactly the
/// silence this function exists to refuse.
fn outcome_name(bytes: &[u8]) -> &'static str {
    let Err(error) = Json::parse(bytes) else {
        return "Ok";
    };
    match error.kind() {
        JsonErrorKind::Empty => "Empty",
        JsonErrorKind::NotUtf8 => "NotUtf8",
        JsonErrorKind::ByteOrderMark => "ByteOrderMark",
        JsonErrorKind::NoValue { .. } => "NoValue",
        JsonErrorKind::EndOfDocument { .. } => "EndOfDocument",
        JsonErrorKind::NonJsonLiteral { .. } => "NonJsonLiteral",
        JsonErrorKind::BadLiteral { .. } => "BadLiteral",
        JsonErrorKind::TrailingText { .. } => "TrailingText",
        JsonErrorKind::DepthLimit { .. } => "DepthLimit",
        JsonErrorKind::ExpectedKey { .. } => "ExpectedKey",
        JsonErrorKind::ExpectedColon { .. } => "ExpectedColon",
        JsonErrorKind::ExpectedCommaOrBraceClose { .. } => "ExpectedCommaOrBraceClose",
        JsonErrorKind::TrailingComma => "TrailingComma",
        JsonErrorKind::ExpectedCommaOrBracketClose { .. } => "ExpectedCommaOrBracketClose",
        JsonErrorKind::ControlCharacterInString { .. } => "ControlCharacterInString",
        JsonErrorKind::BadEscape { .. } => "BadEscape",
        JsonErrorKind::BadHexEscape { .. } => "BadHexEscape",
        JsonErrorKind::LoneHighSurrogate { .. } => "LoneHighSurrogate",
        JsonErrorKind::LoneLowSurrogate { .. } => "LoneLowSurrogate",
        JsonErrorKind::LeadingPlus => "LeadingPlus",
        JsonErrorKind::LeadingZero => "LeadingZero",
        JsonErrorKind::NoIntegerDigits => "NoIntegerDigits",
        JsonErrorKind::NoFractionDigits => "NoFractionDigits",
        JsonErrorKind::NoExponentDigits => "NoExponentDigits",
        JsonErrorKind::NumberTooLong { .. } => "NumberTooLong",
        JsonErrorKind::NotThisKind { .. } => "NotThisKind",
        JsonErrorKind::FractionalInteger => "FractionalInteger",
        JsonErrorKind::IntegerOutOfRange { .. } => "IntegerOutOfRange",
        JsonErrorKind::NumberNotFinite => "NumberNotFinite",
    }
}

/// **Every committed input gets an answer**, the corpus is still as big
/// as it was, and it still reaches as many different answers.
///
/// Probed by deleting the depth check in the parser: the deep seed falls
/// through and parses, `DepthLimit` disappears from the reached set, and
/// this fails naming it — while the distinct count only drops from
/// twenty-six to twenty-five, which is why the named list is here beside
/// the floor rather than instead of it.
#[test]
fn every_recorded_corpus_input_gets_an_answer() {
    let corpus = corpus();

    let mut distinct_inputs: BTreeSet<&[u8]> = BTreeSet::new();
    let mut outcomes: BTreeSet<&str> = BTreeSet::new();
    for bytes in &corpus {
        distinct_inputs.insert(bytes.as_slice());
        outcomes.insert(outcome_name(bytes));
    }

    assert!(
        distinct_inputs.len() >= LOW_WATER,
        "{} distinct inputs of {} files, below the committed floor of {LOW_WATER} — the corpus \
         has been gutted, padded with duplicates, or the checkout is broken",
        distinct_inputs.len(),
        corpus.len()
    );
    for required in REQUIRED_OUTCOMES {
        assert!(
            outcomes.contains(required),
            "no committed input provokes {required} any more — the guard it seeds is unreachable \
             from this corpus, so a change that removed that guard would not be caught here. \
             Reached: {outcomes:?}"
        );
    }
    assert!(
        outcomes.len() >= DISTINCT_OUTCOMES,
        "the corpus reached only {} distinct outcomes ({outcomes:?}), below the floor of \
         {DISTINCT_OUTCOMES} — the seeds have collapsed onto fewer refusals and are no longer \
         seeding what they were written to seed",
        outcomes.len()
    );
}

/// Every value a caller can reach, asked every question there is.
///
/// Returns how many values it visited, so the caller can tell a walk
/// that found a whole document from one that found a single `null`.
fn visit(value: Value<'_>) -> usize {
    assert!(
        !value.text().is_empty(),
        "a value with no source text of its own"
    );
    // Asking the wrong question of a value must answer, not panic.
    let _ = value.as_bool();
    let _ = value.as_str();
    let _ = value.as_u32();
    let _ = value.as_u64();
    let _ = value.as_i64();
    let _ = value.as_f32();
    let _ = value.as_f64();
    let _ = value.get("anything");
    let _ = value.index(0);

    let mut seen = 1;
    match value.kind() {
        Kind::Array => {
            let elements = value.elements().expect("an array walks as an array");
            let mut counted = 0;
            for element in elements {
                counted += 1;
                seen += visit(element);
            }
            assert_eq!(
                counted,
                value.len(),
                "an array disagrees with its own length"
            );
        }
        Kind::Object => {
            let entries = value.entries().expect("an object walks as an object");
            let mut counted = 0;
            for (name, member) in entries {
                counted += 1;
                // A name a caller can read is the whole point of an
                // object, so reading one is part of the walk.
                let decoded = name.decode();
                assert!(
                    value.get(&decoded).is_some(),
                    "an object does not hold a member it just handed over: {decoded:?}"
                );
                seen += visit(member);
            }
            assert_eq!(
                counted,
                value.len(),
                "an object disagrees with its own length"
            );
        }
        _ => {}
    }
    seen
}

/// **Some committed input still parses, and everything in it holds
/// together.**
///
/// The replay above deliberately does not care what an input answers, so
/// on its own it would stay green with a reader that refused every file
/// in the directory. This is the other half — and it walks the accepted
/// documents rather than merely counting them, because "it parsed" is a
/// claim about the bytes and "every member it hands over it can also
/// find again" is a claim about what the parse built.
///
/// **Selected by content, not by name**, for the reason the corpus
/// reader gives: naming files would forbid the minimisation the corpus's
/// own procedure requires.
///
/// Probed by making an object's length count its children rather than
/// its members: the walk fails on the first object seed — "an object
/// disagrees with its own length" — because the walk counts pairs and
/// the length counted nodes.
#[test]
fn every_committed_input_that_parses_holds_together() {
    let corpus = corpus();
    let parsed: Vec<_> = corpus
        .iter()
        .filter(|bytes| Json::parse(bytes).is_ok())
        .collect();

    assert!(
        !parsed.is_empty(),
        "no committed input parses at all — the corpus has lost every valid document, or the \
         reader refuses everything"
    );

    let mut deepest = 0;
    for bytes in parsed {
        let document = Json::parse(bytes).expect("this input parsed a moment ago");
        deepest = deepest.max(visit(document.root()));
    }
    assert!(
        deepest >= 16,
        "the largest document the corpus still holds has {deepest} values in it — the seeds that \
         gave the fuzzer a shape to mutate are gone, and what is left is scalars"
    );
}
