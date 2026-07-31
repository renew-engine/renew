//! Replaying a recorded trace reproduces the run that produced it.
//!
//! The assertions here are built around one question: what would still
//! pass if the file were being ignored? A replay compared only against
//! another replay answers nothing, so each test below is anchored either
//! on a run that actually happened or on a hand-computed number.

use std::path::PathBuf;
use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_input_echo");

fn scratch(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("replay-{name}.trace"))
}

/// Run the binary and report what it said.
///
/// A process that will not start is reported through the same assertions
/// as one that started and misbehaved, rather than through a panic here:
/// the exit code comes back as `None`, which every caller already checks.
fn run(args: &[&str]) -> (String, String, Option<i32>) {
    let Ok(out) = Command::new(BINARY).args(args).output() else {
        return (String::new(), format!("could not run {BINARY}"), None);
    };
    (
        String::from_utf8_lossy(&out.stdout)
            .replace("\r\n", "\n")
            .trim()
            .to_string(),
        String::from_utf8_lossy(&out.stderr)
            .replace("\r\n", "\n")
            .trim()
            .to_string(),
        out.status.code(),
    )
}

fn field<'a>(line: &'a str, name: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{name}=")))
        .unwrap_or_default()
}

/// The round trip: a replay reproduces the simulation of the run recorded.
///
/// Compared against the *recorded run's* own digest, not against a second
/// replay — the whole point is that the file carries the run across
/// processes.
#[test]
fn replaying_a_recording_reproduces_the_run_that_made_it() {
    let path = scratch("roundtrip");
    let (recorded, _, code) = run(&[
        "--headless",
        "--input-trace",
        "walk",
        "--seed",
        "3",
        "--record-trace",
        &path.to_string_lossy(),
    ]);
    assert_eq!(code, Some(0));

    let (replayed, _, code) = run(&["--headless", "--replay-trace", &path.to_string_lossy()]);
    assert_eq!(code, Some(0));

    assert_eq!(
        field(&recorded, "state_hash"),
        field(&replayed, "state_hash")
    );
    assert_eq!(field(&recorded, "ticks"), field(&replayed, "ticks"));
    assert_eq!(field(&recorded, "seed"), field(&replayed, "seed"));
    // Not vacuous: a digest of nothing would also be equal to itself.
    assert!(
        field(&recorded, "state_hash").starts_with("0x"),
        "{recorded}"
    );
    assert_ne!(field(&recorded, "ticks"), "0", "{recorded}");
}

/// Shifting every event by one tick changes the outcome.
///
/// This is the assertion that proves the tick column is read at all. The
/// header is untouched, so the run is the same length and the same shape;
/// only *when* each event arrives moves. A driver that delivered
/// everything at tick zero, or ignored the column entirely, passes every
/// other test here and fails this one.
#[test]
fn moving_every_event_one_tick_later_changes_the_run() {
    let original = scratch("shift-original");
    let _ = run(&[
        "--headless",
        "--input-trace",
        "walk",
        "--seed",
        "0",
        "--record-trace",
        &original.to_string_lossy(),
    ]);
    let text = std::fs::read_to_string(&original).expect("recorded");
    // Both sides of this comparison are REPLAYS, of two files differing
    // only in the tick column. Comparing against the original recorded
    // run instead would pass for the wrong reason: any broken replay
    // differs from a correct recording, so the assertion held even for a
    // driver that ignored ticks entirely. This test was found vacuous
    // exactly that way, by mutating the driver and watching it pass.
    let (before, stderr, code) =
        run(&["--headless", "--replay-trace", &original.to_string_lossy()]);
    assert_eq!(code, Some(0), "{stderr}");

    // Shift only the event ticks, and only where there is headroom: the
    // last event sits at the run's final tick, which has none.
    let shifted: String = text
        .lines()
        .map(|line| {
            let Some(rest) = line.strip_prefix("e ") else {
                return line.to_string();
            };
            let (tick, tail) = rest.split_once(' ').expect("an event has a kind");
            let tick: u64 = tick.parse().expect("a decimal tick");
            format!("e {} {tail}", tick.saturating_add(1).min(19))
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert_ne!(shifted, text, "the shift must actually change the file");

    let moved = scratch("shift-moved");
    std::fs::write(&moved, &shifted).expect("write the shifted trace");
    let (after, stderr, code) = run(&["--headless", "--replay-trace", &moved.to_string_lossy()]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_ne!(
        field(&before, "state_hash"),
        field(&after, "state_hash"),
        "shifting every event left the world unchanged, so the tick column is not being read"
    );
}

/// The header's fields reach the run rather than being parsed and dropped.
///
/// A replay that read the header and then used its own defaults would
/// pass the round trip, because the recording was made with those same
/// defaults. Changing one field has to change the answer.
#[test]
fn a_header_field_the_driver_ignored_would_not_change_the_run() {
    let path = scratch("header-original");
    let (_, _, _) = run(&[
        "--headless",
        "--input-trace",
        "walk",
        "--seed",
        "0",
        "--record-trace",
        &path.to_string_lossy(),
    ]);
    let text = std::fs::read_to_string(&path).expect("recorded");
    let (baseline, _, _) = run(&["--headless", "--replay-trace", &path.to_string_lossy()]);

    // The seed selects the movement speed, so a different one must move
    // the world differently — and the seed lives only in the header.
    let reseeded = text.replace("seed=0", "seed=2");
    assert_ne!(reseeded, text);
    let other = scratch("header-reseeded");
    std::fs::write(&other, &reseeded).expect("write");
    let (changed, stderr, code) = run(&["--headless", "--replay-trace", &other.to_string_lossy()]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(field(&changed, "seed"), "2");
    assert_ne!(
        field(&baseline, "state_hash"),
        field(&changed, "state_hash"),
        "the header's seed never reached the world"
    );
}

/// A missing or malformed trace is refused, never quietly replaced by a
/// built-in one. A silent fallback would make every assertion above pass
/// while the file was ignored.
#[test]
fn a_trace_that_cannot_be_read_is_refused_rather_than_replaced() {
    let missing = scratch("does-not-exist");
    let _ = std::fs::remove_file(&missing);
    let (_, stderr, code) = run(&["--headless", "--replay-trace", &missing.to_string_lossy()]);
    assert_eq!(code, Some(1), "{stderr}");

    let junk = scratch("not-a-trace");
    std::fs::write(&junk, "this is not a trace\n").expect("write");
    let (_, stderr, code) = run(&["--headless", "--replay-trace", &junk.to_string_lossy()]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("trace"), "{stderr}");
}

/// The flags the header owns are refused alongside it, rather than one of
/// them silently winning.
#[test]
fn flags_the_header_owns_are_refused_with_it() {
    let path = scratch("owned-flags");
    let _ = run(&[
        "--headless",
        "--input-trace",
        "walk",
        "--record-trace",
        &path.to_string_lossy(),
    ]);
    for extra in [["--seed", "1"], ["--frames", "5"]] {
        let (_, stderr, code) = run(&[
            "--headless",
            "--replay-trace",
            &path.to_string_lossy(),
            extra[0],
            extra[1],
        ]);
        assert_eq!(code, Some(2), "{} should be refused: {stderr}", extra[0]);
        assert!(stderr.contains(extra[0]), "{stderr}");
    }
    // And a replay against a live window is refused, not silently made headless.
    let (_, stderr, code) = run(&["--replay-trace", &path.to_string_lossy()]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("--headless"), "{stderr}");
}
