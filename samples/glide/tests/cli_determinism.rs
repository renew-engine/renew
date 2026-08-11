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

    // Same world, same input, same length: everything after the source
    // token must agree — frames, ticks, dropped, score, alive, both
    // hashes — and source honestly differs. Comparing only the state
    // hash would let a schedule or length divergence hide behind an
    // agreeing endpoint.
    let tail = |line: &str| {
        let start = line.find(" frames=");
        assert!(start.is_some(), "digest lines carry a frames field: {line}");
        line[start.unwrap_or_default()..].to_string()
    };
    assert_eq!(
        tail(&recorded),
        tail(&replayed),
        "a replay must land on the recorded run's exact line
recorded: {recorded}
replayed: {replayed}"
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

/// The machine-readable face of the same run.
///
/// The cross-platform comparison reads this, not the human line, so a
/// change that broke it would take the only gate that proves this
/// simulation is portable with it — and would do so silently, because
/// every other test here reads the line built for a person.
#[test]
fn the_json_face_carries_the_same_digests_as_the_line() {
    let human = digest_line(&["--seed", "7", "--frames", "600"]);
    let json = run(&["--seed", "7", "--frames", "600", "--json"]);
    assert!(json.success, "the json run failed: {}", json.stderr);
    let object = json
        .stdout
        .lines()
        .map(str::trim)
        .rfind(|line| line.starts_with('{'))
        .unwrap_or_default()
        .to_string();
    assert!(
        !object.is_empty(),
        "no JSON object on stdout: {}",
        json.stdout
    );

    // Every hash the comparison collects, present and identical to the
    // human line's. Compared by extracting from both rather than by
    // recomputing, so a change to either face that left the other behind
    // fails here.
    for field in ["schedule_hash", "state_hash"] {
        let from_line = human
            .split_whitespace()
            .find_map(|token| token.strip_prefix(&format!("{field}=")))
            .unwrap_or_else(|| panic!("the human line carries no {field}: {human}"));
        let quoted = format!("\"{field}\":\"{from_line}\"");
        assert!(
            object.contains(&quoted),
            "the JSON face is missing {quoted}; it reads {object}"
        );
    }

    // Digests are strings, not numbers: a u64 exceeds what a JSON number
    // carries exactly, and a reader that rounded one would call two
    // different states identical.
    assert!(
        object.contains("\"schema_version\":1"),
        "the document must name its schema: {object}"
    );
    assert!(
        !object.contains("schedule_hash\":0x"),
        "hashes must be quoted strings: {object}"
    );
}

/// The menu trace: the same session twice is the same digest — the
/// tree's decisions replay bit for bit through the recorded pointer
/// events and the one quantization seam.
#[test]
fn the_menu_session_reproduces() {
    let first = digest_line(&["--input-trace", "menu", "--frames", "600"]);
    let second = digest_line(&["--input-trace", "menu", "--frames", "600"]);
    assert_eq!(first, second);
    let soar = digest_line(&["--input-trace", "soar", "--frames", "600"]);
    assert_ne!(
        first, soar,
        "a paused-and-restarted run is not an ordinary one"
    );
}

/// Recording the menu session and replaying the recording answer the
/// same digest: pointer events and pauses survive the trace format.
#[test]
fn the_menu_session_survives_a_record_replay_round_trip() {
    let dir = std::env::temp_dir().join("renew-glide-menu-roundtrip");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("menu-rt.trace");
    let path_text = path.to_str().expect("utf-8 temp path");
    let recorded = digest_line(&[
        "--input-trace",
        "menu",
        "--frames",
        "600",
        "--record-trace",
        path_text,
    ]);
    let replayed = digest_line(&["--replay-trace", path_text]);
    // The whole tail after the source field, not just the hash:
    // comparing only the endpoint would let a schedule or length
    // divergence hide behind an agreeing state hash.
    let tail = |line: &str| {
        line.split(" frames=")
            .nth(1)
            .expect("a frames field")
            .to_string()
    };
    assert_eq!(tail(&recorded), tail(&replayed));
    // And the recording IS the committed file, byte for byte: the
    // hand-authored trace is already in canonical form, so it enjoys
    // the same own-golden property the recorded traces do.
    let rerecorded = std::fs::read_to_string(&path).expect("the recording exists");
    assert_eq!(
        rerecorded,
        include_str!("../traces/menu.trace"),
        "re-recording the menu trace must reproduce its committed bytes"
    );
    let _ = std::fs::remove_file(&path);
}
