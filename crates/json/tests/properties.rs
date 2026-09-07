//! The reader, over documents nobody chose.
//!
//! The unit suite asserts what happens to byte strings somebody sat down
//! and wrote; this one asserts what must hold for *every* document in a
//! shape. Five properties. Each was probed by a named mutant before it
//! was committed, and the one place a probe cannot reach is written into
//! that property's own documentation rather than left for a reader to
//! find: a proper prefix of a bare number is a number, so the
//! prefix property is stated over documents whose root is a container.
//!
//! **This is not the fuzz target and does not replace it.** The fuzzer
//! explores; these are closed statements about a generated population,
//! and they run on the stable toolchain as part of the ordinary test
//! suite. The fuzz workspace needs nightly and its own schedule, so
//! between merges this is what actually exercises the reader on bytes
//! nobody wrote.

// A property body is not literally a `#[test]` fn -- the macro wraps it --
// so the lints that allow a test to panic do not see it. These are
// assertions about a reader, and a failure here is a failed test.
#![allow(clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use renew_json::{Json, Kind, MAX_DEPTH, Value};

/// A document, as a tree, before anything writes it down.
///
/// The generator makes one of these and the renderer turns it into text;
/// the properties then ask whether the reader gives the tree back. That
/// is a stronger question than "does it parse", and it is the only way
/// to ask it without trusting the reader to check itself.
#[derive(Clone, Debug, PartialEq)]
enum Doc {
    Null,
    Bool(bool),
    Int(i64),
    Text(String),
    List(Vec<Doc>),
    Map(Vec<(String, Doc)>),
}

/// The characters a generated string is built from.
///
/// Every one of them is a character a writer has to make a decision
/// about: two that must be escaped, three control characters with
/// escapes of their own, one that needs a numeric escape, and text from
/// outside ASCII that needs none at all.
const ALPHABET: [char; 12] = [
    'a',
    'Z',
    '0',
    ' ',
    '"',
    '\\',
    '\n',
    '\t',
    '\u{1}',
    '/',
    '\u{e9}',
    '\u{1f600}',
];

fn text() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(ALPHABET.as_slice()), 0..8)
        .prop_map(|chars| chars.into_iter().collect())
}

/// A tree, capped well under the reader's own depth limit.
///
/// Six levels rather than sixty-four because the cost here is real —
/// every case renders a whole document and walks it back — and depth is
/// the subject of its own property below, where it is generated
/// directly rather than left to a recursion that would almost never
/// reach the interesting number.
fn document() -> impl Strategy<Value = Doc> {
    let leaf = prop_oneof![
        Just(Doc::Null),
        any::<bool>().prop_map(Doc::Bool),
        any::<i64>().prop_map(Doc::Int),
        text().prop_map(Doc::Text),
    ];
    leaf.prop_recursive(6, 48, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Doc::List),
            prop::collection::vec((text(), inner), 0..4).prop_map(Doc::Map),
        ]
    })
}

/// A tree whose root is a container, for the prefix property.
fn container() -> impl Strategy<Value = Doc> {
    document().prop_map(|doc| match doc {
        already @ (Doc::List(_) | Doc::Map(_)) => already,
        scalar => Doc::List(vec![scalar]),
    })
}

/// A string, written the way the format says to write one.
fn quoted(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => {
                out.push('\\');
                out.push('u');
                // Four hexadecimal digits, pushed rather than
                // formatted: a formatted append into a string that
                // is already being built allocates a second one for
                // no reason.
                for shift in [12u32, 8, 4, 0] {
                    let digit = ((other as u32) >> shift) & 0xF;
                    out.push(char::from_digit(digit, 16).unwrap_or('0'));
                }
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// The tree as a document.
fn render(doc: &Doc, out: &mut String) {
    match doc {
        Doc::Null => out.push_str("null"),
        Doc::Bool(true) => out.push_str("true"),
        Doc::Bool(false) => out.push_str("false"),
        Doc::Int(value) => out.push_str(&value.to_string()),
        Doc::Text(value) => quoted(value, out),
        Doc::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                render(item, out);
            }
            out.push(']');
        }
        Doc::Map(members) => {
            out.push('{');
            for (index, (name, item)) in members.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                quoted(name, out);
                out.push(':');
                render(item, out);
            }
            out.push('}');
        }
    }
}

fn written(doc: &Doc) -> String {
    let mut out = String::new();
    render(doc, &mut out);
    out
}

/// Does what the reader hands back say the same thing as the tree?
///
/// Compared through the accessors rather than through the source text,
/// because the source text is what the reader was given and would prove
/// nothing about what it understood.
fn agrees(doc: &Doc, value: Value<'_>) -> bool {
    match doc {
        Doc::Null => value.is_null(),
        Doc::Bool(expected) => value.as_bool() == Ok(*expected),
        Doc::Int(expected) => value.as_i64() == Ok(*expected),
        Doc::Text(expected) => value.as_str().is_ok_and(|text| text.eq_str(expected)),
        Doc::List(items) => value.elements().is_ok_and(|walk| {
            let read: Vec<_> = walk.collect();
            read.len() == items.len()
                && items
                    .iter()
                    .zip(read)
                    .all(|(item, element)| agrees(item, element))
        }),
        Doc::Map(members) => value.entries().is_ok_and(|walk| {
            let read: Vec<_> = walk.collect();
            read.len() == members.len()
                && members
                    .iter()
                    .zip(read)
                    .all(|((name, item), (read_name, element))| {
                        read_name.eq_str(name) && agrees(item, element)
                    })
        }),
    }
}

