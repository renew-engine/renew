//! The Android runner's quoting, held against a real shell.
//!
//! `tools/android-runner.sh` hands one string to `adb shell`, which is
//! re-parsed by a shell on the device. Everything the runner sends has
//! to survive that second parse byte for byte: an argument split in half
//! reaches a pinned simulation as a different argv than cargo asked for,
//! and a determinism lane reads that as a divergence rather than as the
//! plumbing fault it is. A metacharacter that survives unquoted runs a
//! command of its own on the device.
//!
//! Both were once possible here, and both were fixed. This is the
//! regression test that fix shipped without. The property is
//! **quote-then-reparse is identity**, checked by asking a shell to do
//! the re-parsing rather than by predicting what it would do.
//!
//! No device is involved. The device's shell and the host's agree about
//! single-quote parsing — it is the oldest, least surprising corner of
//! the syntax, which is exactly why the runner quotes that way.

use std::path::PathBuf;
use std::process::Command;

/// The runner script, from the crate this test belongs to.
fn runner() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tools/android-runner.sh")
}

/// The `quote_for_device` function, read out of the committed runner.
///
/// Read rather than restated: a copy of the function in this file would
/// pass while the shipped one was broken, which is the one outcome a
/// regression test must not have. A missing or renamed helper is an
/// error rather than a skip — that is a change this test must be told
/// about, not one it should quietly stop covering.
fn helper_source() -> Result<String, String> {
    let path = runner();
    let script = std::fs::read_to_string(&path)
        .map_err(|error| format!("{} is unreadable: {error}", path.display()))?;
    let start = script
        .find("quote_for_device() {")
        .ok_or_else(|| "the runner no longer defines `quote_for_device`".to_string())?;
    let rest = &script[start..];
    let close = rest
        .find("\n}")
        .ok_or_else(|| "`quote_for_device` is never closed".to_string())?;
    Ok(rest[..close + 2].to_string())
}

/// A shell this test trusts, or `None` where there is none.
///
/// **Windows is skipped deliberately, and not because no shell is
/// there.** A Windows box usually has several, and which one a bare
/// `sh` resolves to is unpredictable — on the machine this was written
/// on it finds one that mangles the very escape under test, so the
/// check reports a defect in a script that is correct. The runner is a
/// Linux artifact: cargo invokes it on the CI runner that carries the
/// Android job, and nothing invokes it on Windows. Testing it against
/// whatever `sh` a Windows PATH happens to surface would be measuring
/// that shell rather than this script.
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

/// Quote `argument` with the script's own helper, then have a shell
/// re-parse the result and report what it made of it.
fn round_trip(shell: &str, argument: &str) -> Result<String, String> {
    // The helper's own text, pasted at the top of the script the shell
    // is handed. Two earlier attempts were worse for the same reason:
    // `eval "$(sed …)"` puts the function body through an extra round of
    // expansion before defining it, eating one layer of the backslashes
    // its `sed` depends on, and dot-sourcing a file makes the shell
    // resolve a host path. Pasting the bytes leaves exactly one parse,
    // the same one the runner itself gets.
    //
    // The closing `eval` is the part that matters: it performs the
    // re-parse `adb shell` causes on the device. The sentinel is there
    // so trailing whitespace survives being read back.
    let script = format!(
        "{}\nquoted=\"$(quote_for_device \"$1\")\"\neval \"printf '%s|END' $quoted\"",
        helper_source()?
    );

    let output = Command::new(shell)
        .arg("-c")
        .arg(&script)
        // `sh -c script [name [args…]]` — the first word after the
        // script becomes `$0`, so the argument under test needs a
        // placeholder ahead of it or it lands where nothing reads it.
        .arg("runner-quoting-test")
        .arg(argument)
        .output()
        .map_err(|error| format!("`{shell}` could not be started: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "the shell refused the quoted form of {argument:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    printed
        .strip_suffix("|END")
        .map(str::to_string)
        .ok_or_else(|| format!("no sentinel in {printed:?} for {argument:?}"))
}

/// Everything the runner sends arrives as it was sent.
///
/// The cases are the ones that actually broke it, plus the neighbours of
/// each: a space (which split one argument into two), a semicolon and a
/// backtick and `$(…)` (which ran commands on the device), a single
/// quote (which closed the quoting and let the rest of the string be
/// read as code), and the glob characters, which a shell expands
/// against the device's filesystem rather than the host's.
#[test]
fn every_argument_survives_the_device_shell_reparsing_it() -> Result<(), String> {
    let Some(shell) = shell() else {
        eprintln!(
            "no shell this test trusts on this host, so the runner's quoting is \
             checked by the Linux lanes rather than here"
        );
        return Ok(());
    };

    for argument in [
        "plain",
        "two words",
        "  leading and trailing  ",
        "it's",
        "'",
        "''",
        r"'\''",
        "\"double\"",
        r"back\slash",
        ";touch /data/local/tmp/PWNED",
        "&& touch PWNED",
        "| tee PWNED",
        "`id`",
        "$(id)",
        "${HOME}",
        "$HOME",
        "$$",
        "*",
        "?",
        "[a-z]",
        "--flag=a b",
        "-n",
        "--",
        "héllo",
        "日本語",
        "line one\nline two",
        "tab\there",
        "#comment",
        "a'b\"c`d$e;f g",
    ] {
        let seen = round_trip(shell, argument)?;
        if seen != argument {
            return Err(format!(
                "the device's shell would have read {argument:?} as {seen:?}"
            ));
        }
    }

    Ok(())
}

/// The quoting is what does it, not the harness.
///
/// Without this, a `round_trip` that merely echoed its input back would
/// pass every case above. It asserts the negative: the *unquoted* form
/// of an argument with a space does not survive the same re-parse, so
/// the cases above are exercising the helper rather than the plumbing
/// around it.
#[test]
fn the_same_argument_does_not_survive_unquoted() -> Result<(), String> {
    let Some(shell) = shell() else {
        return Ok(());
    };

    let output = Command::new(shell)
        .arg("-c")
        .arg(r#"eval "printf '%s|END' $1""#)
        .arg("runner-quoting-control")
        .arg("two words")
        .output()
        .map_err(|error| format!("`{shell}` could not be started: {error}"))?;

    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    let seen = printed.strip_suffix("|END").unwrap_or(&printed);
    if seen == "two words" {
        return Err(
            "an unquoted argument survived, so this harness is not exercising the quoting"
                .to_string(),
        );
    }

    Ok(())
}
