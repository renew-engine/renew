//! The `renew` binary: a thin I/O shell over the `renew_cli` library.
//!
//! Exit-code contract: `0` success, `1` command failed or could not run,
//! `2` usage error. Child exit codes are reported raw in the JSON envelope's
//! `exit_code` field (signal deaths as `-1`); the process itself always
//! exits 0, 1, or 2.

use std::env;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};
use std::time::Instant;

use renew_cli::cli::{self, Command, Invocation, Parsed};
use renew_cli::doctor::{self, Facts};
use renew_cli::json::{self, Value};
use renew_cli::plan;
use renew_cli::structure;
use renew_cli::workspace;

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match cli::parse(&arguments) {
        Ok(Parsed::Help { json: false }) => {
            emit_stdout(&cli::usage());
            ExitCode::SUCCESS
        }
        Ok(Parsed::Help { json: true }) => {
            let document = json::result_envelope("help", "ok", 0, 0, &cli::usage(), "");
            emit_stdout_line(&document.render());
            ExitCode::SUCCESS
        }
        Ok(Parsed::Run(invocation)) => run(invocation),
        Err(error) => {
            eprint!("error: {error}\n\n{}", cli::usage());
            ExitCode::from(2)
        }
    }
}

fn run(invocation: Invocation) -> ExitCode {
    match invocation.command {
        Command::Doctor => run_doctor(invocation.json),
        Command::Check => run_check(invocation.json),
        _ => run_steps(invocation),
    }
}

