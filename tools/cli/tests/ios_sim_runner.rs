//! The iOS simulator runner's contract, held against a stub `xcrun`.
//!
//! `tools/ios-sim-runner.sh` is what cargo invokes for
//! `aarch64-apple-ios-sim`, and its whole job is to hand a binary and
//! its arguments to `xcrun simctl spawn` on the right device. Three
//! things about it are load-bearing and none needs a Mac to check:
//! which device it names, that arguments arrive unchanged, and that a
//! missing binary is refused rather than passed on.
//!
//! The stub stands in for `xcrun` on `PATH` and records the argv it was
//! given. That is enough to test everything this script decides, and it
//! deliberately does not test what `simctl` itself does with them —
//! that is the CI lane's job, and it exercises it against a real
//! simulator on every run.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The runner script, from the crate this test belongs to.
fn runner() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tools/ios-sim-runner.sh")
}

/// A shell this test trusts, or `None` where there is none.
///
/// Windows is skipped for the reason its neighbour records: which shell
/// a bare `sh` resolves to there is unpredictable, and this script is a
/// macOS artifact invoked by cargo on a macOS runner. The Linux lanes
/// run this for real.
fn shell() -> Option<&'static str> {
    if cfg!(windows) {
        return None;
    }
    ["bash", "sh"].into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("-c")
            .arg("exit 0")
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// Run the script with a stub `xcrun` ahead of it on `PATH`, and return
/// what that stub was asked to do, one argument per line.
fn spawned(
    shell: &str,
    scratch: &Path,
    device: Option<&str>,
    binary: &str,
    arguments: &[&str],
) -> Result<(String, i32), String> {
    let stub = scratch.join("xcrun");
    std::fs::write(
        &stub,
        "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done\n",
    )
    .map_err(|error| format!("could not write the stub: {error}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("could not make the stub executable: {error}"))?;
    }

    let mut command = Command::new(shell);
    command
        .arg(runner())
        .arg(binary)
        .args(arguments)
        .env(
            "PATH",
            format!(
                "{}:{}",
                scratch.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env_remove("RENEW_IOS_SIM_UDID");

    if let Some(udid) = device {
        command.env("RENEW_IOS_SIM_UDID", udid);
    }

    let output = command
        .output()
        .map_err(|error| format!("could not run the runner: {error}"))?;

    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.code().unwrap_or(-1),
    ))
}

/// A scratch directory of this test's own, removed when it finishes.
fn scratch(name: &str) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("renew-ios-runner-{name}"));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).map_err(|error| format!("no scratch directory: {error}"))?;
    Ok(path)
}

/// The device the lane chose is the device the binary runs on.
///
/// `booted` is the fallback and not the rule: it names whichever
/// simulator happens to be running, which is one machine right up until
/// it is two, and then it is an unspecified one of them. A lane that
/// boots one simulator and measures another would put digests against
/// the wrong platform, which is the single thing it exists to get right.
#[test]
fn the_named_device_is_used_and_booted_is_only_the_fallback() -> Result<(), String> {
    let Some(shell) = shell() else {
        eprintln!("no shell this test trusts here; the macOS lanes check this for real");
        return Ok(());
    };
    let scratch = scratch("device")?;
    let binary = scratch.join("payload");
    std::fs::write(&binary, "not really a binary").map_err(|error| error.to_string())?;
    let binary = binary.display().to_string();

    let (named, _) = spawned(shell, &scratch, Some("UDID-1234"), &binary, &[])?;
    let named: Vec<&str> = named.lines().collect();
    if named.first() != Some(&"simctl") || named.get(1) != Some(&"spawn") {
        return Err(format!("expected `simctl spawn`, got {named:?}"));
    }
    if named.get(2) != Some(&"UDID-1234") {
        return Err(format!("the named device did not reach xcrun: {named:?}"));
    }

    let (fallback, _) = spawned(shell, &scratch, None, &binary, &[])?;
    let fallback: Vec<&str> = fallback.lines().collect();
    if fallback.get(2) != Some(&"booted") {
        return Err(format!(
            "without a named device, expected `booted`: {fallback:?}"
        ));
    }

    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}

/// Every argument arrives as one argument, unchanged.
///
/// This runner needs no quoting because `simctl spawn` execs the binary
/// with an argv rather than handing a string to a shell — the opposite
/// of the Android runner next door, which must quote everything. That
/// difference is a claim about behaviour, so it is checked rather than
/// asserted: a space, a quote and a metacharacter all survive whole.
#[test]
fn arguments_reach_the_simulator_one_for_one() -> Result<(), String> {
    let Some(shell) = shell() else {
        return Ok(());
    };
    let scratch = scratch("argv")?;
    let binary = scratch.join("payload");
    std::fs::write(&binary, "not really a binary").map_err(|error| error.to_string())?;
    let binary = binary.display().to_string();

    let sent = [
        "--seed",
        "7",
        "two words",
        "it's",
        ";touch PWNED",
        "$HOME",
        "*",
    ];
    let (seen, _) = spawned(shell, &scratch, Some("UDID"), &binary, &sent)?;
    let seen: Vec<&str> = seen.lines().collect();

    // `simctl`, `spawn`, the device, the binary, then the arguments.
    let arguments = &seen[4..];
    if arguments != sent {
        return Err(format!("sent {sent:?} and the stub saw {arguments:?}"));
    }
    if seen.get(3) != Some(&binary.as_str()) {
        return Err(format!("the binary path did not arrive whole: {seen:?}"));
    }

    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}

/// A binary that is not there is refused here, not on the device.
///
/// The caller reads a missing digest as a target that reported nothing,
/// so the difference between "the program failed" and "there was no
/// program" has to be made where it is known.
#[test]
fn a_missing_binary_is_refused_before_anything_is_spawned() -> Result<(), String> {
    let Some(shell) = shell() else {
        return Ok(());
    };
    let scratch = scratch("missing")?;
    let absent = scratch.join("no-such-binary").display().to_string();

    let (seen, code) = spawned(shell, &scratch, Some("UDID"), &absent, &[])?;
    if code != 127 {
        return Err(format!(
            "expected exit 127 for a missing binary, got {code}"
        ));
    }
    if !seen.is_empty() {
        return Err(format!(
            "nothing should have been spawned, but xcrun saw {seen:?}"
        ));
    }

    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}
