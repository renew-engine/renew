//! The digest line is a fact about (seed, trace, frames) — proved
//! across processes, which is the only place "bit-identical" means
//! anything: two runs in one process share every lazily initialized
//! table, and two processes share nothing but the build.

use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_glide");

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

fn run(args: &[&str]) -> Run {
    match Command::new(BINARY).args(args).output() {
        Ok(output) => Run {
            stdout: String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
            stderr: String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
            success: output.status.success(),
        },
        // A binary cargo built beside this test but could not start is
        // reported through the same assertions as one that misbehaved.
        Err(error) => Run {
            stdout: String::new(),
            stderr: format!("could not run {BINARY}: {error}"),
            success: false,
        },
    }
}

fn digest_line(args: &[&str]) -> String {
    let run = run(args);
    assert!(run.success, "run failed: {} / {}", run.stdout, run.stderr);
    let line = run
        .stdout
        .lines()
        .map(str::trim_end)
        .rfind(|line| line.starts_with("renew-frame "));
    assert!(
        line.is_some(),
        "every run prints a digest line; stdout was: {}",
        run.stdout
    );
    line.unwrap_or_default().to_string()
}

#[test]
fn the_same_run_in_two_processes_is_bit_identical() {
    let first = digest_line(&["--seed", "7", "--frames", "600"]);
    let second = digest_line(&["--seed", "7", "--frames", "600"]);
    assert_eq!(first, second, "cross-process determinism");
}

#[test]
fn a_different_seed_is_a_different_line() {
    let first = digest_line(&["--seed", "7", "--frames", "600"]);
    let second = digest_line(&["--seed", "8", "--frames", "600"]);
    assert_ne!(first, second, "the seed must reach the digest line");
}

#[test]
fn both_committed_traces_run_and_differ() {
    let soar = digest_line(&["--input-trace", "soar", "--frames", "600"]);
    let sink = digest_line(&["--input-trace", "sink", "--frames", "600"]);
    assert_ne!(soar, sink, "input must reach the digest line");
}

#[test]
fn a_recorded_run_replays_to_the_same_state() {
    let dir = std::env::temp_dir().join("glide-replay-test");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("recorded.trace");
    let path = path.to_string_lossy().to_string();

    let recorded = digest_line(&["--frames", "600", "--record-trace", &path]);
    let replayed = digest_line(&["--replay-trace", &path]);

    // Same world, same input, same length: everything after `source=`
    // must agree, and source honestly differs. Compare the state half.
    let state = |line: &str| {
        line.split("state_hash=")
            .nth(1)
            .expect("digest lines carry a state hash")
            .to_string()
    };
    assert_eq!(
        state(&recorded),
        state(&replayed),
        "a replay must land on the recorded run's exact state\nrecorded: {recorded}\nreplayed: {replayed}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn replay_refuses_a_file_that_is_not_a_trace() {
    let dir = std::env::temp_dir().join("glide-replay-test");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("nonsense.trace");
    std::fs::write(&path, "not a trace at all\n").expect("write fixture");
    let run = run(&["--replay-trace", &path.to_string_lossy()]);
    assert!(!run.success, "nonsense must refuse");
    let stderr = run.stderr;
    assert!(
        stderr.contains("line"),
        "the refusal names a line: {stderr}"
    );
    let _ = std::fs::remove_file(&path);
}