fn run_check(json_mode: bool) -> ExitCode {
    let started = Instant::now();
    let outcome = gather_findings();
    match outcome {
        Ok(findings) => {
            let ok = findings.is_empty();
            if json_mode {
                let items: Vec<Value> = findings
                    .iter()
                    .map(|finding| {
                        Value::Object(vec![
                            ("rule".to_string(), Value::String(finding.rule.to_string())),
                            (
                                "message".to_string(),
                                Value::String(finding.message.clone()),
                            ),
                        ])
                    })
                    .collect();
                let document = Value::Object(vec![
                    ("schema_version".to_string(), Value::Number(1)),
                    ("command".to_string(), Value::String("check".to_string())),
                    (
                        "status".to_string(),
                        Value::String(if ok { "ok" } else { "failed" }.to_string()),
                    ),
                    ("exit_code".to_string(), Value::Number(i64::from(!ok))),
                    (
                        "duration_ms".to_string(),
                        Value::Number(duration_ms(started)),
                    ),
                    ("stdout".to_string(), Value::String(String::new())),
                    ("stderr".to_string(), Value::String(String::new())),
                    ("findings".to_string(), Value::Array(items)),
                ]);
                emit_stdout_line(&document.render());
            } else {
                let mut report = String::new();
                for finding in &findings {
                    let _ = writeln!(report, "FAIL {:<17} {}", finding.rule, finding.message);
                }
                let summary = if ok {
                    "workspace structure looks healthy".to_string()
                } else {
                    format!("{} structure finding(s)", findings.len())
                };
                let _ = writeln!(report, "{summary}");
                emit_stdout(&report);
            }
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(message) => {
            // A check that cannot run has failed — never pass vacuously.
            if json_mode {
                // Same shape as the success path: `findings` is always
                // present, so consumers never see conditional keys.
                let document = Value::Object(vec![
                    ("schema_version".to_string(), Value::Number(1)),
                    ("command".to_string(), Value::String("check".to_string())),
                    ("status".to_string(), Value::String("error".to_string())),
                    ("exit_code".to_string(), Value::Number(1)),
                    (
                        "duration_ms".to_string(),
                        Value::Number(duration_ms(started)),
                    ),
                    ("stdout".to_string(), Value::String(String::new())),
                    ("stderr".to_string(), Value::String(message)),
                    ("findings".to_string(), Value::Array(Vec::new())),
                ]);
                emit_stdout_line(&document.render());
                return ExitCode::FAILURE;
            }
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn gather_findings() -> Result<Vec<structure::Finding>, String> {
    let root = workspace_root()
        .ok_or_else(|| "no workspace root found above the current directory".to_string())?;
    let output = Process::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(&root)
        .output()
        .map_err(|error| format!("failed to run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    findings_from_metadata(&String::from_utf8_lossy(&output.stdout))
}

/// Run the structure rules over `cargo metadata` output. Split out from the
/// spawning half so the rejection of output that is not metadata at all is
/// exercisable without a child process.
fn findings_from_metadata(text: &str) -> Result<Vec<structure::Finding>, String> {
    let document =
        json::parse(text).map_err(|error| format!("unreadable cargo metadata output: {error}"))?;
    let shapes = structure::shapes_from_metadata(&document)?;
    Ok(structure::evaluate(&shapes))
}

fn run_steps(invocation: Invocation) -> ExitCode {
    let started = Instant::now();
    let name = invocation.command.name();
    let Some(root) = workspace_root() else {
        let message = "no workspace root found above the current directory\n";
        if invocation.json {
            return finish_json(name, "error", 1, started, "", message);
        }
        eprint!("error: {message}");
        return ExitCode::FAILURE;
    };

    let mut stdout_all = String::new();
    let mut stderr_all = String::new();
    for step in plan::steps(invocation.command, invocation.smoke) {
        if invocation.json {
            match Process::new(step.program)
                .args(step.args)
                .current_dir(&root)
                .output()
            {
                Ok(output) => {
                    stdout_all.push_str(&String::from_utf8_lossy(&output.stdout));
                    stderr_all.push_str(&String::from_utf8_lossy(&output.stderr));
                    if !output.status.success() {
                        // Raw child code in the envelope (-1 for signal
                        // deaths); the process exit stays within 0/1/2.
                        let code = output.status.code().unwrap_or(-1);
                        return finish_json(
                            name,
                            "failed",
                            code,
                            started,
                            &stdout_all,
                            &stderr_all,
                        );
                    }
                }
                Err(error) => {
                    let _ = writeln!(stderr_all, "failed to run {}: {error}", step.program);
                    return finish_json(name, "error", 1, started, &stdout_all, &stderr_all);
                }
            }
        } else {
            match Process::new(step.program)
                .args(step.args)
                .current_dir(&root)
                .status()
            {
                Ok(status) if status.success() => {}
                // The child streamed its own output; the contract maps any
                // child failure to exit 1.
                Ok(_) => return ExitCode::FAILURE,
                Err(error) => {
                    eprintln!("error: failed to run {}: {error}", step.program);
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    if invocation.json {
        finish_json(name, "ok", 0, started, &stdout_all, &stderr_all)
    } else {
        ExitCode::SUCCESS
    }
}

fn finish_json(
    command: &str,
    status: &str,
    child_code: i32,
    started: Instant,
    stdout: &str,
    stderr: &str,
) -> ExitCode {
    let document = json::result_envelope(
        command,
        status,
        i64::from(child_code),
        duration_ms(started),
        stdout,
        stderr,
    );
    emit_stdout_line(&document.render());
    if status == "ok" {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_doctor(json_mode: bool) -> ExitCode {
    let started = Instant::now();
    let facts = gather_facts();
    let checks = doctor::evaluate(&facts);
    let ok = doctor::all_ok(&checks);

    if json_mode {
        let items: Vec<Value> = checks
            .iter()
            .map(|check| {
                Value::Object(vec![
                    ("name".to_string(), Value::String(check.name.to_string())),
                    ("ok".to_string(), Value::Bool(check.ok)),
                    ("detail".to_string(), Value::String(check.detail.clone())),
                ])
            })
            .collect();
        // Same envelope prefix as every other subcommand, plus the
        // doctor-specific `checks` array.
        let document = Value::Object(vec![
            ("schema_version".to_string(), Value::Number(1)),
            ("command".to_string(), Value::String("doctor".to_string())),
            (
                "status".to_string(),
                Value::String(if ok { "ok" } else { "failed" }.to_string()),
            ),
            ("exit_code".to_string(), Value::Number(i64::from(!ok))),
            (
                "duration_ms".to_string(),
                Value::Number(duration_ms(started)),
            ),
            ("stdout".to_string(), Value::String(String::new())),
            ("stderr".to_string(), Value::String(String::new())),
            ("checks".to_string(), Value::Array(items)),
        ]);
        emit_stdout_line(&document.render());
    } else {
        let mut report = String::new();
        for check in &checks {
            let mark = if check.ok { "ok  " } else { "FAIL" };
            let _ = writeln!(report, "{mark} {:<13} {}", check.name, check.detail);
        }
        let summary = if ok {
            "environment looks healthy"
        } else {
            "environment has problems"
        };
        let _ = writeln!(report, "\n{summary}");
        emit_stdout(&report);
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn gather_facts() -> Facts {
    let root = workspace_root();
    let (rustup_found, active_toolchain) =
        match probe("rustup", &["show", "active-toolchain"], root.as_deref()) {
            Some((true, stdout)) => (true, doctor::first_token(&stdout)),
            Some((false, _)) => (true, None),
            None => (false, None),
        };
    let cargo_version = probe("cargo", &["--version"], root.as_deref())
        .filter(|(success, _)| *success)
        .and_then(|(_, stdout)| doctor::parse_cargo_version(&stdout));
    let git_found =
        probe("git", &["--version"], root.as_deref()).is_some_and(|(success, _)| success);
    let toolchain_file_channel = root
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path.join("rust-toolchain.toml")).ok())
        .and_then(|text| doctor::parse_toolchain_channel(&text));
    let required_cargo = root
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path.join("Cargo.toml")).ok())
        .and_then(|text| doctor::parse_rust_version(&text));

    Facts {
        rustup_found,
        active_toolchain,
        cargo_version,
        toolchain_file_channel,
        required_cargo,
        workspace_root_found: root.is_some(),
        git_found,
    }
}

/// Run a probe command, from the workspace root when one exists so toolchain
/// overrides resolve consistently with the build steps. `None` = could not
/// spawn; `Some((success, stdout))` otherwise.
fn probe(program: &str, args: &[&str], root: Option<&Path>) -> Option<(bool, String)> {
    let mut process = Process::new(program);
    process.args(args);
    if let Some(directory) = root {
        process.current_dir(directory);
    }
    process.output().ok().map(|output| {
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    })
}

fn workspace_root() -> Option<PathBuf> {
    env::current_dir()
        .ok()
        .and_then(|directory| workspace::find_root(&directory))
}

/// Write to stdout without panicking on a closed pipe (the machine-readable
/// path must never abort mid-stream).
fn emit_stdout(text: &str) {
    let _ = std::io::stdout().write_all(text.as_bytes());
}

fn emit_stdout_line(text: &str) {
    let _ = writeln!(std::io::stdout(), "{text}");
}

fn duration_ms(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One healthy engine crate, in the shape `cargo metadata
    /// --format-version 1 --no-deps` emits.
    const METADATA: &str = concat!(
        r#"{"workspace_root":"/w","packages":[{"name":"renew-diag","#,
        r#""manifest_path":"/w/crates/core/diag/Cargo.toml","dependencies":[],"#,
        r#""metadata":{"renew":{"purpose":"p","maturity":"bootstrap","core":true,"#,
        r#""extension_points":[],"simulation":false}}}]}"#,
    );

    #[test]
    fn output_that_is_not_json_is_named_as_unreadable() {
        // A `cargo` that answers successfully with something else must not
        // be read as an empty, therefore passing, workspace.
        let error = findings_from_metadata("cargo said something else")
            .expect_err("non-JSON output must be rejected");
        assert!(
            error.starts_with("unreadable cargo metadata output"),
            "{error}"
        );
    }

    #[test]
    fn json_that_is_not_metadata_is_rejected_too() {
        let error = findings_from_metadata(r#"{"packages":[]}"#)
            .expect_err("JSON without a workspace root is not metadata");
        assert!(error.contains("workspace_root"), "{error}");
    }

    #[test]
    fn well_shaped_metadata_runs_the_structure_rules() {
        let findings = findings_from_metadata(METADATA).expect("metadata is readable");
        assert_eq!(findings, Vec::new());
    }
}
