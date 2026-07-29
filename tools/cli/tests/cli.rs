//! Integration tests: spawn the built binary and check its observable
//! contract — exit codes, usage output, and the JSON envelope.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

// Fallible on purpose: the expect() lives inside each #[test], where the
// lint configuration scopes it.
fn run(arguments: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_renew"))
        .args(arguments)
        .output()
}

fn run_in(directory: &std::path::Path, arguments: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_renew"))
        .args(arguments)
        .current_dir(directory)
        .output()
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
/// Creates a scratch directory holding a deliberately broken workspace: the
/// member listed in the manifest does not exist, so any cargo command fails
/// fast with its own nonzero exit code.
fn broken_workspace(tag: &str) -> std::io::Result<PathBuf> {
    let directory =
        std::env::temp_dir().join(format!("renew-cli-broken-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory)?;
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"missing\"]\n",
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
