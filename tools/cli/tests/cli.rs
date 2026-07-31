//! Integration tests: spawn the built binary and check its observable
//! contract — exit codes, usage output, and the JSON envelope.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// Fallible on purpose: the expect() lives inside each #[test], where the
// lint configuration scopes it.
fn run(arguments: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_renew"))
        .args(arguments)
        .output()
}

fn run_in(directory: &Path, arguments: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_renew"))
        .args(arguments)
        .current_dir(directory)
        .output()
}

/// Run with `PATH` replaced by exactly `search`, so what the binary can
/// and cannot spawn is decided by the test rather than by whatever the
/// machine happens to have installed.
fn run_with_path(directory: &Path, search: &Path, arguments: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_renew"))
        .args(arguments)
        .current_dir(directory)
        .env("PATH", search)
        .output()
}

/// This crate's directory — inside the workspace, so the binary finds a
/// root there.
fn inside_the_workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Minimal JSON validity checker: full grammar walk, no values retained.
/// Catches structurally mangled documents a prefix check would pass;
/// deliberately lenient on number lexemes (defers to float parsing).
fn validate_json(text: &str) -> Result<(), String> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        at: 0,
    };
    parser.value()?;
    parser.skip_ws();
    if parser.at == parser.bytes.len() {
        Ok(())
    } else {
        Err(format!("trailing data at byte {}", parser.at))
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }
    fn eat(&mut self, byte: u8) -> Result<(), String> {
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("expected `{}` at byte {}", byte as char, self.at))
        }
    }
    fn literal(&mut self, word: &str) -> Result<(), String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(())
        } else {
            Err(format!("bad literal at byte {}", self.at))
        }
    }
    fn string(&mut self) -> Result<(), String> {
        self.eat(b'"')?;
        loop {
            match self.bytes.get(self.at) {
                None => return Err("unterminated string".to_string()),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(());
                }
                Some(b'\\') => {
                    self.at += 1;
                    match self.bytes.get(self.at) {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.at += 1;
                        }
                        Some(b'u') => {
                            self.at += 1;
                            for _ in 0..4 {
                                if !self.bytes.get(self.at).is_some_and(u8::is_ascii_hexdigit) {
                                    return Err(format!("bad \\u at byte {}", self.at));
                                }
                                self.at += 1;
                            }
                        }
                        _ => return Err(format!("bad escape at byte {}", self.at)),
                    }
                }
                Some(control) if *control < 0x20 => {
                    return Err(format!("raw control byte at {}", self.at));
                }
                Some(_) => self.at += 1,
            }
        }
    }
    fn number(&mut self) -> Result<(), String> {
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.at += 1;
        }
        let slice = std::str::from_utf8(&self.bytes[start..self.at])
            .map_err(|_| "non-utf8 number".to_string())?;
        slice
            .parse::<f64>()
            .map(|_| ())
            .map_err(|_| format!("bad number `{slice}` at byte {start}"))
    }
    fn value(&mut self) -> Result<(), String> {
        self.skip_ws();
        match self.bytes.get(self.at) {
            Some(b'{') => {
                self.at += 1;
                self.skip_ws();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(());
                }
                loop {
                    self.skip_ws();
                    self.string()?;
                    self.skip_ws();
                    self.eat(b':')?;
                    self.value()?;
                    self.skip_ws();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(());
                        }
                        _ => return Err(format!("bad object at byte {}", self.at)),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                self.skip_ws();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(());
                }
                loop {
                    self.value()?;
                    self.skip_ws();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(());
                        }
                        _ => return Err(format!("bad array at byte {}", self.at)),
                    }
                }
            }
            Some(b'"') => self.string(),
            Some(b't') => self.literal("true"),
            Some(b'f') => self.literal("false"),
            Some(b'n') => self.literal("null"),
            Some(_) => self.number(),
            None => Err("empty value".to_string()),
        }
    }
}

// Fallible on purpose (like `run`): the expect() lives inside each #[test].
/// An empty scratch directory of its own, named after the test that owns it.
fn scratch_directory(tag: &str) -> std::io::Result<PathBuf> {
    let directory = std::env::temp_dir().join(format!("renew-cli-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

/// Creates a scratch directory holding a deliberately broken workspace: the
/// member listed in the manifest does not exist, so any cargo command fails
/// fast with its own nonzero exit code.
fn broken_workspace(tag: &str) -> std::io::Result<PathBuf> {
    let directory = scratch_directory(&format!("broken-{tag}"))?;
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"missing\"]\n",
    )?;
    Ok(directory)
}

/// Run `renew` in `directory` with a build environment of its own.
///
/// Both parts matter for a test that makes the binary build something.
/// A target directory of its own, so the child cargo cannot contend for
/// the lock the outer cargo holds on this workspace's. And no inherited
/// compiler flags, so a throwaway crate compiled here is not built with
/// the outer run's coverage instrumentation and does not drop counters
/// into its profile. The profile *destination* is deliberately left
/// alone: the binary under test is instrumented, and its own counters
/// have to keep landing where the run collecting them expects.
fn run_building_in(directory: &Path, arguments: &[&str]) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_renew"));
    command
        .args(arguments)
        .current_dir(directory)
        .env("CARGO_TARGET_DIR", directory.join("target"));
    for inherited in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTDOCFLAGS",
        "CARGO_ENCODED_RUSTDOCFLAGS",
    ] {
        command.env_remove(inherited);
    }
    command.output()
}

