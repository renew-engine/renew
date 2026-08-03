//! Every arm of the library entry, driven in-process.
//!
//! The cross-process suite beside this one proves determinism — the
//! claim that only means something across process boundaries. This one
//! exists for the arms: usage refusals, failure reporting, and the
//! record-then-replay round trip, exercised through `run_cli` directly
//! so their coverage is a fact about this test binary rather than
//! about subprocess profile plumbing.

use renew_sample_glide::run_cli;

fn run(args: &[&str]) -> u8 {
    run_cli(args.iter().map(ToString::to_string))
}

#[test]
fn a_happy_run_exits_zero() {
    assert_eq!(run(&["--frames", "120"]), 0);
}

#[test]
fn an_unknown_flag_is_a_usage_exit() {
    assert_eq!(run(&["--fly"]), 2);
}

#[test]
fn an_unknown_trace_is_a_usage_exit() {
    assert_eq!(run(&["--input-trace", "swim"]), 2);
}

#[test]
fn a_missing_replay_file_is_a_failure_exit() {
    assert_eq!(run(&["--replay-trace", "no-such-file.trace"]), 1);
}

#[test]
fn a_file_that_is_not_a_trace_is_a_failure_exit() {
    let dir = std::env::temp_dir().join("glide-library-paths");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("nonsense.trace");
    std::fs::write(&path, "not a trace at all\n").expect("fixture");
    assert_eq!(run(&["--replay-trace", &path.to_string_lossy()]), 1);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn record_then_replay_round_trips_in_process() {
    let dir = std::env::temp_dir().join("glide-library-paths");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("in-process.trace");
    let path_text = path.to_string_lossy().to_string();

    assert_eq!(
        run(&["--frames", "300", "--record-trace", &path_text]),
        0,
        "recording run"
    );
    let written = std::fs::read_to_string(&path).expect("the recording was written");
    assert!(
        written.starts_with("renew-trace 0 sample=glide"),
        "the recording carries the sample's own header: {written}"
    );
    assert_eq!(run(&["--replay-trace", &path_text]), 0, "replay run");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn errors_display_their_context() {
    // The Display path and the `failed` constructor, directly: the
    // formatting is part of what a person debugging a red lane reads.
    let error = renew_sample_glide::SampleError::failed("reading the file", &"gone");
    assert_eq!(error.to_string(), "reading the file: gone");
    let usage = renew_sample_glide::SampleError::Usage("bad flag".to_string());
    assert_eq!(usage.to_string(), "bad flag");
}

#[test]
fn a_trace_whose_header_lacks_a_seed_is_refused() {
    // A structurally valid trace that names no seed: the replay cannot
    // know which world to build, and guessing one would replay into a
    // different game with nothing to explain the divergence.
    let dir = std::env::temp_dir().join("glide-library-paths");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("seedless.trace");
    std::fs::write(
        &path,
        "renew-trace 0 sample=glide ticks=5 timestep_ns=16666667 budget=5\n",
    )
    .expect("fixture");
    assert_eq!(run(&["--replay-trace", &path.to_string_lossy()]), 1);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_repeated_flag_is_refused_by_name() {
    assert_eq!(run(&["--record-trace", "a", "--record-trace", "b"]), 2);
    assert_eq!(run(&["--replay-trace", "a", "--replay-trace", "b"]), 2);
    assert_eq!(run(&["--window", "--window"]), 2);
}

#[test]
fn window_refuses_the_flags_that_contradict_it() {
    // The flag parses in every build; only the arm behind it is
    // feature-gated. Playing from the keyboard contradicts scripted
    // input and recording; replay owns everything; a zero-tick window
    // is a contradiction of its own.
    assert_eq!(run(&["--window", "--input-trace", "soar"]), 2);
    assert_eq!(run(&["--window", "--record-trace", "out.trace"]), 2);
    assert_eq!(run(&["--replay-trace", "a.trace", "--window"]), 2);
    assert_eq!(run(&["--window", "--frames", "0"]), 2);
}

#[cfg(not(feature = "window"))]
#[test]
fn a_window_in_a_headless_build_is_refused_by_name() {
    // The twin function: same signature, honest answer, exit 2.
    assert_eq!(run(&["--window"]), 2);
}

#[test]
fn a_trace_recorded_at_another_timestep_is_refused() {
    // The header carries the run's clock; replaying a 120Hz recording
    // through this driver's fixed 60Hz loop would be a different run
    // wearing the recording's name.
    let dir = std::env::temp_dir().join("glide-library-paths");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("wrong-clock.trace");
    std::fs::write(
        &path,
        "renew-trace 0 sample=glide ticks=5 timestep_ns=8333333 budget=5 seed=7\n",
    )
    .expect("fixture");
    assert_eq!(run(&["--replay-trace", &path.to_string_lossy()]), 1);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_trace_recorded_at_another_budget_is_refused() {
    let dir = std::env::temp_dir().join("glide-library-paths");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("wrong-budget.trace");
    std::fs::write(
        &path,
        "renew-trace 0 sample=glide ticks=5 timestep_ns=16666667 budget=3 seed=7\n",
    )
    .expect("fixture");
    assert_eq!(run(&["--replay-trace", &path.to_string_lossy()]), 1);
    let _ = std::fs::remove_file(&path);
}
