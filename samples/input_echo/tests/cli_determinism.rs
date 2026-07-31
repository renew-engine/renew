//! Cross-process determinism: three runs of the real binary, one digest
//! line — with no GPU and no display anywhere in sight.
//!
//! This is the sample that has nothing to skip for. It needs no adapter
//! and no compositor, so on every machine and every CI lane these
//! assertions actually run, which makes it the frame loop's cheapest
//! standing proof that a scripted run is reproducible across processes.

use std::path::PathBuf;
use std::process::Command;

/// The binary this test drives, built by Cargo alongside it.
const BINARY: &str = env!("CARGO_BIN_EXE_input_echo");

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

fn digest_line(args: &[&str]) -> String {
    let run = run(args);
    assert_eq!(run.code, Some(0), "{} / {}", run.stdout, run.stderr);
    let line = run.stdout.trim_end().to_string();
    assert!(
        !line.starts_with("SKIP:"),
        "a scripted run cannot skip: {line}"
    );
    line
}

#[test]
fn three_processes_print_the_same_digest_line() {
    let arguments = ["--headless", "--input-trace", "walk", "--frames", "600"];
    let lines: Vec<String> = (0..3).map(|_| digest_line(&arguments)).collect();
    assert_eq!(lines[0], lines[1]);
    assert_eq!(lines[1], lines[2]);
    // Not vacuous: the trace's close request ends the run at frame
    // twenty, so the line names twenty frames and twenty ticks — the
    // schedule the trace describes, proven by a separate process.
    let expected_prefix = "renew-frame sample=input_echo seed=0 source=walk frames=20 ticks=20 \
                           dropped=0 schedule_hash=0x";
    assert!(lines[0].starts_with(expected_prefix), "{}", lines[0]);
    assert!(lines[0].contains(" state_hash=0x"), "{}", lines[0]);
}

#[test]
fn a_different_run_is_a_different_line() {
    let walk = digest_line(&["--headless", "--input-trace", "walk", "--frames", "600"]);
    let idle = digest_line(&["--headless", "--input-trace", "idle", "--frames", "20"]);
    let seeded = digest_line(&[
        "--headless",
        "--input-trace",
        "walk",
        "--frames",
        "600",
        "--seed",
        "3",
    ]);
    assert!(idle.contains("source=idle frames=20 ticks=20"), "{idle}");
    assert_ne!(walk, idle, "no input must not hash like some input");
    assert_ne!(walk, seeded, "another seed must move the digest");
}

/// The final position the run reported, read out of its stats document.
///
/// This, not the state digest, is what distinguishes one seed's world
/// from another's. The digest absorbs the seed itself, so two seeds
/// produce two digests whether or not the seed ever reached the
/// simulation — comparing digests across seeds proves arithmetic, not
/// behaviour. The position is behaviour: the seed picks a speed, and the
/// speed is how far the world moved.
fn reported_position(json: &str) -> String {
    json.split("\"position\":")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .map(|value| format!("{value}]"))
        .unwrap_or_default()
}

/// How many times each seed runs. Three catches a process-to-process
/// difference on every push; a longer campaign can ask for more without
/// touching this file.
fn runs_per_seed() -> usize {
    std::env::var("RENEW_DETERMINISM_RUNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|runs| *runs >= 2)
        .unwrap_or(3)
}

/// The seed matrix: every seed reproduces itself across processes, and
/// no two seeds move the world the same way.
///
/// Both halves earn their place, and the second one is the reason this
/// test exists at all. Identity within a seed is the determinism claim.
/// Distinctness across seeds is what proves the seed REACHES the
/// simulation: a seed that is parsed, printed, hashed and then ignored
/// would satisfy identity perfectly, and would still yield a different
/// digest for every seed, because the digest absorbs the seed. Only an
/// observable the seed has to travel through catches that — here, how
/// far the world actually moved.
///
/// The seeds are 0 to 3 rather than an arbitrary scatter because this
/// sample derives one speed from the seed modulo four. Five arbitrary
/// seeds would collide by pigeonhole and fail an assertion the engine
/// never made. When a real random-number service widens the space, the
/// matrix widens with it.
#[test]
fn every_seed_reproduces_itself_and_no_two_seeds_move_the_world_alike() {
    const SEEDS: [&str; 4] = ["0", "1", "2", "3"];
    let runs = runs_per_seed();
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let mut behaviours: Vec<(&str, String)> = Vec::with_capacity(SEEDS.len());

    for seed in SEEDS {
        let path = directory.join(format!("input_echo-seed-{seed}.json"));
        let text = path.to_string_lossy().to_string();
        let arguments = [
            "--headless",
            "--input-trace",
            "walk",
            "--seed",
            seed,
            "--dump-stats",
            &text,
        ];
        let first = digest_line(&arguments);
        for attempt in 1..runs {
            assert_eq!(
                digest_line(&arguments),
                first,
                "seed {seed} did not reproduce on run {attempt}"
            );
        }
        let json = std::fs::read_to_string(&path).expect("the stats file the run was asked for");
        let position = reported_position(&json);
        assert!(!position.is_empty(), "no position in {json}");
        behaviours.push((seed, position));
    }

    for (index, (seed, position)) in behaviours.iter().enumerate() {
        for (other_seed, other_position) in &behaviours[index + 1..] {
            assert_ne!(
                position, other_position,
                "seeds {seed} and {other_seed} moved the world identically, \
                 so the seed is not reaching the simulation"
            );
        }
    }
}

#[test]
fn the_stats_file_is_written_after_the_run_and_carries_the_input_tally() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("input_echo-stats.json");
    let _ = std::fs::remove_file(&path);
    let text = path.to_string_lossy();
    let line = digest_line(&[
        "--headless",
        "--input-trace",
        "walk",
        "--frames",
        "600",
        "--dump-stats",
        &text,
    ]);
    let json = std::fs::read_to_string(&path).expect("the stats file the run was asked for");
    assert!(json.starts_with("{\"schema_version\":1,"), "{json}");
    assert!(json.contains("\"sample\":\"input_echo\""), "{json}");
    assert!(
        json.contains("\"frame\":{\"frames\":20,\"ticks\":20,"),
        "{json}"
    );
    // The trace held one key for twelve ticks and another for four; the
    // position is the loop's arithmetic, reported by a separate process.
    assert!(json.contains("\"position\":[12,4]"), "{json}");
    let hash = json
        .split("\"state_hash\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_default();
    assert!(line.contains(&format!("state_hash={hash}")), "{line}");
}

/// The command line is part of the contract too.
#[test]
fn a_trace_asked_for_outside_headless_mode_is_refused() {
    let refused = run(&["--input-trace", "walk"]);
    assert_eq!(refused.code, Some(2), "{}", refused.stderr);
    assert!(refused.stderr.contains("--headless"), "{}", refused.stderr);
    assert!(refused.stdout.is_empty(), "{}", refused.stdout);

    let unknown = run(&["--headless", "--input-trace", "moonwalk"]);
    assert_eq!(unknown.code, Some(2), "{}", unknown.stderr);
    assert!(unknown.stderr.contains("moonwalk"), "{}", unknown.stderr);
}