/// A scratch workspace holding one trivial sample: it echoes the command
/// line it was handed, and exits 3 when told to fail. Enough to prove
/// the pass-through and the exit-code contract in a compile this test
/// can afford, which building a real sample here would not be.
///
/// Its package name and its binary name differ, as the real samples'
/// do, so the lookup has to go through the binary target rather than
/// guessing that the two are the same word.
fn sample_workspace(tag: &str) -> std::io::Result<PathBuf> {
    let directory = scratch_directory(&format!("run-{tag}"))?;
    let sample = directory.join("samples").join("echo");
    fs::create_dir_all(sample.join("src"))?;
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"samples/echo\"]\n",
    )?;
    // A tree in the temp directory inherits none of this repository's
    // configuration, so the toolchain is pinned here explicitly rather
    // than left to whatever the machine's default happens to be.
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml"),
        directory.join("rust-toolchain.toml"),
    )?;
    fs::write(
        sample.join("Cargo.toml"),
        concat!(
            "[package]\nname = \"scratch-sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n",
            "[[bin]]\nname = \"echo_sample\"\npath = \"src/main.rs\"\n",
        ),
    )?;
    fs::write(
        sample.join("src").join("main.rs"),
        concat!(
            "fn main() {\n",
            "    let args: Vec<String> = std::env::args().skip(1).collect();\n",
            "    println!(\"echo_sample saw [{}]\", args.join(\" \"));\n",
            // A real sample ends a replay by printing its digest. This
            // one imitates that when it is replaying, so `replay`'s
            // envelope has a line to lift and `record`'s has none.
            "    let quiet = args.iter().any(|argument| argument == \"--quiet\");\n",
            "    if !quiet && args.iter().any(|argument| argument == \"--replay-trace\") {\n",
            "        println!(\"renew-frame sample=echo_sample state_hash=0x00000000000000ab\");\n",
            "    }\n",
            "    if args.iter().any(|argument| argument == \"--fail\") {\n",
            "        std::process::exit(3);\n",
            "    }\n",
            "}\n",
        ),
    )?;
    Ok(directory)
}

