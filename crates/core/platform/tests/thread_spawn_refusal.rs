//! The named-thread seam reports an operating-system refusal as
//! [`ThreadError::SpawnFailed`] naming the thread — not as a panic.
//!
//! Getting the OS to refuse takes an unsatisfiable stack reservation,
//! which `std` reads once per process from `RUST_MIN_STACK` and then
//! caches; setting an environment variable in-process is `unsafe`
//! (denied workspace-wide) and racy with every other test's threads, so
//! this binary re-runs *itself* as a child with the variable set. Own
//! harness (`harness = false`): the default one spawns a thread per test
//! and would be refused before reaching the code under test.

use std::process::{Command, ExitCode};

use renew_platform::thread::{ThreadError, spawn_named};

/// Marks the child run. Passed as an argument rather than an
/// environment variable so the mode cannot be lost in inheritance —
/// a child that mistook itself for a parent would fork endlessly.
const CHILD_ARG: &str = "--spawn-refusal-child";

/// The child's requested stack: half the address space, page-aligned.
/// No machine can reserve it, on 32-bit or 64-bit targets alike, so the
/// refusal is deterministic rather than a race against real memory
/// pressure; page alignment keeps `std`'s own rounding-up clear of
/// overflow.
fn unsatisfiable_stack_bytes() -> usize {
    (usize::MAX / 2) & !0xFFF
}

/// The child: one spawn attempt, which the OS must refuse.
fn child() -> ExitCode {
    match spawn_named("renew-test-refused", || ()) {
        Err(ThreadError::SpawnFailed { name, kind }) => {
            assert_eq!(name, "renew-test-refused", "the error must name the thread");
            println!("child: the operating system refused the thread ({kind})");
            ExitCode::SUCCESS
        }
        Err(other) => {
            eprintln!("child: expected SpawnFailed, got {other}");
            ExitCode::FAILURE
        }
        Ok(handle) => {
            eprintln!("child: the operating system accepted an impossible stack reservation");
            let _ = handle.join();
            ExitCode::FAILURE
        }
    }
}

/// The parent: run the child with the poisoned stack size and require
/// it to have observed the refusal.
fn parent() -> ExitCode {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("thread spawn refusal: FAILED: cannot locate this test binary: {error}");
            return ExitCode::FAILURE;
        }
    };
    let status = Command::new(executable)
        .arg(CHILD_ARG)
        .env("RUST_MIN_STACK", unsatisfiable_stack_bytes().to_string())
        .status();
    match status {
        Ok(status) if status.success() => {
            println!("thread spawn refusal: ok");
            ExitCode::SUCCESS
        }
        Ok(status) => {
            eprintln!("thread spawn refusal: FAILED: the child exited with {status}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("thread spawn refusal: FAILED: the child could not be run: {error}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    if std::env::args().any(|argument| argument == CHILD_ARG) {
        child()
    } else {
        parent()
    }
}
