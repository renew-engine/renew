//! Cross-process determinism: three runs of the real binary, one digest
//! line.
//!
//! In-process repetition can share a warm cache, a lazily initialized
//! table, or a global somebody forgot about. Separate processes cannot,
//! which is why the gate is three of them — and why it runs the shipped
//! binary rather than a special test build.
//!
//! Skips where there is no Vulkan runtime; `RENEW_FRAME_STRICT=1` makes
//! a skip a failure, because a lane that passes by skipping proves
//! nothing.

use std::path::PathBuf;
use std::process::Command;

/// The binary this test drives, built by Cargo alongside it.
const BINARY: &str = env!("CARGO_BIN_EXE_hello_triangle");

fn strict() -> bool {
    std::env::var_os("RENEW_FRAME_STRICT").is_some_and(|value| value == "1")
}

struct Run {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

fn run(args: &[&str]) -> Run {
    match Command::new(BINARY).args(args).output() {
        Ok(output) => Run {
            stdout: String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
            stderr: String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
            code: output.status.code(),
        },
        // Cargo built this binary beside the test; a process that will
        // not start is reported through the same assertions as a process
        // that started and misbehaved.
        Err(error) => Run {
            stdout: String::new(),
            stderr: format!("could not run {BINARY}: {error}"),
            code: None,
        },
    }
}

/// The digest line, or `None` when the run skipped for want of a GPU.
fn digest_line(args: &[&str]) -> Option<String> {
    let run = run(args);
    assert_eq!(run.code, Some(0), "{} / {}", run.stdout, run.stderr);
    let line = run.stdout.trim_end().to_string();
    if line.starts_with("SKIP:") {
        assert!(!strict(), "strict mode, but the run skipped: {line}");
        eprintln!("{line}");
        return None;
    }
    Some(line)
}

#[test]
fn three_processes_print_the_same_digest_line() {
    let arguments = ["--headless", "--frames", "16", "--seed", "3"];
    let mut lines = Vec::new();
    for _ in 0..3 {
        let Some(line) = digest_line(&arguments) else {
            return;
        };
        lines.push(line);
    }
    assert_eq!(lines[0], lines[1]);
    assert_eq!(lines[1], lines[2]);
    // Not vacuous: the line is the digest line, and it names the run it
    // came from. A binary printing nothing would fail here first.
    let expected_prefix =
        "renew-frame sample=hello_triangle seed=3 frames=16 ticks=16 dropped=0 schedule_hash=0x";
    assert!(lines[0].starts_with(expected_prefix), "{}", lines[0]);
    assert!(lines[0].contains(" state_hash=0x"), "{}", lines[0]);
}

#[test]
fn a_different_run_is_a_different_line() {
    let Some(base) = digest_line(&["--headless", "--frames", "16", "--seed", "3"]) else {
        return;
    };
    let Some(longer) = digest_line(&["--headless", "--frames", "17", "--seed", "3"]) else {
        return;
    };
    let Some(seeded) = digest_line(&["--headless", "--frames", "16", "--seed", "4"]) else {
        return;
    };
    assert_ne!(base, longer, "one more frame must move the digest");
    assert_ne!(base, seeded, "another seed must move the digest");
}

#[test]
fn the_stats_file_is_written_after_the_run_and_carries_both_halves() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hello_triangle-stats.json");
    let _ = std::fs::remove_file(&path);
    let text = path.to_string_lossy();
    let arguments = ["--headless", "--frames", "8", "--dump-stats", &text];
    let Some(line) = digest_line(&arguments) else {
        return;
    };
    let json = std::fs::read_to_string(&path).expect("the stats file the run was asked for");
    assert!(json.starts_with("{\"schema_version\":1,"), "{json}");
    assert!(json.contains("\"sample\":\"hello_triangle\""), "{json}");
    assert!(
        json.contains("\"frame\":{\"frames\":8,\"ticks\":8,"),
        "{json}"
    );
    assert!(json.contains("\"timing\":{\"count\":8,"), "{json}");
    // The two channels agree on the half they share.
    let hash = json
        .split("\"state_hash\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_default();
    assert!(line.contains(&format!("state_hash={hash}")), "{line}");
}

/// The command line is part of the contract too: a flag this build does
/// not know is a usage error with its own exit code, not a crash and not
/// a silent default.
#[test]
fn an_unknown_flag_is_refused_with_the_usage_exit_code() {
    let refused = run(&["--turbo"]);
    assert_eq!(refused.code, Some(2), "{}", refused.stderr);
    assert!(refused.stderr.contains("--turbo"), "{}", refused.stderr);
    assert!(refused.stdout.is_empty(), "{}", refused.stdout);
}
