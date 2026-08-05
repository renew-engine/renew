//! The file-logging installer, exercised end to end in its own process.
//!
//! **An integration test rather than a unit test, and the reason is the
//! shape of the thing being tested.** Installing a diagnostics sink is
//! process-wide and may happen once — a second call is a contract
//! violation the reporting crate asserts on. A unit test would therefore
//! be at the mercy of whatever else in the same binary had already
//! installed one, and two such tests could never both run. Each
//! integration test is its own binary and its own process, so this one
//! owns the slot outright.
//!
//! That is also why it is worth having: `log_to_file` is the whole
//! feature, and every other test in this crate can only reach the pieces
//! it is assembled from.

use std::path::PathBuf;

/// A path in the temporary directory, unique to this test binary, with
/// no run of a clock or a random number — the file name is a constant
/// because exactly one process ever writes it.
fn log_path() -> PathBuf {
    std::env::temp_dir().join("renew-platform-log-to-file.log")
}

/// Install, emit, panic, and read the whole lot back.
///
/// One test rather than three, deliberately: the installation can only
/// happen once in a process, so everything that depends on it has to
/// share a test. Splitting them would mean either three binaries or two
/// tests silently doing nothing.
#[test]
fn records_and_panics_both_reach_the_file() {
    let path = log_path();
    // A previous run's file would make an append-only sink look like it
    // had written things it did not.
    let _ = std::fs::remove_file(&path);

    renew_platform::diag::log_to_file(Some(&path), Some("a note the caller wanted first"))
        .expect("a writable path installs");

    renew_diag::error!(target: "test", "an error with a value: {}", 7);
    renew_diag::warn!(target: "test", "a warning");

    // The panic hook is chained, so the default one still runs and still
    // prints. `catch_unwind` keeps this test alive to read the file.
    let panicked = std::panic::catch_unwind(|| {
        panic!("deliberate, to prove the hook records it");
    });
    assert!(panicked.is_err(), "the panic must have happened");

    let text = std::fs::read_to_string(&path).expect("the sink created the file");

    assert!(
        text.contains("a note the caller wanted first"),
        "the caller's opening note is missing from {text:?}"
    );

    assert!(
        text.contains("ERROR test: an error with a value: 7"),
        "the error record and its formatted value are missing from {text:?}"
    );
    assert!(
        text.contains("WARN test: a warning"),
        "the warning is missing from {text:?}"
    );
    assert!(
        text.contains("deliberate, to prove the hook records it"),
        "the panic message is missing from {text:?}"
    );
    assert!(
        text.contains("panic"),
        "the panic record should name itself as one, in {text:?}"
    );
    // The panic arrived after the records that preceded it: the file is
    // a log, so order is part of what it is for.
    let error_at = text.find("an error with a value").expect("error present");
    let panic_at = text.find("deliberate, to prove").expect("panic present");
    assert!(
        error_at < panic_at,
        "records must appear in the order they happened, in {text:?}"
    );

    let _ = std::fs::remove_file(&path);
}

/// A path that cannot be written is reported, and installs nothing.
///
/// Separate from the test above because it must run *before* anything is
/// installed to be meaningful — and it does, because a failing path
/// returns before it touches the sink slot at all.
#[test]
fn an_unwritable_path_is_reported_rather_than_silently_ignored() {
    // A directory component that is not a directory: the open fails on
    // every supported platform.
    let bad = std::env::temp_dir()
        .join("renew-platform-not-a-directory.log")
        .join("nested")
        .join("run.log");
    let refused = renew_platform::diag::log_to_file(Some(bad), None);
    assert!(
        refused.is_err(),
        "a path that cannot be written must be said out loud, or a mistyped one          looks exactly like a run with nothing to report"
    );
}
