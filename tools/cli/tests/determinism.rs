//! `renew determinism`, end to end through the real binary.
//!
//! The unit tests beside the comparison prove its rules. What they cannot
//! prove is that the subcommand is *wired* — and being unwired is the
//! specific danger here, because the dispatcher's last arm sends anything
//! it does not recognise to a runner with an empty step list, which
//! reports success having done nothing. For a gate whose entire purpose
//! is refusing to pass vacuously, that failure mode is the one worth a
//! test through the process boundary.
//!
//! The comparison half is exercised against hand-written reports rather
//! than by running the simulations three times: what is under test here
//! is the wiring and the exit codes, and a report is a report whether a
//! simulation or a test wrote it.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

// Fallible on purpose, matching the sibling suites: the expect() lives
// inside each #[test], where the lint configuration scopes it.
fn run(arguments: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_renew"))
        .args(arguments)
        .output()
}

fn scratch(tag: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("renew-det-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// A report as `--emit` writes one, with every digest the pinned list
/// binds set to the value the caller names.
///
/// The whole set, not a stand-in: the comparison holds each leg against
/// the pinned digest names, so a leg carrying one invented name is a leg
/// that ran a fraction of the claim, and these tests are about the
/// wiring rather than about that refusal. Each report claims its own
/// (os, arch) row — a duplicated row is one target reported twice, and
/// the comparison refuses that as inconclusive.
fn report(os: &str, arch: &str, digest: &str) -> String {
    let names = renew_cli::determinism::expected_digest_names();
    let digests: Vec<String> = names
        .iter()
        .map(|name| format!("    \"{name}\": \"{digest}\""))
        .collect();
    format!(
        concat!(
            "{{\n  \"schema_version\": 1,\n  \"os\": \"{}\",\n  \"arch\": \"{}\",\n",
            "  \"toolchain\": \"rustc 1.0.0\",\n  \"digests\": {{\n{}\n  }}\n}}\n"
        ),
        os,
        arch,
        digests.join(",\n")
    )
}

/// One digest name the pinned list binds, for a test that needs to name
/// one in an assertion. Empty if the list is somehow empty — which the
/// assertion using it then reports as the mismatch it is.
fn a_pinned_digest_name() -> String {
    renew_cli::determinism::expected_digest_names()
        .first()
        .cloned()
        .unwrap_or_default()
}

/// Write three reports and hand them to the comparison.
fn compare_three(tag: &str, digests: [&str; 3]) -> std::io::Result<Output> {
    let dir = scratch(tag)?;
    let rows = [
        ("linux", "x86_64"),
        ("windows", "x86_64"),
        ("macos", "aarch64"),
    ];
    let mut paths = Vec::new();
    for (index, (digest, (os, arch))) in digests.iter().zip(rows).enumerate() {
        let path = dir.join(format!("leg{index}.json"));
        fs::write(&path, report(os, arch, digest))?;
        paths.push(path.to_string_lossy().into_owned());
    }
    let mut arguments = vec!["determinism".to_string()];
    for path in paths {
        arguments.push("--compare".to_string());
        arguments.push(path);
    }
    let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();
    run(&borrowed)
}

#[test]
fn three_agreeing_reports_exit_zero() {
    let output = compare_three("agree", ["0xabc", "0xabc", "0xabc"]).expect("the binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "agreement must exit 0; stdout {stdout}, stderr {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("agree"), "{stdout}");
}

/// The finding this whole lane exists to produce, through the process
/// boundary: a non-zero exit, because a message nobody's CI reads is not
/// a gate.
#[test]
fn one_disagreeing_report_exits_non_zero_and_names_the_digest() {
    let output = compare_three("diverge", ["0xabc", "0xabc", "0xdef"]).expect("the binary runs");
    assert!(!output.status.success(), "divergence must not exit 0");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains(&a_pinned_digest_name()), "{text}");
    assert!(text.contains("0xdef"), "{text}");
}

/// An unrunnable comparison must never read like a passing one.
#[test]
fn a_missing_report_exits_non_zero() {
    let output = run(&["determinism", "--compare", "no-such-file.json"]).expect("the binary runs");
    assert!(!output.status.success(), "a missing report must not exit 0");
}

/// Neither mode is a subcommand asked to do nothing. Exit 2 is this
/// binary's documented "the command line was unreadable", distinct from
/// exit 1's "it ran and failed".
#[test]
fn determinism_without_a_mode_is_a_usage_error() {
    let output = run(&["determinism"]).expect("the binary runs");
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The wiring test proper. `--json` proves the envelope is produced by
/// the determinism path rather than by the do-nothing fallback the
/// dispatcher's last arm would give an unrecognised subcommand.
#[test]
fn the_json_envelope_names_the_subcommand_that_ran() {
    let output = compare_three("envelope", ["0xabc", "0xabc", "0xabc"]).expect("the binary runs");
    assert!(output.status.success());

    let dir = scratch("envelope-json").expect("scratch");
    let path = dir.join("leg.json");
    fs::write(&path, report("linux", "x86_64", "0xabc")).expect("write");
    let json = run(&[
        "--json",
        "determinism",
        "--compare",
        &path.to_string_lossy(),
    ])
    .expect("the binary runs");
    let stdout = String::from_utf8_lossy(&json.stdout);
    // One leg against three rows: a failure, and the envelope must say so
    // rather than omit the field.
    assert!(stdout.contains("\"command\":\"determinism\""), "{stdout}");
    assert!(stdout.contains("\"schema_version\""), "{stdout}");
    assert!(!json.status.success(), "one leg is not three: {stdout}");
}

/// A report that exists and is not a report. Distinct from a missing one
/// on purpose: the reasons a comparison cannot run are worth telling
/// apart, and "I could not read this" sends its reader somewhere
/// different from "this was not there".
#[test]
fn a_malformed_report_exits_non_zero_and_names_the_file() {
    let dir = scratch("malformed").expect("scratch");
    let path = dir.join("broken.json");
    fs::write(&path, "{ this is not a report").expect("write");
    let output =
        run(&["determinism", "--compare", &path.to_string_lossy()]).expect("the binary runs");
    assert!(
        !output.status.success(),
        "a malformed report must not exit 0"
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("broken.json"), "{text}");
}
