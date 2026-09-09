//! Write the reader's seed corpus.
//!
//! **Every input here is first-party**, so no licence question comes
//! with the corpus. That is the same rule the sample atlases and the
//! image decoder's seeds follow: a borrowed file would be a dependency
//! with a licence, and a directory of them would be a dependency nobody
//! recorded.
//!
//! A fuzzer finds its own way past a quote eventually, but it wastes
//! most of a budget doing it. These seeds put it on the far side of each
//! early refusal: the valid documents give it a shape to mutate — one of
//! every kind, a nesting at exactly the depth limit, a string carrying
//! every escape the format defines, and one shaped like the asset
//! metadata this reader was sized for — and each malformed one lands on
//! a refusal no other seed reaches, so a mutation of it starts from
//! somewhere the random walk rarely gets to.
//!
//! Between them the seeds reach twenty-six different answers, counted
//! rather than guessed: every refusal the parse can make, plus `Ok`. The
//! replay test beside the crate holds a floor under that count **and**
//! names five refusals that must stay reachable, because a count alone
//! does not notice one specific guard going unseeded.
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-json --example make_json_corpus
//! ```
//!
//! Existing files are left alone. The fuzzer adds its own finds to this
//! directory over time, and this program must never delete them.
//!
//! It exits non-zero if any seed it meant to write is missing afterwards.
//! Every bail here prints its reason, because a generator that prints a
//! failure and then reports success is worse than one that crashes: the
//! caller sees a zero and believes the corpus is whole.
//!
//! **The seeds are named `.seed` rather than `.json`, deliberately.**
//! Most of them are not documents: they are byte strings written to be
//! wrong, and two are wrong in exactly the ways the tree's sweep over
//! source text exists to catch — one opens with a byte order mark, one
//! carries a raw tab. Calling them JSON would put a directory of
//! deliberate damage in front of a guard whose whole job is to find
//! accidental damage. The extension was never durable in any case:
//! `cargo fuzz cmin` renames what it keeps to a bare content hash.

// The crate bans filesystem access because the library never touches a
// file -- a caller that reads one owns it, and owns the bound on reading
// it. This program is that caller: writing the corpus is its whole job,
// and the same allowance sits on the replay test beside it.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes and never a path.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;
use std::process::ExitCode;

/// The deepest nesting the reader follows.
///
/// Written out here rather than read from the crate, for the same reason
/// the image decoder's generator writes out its own checksum: this
/// program is test support, and widening a public surface so a generator
/// can borrow a constant would be the worse trade. It is not a
/// duplicated *rule* either way — the reader's own tests pin the limit,
/// and if these two ever disagree the replay gate says so, because the
/// seed that should reach `DepthLimit` stops reaching it.
const DEPTH: usize = 64;

/// A string of `codes` written as unicode escapes, quotes included.
///
/// Assembled from its pieces rather than written as a literal, so that
/// nothing in this file has to be read through two layers of escaping at
/// once — Rust's and the format's.
fn escapes(before: &str, codes: &[&str], after: &str) -> String {
    let mut out = String::from("\"");
    out.push_str(before);
    for code in codes {
        out.push('\\');
        out.push('u');
        out.push_str(code);
    }
    out.push_str(after);
    out.push('"');
    out
}

/// `depth` nested arrays around a single number.
fn nested(depth: usize) -> String {
    let mut out = String::with_capacity(depth * 2 + 1);
    for _ in 0..depth {
        out.push('[');
    }
    out.push('1');
    for _ in 0..depth {
        out.push(']');
    }
    out
}

/// A document shaped like the asset metadata this reader was sized for.
///
/// Written out here rather than copied from any real file: sibling
/// arrays with indices between them, a nested object, an empty array, a
/// member holding arbitrary application data, and numbers in every
/// spelling a whole number is allowed to take. That last part is what
/// makes it a *useful* seed rather than a decorative one — it is the
/// shape a mutation has to start from to reach the number reader at all.
fn scene() -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"asset\": {\"version\": \"2.0\", \"generator\": \"renew\"},\n");
    out.push_str("  \"scene\": 0,\n");
    out.push_str("  \"scenes\": [{\"nodes\": [0, 1]}],\n");
    out.push_str("  \"nodes\": [\n");
    out.push_str("    {\"mesh\": 0, \"translation\": [0.0, -1.5, 2.25e0]},\n");
    out.push_str("    {\"mesh\": 0, \"children\": []}\n");
    out.push_str("  ],\n");
    out.push_str("  \"meshes\": [\n");
    out.push_str(
        "    {\"name\": \"hull\", \"primitives\": [{\"attributes\": {\"POSITION\": 0}, \"indices\": 1}]}\n",
    );
    out.push_str("  ],\n");
    out.push_str("  \"accessors\": [\n");
    out.push_str("    {\"componentType\": 5126, \"count\": 24.0, \"type\": \"VEC3\"},\n");
    out.push_str(
        "    {\"componentType\": 5.123e3, \"count\": 36, \"normalized\": false, \"extras\": {\"note\": null}}\n",
    );
    out.push_str("  ]\n");
    out.push('}');
    out
}

/// A document that exercises the numeric readings a caller makes after
/// the parse has already said yes.
///
/// Four of this crate's refusals are not the parse's at all: they happen
/// when somebody asks for a whole number and finds a fraction, or asks
/// for a `u32` and finds something larger. The fuzz target walks an
/// accepted document and asks every question of every value, so this
/// seed is what gets it to those four.
fn numbers() -> String {
    "[0,-0,1,-1,3.0,3e2,30e-1,0.03e2,3.5,1e-1,9007199254740993,18446744073709551615,\
     99999999999999999999999999999999999999999,1e999,3e300,1e-999,-9223372036854775808]"
        .to_owned()
}

