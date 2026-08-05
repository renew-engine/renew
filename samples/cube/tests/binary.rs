//! The binary, run as a binary.
//!
//! Everything else here drives the library directly, which is faster and says
//! nothing about whether the executable works. This spawns it — the same way
//! the determinism lane does on three platforms — so the process shell, the
//! argument plumbing and the exit codes are exercised as a caller meets them.

use std::process::Command;

/// The binary cargo just built, whichever profile and target directory that is.
const BINARY: &str = env!("CARGO_BIN_EXE_cube");

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
fn the_binary_runs_a_script_and_prints_a_digest() {
    let (ok, out, _) = invoke(&["--script", "patrol", "--ticks", "120"]);
    assert!(ok, "a well-formed run should succeed");
    assert!(out.starts_with("cube script=patrol"), "got: {out}");
    assert!(out.contains("ticks=120"), "got: {out}");
    assert!(out.contains("digest="), "got: {out}");
}

/// **The property the lane exists for**: the same command answers the same,
/// which is what makes comparing three platforms' output meaningful.
#[test]
fn the_binary_answers_the_same_twice() {
    let first = invoke(&["--script", "build", "--ticks", "150"]).1;
    let second = invoke(&["--script", "build", "--ticks", "150"]).1;
    assert_eq!(first, second, "the same command must answer the same");
    assert!(!first.trim().is_empty(), "and must answer at all");
}

#[test]
fn the_binary_prints_json_when_asked() {
    let (ok, out, _) = invoke(&["--script", "stand", "--ticks", "60", "--json"]);
    assert!(ok);
    let line = out.trim();
    assert!(line.starts_with('{') && line.ends_with('}'), "got: {line}");
    assert!(line.contains("\"schema_version\":1"), "got: {line}");
    assert!(line.contains("\"sample\":\"cube\""), "got: {line}");
}

#[test]
fn the_binary_prints_usage_and_stops() {
    let (ok, out, _) = invoke(&["--help"]);
    assert!(ok, "help is not a failure");
    assert!(out.contains("--script"), "got: {out}");
    assert!(!out.contains("digest="), "and it did not run anything");
}

/// A refusal goes to the error stream and sets a failing status, so a script
/// calling this can tell without parsing anything.
#[test]
fn the_binary_refuses_a_bad_command_line_on_the_error_stream() {
    let (ok, out, err) = invoke(&["--wat"]);
    assert!(!ok, "an unknown flag must fail");
    assert!(out.is_empty(), "and print nothing to the output stream");
    assert!(err.contains("--wat"), "while naming the flag: {err}");

    let (ok, _, err) = invoke(&["--script", "fly"]);
    assert!(!ok);
    assert!(err.contains("fly"), "got: {err}");
}