/// Ask every typed question of every value, and check what comes back
/// against what the value says it is.
///
/// The point is not the answers; it is that asking never panics and
/// never disagrees with [`Value::kind`]. A reader is allowed to refuse
/// anything, and is not allowed to say a value is a number and then fail
/// to hand one over.
fn walk_everything(value: Value<'_>) -> bool {
    let kind = value.kind();
    let consistent = match kind {
        Kind::Null => value.is_null() && value.as_bool().is_err() && value.as_str().is_err(),
        Kind::Bool => value.as_bool().is_ok() && value.as_u32().is_err(),
        // A number is deliberately *not* claimed to read as a double.
        // Every one of the numeric readings may legitimately refuse —
        // a fraction where a whole number was wanted, a magnitude that
        // does not fit, a value that rounds to an infinity — and the
        // claim that survives all of that is that whatever comes back
        // is finite and that a number is not also a string.
        Kind::Number => {
            !value.text().is_empty()
                && value.as_str().is_err()
                && value.as_bool().is_err()
                && match value.as_f64() {
                    Ok(number) => number.is_finite(),
                    Err(_) => true,
                }
        }
        Kind::String => value.as_str().is_ok() && value.as_i64().is_err(),
        Kind::Array => value.elements().is_ok() && value.entries().is_err(),
        Kind::Object => value.entries().is_ok() && value.elements().is_err(),
    };
    if !consistent {
        return false;
    }
    // Every accessor, on every value, whatever it is.
    let _ = value.at();
    let _ = value.text();
    let _ = value.is_empty();
    let _ = value.index(0);
    let _ = value.get("anything");
    let _ = value.get_all("anything").count();
    let _ = value.as_u64();
    let _ = value.as_u32();

    let counted = match kind {
        Kind::Array => value
            .elements()
            .is_ok_and(|walk| walk.count() == value.len())
            .then(|| value.elements().ok()),
        Kind::Object => value
            .entries()
            .is_ok_and(|walk| walk.count() == value.len())
            .then(|| value.elements().ok()),
        _ => Some(None),
    };
    let Some(children) = counted else {
        return false;
    };
    match kind {
        Kind::Array => children.is_some_and(|walk| walk.into_iter().all(walk_everything)),
        Kind::Object => value
            .entries()
            .is_ok_and(|walk| walk.into_iter().all(|(_, child)| walk_everything(child))),
        _ => true,
    }
}

