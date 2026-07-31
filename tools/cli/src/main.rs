//! The `renew` binary: a thin I/O shell over the `renew_cli` library.
//!
//! Exit-code contract: `0` success, `1` command failed or could not run,
//! `2` usage error. Child exit codes are reported raw in the JSON envelope's
//! `exit_code` field (signal deaths as `-1`); the process itself always
//! exits 0, 1, or 2.

use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};
use std::time::Instant;

use renew_cli::cli::{self, Command, Invocation, Parsed};
use renew_cli::coverage::{self, Outcome};
use renew_cli::doctor::{self, Facts};
use renew_cli::json::{self, Value};
use renew_cli::plan;
use renew_cli::samples;
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
        Ok(Parsed::Run(invocation)) => run(&invocation),
        Err(error) => {
            eprint!("error: {error}\n\n{}", cli::usage());
            ExitCode::from(2)
        }
    }
}

fn run(invocation: &Invocation) -> ExitCode {
    match invocation.command {
        Command::Doctor => run_doctor(invocation.json),
        Command::Check => run_check(invocation.json),
        // Parsing guarantees the path; an empty one would simply fail to
        // open, which is the same answer by a shorter road.
        Command::Coverage => run_coverage(
            invocation.report.as_deref().unwrap_or_default(),
            invocation.json,
        ),
        Command::Run => run_sample(invocation),
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
    let metadata = cargo_metadata(&root)?;
    let mut findings = findings_from_metadata(&metadata)?;
    // The lint-file rule is the one rule that has to look at the disk, so
    // it is applied here rather than inside the pure rule set, and it is
    // handed a predicate rather than reaching for the filesystem itself.
    let shapes = structure::shapes_from_metadata(&parsed_metadata(&metadata)?)?;
    findings.extend(structure::lint_file_findings(&shapes, &|path| {
        Path::new(path).exists()
    }));
    Ok(findings)
}

/// Ask cargo to describe this workspace. Split from its readers so the
/// spawn — and both ways it can fail — happen in one place, whether the
/// caller wants the structure rules or the list of samples.
fn cargo_metadata(root: &Path) -> Result<String, String> {
    let output = Process::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Read cargo's answer, naming output that is not metadata at all rather
/// than letting it pass for an empty workspace. Split from the spawning
/// half so that rejection is exercisable without a child process.
fn parsed_metadata(text: &str) -> Result<Value, String> {
    json::parse(text).map_err(|error| format!("unreadable cargo metadata output: {error}"))
}

/// Run the structure rules over `cargo metadata` output.
fn findings_from_metadata(text: &str) -> Result<Vec<structure::Finding>, String> {
    let shapes = structure::shapes_from_metadata(&parsed_metadata(text)?)?;
    Ok(structure::evaluate(&shapes))
}

/// Read the runnable samples out of `cargo metadata` output.
fn samples_from_metadata(text: &str) -> Result<Vec<samples::Sample>, String> {
    samples::from_metadata(&parsed_metadata(text)?)
}

fn run_coverage(report_path: &str, json_mode: bool) -> ExitCode {
    let started = Instant::now();
    match evaluate_coverage(report_path) {
        Ok(outcome) => {
            let ok = outcome.passes();
            if json_mode {
                emit_stdout_line(&coverage_envelope(&outcome, started).render());
            } else {
                emit_stdout(&coverage_report(&outcome));
            }
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(message) => {
            // A gate that cannot read its inputs has failed — an empty
            // uncovered set is never a pass.
            if json_mode {
                // Same shape as the success path: every coverage key is
                // present, so consumers never see a conditional one.
                let document = Value::Object(vec![
                    ("schema_version".to_string(), Value::Number(1)),
                    ("command".to_string(), Value::String("coverage".to_string())),
                    ("status".to_string(), Value::String("error".to_string())),
                    ("exit_code".to_string(), Value::Number(1)),
                    (
                        "duration_ms".to_string(),
                        Value::Number(duration_ms(started)),
                    ),
                    ("stdout".to_string(), Value::String(String::new())),
                    ("stderr".to_string(), Value::String(message)),
                    ("measured_files".to_string(), Value::Number(0)),
                    ("exempt_lines".to_string(), Value::Number(0)),
                    ("uncovered".to_string(), Value::Array(Vec::new())),
                    ("stale".to_string(), Value::Array(Vec::new())),
                ]);
                emit_stdout_line(&document.render());
                return ExitCode::FAILURE;
            }
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The same envelope prefix as every other subcommand, plus one array per
/// direction of the ratchet.
fn coverage_envelope(outcome: &Outcome, started: Instant) -> Value {
    let ok = outcome.passes();
    let uncovered: Vec<Value> = outcome.gaps.iter().map(site_fields).collect();
    let stale: Vec<Value> = outcome
        .stale
        .iter()
        .map(|stale| {
            let mut fields = site_pairs(&stale.site);
            fields.push((
                "state".to_string(),
                Value::String(stale.kind.label().to_string()),
            ));
            fields.push(("reason".to_string(), Value::String(stale.reason.clone())));
            Value::Object(fields)
        })
        .collect();
    Value::Object(vec![
        ("schema_version".to_string(), Value::Number(1)),
        ("command".to_string(), Value::String("coverage".to_string())),
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
        (
            "measured_files".to_string(),
            Value::Number(count(outcome.measured_files)),
        ),
        (
            "exempt_lines".to_string(),
            Value::Number(count(outcome.exempt_lines)),
        ),
        ("uncovered".to_string(), Value::Array(uncovered)),
        ("stale".to_string(), Value::Array(stale)),
    ])
}

/// One line per finding, in `check`'s shape, then a summary.
fn coverage_report(outcome: &Outcome) -> String {
    let mut report = String::new();
    for site in &outcome.gaps {
        let _ = writeln!(
            report,
            "FAIL {:<17} {site} is uncovered and has no exemption",
            "uncovered"
        );
    }
    for stale in &outcome.stale {
        let _ = writeln!(
            report,
            "FAIL {:<17} {} {}",
            "stale-exemption",
            stale.site,
            stale.kind.explanation()
        );
    }
    let summary = if outcome.passes() {
        format!(
            "coverage is complete: {} file(s) measured, {} exempt line(s), no gaps",
            outcome.measured_files, outcome.exempt_lines
        )
    } else {
        format!(
            "{} coverage finding(s) against {} exempt line(s)",
            outcome.findings(),
            outcome.exempt_lines
        )
    };
    let _ = writeln!(report, "{summary}");
    report
}

/// Read the manifest and the report, then hold them against each other.
/// Split out from the rendering half so every way the gate can fail to run
/// is nameable in one place.
fn evaluate_coverage(report_path: &str) -> Result<Outcome, String> {
    let root = workspace_root()
        .ok_or_else(|| "no workspace root found above the current directory".to_string())?;
    let manifest_path = root.join(coverage::MANIFEST);
    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let exemptions = coverage::parse_manifest(&manifest)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let text = std::fs::read_to_string(report_path)
        .map_err(|error| format!("failed to read {report_path}: {error}"))?;
    let document = json::parse(&text)
        .map_err(|error| format!("unreadable llvm-cov export {report_path}: {error}"))?;
    let measured = coverage::measure(&document, &root.to_string_lossy())?;
    Ok(coverage::compare(&measured, &exemptions))
}

fn site_pairs(site: &coverage::Site) -> Vec<(String, Value)> {
    vec![
        ("file".to_string(), Value::String(site.file.clone())),
        ("line".to_string(), Value::Number(i64::from(site.line))),
    ]
}

fn site_fields(site: &coverage::Site) -> Value {
    Value::Object(site_pairs(site))
}

/// The child processes of one invocation: the context every child shares,
/// plus the output `--json` accumulates across them.
///
/// One road for every subcommand that spawns something, so a command
/// built from the fixed table and a command built from the command line
/// reach their children the same way and fail in the same words.
struct Runner<'a> {
    /// The subcommand name the envelope reports.
    name: &'a str,
    /// Children run from the workspace root, never the caller's directory.
    root: PathBuf,
    json: bool,
    started: Instant,
    stdout_all: String,
    stderr_all: String,
}

impl<'a> Runner<'a> {
    /// Anchor a runner at the enclosing workspace. `Err` carries the exit
    /// code of an invocation that has no tree to run in — a refusal,
    /// because a command that never found its workspace has not passed.
    fn anchored(name: &'a str, json: bool) -> Result<Self, ExitCode> {
        let started = Instant::now();
        let Some(root) = workspace_root() else {
            return Err(report_error(
                name,
                json,
                started,
                "",
                "",
                "no workspace root found above the current directory",
            ));
        };
        Ok(Self {
            name,
            root,
            json,
            started,
            stdout_all: String::new(),
            stderr_all: String::new(),
        })
    }

    /// Run one child from the workspace root. `Ok(())` means it succeeded
    /// and the invocation may go on; `Err(code)` means the invocation is
    /// over, with its envelope already emitted in JSON mode.
    ///
    /// In plain mode the child inherits this process's stdout and stderr,
    /// so its output reaches the caller as it is written, in its own
    /// order — which is what lets a downstream reader grep a line the
    /// child printed. `--json` captures instead, because that mode
    /// promises exactly one document on stdout.
    fn execute<A: AsRef<OsStr>>(&mut self, program: &str, args: &[A]) -> Result<(), ExitCode> {
        let mut process = Process::new(program);
        process.args(args).current_dir(&self.root);
        if self.json {
            match process.output() {
                Ok(output) => {
                    self.stdout_all
                        .push_str(&String::from_utf8_lossy(&output.stdout));
                    self.stderr_all
                        .push_str(&String::from_utf8_lossy(&output.stderr));
                    if output.status.success() {
                        return Ok(());
                    }
                    // Raw child code in the envelope (-1 for signal
                    // deaths); the process exit stays within 0/1/2.
                    let code = output.status.code().unwrap_or(-1);
                    Err(finish_json(
                        self.name,
                        "failed",
                        code,
                        self.started,
                        &self.stdout_all,
                        &self.stderr_all,
                    ))
                }
                Err(error) => Err(self.fail(&format!("failed to run {program}: {error}"))),
            }
        } else {
            match process.status() {
                Ok(status) if status.success() => Ok(()),
                // The child streamed its own output; the contract maps any
                // child failure to exit 1.
                Ok(_) => Err(ExitCode::FAILURE),
                Err(error) => Err(self.fail(&format!("failed to run {program}: {error}"))),
            }
        }
    }

    /// The invocation is over without a child having finished — nothing
    /// ran, or nothing could.
    fn fail(&self, message: &str) -> ExitCode {
        report_error(
            self.name,
            self.json,
            self.started,
            &self.stdout_all,
            &self.stderr_all,
            message,
        )
    }

    /// Every child succeeded.
    fn finish(&self) -> ExitCode {
        if self.json {
            return finish_json(
                self.name,
                "ok",
                0,
                self.started,
                &self.stdout_all,
                &self.stderr_all,
            );
        }
        ExitCode::SUCCESS
    }
}

/// Say why an invocation could not run, the way the running mode says it.
/// `stdout`/`stderr` are whatever earlier children produced; `message`
/// joins the reported stderr, because in JSON mode the envelope is the
/// only place a reader can find it.
fn report_error(
    command: &str,
    json_mode: bool,
    started: Instant,
    stdout: &str,
    stderr: &str,
    message: &str,
) -> ExitCode {
    if json_mode {
        return finish_json(
            command,
            "error",
            1,
            started,
            stdout,
            &format!("{stderr}{message}\n"),
        );
    }
    eprintln!("error: {message}");
    ExitCode::FAILURE
}

fn run_steps(invocation: &Invocation) -> ExitCode {
    let mut runner = match Runner::anchored(invocation.command.name(), invocation.json) {
        Ok(runner) => runner,
        Err(exit) => return exit,
    };
    for step in plan::steps(invocation.command, invocation.smoke) {
        if let Err(exit) = runner.execute(step.program, step.args) {
            return exit;
        }
    }
    runner.finish()
}

/// Build and run one sample, then hand it the rest of the command line.
///
/// Which samples exist is discovered from the workspace on every
/// invocation rather than listed in this binary: a table here would be a
/// second place to edit whenever a sample is added or renamed, and the
/// copy nobody runs is the copy that goes stale.
fn run_sample(invocation: &Invocation) -> ExitCode {
    // Parsing guarantees the name; an empty one would simply match no
    // sample, which is the same answer by a shorter road.
    let requested = invocation.sample.as_deref().unwrap_or_default();
    let mut runner = match Runner::anchored(invocation.command.name(), invocation.json) {
        Ok(runner) => runner,
        Err(exit) => return exit,
    };
    let known = match cargo_metadata(&runner.root).and_then(|text| samples_from_metadata(&text)) {
        Ok(known) => known,
        // A list that cannot be read is not an empty list: refuse, rather
        // than tell the caller their sample does not exist.
        Err(message) => return runner.fail(&message),
    };
    let Some(sample) = samples::find(&known, requested) else {
        // A name matching nothing is an unreadable command line, and is
        // reported like every other one: stderr, usage text, exit 2, and
        // no envelope, because no run happened to report on.
        eprint!(
            "error: {}\n\n{}",
            samples::unknown(requested, &known),
            cli::usage()
        );
        return ExitCode::from(2);
    };
    let args = plan::sample_step(&sample.package, &sample.name, &invocation.sample_args);
    match runner.execute("cargo", &args) {
        Ok(()) => runner.finish(),
        Err(exit) => exit,
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

fn count(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
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
