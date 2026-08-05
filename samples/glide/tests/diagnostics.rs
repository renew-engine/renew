//! A log path that cannot be written is said out loud, once, and
//! changes nothing else about the run.
//!
//! **Spawned rather than called, because the promise is about a
//! process.** The complaint goes to the error stream before anything
//! else happens, and the engine crates are forbidden to print at all --
//! so the only place this behaviour exists is in the binary, and the
//! only way to observe it is to run one.
//!
//! Two runs rather than one: the same command with the variable unset
//! and with it pointing somewhere unwritable. Comparing them is what
//! makes the assertions mean anything -- one complaint appears, the
//! other run has none, and the exit code is the same either way. A
//! single run could show the complaint without showing that it cost
//! nothing.

use std::process::Command;

/// The binary cargo just built, whichever profile and target directory
/// that is.
const BINARY: &str = env!("CARGO_BIN_EXE_glide");

/// The cheapest invocation that still goes through the whole entry
/// point. Diagnostics are installed before the command line is even
/// parsed, so nothing expensive is needed to reach them.
const ARGUMENTS: &[&str] = &["--frames", "1"];

/// A path that cannot be opened for writing anywhere: its parent
/// directory does not exist, and nothing here creates it.
///
/// Named per sample so a failure message says which run left it, though
/// no file is ever created under either name.
fn unwritable_path() -> std::path::PathBuf {
    std::env::temp_dir()
        .join("renew-no-such-directory-glide")
        .join("run.log")
}

/// Run the binary, with `RENEW_LOG` either absent or unwritable.
///
/// Removed rather than merely left alone: a developer running the suite
/// with the variable already set in their shell would otherwise see the
/// clean run inherit it, and the comparison below would quietly stop
/// comparing anything.
///
/// The error stream comes back unnormalised. Nothing here reads it as
/// lines -- the assertions count one substring and compare two exit
/// codes -- so the line endings never enter the question.
#[expect(
    clippy::expect_used,
    reason = "a test fixture; a binary that will not launch should fail loudly"
)]
fn run(log: Option<std::path::PathBuf>) -> (Option<i32>, String) {
    let mut command = Command::new(BINARY);
    command.args(ARGUMENTS);
    match log {
        Some(path) => command.env("RENEW_LOG", path),
        None => command.env_remove("RENEW_LOG"),
    };
    let output = command
        .output()
        .expect("the binary cargo just built should run");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn an_unwritable_log_path_is_said_once_and_costs_the_run_nothing() {
    let (clean_code, clean_stderr) = run(None);
    let (broken_code, broken_stderr) = run(Some(unwritable_path()));

    // Said: silence would look exactly like a run with nothing to
    // report. Once: a repeat would bury the output the run is for.
    assert_eq!(
        broken_stderr.matches("RENEW_LOG:").count(),
        1,
        "a mistyped path must be said, and said once: {broken_stderr}"
    );
    assert_eq!(
        clean_stderr.matches("RENEW_LOG:").count(),
        0,
        "the complaint must come from the log path and nothing else: {clean_stderr}"
    );
    // The whole point of complaining rather than failing: a broken log
    // must not become a broken run.
    assert_eq!(
        clean_code, broken_code,
        "an unopenable log must not change the outcome: {clean_stderr} / {broken_stderr}"
    );
}