// The seed table is the body of this program, and it is one table on
// purpose: every entry is a name and the bytes it names, and splitting
// it into groups would put the count in two places.
#[expect(
    clippy::too_many_lines,
    reason = "one table of seeds, kept whole so the count lives in one place"
)]
fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/json_parse");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("{}: {error}", dir.display());
        return ExitCode::FAILURE;
    }

    // A string carrying every escape the format defines, including a
    // surrogate pair — the two-escape spelling of a character outside
    // the basic plane, which is what a writer that escapes all non-ASCII
    // emits and which nothing else in the corpus reaches.
    let every_escape = {
        let mut out = String::from("[\"");
        out.push_str(r#"\" \\ \/ \b \f \n \r \t "#);
        out.push('\\');
        out.push('u');
        out.push_str("0041");
        out.push('"');
        out.push(',');
        out.push_str(&escapes("emoji ", &["D83D", "DE00"], " end"));
        out.push(']');
        out
    };

    // A byte string that is not text at all. Every other seed here is,
    // so without this the reader's very first check has no input.
    let not_text = vec![b'"', 0xFF, 0xFE, b'"'];

    let byte_order_mark = {
        let mut out = String::from("\u{feff}");
        out.push_str("{}");
        out
    };

    let long_number = {
        let mut out = String::from("1");
        for _ in 0..200 {
            out.push('0');
        }
        out
    };

    let text_seeds: [(&str, String); 36] = [
        // Valid documents: one of every kind, and the shapes worth
        // mutating.
        ("valid-null.seed", "null".to_owned()),
        ("valid-true.seed", "true".to_owned()),
        ("valid-false.seed", "false".to_owned()),
        ("valid-number.seed", "-12.75e+3".to_owned()),
        ("valid-string.seed", "\"hull\"".to_owned()),
        ("valid-empty-object.seed", "{}".to_owned()),
        ("valid-empty-array.seed", "[]".to_owned()),
        ("valid-escapes.seed", every_escape),
        ("valid-numbers.seed", numbers()),
        ("valid-scene.seed", scene()),
        // Duplicate member names, which the format blesses and this
        // reader resolves last-wins rather than refusing.
        (
            "valid-duplicate-names.seed",
            "{\"a\":1,\"b\":2,\"a\":3}".to_owned(),
        ),
        // Nesting at exactly the limit: the seed a mutation only has to
        // lengthen by one to reach the refusal.
        ("valid-deep.seed", nested(DEPTH)),
        // Whitespace on both sides, which is the shape a JSON chunk
        // padded out to an alignment boundary by a container format
        // arrives in.
        ("valid-padded.seed", "  \t\r\n{\"a\":1}\n  ".to_owned()),
        // One malformed document per refusal the parse can make.
        ("refuse-empty.seed", String::new()),
        ("refuse-byte-order-mark.seed", byte_order_mark),
        ("refuse-no-value.seed", "@".to_owned()),
        ("refuse-ends-early.seed", "[".to_owned()),
        ("refuse-not-json-literal.seed", "NaN".to_owned()),
        ("refuse-bad-literal.seed", "tru".to_owned()),
        ("refuse-trailing-text.seed", "1 2".to_owned()),
        ("refuse-too-deep.seed", nested(DEPTH + 1)),
        ("refuse-expected-key.seed", "{1:2}".to_owned()),
        ("refuse-expected-colon.seed", "{\"a\" 1}".to_owned()),
        (
            "refuse-expected-comma-or-brace.seed",
            "{\"a\":1 \"b\":2}".to_owned(),
        ),
        ("refuse-trailing-comma.seed", "[1,]".to_owned()),
        ("refuse-expected-comma-or-bracket.seed", "[1 2]".to_owned()),
        ("refuse-control-in-string.seed", "\"a\nb\"".to_owned()),
        ("refuse-bad-escape.seed", "\"\\q\"".to_owned()),
        ("refuse-bad-hex-escape.seed", escapes("", &["ZZZZ"], "")),
        (
            "refuse-lone-high-surrogate.seed",
            escapes("", &["D800"], ""),
        ),
        ("refuse-lone-low-surrogate.seed", escapes("", &["DC00"], "")),
        ("refuse-leading-plus.seed", "+1".to_owned()),
        ("refuse-leading-zero.seed", "01".to_owned()),
        ("refuse-no-integer-digits.seed", ".5".to_owned()),
        ("refuse-no-fraction-digits.seed", "1.".to_owned()),
        ("refuse-no-exponent-digits.seed", "1e".to_owned()),
    ];

    let mut seeds: Vec<(&str, Vec<u8>)> = text_seeds
        .into_iter()
        .map(|(name, text)| (name, text.into_bytes()))
        .collect();
    seeds.push(("refuse-not-text.seed", not_text));
    seeds.push(("refuse-number-too-long.seed", long_number.into_bytes()));

    let mut written = 0usize;
    let mut kept = 0usize;
    let mut failed = 0usize;
    for (name, bytes) in seeds {
        let path = dir.join(name);
        if path.exists() {
            kept += 1;
            continue;
        }
        if let Err(error) = std::fs::write(&path, &bytes) {
            eprintln!("{}: {error}", path.display());
            failed += 1;
            continue;
        }
        written += 1;
    }
    println!("{written} written, {kept} already present, {failed} failed");
    // The loop keeps going past a failed write so that one unwritable
    // path does not cost the rest of the corpus, which is why the count
    // has to be carried out to here: without it a run that wrote nothing
    // at all still exits zero.
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