#[test]
fn no_arguments_is_a_usage_error() {
    let output = run(&[]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage:"), "stderr was: {stderr}");
}

#[test]
fn unknown_command_is_a_usage_error() {
    let output = run(&["deploy"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown command"), "stderr was: {stderr}");
}

#[test]
fn help_prints_usage_and_succeeds() {
    let output = run(&["help"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("usage:"), "stdout was: {stdout}");
    assert!(stdout.contains("doctor"), "stdout was: {stdout}");
}

#[test]
fn help_json_emits_a_single_valid_document() {
    let output = run(&["help", "--json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let document = stdout.trim();
    validate_json(document).expect("help --json must be valid JSON");
    assert!(
        document.starts_with("{\"schema_version\":1,\"command\":\"help\""),
        "stdout was: {stdout}"
    );
    assert!(document.contains("usage:"), "stdout was: {stdout}");
}

#[test]
fn configure_json_emits_a_schema_versioned_document() {
    let output = run(&["configure", "--json"]).expect("binary should spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let document = stdout.trim();
    validate_json(document).expect("configure --json must be valid JSON");
    assert!(
        document.starts_with("{\"schema_version\":1,\"command\":\"configure\""),
        "stdout was: {stdout}"
    );
    // The environment may legitimately lack rustup; the contract under test
    // is the envelope, not the environment.
    assert!(
        document.contains("\"status\":\"ok\"")
            || document.contains("\"status\":\"failed\"")
            || document.contains("\"status\":\"error\""),
        "stdout was: {stdout}"
    );
}

#[test]
fn configure_plain_streams_child_output() {
    let output = run(&["configure"]).expect("binary should spawn");
    // The environment may legitimately lack rustup (exit 1); when the
    // toolchain is present, child output must stream through on stdout.
    match output.status.code() {
        Some(0) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("cargo"), "stdout was: {stdout}");
        }
        Some(1) => {}
        other => panic!("unexpected exit: {other:?}"),
    }
}

#[test]
fn doctor_plain_reports_named_checks() {
    let output = run(&["doctor"]).expect("binary should spawn");
    // Doctor's verdict depends on the machine; its report shape must not.
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "unexpected exit: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in ["rustup", "toolchain", "cargo", "workspace", "git"] {
        assert!(stdout.contains(name), "missing check `{name}` in: {stdout}");
    }
}

#[test]
fn doctor_json_reports_named_checks_with_uniform_envelope() {
    let output = run(&["doctor", "--json"]).expect("binary should spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let document = stdout.trim();
    validate_json(document).expect("doctor --json must be valid JSON");
    assert!(
        document.starts_with("{\"schema_version\":1,\"command\":\"doctor\""),
        "stdout was: {stdout}"
    );
    // Uniform envelope: doctor carries the same stdout/stderr fields as
    // every other subcommand, plus its checks array.
    assert!(document.contains("\"stdout\":\"\""), "stdout was: {stdout}");
    assert!(document.contains("\"stderr\":\"\""), "stdout was: {stdout}");
    for name in ["rustup", "toolchain", "cargo", "workspace", "git"] {
        assert!(
            document.contains(&format!("\"name\":\"{name}\"")),
            "missing check `{name}` in: {stdout}"
        );
    }
}

#[test]
fn check_passes_against_this_workspace() {
    let output = run(&["check"]).expect("binary should spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "stdout was: {stdout}");
    assert!(stdout.contains("healthy"), "stdout was: {stdout}");
}

#[test]
fn check_json_reports_an_empty_findings_array_here() {
    let output = run(&["check", "--json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let document = stdout.trim();
    validate_json(document).expect("check --json must be valid JSON");
    assert!(
        document.starts_with("{\"schema_version\":1,\"command\":\"check\""),
        "stdout was: {stdout}"
    );
    assert!(document.contains("\"findings\":[]"), "stdout was: {stdout}");
}

#[test]
fn check_flags_a_workspace_with_broken_metadata() {
    let directory = std::env::temp_dir().join(format!("renew-cli-check-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    let member = directory.join("bad");
    fs::create_dir_all(member.join("src")).expect("scratch dirs");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"bad\"]\n",
    )
    .expect("scratch root manifest");
    fs::write(
        member.join("Cargo.toml"),
        concat!(
            "[package]\nname = \"bad\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n",
            "[package.metadata.renew]\npurpose = \"x\"\nmaturity = \"wrong\"\n",
            "core = false\nextension_points = []\nsimulation = false\n",
        ),
    )
    .expect("scratch member manifest");
    fs::write(member.join("src").join("lib.rs"), "").expect("scratch lib");

    let output = run_in(&directory, &["check"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("maturity"), "stdout was: {stdout}");

    // The machine mode carries the same findings in the envelope.
    let output = run_in(&directory, &["check", "--json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let document = stdout.trim();
    validate_json(document).expect("failing check --json must be valid JSON");
    assert!(
        document.contains("\"rule\":\"schema\""),
        "stdout was: {stdout}"
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn failed_child_maps_to_exit_one_in_plain_mode() {
    let directory = broken_workspace("plain").expect("scratch workspace should be creatable");
    let output = run_in(&directory, &["build"]).expect("binary should spawn");
    // cargo fails with its own nonzero code (101); the contract maps any
    // child failure to exit 1, never the raw child code.
    assert_eq!(output.status.code(), Some(1));
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn failed_child_maps_to_exit_one_with_raw_code_in_the_envelope() {
    let directory = broken_workspace("json").expect("scratch workspace should be creatable");
    let output = run_in(&directory, &["build", "--json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let document = stdout.trim();
    validate_json(document).expect("failure envelope must be valid JSON");
    assert!(
        document.contains("\"status\":\"failed\""),
        "stdout was: {stdout}"
    );
    // The envelope preserves the child's real exit code even though the
    // process exit is normalized to 1.
    assert!(
        document.contains("\"exit_code\":101"),
        "stdout was: {stdout}"
    );
    let _ = fs::remove_dir_all(&directory);
}

// --- Structural envelope validation -------------------------------------
//
// The prefix assertions above prove field ordering at the byte level;
// these prove the full envelope contract with the crate's own JSON
// parser — every required field present and correctly typed — so the
// emitter and the parser cross-check each other.

use renew_cli::json::{self, Value};

/// Fallible on purpose: the `expect()` lives inside the test, where the
/// lint configuration scopes it.
fn validated_envelope(document: &str, command: &str) -> Result<Value, String> {
    let value = json::parse(document).map_err(|error| error.to_string())?;
    let Some(fields) = value.as_object() else {
        return Err("envelope is not an object".to_string());
    };
    match fields.first() {
        Some((key, Value::Number(1))) if key.as_str() == "schema_version" => {}
        other => {
            return Err(format!(
                "schema_version must lead with value 1, got {other:?}"
            ));
        }
    }
    if value.get("command").and_then(Value::as_str) != Some(command) {
        return Err(format!("command field does not say {command:?}"));
    }
    for name in ["status", "stdout", "stderr"] {
        if value.get(name).and_then(Value::as_str).is_none() {
            return Err(format!("missing or mistyped string field `{name}`"));
        }
    }
    for name in ["exit_code", "duration_ms"] {
        if !matches!(value.get(name), Some(Value::Number(_))) {
            return Err(format!("missing or mistyped number field `{name}`"));
        }
    }
    Ok(value)
}

#[test]
fn every_cheap_subcommand_emits_the_full_typed_envelope() {
    // The expensive subcommands (build/test/bench/lint) share the same
    // envelope emitter, covered by unit tests; spawning them here would
    // recurse the whole workspace build inside the test suite.
    for command in ["help", "configure", "doctor", "check"] {
        let output = run(&[command, "--json"]).expect("binary should spawn");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let envelope = validated_envelope(stdout.trim(), command)
            .unwrap_or_else(|reason| panic!("{command} --json: {reason}"));
        match command {
            "doctor" => {
                let checks = envelope.get("checks").and_then(Value::as_array);
                assert!(
                    checks.is_some_and(|items| !items.is_empty()),
                    "doctor must carry a non-empty checks array"
                );
            }
            "check" => {
                assert!(
                    envelope.get("findings").and_then(Value::as_array).is_some(),
                    "check must carry a findings array"
                );
            }
            _ => {}
        }
    }
}

#[test]
fn smoke_with_a_non_bench_subcommand_is_a_usage_error() {
    let output = run(&["test", "--smoke"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--smoke"), "stderr was: {stderr}");
}

#[test]
fn usage_documents_the_smoke_flag() {
    let output = run(&["help"]).expect("binary should spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--smoke"), "stdout was: {stdout}");
}

#[test]
fn smoke_usage_error_emits_no_envelope_even_with_json() {
    let output = run(&["test", "--smoke", "--json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "usage errors never emit an envelope"
    );
}

// --- Failure paths ------------------------------------------------------
//
// Every way a run can fail before or during its steps, driven through the
// binary so the exit code and the envelope are the things under test.

/// Reads the `stderr` field out of an envelope.
fn envelope_stderr(envelope: &Value) -> &str {
    envelope.get("stderr").and_then(Value::as_str).unwrap_or("")
}

#[test]
fn a_run_outside_any_workspace_fails_in_both_modes() {
    let directory = scratch_directory("noroot").expect("scratch directory should be creatable");
    // Asserted, not assumed: if this machine really does keep a Cargo
    // workspace above its temp directory, say so rather than quietly
    // measuring the success path instead.
    assert!(
        renew_cli::workspace::find_root(&directory).is_none(),
        "precondition: no workspace manifest may sit above {}",
        directory.display()
    );

    let output = run_in(&directory, &["build"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "the plain path reports on stderr only"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no workspace root"), "stderr was: {stderr}");

    let output = run_in(&directory, &["build", "--json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "build")
        .unwrap_or_else(|reason| panic!("build --json: {reason}"));
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("error")
    );
    assert_eq!(envelope.get("exit_code"), Some(&Value::Number(1)));
    assert!(
        envelope_stderr(&envelope).contains("no workspace root"),
        "stdout was: {stdout}"
    );

    // `check` resolves the root for itself, and must refuse the same way
    // rather than passing vacuously on a tree it never found.
    let output = run_in(&directory, &["check"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no workspace root"), "stderr was: {stderr}");

    let _ = fs::remove_dir_all(&directory);
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "replaces PATH, which under ASan on Windows also hides the sanitizer runtime DLL"
)]
fn a_step_that_cannot_be_spawned_fails_in_both_modes() {
    let empty = scratch_directory("nopath").expect("scratch directory should be creatable");
    // `configure`'s first step is `rustup`; an empty search path makes it
    // unspawnable without touching the machine's real installation.
    let output =
        run_with_path(inside_the_workspace(), &empty, &["configure"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to run rustup"),
        "stderr was: {stderr}"
    );

    let output = run_with_path(inside_the_workspace(), &empty, &["configure", "--json"])
        .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "configure")
        .unwrap_or_else(|reason| panic!("configure --json: {reason}"));
    // A step that never ran is an `error`, not a `failed` child.
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("error")
    );
    assert_eq!(envelope.get("exit_code"), Some(&Value::Number(1)));
    assert!(
        envelope_stderr(&envelope).contains("failed to run rustup"),
        "stdout was: {stdout}"
    );

    let _ = fs::remove_dir_all(&empty);
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "replaces PATH, which under ASan on Windows also hides the sanitizer runtime DLL"
)]
fn check_fails_loudly_when_cargo_metadata_cannot_be_read() {
    let directory = broken_workspace("metadata").expect("scratch workspace should be creatable");
    // Pin the search path to the cargo that built this test, so the child
    // really does spawn `cargo metadata` (and fails on the manifest)
    // instead of failing to find cargo at all.
    let cargo_directory = Path::new(env!("CARGO"))
        .parent()
        .expect("cargo lives in a directory");

    let output = run_with_path(&directory, cargo_directory, &["check"]).expect("binary spawns");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "a check that cannot run reports on stderr only"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cargo metadata"), "stderr was: {stderr}");

    let output =
        run_with_path(&directory, cargo_directory, &["check", "--json"]).expect("binary spawns");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "check")
        .unwrap_or_else(|reason| panic!("check --json: {reason}"));
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("error")
    );
    assert_eq!(envelope.get("exit_code"), Some(&Value::Number(1)));
    assert!(
        envelope_stderr(&envelope).contains("cargo metadata"),
        "stdout was: {stdout}"
    );
    // The key is unconditional: a check that could not run still reports an
    // empty findings array rather than omitting it.
    assert_eq!(
        envelope.get("findings"),
        Some(&Value::Array(Vec::new())),
        "stdout was: {stdout}"
    );

    // The other way the same step can fail: no cargo to spawn at all.
    let empty = scratch_directory("nocargo").expect("scratch directory should be creatable");
    let output = run_with_path(&directory, &empty, &["check"]).expect("binary spawns");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to run cargo metadata"),
        "stderr was: {stderr}"
    );

    let _ = fs::remove_dir_all(&empty);
    let _ = fs::remove_dir_all(&directory);
}

// --- The coverage gate ---------------------------------------------------
//
// Driven through a scratch workspace holding its own manifest and its own
// llvm-cov export, so both directions of the ratchet — and every way the
// gate can fail to run — are exercised without a real coverage collection.

/// An `llvm-cov` export naming one file under `root`, with the given
/// `[line_start, column_start, line_end, column_end, count, file_id, …]`
/// regions. Forward slashes on both platforms: the reader normalizes the
/// root it compares against.
fn coverage_export(root: &Path, regions: &str) -> String {
    let base = root.to_string_lossy().replace('\\', "/");
    format!(
        concat!(
            "{{\"data\":[{{\"files\":[{{\"filename\":\"{base}/crates/a.rs\"}}],",
            "\"functions\":[{{\"filenames\":[\"{base}/crates/a.rs\"],",
            "\"regions\":[{regions}]}}]}}]}}"
        ),
        base = base,
        regions = regions
    )
}

/// A scratch workspace carrying a coverage manifest and a report.
fn coverage_workspace(tag: &str, manifest: &str, regions: &str) -> std::io::Result<PathBuf> {
    let directory = scratch_directory(&format!("coverage-{tag}"))?;
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = []\n",
    )?;
    fs::write(directory.join("coverage-exemptions.toml"), manifest)?;
    fs::write(
        directory.join("report.json"),
        coverage_export(&directory, regions),
    )?;
    Ok(directory)
}

/// Line 10 never ran; line 20 did.
const ONE_GAP: &str = "[10,1,10,9,0,0,0,0],[20,1,20,9,3,0,0,0]";

fn exempting(file: &str, lines: &str) -> String {
    format!("[[exempt]]\nfile = \"{file}\"\nlines = {lines}\nreason = \"documented\"\n")
}

#[test]
fn coverage_passes_when_every_uncovered_line_is_exempt() {
    let directory = coverage_workspace("pass", &exempting("crates/a.rs", "[10]"), ONE_GAP)
        .expect("scratch workspace should be creatable");

    let output =
        run_in(&directory, &["coverage", "--report", "report.json"]).expect("binary should spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "stdout was: {stdout}");
    assert!(
        stdout.contains("coverage is complete"),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains("1 exempt line(s)"), "stdout was: {stdout}");

    let output = run_in(
        &directory,
        &["coverage", "--report", "report.json", "--json"],
    )
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "coverage")
        .unwrap_or_else(|reason| panic!("coverage --json: {reason}"));
    assert_eq!(envelope.get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(envelope.get("uncovered"), Some(&Value::Array(Vec::new())));
    assert_eq!(envelope.get("stale"), Some(&Value::Array(Vec::new())));
    assert_eq!(envelope.get("exempt_lines"), Some(&Value::Number(1)));
    assert_eq!(envelope.get("measured_files"), Some(&Value::Number(1)));

    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn coverage_fails_on_an_uncovered_line_with_no_exemption() {
    let directory = coverage_workspace("gap", "# nothing is exempt\n", ONE_GAP)
        .expect("scratch workspace should be creatable");

    let output =
        run_in(&directory, &["coverage", "--report", "report.json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FAIL uncovered"), "stdout was: {stdout}");
    assert!(stdout.contains("crates/a.rs:10"), "stdout was: {stdout}");
    assert!(
        stdout.contains("1 coverage finding(s)"),
        "stdout was: {stdout}"
    );

    let output = run_in(
        &directory,
        &["coverage", "--report", "report.json", "--json"],
    )
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "coverage")
        .unwrap_or_else(|reason| panic!("coverage --json: {reason}"));
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("failed")
    );
    let uncovered = envelope
        .get("uncovered")
        .and_then(Value::as_array)
        .unwrap_or_default();
    assert_eq!(uncovered.len(), 1, "stdout was: {stdout}");
    assert_eq!(
        uncovered.first().and_then(|site| site.get("line")),
        Some(&Value::Number(10))
    );

    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn coverage_fails_on_an_exemption_whose_line_is_covered_now() {
    // The other direction of the ratchet: line 20 ran, so its exemption is
    // a hole in the gate and has to be deleted.
    let directory = coverage_workspace("stale", &exempting("crates/a.rs", "[10, 20]"), ONE_GAP)
        .expect("scratch workspace should be creatable");

    let output =
        run_in(&directory, &["coverage", "--report", "report.json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("FAIL stale-exemption"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("crates/a.rs:20 is covered now"),
        "stdout was: {stdout}"
    );

    let output = run_in(
        &directory,
        &["coverage", "--report", "report.json", "--json"],
    )
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "coverage")
        .unwrap_or_else(|reason| panic!("coverage --json: {reason}"));
    let stale = envelope
        .get("stale")
        .and_then(Value::as_array)
        .unwrap_or_default();
    assert_eq!(stale.len(), 1, "stdout was: {stdout}");
    let entry = stale.first().expect("one stale entry");
    assert_eq!(entry.get("line"), Some(&Value::Number(20)));
    assert_eq!(
        entry.get("state").and_then(Value::as_str),
        Some("now-covered")
    );
    assert_eq!(
        entry.get("reason").and_then(Value::as_str),
        Some("documented"),
        "the entry's own reason travels with the finding"
    );

    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn coverage_fails_on_an_exemption_the_report_never_measured() {
    let directory = coverage_workspace(
        "absent",
        &format!(
            "{}{}",
            exempting("crates/a.rs", "[10]"),
            exempting("crates/gone.rs", "[5]")
        ),
        ONE_GAP,
    )
    .expect("scratch workspace should be creatable");

    let output =
        run_in(&directory, &["coverage", "--report", "report.json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("crates/gone.rs:5 is not in the report"),
        "stdout was: {stdout}"
    );

    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_coverage_gate_that_cannot_read_its_inputs_fails_loudly() {
    let directory = coverage_workspace("inputs", &exempting("crates/a.rs", "[10]"), ONE_GAP)
        .expect("scratch workspace should be creatable");
    let manifest = directory.join("coverage-exemptions.toml");
    let report = directory.join("report.json");

    // A report that is not there at all.
    let output =
        run_in(&directory, &["coverage", "--report", "missing.json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to read missing.json"),
        "stderr was: {stderr}"
    );

    // A report that is not JSON, and one that is JSON but not an export.
    for (body, expected) in [
        ("not json at all", "unreadable llvm-cov export"),
        ("{\"nope\":1}", "no `data` array"),
    ] {
        fs::write(&report, body).expect("scratch report");
        let output = run_in(&directory, &["coverage", "--report", "report.json"])
            .expect("binary should spawn");
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "stderr was: {stderr}");
    }

    // A manifest that does not parse, reported against its own line.
    fs::write(&manifest, "[[exempt]]\nfile = nope\n").expect("scratch manifest");
    let output =
        run_in(&directory, &["coverage", "--report", "report.json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("line 2:"), "stderr was: {stderr}");
    assert!(
        stderr.contains("coverage-exemptions.toml"),
        "stderr was: {stderr}"
    );

    // No manifest at all: silence is not a pass. The machine mode carries
    // the same refusal, with both arrays present and empty.
    fs::remove_file(&manifest).expect("scratch manifest should be removable");
    let output = run_in(
        &directory,
        &["coverage", "--report", "report.json", "--json"],
    )
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "coverage")
        .unwrap_or_else(|reason| panic!("coverage --json: {reason}"));
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("error")
    );
    assert_eq!(envelope.get("exit_code"), Some(&Value::Number(1)));
    assert_eq!(envelope.get("uncovered"), Some(&Value::Array(Vec::new())));
    assert_eq!(envelope.get("stale"), Some(&Value::Array(Vec::new())));
    assert!(
        envelope_stderr(&envelope).contains("coverage-exemptions.toml"),
        "stdout was: {stdout}"
    );

    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn coverage_outside_any_workspace_has_no_manifest_to_read() {
    let directory =
        scratch_directory("coverage-noroot").expect("scratch directory should be creatable");
    assert!(
        renew_cli::workspace::find_root(&directory).is_none(),
        "precondition: no workspace manifest may sit above {}",
        directory.display()
    );
    let output =
        run_in(&directory, &["coverage", "--report", "report.json"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no workspace root"), "stderr was: {stderr}");

    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn coverage_without_a_report_is_a_usage_error() {
    let output = run(&["coverage"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--report"), "stderr was: {stderr}");

    let output = run(&["coverage", "--report"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`--report` needs a value"),
        "stderr was: {stderr}"
    );
}

// --- Running a sample ----------------------------------------------------

#[test]
fn run_without_a_sample_is_a_usage_error() {
    let output = run(&["run"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("needs a sample"), "stderr was: {stderr}");
    assert!(stderr.contains("usage:"), "stderr was: {stderr}");
}

#[test]
fn usage_documents_how_run_hands_the_command_line_over() {
    let output = run(&["help"]).expect("binary should spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("run <sample> [--] [sample arguments...]"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("goes to the sample untouched"),
        "stdout was: {stdout}"
    );
}

#[test]
fn an_unknown_sample_is_a_usage_error_naming_the_samples_that_exist() {
    // Run against this workspace, so the list is discovered rather than
    // read out of a fixture: these are the samples that really are here.
    let output = run(&["run", "helo_triangle"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "usage errors never emit an envelope"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown sample `helo_triangle`"),
        "stderr was: {stderr}"
    );
    for name in ["hello_triangle", "input_echo"] {
        assert!(stderr.contains(name), "missing `{name}` in: {stderr}");
    }

    // Machine mode changes nothing: a command line that cannot be read
    // produced no run, so there is no envelope to report one.
    let output = run(&["--json", "run", "helo_triangle"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "usage errors never emit an envelope"
    );
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "replaces PATH, which under ASan on Windows also hides the sanitizer runtime DLL"
)]
fn a_run_that_cannot_read_the_sample_list_refuses_instead_of_denying_it() {
    // No workspace at all: the same refusal every other subcommand gives.
    let empty = scratch_directory("run-noroot").expect("scratch directory should be creatable");
    assert!(
        renew_cli::workspace::find_root(&empty).is_none(),
        "precondition: no workspace manifest may sit above {}",
        empty.display()
    );
    let output = run_in(&empty, &["run", "hello_triangle"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no workspace root"), "stderr was: {stderr}");

    // A workspace cargo itself cannot describe. An unreadable list is
    // not an empty one: this must not come back as "unknown sample",
    // which would send the caller off to check their spelling.
    let broken = broken_workspace("run").expect("scratch workspace should be creatable");
    let cargo_directory = Path::new(env!("CARGO"))
        .parent()
        .expect("cargo lives in a directory");
    let output =
        run_with_path(&broken, cargo_directory, &["run", "hello_triangle"]).expect("binary spawns");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cargo metadata"), "stderr was: {stderr}");
    assert!(!stderr.contains("unknown sample"), "stderr was: {stderr}");

    let _ = fs::remove_dir_all(&broken);
    let _ = fs::remove_dir_all(&empty);
}

/// The whole translation chain, end to end: `renew`'s flag becomes the
/// sample's flag, in front of the caller's own arguments, and only a
/// replay's envelope grows a digest.
#[test]
fn record_and_replay_translate_their_flag_and_lead_the_samples_line() {
    let directory = sample_workspace("trace").expect("scratch workspace should be creatable");

    for (subcommand, renew_flag, sample_flag) in [
        ("record", "--output", "--record-trace"),
        ("replay", "--input", "--replay-trace"),
    ] {
        // The caller's own `--output` comes after the sample name and so
        // is the sample's, while renew's identically-spelled flag before
        // it is renew's. Both must appear, in that order, with renew's
        // first — which is the failure the ordering rule exists to stop.
        let output = run_building_in(
            &directory,
            &[
                subcommand,
                renew_flag,
                "renew.trace",
                "echo_sample",
                "--output",
                "callers.txt",
            ],
        )
        .expect("binary should spawn");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout: {stdout}
stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = format!("echo_sample saw [{sample_flag} renew.trace --output callers.txt]");
        assert!(stdout.contains(&expected), "stdout was: {stdout}");
    }

    // A replay's result is its digest, and in JSON mode the child's
    // stdout is captured — so the line has to be lifted out as a field
    // or a caller would have to parse it back out of a string.
    let output = run_building_in(
        &directory,
        &["--json", "replay", "--input", "walk.trace", "echo_sample"],
    )
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "replay")
        .unwrap_or_else(|reason| panic!("replay --json: {reason}"));
    assert_eq!(
        envelope.get("digest").and_then(Value::as_str),
        Some("renew-frame sample=echo_sample state_hash=0x00000000000000ab"),
        "stdout was: {stdout}"
    );

    // Recording produces a file, not a digest, so the field is absent
    // rather than present and empty.
    let output = run_building_in(
        &directory,
        &["--json", "record", "--output", "walk.trace", "echo_sample"],
    )
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "record")
        .unwrap_or_else(|reason| panic!("record --json: {reason}"));
    assert_eq!(envelope.get("digest"), None, "stdout was: {stdout}");

    let _ = fs::remove_dir_all(&directory);
}

/// A replay whose sample printed no digest. The field is present and
/// null, not absent: the run happened and produced nothing to report,
/// which is a different fact from a subcommand that never carries one.
#[test]
fn a_replay_that_produced_no_digest_reports_null_rather_than_nothing() {
    let directory = sample_workspace("nodigest").expect("scratch workspace should be creatable");
    let output = run_building_in(
        &directory,
        &[
            "--json",
            "replay",
            "--input",
            "walk.trace",
            "echo_sample",
            "--quiet",
        ],
    )
    .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "replay")
        .unwrap_or_else(|reason| panic!("quiet replay --json: {reason}"));
    // The sample really did stay quiet, or this asserts nothing.
    assert!(
        envelope
            .get("stdout")
            .and_then(Value::as_str)
            .is_some_and(|captured| !captured.contains("renew-frame")),
        "the sample must not have printed a digest: {stdout}"
    );
    assert_eq!(
        envelope.get("digest"),
        Some(&Value::Null),
        "stdout was: {stdout}"
    );

    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn the_trace_subcommands_refuse_a_command_line_that_cannot_work() {
    // Neither of these reaches a workspace or a build: parsing refuses
    // first, which is why they are cheap enough to drive through the
    // real binary.
    for (line, named) in [
        (vec!["record", "echo_sample"], "--output"),
        (vec!["replay", "echo_sample"], "--input"),
        (
            vec!["record", "--input", "t.trace", "echo_sample"],
            "--input",
        ),
        (
            vec!["replay", "--output", "t.trace", "echo_sample"],
            "--output",
        ),
        (vec!["record", "--output", "t.trace"], "record"),
        (vec!["replay", "--input", "t.trace"], "replay"),
    ] {
        let output = run(&line).expect("binary should spawn");
        assert_eq!(output.status.code(), Some(2), "`{line:?}` must be refused");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(named),
            "`{line:?}` should have named `{named}`; stderr was: {stderr}"
        );
        assert!(
            output.stdout.is_empty(),
            "usage errors never emit an envelope"
        );
    }
}

#[test]
fn usage_documents_the_trace_subcommands_and_their_flags() {
    let output = run(&["help"]).expect("binary should spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "record",
        "replay",
        "--output",
        "--input",
        "--record-trace",
        "--replay-trace",
    ] {
        assert!(
            stdout.contains(expected),
            "usage omits {expected}: {stdout}"
        );
    }
}

#[test]
fn a_sample_gets_its_command_line_verbatim_and_its_exit_code_comes_back() {
    let directory = sample_workspace("echo").expect("scratch workspace should be creatable");

    // With the separator, and without it: the sample cannot tell which
    // spelling the caller used.
    for line in [
        vec!["run", "echo_sample", "--", "--headless", "--frames", "8"],
        vec!["run", "echo_sample", "--headless", "--frames", "8"],
    ] {
        let output = run_building_in(&directory, &line).expect("binary should spawn");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        // The sample's own line reaches this caller's stdout intact —
        // the property a downstream grep depends on.
        assert!(
            stdout.contains("echo_sample saw [--headless --frames 8]"),
            "stdout was: {stdout}"
        );
    }

    // A flag renew itself knows still belongs to the sample once the
    // sample has been named.
    let output = run_building_in(&directory, &["run", "echo_sample", "--json"])
        .expect("binary should spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "stdout was: {stdout}");
    assert!(
        stdout.contains("echo_sample saw [--json]"),
        "the sample, not renew, must have been given --json: {stdout}"
    );

    // Before the sample name, the same flag is renew's, and the sample's
    // output travels inside the envelope rather than beside it.
    let output = run_building_in(&directory, &["--json", "run", "echo_sample", "--", "-q"])
        .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "run")
        .unwrap_or_else(|reason| panic!("run --json: {reason}"));
    assert_eq!(envelope.get("status").and_then(Value::as_str), Some("ok"));
    assert!(
        envelope
            .get("stdout")
            .and_then(Value::as_str)
            .is_some_and(|captured| captured.contains("echo_sample saw [-q]")),
        "stdout was: {stdout}"
    );

    // A failing sample: the process exit is normalized to 1 like any
    // other failing child, and the raw code survives in the envelope.
    let output = run_building_in(&directory, &["run", "echo_sample", "--fail"])
        .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));

    let output = run_building_in(&directory, &["--json", "run", "echo_sample", "--fail"])
        .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "run")
        .unwrap_or_else(|reason| panic!("failing run --json: {reason}"));
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(envelope.get("exit_code"), Some(&Value::Number(3)));

    let _ = fs::remove_dir_all(&directory);
}

/// One named check out of a doctor envelope.
fn doctor_check<'a>(envelope: &'a Value, name: &str) -> Option<&'a Value> {
    envelope
        .get("checks")?
        .as_array()?
        .iter()
        .find(|check| check.get("name").and_then(Value::as_str) == Some(name))
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "replaces PATH, which under ASan on Windows also hides the sanitizer runtime DLL"
)]
fn doctor_fails_when_no_tool_is_on_the_search_path() {
    let empty = scratch_directory("doctor-bare").expect("scratch directory should be creatable");
    let output =
        run_with_path(inside_the_workspace(), &empty, &["doctor"]).expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("environment has problems"),
        "stdout was: {stdout}"
    );
    for line in [
        "FAIL rustup",
        "FAIL cargo",
        "FAIL git",
        // The tree is still found; only the tools are missing.
        "ok   workspace",
    ] {
        assert!(stdout.contains(line), "missing `{line}` in: {stdout}");
    }
    let _ = fs::remove_dir_all(&empty);
}

#[test]
#[cfg_attr(
    feature = "sanitized",
    ignore = "replaces PATH, which under ASan on Windows also hides the sanitizer runtime DLL"
)]
fn a_rustup_that_cannot_name_its_toolchain_still_counts_as_found() {
    let directory =
        scratch_directory("doctor-rustup").expect("scratch directory should be creatable");
    // The binary under test doubles as the stand-in: `renew show
    // active-toolchain` is a usage error, so the probe spawns and comes
    // back unsuccessful — the shape `rustup` has when no default toolchain
    // is configured. Nothing about the real installation is touched.
    let stand_in = directory.join(if cfg!(windows) {
        "rustup.exe"
    } else {
        "rustup"
    });
    fs::copy(env!("CARGO_BIN_EXE_renew"), &stand_in).expect("stand-in binary should copy");

    let output = run_with_path(inside_the_workspace(), &directory, &["doctor", "--json"])
        .expect("binary should spawn");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = validated_envelope(stdout.trim(), "doctor")
        .unwrap_or_else(|reason| panic!("doctor --json: {reason}"));

    let rustup = doctor_check(&envelope, "rustup").expect("rustup check present");
    assert_eq!(
        rustup.get("ok"),
        Some(&Value::Bool(true)),
        "a rustup that answers at all is found: {stdout}"
    );
    let toolchain = doctor_check(&envelope, "toolchain").expect("toolchain check present");
    assert_eq!(toolchain.get("ok"), Some(&Value::Bool(false)));
    assert_eq!(
        toolchain.get("detail").and_then(Value::as_str),
        Some("not detected"),
        "stdout was: {stdout}"
    );

    let _ = fs::remove_dir_all(&directory);
}
