//! `renew asset-pack` and `renew asset-inspect`, end to end.
//!
//! Exercised through the real binary against real directories, because
//! the parts most worth testing here are the ones the crate deliberately
//! does not have: walking a tree, naming entries by their relative path,
//! and refusing the paths that do not exist.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use renew_cli::json::{self, Value};

// Fallible on purpose, matching the sibling suite: the expect() lives
// inside each #[test], where the lint configuration scopes it. A helper
// in a test file is not test code as far as the lint is concerned.
fn run(arguments: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_renew"))
        .args(arguments)
        .output()
}

/// A scratch directory unique to this test binary's process.
fn scratch(tag: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("renew-asset-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

/// The parsed envelope, having checked the two fields every one carries.
fn envelope(text: &str, command: &str) -> Result<Value, String> {
    let document = json::parse(text.trim()).map_err(|error| error.to_string())?;
    if document.get("command").and_then(Value::as_str) != Some(command) {
        return Err(format!("envelope is not `{command}`: {text}"));
    }
    if document.get("schema_version") != Some(&Value::Number(1)) {
        return Err(format!("envelope carries no schema_version: {text}"));
    }
    Ok(document)
}

/// Packing a tree, then reading it back through the tool.
#[test]
fn a_directory_packs_and_inspects() {
    let dir = scratch("roundtrip").expect("scratch dir");
    let src = dir.join("src");
    write(&src.join("notes.txt"), b"hello pack").expect("scratch file");
    write(&src.join("shader/tri.spv"), &[1, 2, 3, 4]).expect("scratch file");
    write(&src.join("nothing.bin"), b"").expect("scratch file");
    let pack = dir.join("out.rpk");

    let output = run(&[
        "asset-pack",
        "--from",
        &src.to_string_lossy(),
        "--pack",
        &pack.to_string_lossy(),
    ])
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains('3'), "stdout was: {stdout}");
    assert!(pack.is_file(), "the pack was not written");

    let output =
        run(&["asset-inspect", "--pack", &pack.to_string_lossy()]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Nested files are named by their forward-slashed relative path on
    // every platform, which is what makes a pack built on Windows equal
    // to one built on Linux.
    assert!(stdout.contains("shader/tri.spv"), "stdout was: {stdout}");
    assert!(stdout.contains("notes.txt"), "stdout was: {stdout}");
    assert!(
        !stdout.contains('\\'),
        "a backslash reached a name: {stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// The JSON envelopes carry what a script needs, with unconditional keys.
#[test]
fn both_subcommands_emit_the_typed_envelope() {
    let dir = scratch("json").expect("scratch dir");
    let src = dir.join("src");
    write(&src.join("one"), b"a").expect("scratch file");
    write(&src.join("two"), b"bb").expect("scratch file");
    let pack = dir.join("out.rpk");

    let output = run(&[
        "asset-pack",
        "--from",
        &src.to_string_lossy(),
        "--pack",
        &pack.to_string_lossy(),
        "--json",
    ])
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let document =
        envelope(&String::from_utf8_lossy(&output.stdout), "asset-pack").expect("a typed envelope");
    assert_eq!(document.get("entries"), Some(&Value::Number(2)));
    assert_eq!(document.get("status").and_then(Value::as_str), Some("ok"));

    // The same check without `--json`, because the two paths print from
    // different code and only the JSON one was ever asserted. The
    // human-readable success line went unexercised until a match arm made
    // that visible -- the summary was one line then, covered incidentally
    // by the corrupt-pack test taking the same branch.
    let plain = run(&[
        "asset-inspect",
        "--pack",
        &pack.to_string_lossy(),
        "--verify",
    ])
    .expect("binary should spawn");
    assert_eq!(plain.status.code(), Some(0));
    let text = String::from_utf8_lossy(&plain.stdout);
    assert!(
        text.contains("2 entries, verified"),
        "a clean verify must say so: {text}"
    );
    assert!(!text.contains("MISMATCH"), "nothing is wrong here: {text}");

    let output = run(&[
        "asset-inspect",
        "--pack",
        &pack.to_string_lossy(),
        "--verify",
        "--json",
    ])
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let document = envelope(&String::from_utf8_lossy(&output.stdout), "asset-inspect")
        .expect("a typed envelope");
    assert_eq!(document.get("verified"), Some(&Value::Bool(true)));
    assert_eq!(document.get("mismatched"), Some(&Value::Array(Vec::new())));
    let entries = document
        .get("entries")
        .and_then(Value::as_array)
        .expect("an entries array");
    assert_eq!(entries.len(), 2);
    for entry in entries {
        for key in ["name", "hash", "bytes"] {
            assert!(entry.get(key).is_some(), "an entry is missing `{key}`");
        }
    }

    let _ = fs::remove_dir_all(&dir);
}

/// The same tree packs to the same bytes twice. The tool's half of the
/// determinism claim: the crate sorts, but the walk happens here.
#[test]
fn the_same_tree_packs_to_the_same_bytes() {
    let dir = scratch("determinism").expect("scratch dir");
    let src = dir.join("src");
    write(&src.join("b/second"), b"two").expect("scratch file");
    write(&src.join("a/first"), b"one").expect("scratch file");
    write(&src.join("c"), b"three").expect("scratch file");

    let mut built = Vec::new();
    for name in ["one.rpk", "two.rpk"] {
        let pack = dir.join(name);
        let output = run(&[
            "asset-pack",
            "--from",
            &src.to_string_lossy(),
            "--pack",
            &pack.to_string_lossy(),
        ])
        .expect("binary should spawn");
        assert_eq!(output.status.code(), Some(0));
        built.push(fs::read(&pack).expect("the pack is readable"));
    }
    assert_eq!(built[0], built[1], "two packs of one tree differ");

    let _ = fs::remove_dir_all(&dir);
}

/// Verification finds a payload someone changed under the pack.
#[test]
fn a_corrupted_payload_fails_verification_and_only_verification() {
    let dir = scratch("corrupt").expect("scratch dir");
    let src = dir.join("src");
    write(&src.join("payload"), b"original bytes").expect("scratch file");
    let pack = dir.join("out.rpk");
    let output = run(&[
        "asset-pack",
        "--from",
        &src.to_string_lossy(),
        "--pack",
        &pack.to_string_lossy(),
    ])
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));

    // Flip the last byte: the structure is untouched, so the pack still
    // reads and only verification can tell.
    let mut bytes = fs::read(&pack).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    fs::write(&pack, &bytes).expect("write");

    let listed =
        run(&["asset-inspect", "--pack", &pack.to_string_lossy()]).expect("binary should spawn");
    assert_eq!(listed.status.code(), Some(0), "listing does not verify");

    let checked = run(&[
        "asset-inspect",
        "--pack",
        &pack.to_string_lossy(),
        "--verify",
    ])
    .expect("binary should spawn");
    assert_eq!(checked.status.code(), Some(1), "verification must fail");
    let stdout = String::from_utf8_lossy(&checked.stdout);
    assert!(stdout.contains("MISMATCH"), "stdout was: {stdout}");

    // The summary line has to agree with the line above it. This test
    // asserted the exit code and the MISMATCH and stopped there, so for a
    // while the output read `MISMATCH b.txt` and then `2 entries,
    // verified` two lines later -- the code correct, the JSON correct,
    // and the only part a person reads contradicting itself.
    assert!(
        !stdout.contains(", verified"),
        "a failed run must not call itself verified: {stdout}"
    );
    assert!(
        stdout.contains("FAILED verification"),
        "the summary must say verification failed: {stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// Every refusal names what was wrong, on both output paths.
#[test]
fn the_refusals_say_what_was_wrong() {
    let dir = scratch("refusals").expect("scratch dir");
    let missing = dir.join("nope");
    let pack = dir.join("out.rpk");

    // A source that is not a directory.
    let output = run(&[
        "asset-pack",
        "--from",
        &missing.to_string_lossy(),
        "--pack",
        &pack.to_string_lossy(),
    ])
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a directory"), "stderr was: {stderr}");

    // The same, in JSON: the envelope carries the reason and the keys are
    // present even on the failure path.
    let output = run(&[
        "asset-pack",
        "--from",
        &missing.to_string_lossy(),
        "--pack",
        &pack.to_string_lossy(),
        "--json",
    ])
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let document =
        envelope(&String::from_utf8_lossy(&output.stdout), "asset-pack").expect("a typed envelope");
    assert_eq!(
        document.get("status").and_then(Value::as_str),
        Some("error")
    );
    assert_eq!(document.get("entries"), Some(&Value::Array(Vec::new())));

    // A pack file that does not exist.
    let output =
        run(&["asset-inspect", "--pack", &missing.to_string_lossy()]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());

    // A file that exists and is not a pack, refused at the magic.
    let bogus = dir.join("bogus.rpk");
    write(&bogus, b"\x89PNG\r\n\x1a\n not a pack at all").expect("scratch file");
    let output = run(&[
        "asset-inspect",
        "--pack",
        &bogus.to_string_lossy(),
        "--json",
    ])
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let document = envelope(&String::from_utf8_lossy(&output.stdout), "asset-inspect")
        .expect("a typed envelope");
    let stderr = document
        .get("stderr")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(stderr.contains("magic"), "stderr was: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}

/// An empty directory is a real answer, not an error.
#[test]
fn an_empty_directory_packs_to_an_empty_pack() {
    let dir = scratch("empty").expect("scratch dir");
    let src = dir.join("src");
    fs::create_dir_all(&src).expect("scratch dir");
    let pack = dir.join("out.rpk");

    let output = run(&[
        "asset-pack",
        "--from",
        &src.to_string_lossy(),
        "--pack",
        &pack.to_string_lossy(),
    ])
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));

    let output = run(&["asset-inspect", "--pack", &pack.to_string_lossy(), "--json"])
        .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let document = envelope(&String::from_utf8_lossy(&output.stdout), "asset-inspect")
        .expect("a typed envelope");
    assert_eq!(document.get("entries"), Some(&Value::Array(Vec::new())));

    let _ = fs::remove_dir_all(&dir);
}

/// The flags belong to their own subcommands, and each is required.
#[test]
fn the_asset_flags_are_checked_against_their_subcommands() {
    // A flag on the wrong subcommand.
    let output = run(&["build", "--pack", "x.rpk"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--pack"));

    let output =
        run(&["asset-inspect", "--pack", "x.rpk", "--from", "d"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--from"));

    let output = run(&["asset-pack", "--from", "d", "--pack", "x.rpk", "--verify"])
        .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--verify"));

    // And each subcommand's required input.
    for line in [
        vec!["asset-pack", "--from", "d"],
        vec!["asset-pack", "--pack", "x.rpk"],
        vec!["asset-inspect"],
    ] {
        let output = run(&line).expect("binary should spawn");
        assert_eq!(output.status.code(), Some(2), "line was {line:?}");
    }
}