proptest! {
    /// **What a writer wrote, the reader gives back — exactly.**
    ///
    /// The tree goes in, the text comes out of the renderer, and the
    /// reader has to reproduce the tree through its own accessors:
    /// every member name, in order, with duplicates left where they
    /// were, and every scalar as the type it was written as.
    ///
    /// The generated strings are the interesting half. Each is built
    /// from an alphabet of exactly the characters a writer has to make a
    /// decision about — the two that must be escaped, the control
    /// characters with escapes of their own, one that needs a numeric
    /// escape, and text from outside ASCII that needs none — so the
    /// escape validator and the escape decoder are both under this
    /// property rather than under a note saying they ought to be.
    ///
    /// Probed by decoding `\n` as a literal `n`: the property fails and
    /// shrinks to the smallest document that carries one — "the reader
    /// disagrees with the tree that was written as `{"\n":null}`".
    #[test]
    fn what_a_writer_wrote_the_reader_gives_back(doc in document()) {
        let source = written(&doc);
        let parsed = Json::parse(source.as_bytes())
            .unwrap_or_else(|error| panic!("`{source}` was refused: {error}"));
        prop_assert!(
            agrees(&doc, parsed.root()),
            "the reader disagrees with the tree that was written as `{source}`"
        );
    }

    /// **Every byte string gets an answer**, and an answer that says yes
    /// describes a document that holds together.
    ///
    /// A reader is allowed to refuse anything; it is not allowed to
    /// panic, to read past the end, or to hand back a value that says
    /// it is a number and then refuses to be one.
    ///
    /// **The mixture is load-bearing, and it took a failed probe to
    /// learn it.** Uniform random bytes essentially never parse, so a
    /// population of noise alone tests the refusal path and nothing
    /// else — the second half of this property, the walk over an
    /// accepted document, was never reached, and a mutant that panicked
    /// in that walk went green. Four sources now: raw noise; noise drawn
    /// from the characters JSON is made of; a whole rendered document;
    /// and a rendered document with one byte replaced, which is where
    /// the nearly-valid inputs come from.
    ///
    /// Probed by walking one node past what a subtree holds, with the
    /// clamp on the child walk removed: the property fails as a panic —
    /// `mid > len` — which is the exact shape the clamp exists to
    /// refuse.
    #[test]
    fn every_byte_string_gets_an_answer(
        doc in document(),
        noise in prop::collection::vec(any::<u8>(), 0..512),
        shaped in prop::collection::vec(
            prop::sample::select(br#"{}[],:"\ 0123456789.eE-+truefalsn"#.as_slice()),
            0..512,
        ),
        source in 0u8..4,
        at in any::<prop::sample::Index>(),
        replacement in any::<u8>(),
    ) {
        let rendered = written(&doc).into_bytes();
        let bytes = match source {
            0 => noise,
            1 => shaped,
            2 => rendered,
            _ => {
                let mut damaged = rendered;
                let index = at.index(damaged.len());
                if let Some(slot) = damaged.get_mut(index) {
                    *slot = replacement;
                }
                damaged
            }
        };
        if let Ok(document) = Json::parse(&bytes) {
            prop_assert!(
                walk_everything(document.root()),
                "an accepted document disagrees with itself: {:?}",
                String::from_utf8_lossy(&bytes)
            );
        }
    }

    /// **No proper prefix of a document is a document.**
    ///
    /// A half-arrived download must not look like a whole shorter file.
    /// Every prefix of a container cuts something before its closing
    /// bracket, and there is no length at which the reader should be
    /// satisfied except the whole.
    ///
    /// **Stated over containers, and that is a real limit rather than a
    /// convenience.** A proper prefix of a bare number *is* a number —
    /// `12` cut short is `1` — so the property is false at the top level
    /// for a scalar document, and would be false for any format that
    /// lets a value end at the end of the file. Saying so here is
    /// cheaper than a reader rediscovering it.
    ///
    /// Probed by closing an unterminated container when the document
    /// runs out instead of refusing it: "7 bytes of the 12-byte
    /// document `{"":[-1000]}` parsed on their own".
    #[test]
    fn no_proper_prefix_of_a_document_parses(
        doc in container(),
        cut in any::<prop::sample::Index>(),
    ) {
        let source = written(&doc);
        let keep = cut.index(source.len());
        let prefix = source.as_bytes().get(..keep).unwrap_or_default();
        prop_assert!(
            Json::parse(prefix).is_err(),
            "{keep} bytes of the {}-byte document `{source}` parsed on their own",
            source.len()
        );
    }

    /// **Nesting is bounded, and the wall is where the constant says.**
    ///
    /// The two halves have to be asserted together. A reader with no
    /// bound passes a test that only checks the shallow case; a reader
    /// that refuses at some arbitrary depth passes a test that only
    /// checks the deep one. What matters is that a document is accepted
    /// exactly while it is within the stated limit — and that going past
    /// it is a *refusal*, which is the whole reason the parser holds its
    /// own stack.
    ///
    /// The mixture of brackets matters: a reader that counted one kind
    /// of nesting and not the other would pass over arrays alone.
    ///
    /// Probed by counting only arrays towards the depth: "a document
    /// nested 65 deep was accepted", shrunk to `depth = 65, objects =
    /// true` — the smallest case that tells the two brackets apart.
    #[test]
    fn nesting_is_accepted_exactly_while_it_is_within_the_limit(
        depth in 1usize..=(MAX_DEPTH + 16),
        objects in any::<bool>(),
    ) {
        let (open, close) = if objects { ("{\"a\":", "}") } else { ("[", "]") };
        let mut source = String::new();
        for _ in 0..depth {
            source.push_str(open);
        }
        source.push('1');
        for _ in 0..depth {
            source.push_str(close);
        }

        let accepted = Json::parse(source.as_bytes()).is_ok();
        prop_assert_eq!(
            accepted,
            depth <= MAX_DEPTH,
            "a document nested {} deep was {}",
            depth,
            if accepted { "accepted" } else { "refused" }
        );
    }

    /// **A value's own text parses back to the same value.**
    ///
    /// This is what makes [`Value::text`] usable rather than decorative:
    /// a layer that wants to hand a subtree to something else — a cache,
    /// a message, a second reader — needs the bytes it cuts out to be a
    /// document in their own right. A span that was off by one at either
    /// end would still look like a plausible slice of the file.
    ///
    /// Probed by ending a container's span one byte before its closing
    /// bracket: "`[`, cut out of `[[]]`, was refused: at byte 1: the
    /// document ends before a value".
    #[test]
    fn a_value_s_own_text_parses_back_to_the_same_value(doc in container()) {
        let source = written(&doc);
        let parsed = Json::parse(source.as_bytes())
            .unwrap_or_else(|error| panic!("`{source}` was refused: {error}"));
        let root = parsed.root();
        let first = match root.kind() {
            Kind::Array => root.index(0),
            _ => root.entries().ok().and_then(|mut walk| walk.next()).map(|(_, value)| value),
        };
        let Some(child) = first else {
            return Ok(());
        };

        let cut = child.text();
        let again = Json::parse(cut.as_bytes())
            .unwrap_or_else(|error| panic!("`{cut}`, cut out of `{source}`, was refused: {error}"));
        prop_assert_eq!(again.root().text(), cut);
        prop_assert_eq!(again.root().kind(), child.kind());
    }
}
