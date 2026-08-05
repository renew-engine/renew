//! The binary, run as a binary.
//!
//! Everything else here drives the library directly, which is faster and says
//! nothing about whether the executable works. This spawns it — the same way
//! the determinism lane will on three platforms — so the process shell, the
//! argument plumbing and the exit codes are exercised as a caller meets them.

use std::process::Command;

/// The binary cargo just built, whichever profile and target directory that is.
const BINARY: &str = env!("CARGO_BIN_EXE_chess");

/// Run it, and give back what it said and whether it succeeded.
///
/// The allowance is explicit because the crate's lint configuration exempts
/// test bodies and this is a free helper beside them. A binary cargo has just
/// built and cannot run is not a condition worth encoding a recovery for.
#[expect(
    clippy::expect_used,
    reason = "a test fixture; a binary that will not launch should fail loudly"
)]
fn invoke(arguments: &[&str]) -> (bool, String, String) {
    let output = Command::new(BINARY)
        .args(arguments)
        .output()
        .expect("the binary cargo just built should run");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn the_binary_counts_and_prints_the_published_number() {
    let (ok, out, _) = invoke(&["--count", "--depth", "3"]);
    assert!(ok, "a well-formed run should succeed");
    assert!(out.starts_with("chess count depth=3"), "got: {out}");
    assert!(
        out.contains("nodes=8902"),
        "the published count for depth three: {out}"
    );
}

/// The same command answers the same, which is what makes comparing three
/// platforms' output mean anything.
#[test]
fn the_binary_answers_the_same_twice() {
    let first = invoke(&["--play", "--depth", "20"]).1;
    let second = invoke(&["--play", "--depth", "20"]).1;
    assert_eq!(first, second, "the same command must answer the same");
    assert!(
        first.contains("digest="),
        "and must answer with one: {first}"
    );
}

#[test]
fn the_binary_prints_json_when_asked() {
    let (ok, out, _) = invoke(&["--count", "--depth", "2", "--json"]);
    assert!(ok);
    let line = out.trim();
    assert!(line.starts_with('{') && line.ends_with('}'), "got: {line}");
    assert!(line.contains("\"schema_version\":1"), "got: {line}");
    assert!(line.contains("\"sample\":\"chess\""), "got: {line}");
    assert!(line.contains("\"nodes\":400"), "got: {line}");
}

#[test]
fn the_binary_takes_a_position() {
    let (ok, out, _) = invoke(&[
        "--count",
        "--depth",
        "2",
        "--fen",
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    ]);
    assert!(ok, "a published position should parse");
    assert!(out.contains("nodes=2039"), "the published count: {out}");
}

#[test]
fn the_binary_prints_usage_and_stops() {
    let (ok, out, _) = invoke(&["--help"]);
    assert!(ok, "help is not a failure");
    assert!(out.contains("--depth"), "got: {out}");
    assert!(!out.contains("nodes="), "and it counted nothing");
}

/// A refusal goes to the error stream with a failing status, so a script
/// calling this can tell without parsing anything.
#[test]
fn the_binary_refuses_a_bad_command_line_on_the_error_stream() {
    let (ok, out, err) = invoke(&["--wat"]);
    assert!(!ok, "an unknown flag must fail");
    assert!(out.is_empty(), "and print nothing to the output stream");
    assert!(err.contains("--wat"), "while naming the flag: {err}");

    let (ok, _, err) = invoke(&["--fen", "banana"]);
    assert!(!ok);
    assert!(err.contains("banana"), "got: {err}");

    let (ok, _, err) = invoke(&["--count", "--depth", "9"]);
    assert!(!ok, "a depth that would outlive the caller must fail");
    assert!(err.contains('9'), "and say how deep is too deep: {err}");
}
