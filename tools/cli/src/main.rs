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

use std::collections::BTreeMap;
use std::fs;

use renew_cli::cli::{self, Command, Invocation, Parsed};
use renew_cli::coverage::{self, Outcome};
use renew_cli::determinism;
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
            let document = json::result_envelope("help", "ok", 0, 0, &cli::usage(), "", Vec::new());
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
        Command::Modules => run_modules(invocation.json),
        // Parsing guarantees both paths for pack and the one for inspect.
        Command::AssetPack => run_asset_pack(
            invocation.from.as_deref().unwrap_or_default(),
            invocation.pack.as_deref().unwrap_or_default(),
            invocation.json,
        ),
        Command::AssetInspect => run_asset_inspect(
            invocation.pack.as_deref().unwrap_or_default(),
            invocation.verify,
            invocation.json,
        ),
        // Parsing guarantees both paths, as it does for the pack.
        Command::UiCompile => run_ui_compile(
            invocation.from.as_deref().unwrap_or_default(),
            invocation.out.as_deref().unwrap_or_default(),
            invocation.json,
        ),
        // Parsing guarantees the path; an empty one would simply fail to
        // open, which is the same answer by a shorter road.
        Command::Coverage => run_coverage(
            invocation.report.as_deref().unwrap_or_default(),
            invocation.json,
        ),
        Command::Run | Command::Record | Command::Replay => run_sample(invocation),
        // Dispatched explicitly, and the wildcard below is why it has to
        // be. A subcommand that falls through to `run_steps` runs an
        // empty step list and reports `ok` in exit code and envelope
        // alike — for a gate whose whole purpose is refusing to pass
        // vacuously, being forgotten here is the worst available bug and
        // the compiler cannot catch it.
        Command::Determinism => {
            if invocation.compare.is_empty() {
                run_determinism_emit(
                    invocation.emit.as_deref().unwrap_or_default(),
                    invocation.json,
                )
            } else {
                run_determinism_compare(&invocation.compare, invocation.json)
            }
        }
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

/// Every file under `root`, with the forward-slashed relative path each
/// will be named by in the pack.
///
/// Forward slashes on every platform, deliberately: a pack built on
/// Windows and one built on Linux from the same tree must be byte
/// identical, and a backslash in a name would make them differ. Order
/// does not matter here because the builder sorts, which is the point of
/// doing it there.
fn asset_files(
    root: &Path,
    prefix: &str,
    found: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(root)
        .map_err(|error| format!("cannot read {}: {error}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read {}: {error}", root.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        let joined = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        if path.is_dir() {
            asset_files(&path, &joined, found)?;
        } else {
            found.push((joined, path));
        }
    }
    Ok(())
}

/// `renew asset-pack` -- build a pack from a directory.
fn run_asset_pack(from: &str, pack_path: &str, json_mode: bool) -> ExitCode {
    let started = Instant::now();
    match build_pack(Path::new(from), Path::new(pack_path)) {
        Ok(count) => {
            if json_mode {
                let document = Value::Object(vec![
                    ("schema_version".to_string(), Value::Number(1)),
                    (
                        "command".to_string(),
                        Value::String("asset-pack".to_string()),
                    ),
                    ("status".to_string(), Value::String("ok".to_string())),
                    ("exit_code".to_string(), Value::Number(0)),
                    (
                        "duration_ms".to_string(),
                        Value::Number(duration_ms(started)),
                    ),
                    ("stdout".to_string(), Value::String(String::new())),
                    ("stderr".to_string(), Value::String(String::new())),
                    ("entries".to_string(), Value::Number(count)),
                    ("pack".to_string(), Value::String(pack_path.to_string())),
                ]);
                emit_stdout_line(&document.render());
            } else {
                emit_stdout(&format!("packed {count} entries into {pack_path}\n"));
            }
            ExitCode::SUCCESS
        }
        Err(message) => asset_failure("asset-pack", &message, json_mode, started),
    }
}

/// Read the directory, build the pack, write it. Split from the reporting
/// so the whole fallible part has one exit.
fn build_pack(from: &Path, pack_path: &Path) -> Result<i64, String> {
    if !from.is_dir() {
        return Err(format!("{} is not a directory", from.display()));
    }
    let mut found = Vec::new();
    asset_files(from, "", &mut found)?;

    let mut builder = renew_asset::PackBuilder::new();
    for (name, path) in &found {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        builder
            .insert(name, &bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let count = i64::try_from(builder.len()).unwrap_or(i64::MAX);
    let bytes = builder.finish().map_err(|error| error.to_string())?;
    std::fs::write(pack_path, &bytes)
        .map_err(|error| format!("cannot write {}: {error}", pack_path.display()))?;
    Ok(count)
}

/// `renew asset-inspect` -- list a pack, optionally verifying payloads.
fn run_asset_inspect(pack_path: &str, verify: bool, json_mode: bool) -> ExitCode {
    let started = Instant::now();
    let bytes = match std::fs::read(pack_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return asset_failure(
                "asset-inspect",
                &format!("cannot read {pack_path}: {error}"),
                json_mode,
                started,
            );
        }
    };
    let pack = match renew_asset::Pack::read(&bytes) {
        Ok(pack) => pack,
        Err(error) => {
            return asset_failure("asset-inspect", &error.to_string(), json_mode, started);
        }
    };
    // Computed before reporting either way, so the two output modes
    // cannot disagree about what was checked.
    let bad: Vec<String> = if verify {
        pack.mismatched()
            .iter()
            .map(|entry| entry.name.to_string())
            .collect()
    } else {
        Vec::new()
    };
    let ok = bad.is_empty();

    if json_mode {
        let items: Vec<Value> = pack
            .entries()
            .map(|entry| {
                Value::Object(vec![
                    ("name".to_string(), Value::String(entry.name.to_string())),
                    (
                        "hash".to_string(),
                        Value::String(format!("{:016x}", entry.hash)),
                    ),
                    (
                        "bytes".to_string(),
                        Value::Number(i64::try_from(entry.bytes.len()).unwrap_or(i64::MAX)),
                    ),
                ])
            })
            .collect();
        let document = Value::Object(vec![
            ("schema_version".to_string(), Value::Number(1)),
            (
                "command".to_string(),
                Value::String("asset-inspect".to_string()),
            ),
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
            ("verified".to_string(), Value::Bool(verify)),
            (
                "mismatched".to_string(),
                Value::Array(bad.iter().map(|n| Value::String(n.clone())).collect()),
            ),
            ("entries".to_string(), Value::Array(items)),
        ]);
        emit_stdout_line(&document.render());
    } else {
        let widest = pack.entries().map(|e| e.name.len()).max().unwrap_or(0);
        let mut report = String::new();
        for entry in pack.entries() {
            let _ = writeln!(
                report,
                "{:<width$}  {:>10}  {:016x}",
                entry.name,
                entry.bytes.len(),
                entry.hash,
                width = widest
            );
        }
        for name in &bad {
            let _ = writeln!(report, "MISMATCH {name}");
        }
        // The suffix reports the *outcome*, not the request. It used to
        // read `if verify { ", verified" }`, so a pack with a corrupt
        // payload printed `MISMATCH b.txt` and then `2 entries, verified`
        // two lines later. The exit code was right and the JSON was
        // right; only the line a person reads contradicted itself.
        let checked = match (verify, bad.len()) {
            (false, _) => String::new(),
            (true, 0) => ", verified".to_string(),
            (true, failed) => format!(", {failed} FAILED verification"),
        };
        let _ = writeln!(report, "{} entries{checked}", pack.len());
        emit_stdout(&report);
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `renew ui-compile` -- compile a text document into the binary blob.
fn run_ui_compile(from: &str, out_path: &str, json_mode: bool) -> ExitCode {
    let started = Instant::now();
    let source = match std::fs::read_to_string(from) {
        Ok(source) => source,
        Err(error) => {
            let message = format!("cannot read {from}: {error}");
            return ui_compile_failure(&message, None, json_mode, started);
        }
    };
    let compiled = match renew_cli::ui_compile::compile(&source) {
        Ok(compiled) => compiled,
        Err(diagnostic) => {
            // The human line leads with the file, the way compilers
            // do; the JSON carries the parts separately.
            let message = format!("{from}:{diagnostic}");
            return ui_compile_failure(&message, Some(&diagnostic), json_mode, started);
        }
    };
    // One direct write: a failure part-way (a full disk) can truncate
    // an existing blob at this path. Accepted for a build-time tool
    // whose output is regenerated by rerunning; temp-and-rename
    // arrives if a consumer ever loads blobs it also recompiles.
    if let Err(error) = std::fs::write(out_path, &compiled.bytes) {
        let message = format!("cannot write {out_path}: {error}");
        return ui_compile_failure(&message, None, json_mode, started);
    }
    let nodes = i64::from(compiled.nodes);
    let size = i64::try_from(compiled.bytes.len()).unwrap_or(i64::MAX);
    if json_mode {
        let document = Value::Object(vec![
            ("schema_version".to_string(), Value::Number(1)),
            (
                "command".to_string(),
                Value::String("ui-compile".to_string()),
            ),
            ("status".to_string(), Value::String("ok".to_string())),
            ("exit_code".to_string(), Value::Number(0)),
            (
                "duration_ms".to_string(),
                Value::Number(duration_ms(started)),
            ),
            ("stdout".to_string(), Value::String(String::new())),
            ("stderr".to_string(), Value::String(String::new())),
            ("errors".to_string(), Value::Array(Vec::new())),
            ("nodes".to_string(), Value::Number(nodes)),
            ("bytes".to_string(), Value::Number(size)),
            ("out".to_string(), Value::String(out_path.to_string())),
        ]);
        emit_stdout_line(&document.render());
    } else {
        emit_stdout(&format!(
            "compiled {nodes} nodes into {out_path} ({size} bytes)\n"
        ));
    }
    ExitCode::SUCCESS
}

/// The compile subcommand's error half: the message always, and the
/// diagnostic's place as structured fields when the refusal has one —
/// an unreadable file does not, a grammar refusal does.
fn ui_compile_failure(
    message: &str,
    diagnostic: Option<&renew_cli::ui_compile::Diagnostic>,
    json_mode: bool,
    started: Instant,
) -> ExitCode {
    if json_mode {
        let errors = diagnostic
            .map(|found| {
                vec![Value::Object(vec![
                    ("line".to_string(), Value::Number(i64::from(found.line))),
                    ("column".to_string(), Value::Number(i64::from(found.column))),
                    ("message".to_string(), Value::String(found.message.clone())),
                ])]
            })
            .unwrap_or_default();
        let document = Value::Object(vec![
            ("schema_version".to_string(), Value::Number(1)),
            (
                "command".to_string(),
                Value::String("ui-compile".to_string()),
            ),
            ("status".to_string(), Value::String("error".to_string())),
            ("exit_code".to_string(), Value::Number(1)),
            (
                "duration_ms".to_string(),
                Value::Number(duration_ms(started)),
            ),
            ("stdout".to_string(), Value::String(String::new())),
            ("stderr".to_string(), Value::String(message.to_string())),
            ("errors".to_string(), Value::Array(errors)),
        ]);
        emit_stdout_line(&document.render());
    } else {
        eprintln!("error: {message}");
    }
    ExitCode::FAILURE
}

/// One refusal shape for both asset subcommands.
fn asset_failure(command: &str, message: &str, json_mode: bool, started: Instant) -> ExitCode {
    if json_mode {
        let document = Value::Object(vec![
            ("schema_version".to_string(), Value::Number(1)),
            ("command".to_string(), Value::String(command.to_string())),
            ("status".to_string(), Value::String("error".to_string())),
            ("exit_code".to_string(), Value::Number(1)),
            (
                "duration_ms".to_string(),
                Value::Number(duration_ms(started)),
            ),
            ("stdout".to_string(), Value::String(String::new())),
            ("stderr".to_string(), Value::String(message.to_string())),
            ("entries".to_string(), Value::Array(Vec::new())),
        ]);
        emit_stdout_line(&document.render());
    } else {
        eprintln!("error: {message}");
    }
    ExitCode::FAILURE
}

/// Read the runnable samples out of `cargo metadata` output.
fn samples_from_metadata(text: &str) -> Result<Vec<samples::Sample>, String> {
    samples::from_metadata(&parsed_metadata(text)?)
}

/// One row of the module table, already reduced to what is printed.
///
/// A crate whose metadata does not parse still gets a row, carrying the
/// reason instead of its fields. Dropping it would make an inventory
/// quietly shorter than the workspace, and an inventory that omits what
/// it could not read is the kind that gets believed.
struct ModuleRow {
    name: String,
    maturity: String,
    core: Option<bool>,
    problem: Option<String>,
}

/// Every workspace crate with its declared maturity, from the manifests.
///
/// Sorted by maturity then name rather than alphabetically: the question
/// this answers is "what does this release promise", and that is read off
/// the maturity groups, not the alphabet.
fn module_rows(text: &str) -> Result<Vec<ModuleRow>, String> {
    let shapes = structure::shapes_from_metadata(&parsed_metadata(text)?)?;
    let mut rows: Vec<ModuleRow> = shapes
        .iter()
        .map(|shape| match &shape.meta {
            Ok(meta) => ModuleRow {
                name: shape.name.clone(),
                maturity: meta.maturity.clone(),
                core: Some(meta.core),
                problem: None,
            },
            Err(problems) => ModuleRow {
                name: shape.name.clone(),
                maturity: "unreadable".to_string(),
                core: None,
                problem: Some(problems.join("; ")),
            },
        })
        .collect();
    let rank = |maturity: &str| match maturity {
        "stable" => 0,
        "internal" => 1,
        "bootstrap" => 2,
        _ => 3,
    };
    rows.sort_by(|a, b| {
        rank(&a.maturity)
            .cmp(&rank(&b.maturity))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(rows)
}

/// `renew modules` — the module inventory, for people and for releases.
///
/// This exists because the maturity of a module is declared in exactly one
/// place, its manifest, and until now nothing could read that place out
/// loud. Anyone needing the table — a release note stating what a version
/// promises, a document listing the optional crates — had to retype it,
/// and a retyped table is a second home for a fact that goes stale without
/// telling anyone.
///
/// It reports rather than gates: no finding, no failure, exit zero unless
/// the workspace cannot be read at all.
fn run_modules(json_mode: bool) -> ExitCode {
    let started = Instant::now();
    let outcome = workspace_root()
        .ok_or_else(|| "no workspace root found above the current directory".to_string())
        .and_then(|root| cargo_metadata(&root))
        .and_then(|text| module_rows(&text));

    let rows = match outcome {
        Ok(rows) => rows,
        Err(message) => {
            if json_mode {
                let document = Value::Object(vec![
                    ("schema_version".to_string(), Value::Number(1)),
                    ("command".to_string(), Value::String("modules".to_string())),
                    ("status".to_string(), Value::String("error".to_string())),
                    ("exit_code".to_string(), Value::Number(1)),
                    (
                        "duration_ms".to_string(),
                        Value::Number(duration_ms(started)),
                    ),
                    ("stdout".to_string(), Value::String(String::new())),
                    ("stderr".to_string(), Value::String(message)),
                    // Always present, never conditional: a consumer that
                    // reads `modules` must not have to test for the key.
                    ("modules".to_string(), Value::Array(Vec::new())),
                ]);
                emit_stdout_line(&document.render());
            } else {
                eprintln!("error: {message}");
            }
            return ExitCode::FAILURE;
        }
    };

    if json_mode {
        let items: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut fields = vec![
                    ("name".to_string(), Value::String(row.name.clone())),
                    ("maturity".to_string(), Value::String(row.maturity.clone())),
                ];
                fields.push((
                    "core".to_string(),
                    row.core.map_or(Value::Null, Value::Bool),
                ));
                fields.push((
                    "problem".to_string(),
                    row.problem
                        .as_ref()
                        .map_or(Value::Null, |text| Value::String(text.clone())),
                ));
                Value::Object(fields)
            })
            .collect();
        let document = Value::Object(vec![
            ("schema_version".to_string(), Value::Number(1)),
            ("command".to_string(), Value::String("modules".to_string())),
            ("status".to_string(), Value::String("ok".to_string())),
            ("exit_code".to_string(), Value::Number(0)),
            (
                "duration_ms".to_string(),
                Value::Number(duration_ms(started)),
            ),
            ("stdout".to_string(), Value::String(String::new())),
            ("stderr".to_string(), Value::String(String::new())),
            ("modules".to_string(), Value::Array(items)),
        ]);
        emit_stdout_line(&document.render());
        return ExitCode::SUCCESS;
    }

    let widest = rows.iter().map(|row| row.name.len()).max().unwrap_or(0);
    let mut report = String::new();
    for row in &rows {
        let core = match row.core {
            Some(true) => "core",
            Some(false) => "optional",
            None => "?",
        };
        let _ = writeln!(
            report,
            "{:<width$}  {:<10}  {core}",
            row.name,
            row.maturity,
            width = widest
        );
        if let Some(problem) = &row.problem {
            let _ = writeln!(report, "{:<width$}  {problem}", "", width = widest);
        }
    }
    let stable = rows.iter().filter(|row| row.maturity == "stable").count();
    // The count is the point, not decoration: a version's compatibility
    // promise covers `stable` modules, so a reader needs to see at a
    // glance how much of the tree that is.
    let _ = writeln!(report, "{} module(s), {stable} stable", rows.len());
    emit_stdout(&report);
    ExitCode::SUCCESS
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

/// The unexempted gaps in one file, rendered as the parenthetical a
/// drifted exemption needs.
///
/// Line numbers move when the text above them moves. The gate then reports
/// the old line as covered — which is true, and reads as "delete this",
/// which is wrong when the code has merely moved down. Both halves of that
/// fact are already computed on every run; only the correlation was
/// missing, so a developer had to delete the entry, watch the next run
/// fail on the new line, and re-add it there.
///
/// The wording names the adjacent gaps and stops. A file can hold an
/// unrelated new gap beside a genuinely dead exemption, so this is a fact
/// offered to a reader, never an inference about what happened.
///
/// Capped: a collection that failed wholesale would otherwise print
/// hundreds of line numbers into a single finding.
fn drift_hint(outcome: &Outcome, file: &str) -> Option<String> {
    const SHOWN: usize = 6;
    if let Some(moved) = relocation_hint(outcome, file) {
        return Some(moved);
    }
    let lines: Vec<u32> = outcome
        .gaps
        .iter()
        .filter(|site| site.file == file)
        .map(|site| site.line)
        .collect();
    let (shown, rest) = lines.split_at(lines.len().min(SHOWN));
    let listed = shown
        .iter()
        .map(u32::to_string)
        .collect::<Vec<String>>()
        .join(", ");
    match (shown.len(), rest.len()) {
        (0, _) => None,
        (_, 0) => Some(format!(
            " (unless the code moved: this file is uncovered and unexempted at {listed})"
        )),
        (_, more) => Some(format!(
            " (unless the code moved: this file is uncovered and unexempted at {listed}, and {more} more)"
        )),
    }
}

/// The same fact, sharpened when the whole block moved together.
///
/// A file whose stale pins are each the *same distance* from an
/// unexempted gap is a file where text was inserted above them. That has
/// happened four times running in one stack — once from a change that
/// added nothing but documentation — and each time the fix was to work
/// out the offset by hand, verify a few lines by content, and re-pin.
/// Every input to that arithmetic is already on the screen; only the
/// subtraction was missing.
///
/// **Offered as an observation, never as an inference.** A uniform offset
/// is strong evidence that a block slid and no evidence at all that the
/// code inside it still means what the exemption says — so this prints
/// the corrected numbers and says to check them, rather than implying
/// the entry is fine where it lands. Anchoring on content is still the
/// step that decides; this only removes the counting.
///
/// Silent unless the correspondence is exact and one-to-one: any stale
/// pin without a gap at the same offset, any offset of zero, or fewer
/// than two pins to agree with each other, and the ordinary listing is
/// the honest answer.
fn relocation_hint(outcome: &Outcome, file: &str) -> Option<String> {
    let stale: Vec<u32> = outcome
        .stale
        .iter()
        .filter(|entry| entry.site.file == file && entry.kind == coverage::StaleKind::NowCovered)
        .map(|entry| entry.site.line)
        .collect();
    let gaps: Vec<u32> = outcome
        .gaps
        .iter()
        .filter(|site| site.file == file)
        .map(|site| site.line)
        .collect();
    // Two pins are the fewest that can agree on an offset. One pin and
    // one gap always "agree", which would make this fire on every
    // ordinary single-line change.
    if stale.len() < 2 || gaps.len() != stale.len() {
        return None;
    }
    let first = *stale.first()?;
    let offset = i64::from(*gaps.first()?) - i64::from(first);
    let moved: Vec<u32> = stale
        .iter()
        .map(|line| i64::from(*line).saturating_add(offset))
        .map(|line| u32::try_from(line).unwrap_or(0))
        .collect();
    // A zero offset is folded into the same guard rather than refused
    // above it. It cannot happen — a line the report calls covered and a
    // line it calls uncovered are never the same line — and an arm no
    // input can reach is an arm no test can check, so it is written where
    // the condition that already subsumes it lives.
    if offset == 0 || moved != gaps {
        return None;
    }
    let listed = moved
        .iter()
        .map(u32::to_string)
        .collect::<Vec<String>>()
        .join(", ");
    let direction = if offset > 0 { "down" } else { "up" };
    let distance = offset.abs();
    Some(format!(
        " (every exemption in this file is {distance} lines above an unexempted gap, so the block \
         moved {direction}: lines = [{listed}] — check the content at those lines before pinning \
         them, because a block that moved and a block that changed look the same from here)"
    ))
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
        // Only the covered-now direction can be a drift: `FileAbsent` means
        // the report does not measure the file at all, so it has no gaps to
        // name and a hint would be incoherent.
        let hint = match stale.kind {
            coverage::StaleKind::NowCovered => drift_hint(outcome, &stale.site.file),
            coverage::StaleKind::FileAbsent => None,
        };
        let _ = writeln!(
            report,
            "FAIL {:<17} {} {}{}",
            "stale-exemption",
            stale.site,
            stale.kind.explanation(),
            hint.as_deref().unwrap_or_default()
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
        self.finish_with(Vec::new())
    }

    /// Every child succeeded, with subcommand-specific fields folded into
    /// the envelope after the shared ones. Ignored outside `--json`,
    /// where the child's own output already reached the caller.
    fn finish_with(&self, extra: Vec<(String, Value)>) -> ExitCode {
        if self.json {
            return finish_json_with(
                self.name,
                "ok",
                0,
                self.started,
                &self.stdout_all,
                &self.stderr_all,
                extra,
            );
        }
        ExitCode::SUCCESS
    }

    /// What the children wrote to stdout, as captured. Empty outside
    /// `--json`, where children inherit this process's stdout and nothing
    /// passes through here.
    fn captured_stdout(&self) -> &str {
        &self.stdout_all
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

/// The simulations the cross-platform lane compares, and the exact
/// arguments that pin them.
///
/// Each entry names a run whose every output is a function of the flags
/// beside it. Widening this list widens what the claim covers; it is a
/// list rather than one run because a single configuration
/// exercises one path through the world, and a divergence in a path the
/// list never walks is a divergence the lane never sees.
/// One simulation the lane pins: what to call it, which package answers,
/// what to pass, and which fields of the answer carry a digest.
type PinnedRun = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

/// Glide reports two hashes; every run added since reports one.
const GLIDE_FIELDS: &[&str] = &["schedule_hash", "state_hash"];
const ONE_DIGEST: &[&str] = &["digest"];

const PINNED_RUNS: [PinnedRun; 11] = [
    // The widget tree: a scripted menu session through the fixed-point
    // solver, the hit-tester, and the decision fold. Everything in it
    // is integer arithmetic, and this row is what turns that claim
    // into three targets agreeing rather than one target asserting.
    ("ui/menu-16", "renew-ui", &[], ONE_DIGEST),
    // Four lockstep peers played against a scripted hostile link: loss,
    // duplication, reordering, one-way blackouts and a silent peer. The
    // digest folds the confirmed input stream and nothing else, so what
    // three targets are being asked to agree about is that arrival was
    // unobservable — not that their networks behaved the same, which
    // they did not even within one process.
    ("net/lockstep-4x600", "renew-net", &[], ONE_DIGEST),
    (
        "glide/seed-7-600",
        "renew-sample-glide",
        &["--seed", "7", "--frames", "600", "--json"],
        GLIDE_FIELDS,
    ),
    (
        "glide/seed-7-2000",
        "renew-sample-glide",
        &["--seed", "7", "--frames", "2000", "--json"],
        GLIDE_FIELDS,
    ),
    (
        "glide/seed-99-600",
        "renew-sample-glide",
        &["--seed", "99", "--frames", "600", "--json"],
        GLIDE_FIELDS,
    ),
    (
        "glide/sink-1500",
        "renew-sample-glide",
        &[
            "--seed",
            "3",
            "--frames",
            "1500",
            "--input-trace",
            "sink",
            "--json",
        ],
        GLIDE_FIELDS,
    ),
    // The platformer: swept motion against geometry, where a divergence would
    // come from the collision arithmetic rather than from a generator.
    (
        "leap/dash-600",
        "renew-sample-leap",
        &["--script", "dash", "--ticks", "600", "--json"],
        ONE_DIGEST,
    ),
    (
        "leap/hop-900",
        "renew-sample-leap",
        &["--script", "hop", "--ticks", "900", "--json"],
        ONE_DIGEST,
    ),
    // The voxel world: the same arithmetic in three dimensions, plus terrain
    // that the run itself edits.
    (
        "cube/patrol-600",
        "renew-sample-cube",
        &["--script", "patrol", "--ticks", "600", "--json"],
        ONE_DIGEST,
    ),
    (
        "cube/build-900",
        "renew-sample-cube",
        &["--script", "build", "--ticks", "900", "--json"],
        ONE_DIGEST,
    ),
    // Chess: no floating point and no geometry at all, so a divergence here
    // would be in the integer state itself rather than in any arithmetic the
    // other three share. A different kind of witness for the same claim.
    (
        "chess/play-60",
        "renew-sample-chess",
        &["--play", "--depth", "60", "--json"],
        ONE_DIGEST,
    ),
];

/// Run the pinned simulations and write this target's report.
///
/// Each run contributes whichever digests its own report carries — two for
/// the glide sample, which computes a frame-schedule hash beside its world
/// hash, and one apiece for every run added since. A lane that assumed one
/// shape would tell the others their report was missing a field, which is
/// true and useless.
fn run_determinism_emit(output_path: &str, json_mode: bool) -> ExitCode {
    let runner = match Runner::anchored("determinism", json_mode) {
        Ok(runner) => runner,
        Err(exit) => return exit,
    };

    // The toolchain that built the binaries being compared. Read from
    // the compiler rather than from a file, because a file records an
    // intention and this has to record what actually ran.
    let toolchain = match probe("rustc", &["--version"], Some(&runner.root)) {
        Ok((true, text)) => text.trim().to_string(),
        Ok((false, _)) | Err(_) => {
            return runner.fail(
                "could not read `rustc --version`, and a comparison that cannot name its \
                 compiler is inconclusive rather than passing",
            );
        }
    };

    let mut digests = BTreeMap::new();
    for (name, package, args, fields) in PINNED_RUNS {
        let mut invocation = vec!["run", "--quiet", "--package", package, "--"];
        invocation.extend_from_slice(args);
        let (ok, stdout) = match probe("cargo", &invocation, Some(&runner.root)) {
            Ok(result) => result,
            Err(error) => return runner.fail(&format!("could not start `{name}`: {error}")),
        };
        if !ok {
            return runner.fail(&format!(
                "the pinned run `{name}` failed, so this target contributes nothing and \
                 the comparison must not be told otherwise"
            ));
        }
        match determinism::digests_from_output(name, &stdout, fields) {
            Ok(pairs) => digests.extend(pairs),
            Err(message) => return runner.fail(&message),
        }
    }

    let leg = determinism::Leg {
        origin: output_path.to_string(),
        os: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
        toolchain,
        digests,
    };
    let rendered = determinism::render_leg(&leg);
    if let Err(error) = fs::write(output_path, &rendered) {
        return runner.fail(&format!("could not write `{output_path}`: {error}"));
    }
    println!(
        "wrote {} digests for {}/{} to {output_path}",
        leg.digests.len(),
        leg.os,
        leg.arch
    );
    runner.finish()
}

/// Hold several targets' reports against each other.
fn run_determinism_compare(paths: &[String], json_mode: bool) -> ExitCode {
    let runner = match Runner::anchored("determinism", json_mode) {
        Ok(runner) => runner,
        Err(exit) => return exit,
    };

    let arches = determinism::expected_arches();

    let mut legs = Vec::new();
    for path in paths {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            // A missing artifact is a leg that did not report, which is
            // an untested target — never an absent objection.
            Err(error) => return runner.fail(&format!("could not read `{path}`: {error}")),
        };
        match determinism::parse_leg(path, &text) {
            Ok(leg) => legs.push(leg),
            Err(message) => return runner.fail(&message),
        }
    }

    let verdict = determinism::compare(&legs, &arches);
    let report = determinism::describe(&verdict);
    if verdict.is_pass() {
        println!("{report}");
        runner.finish()
    } else {
        runner.fail(&report)
    }
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
    // Parsing guarantees the path whenever the subcommand carries a
    // flag, so the pair is either wholly present or wholly absent.
    let lead = invocation
        .command
        .trace_flags()
        .zip(invocation.trace.as_deref())
        .map(|((_, child_flag), path)| (child_flag, path));
    let args = plan::sample_step(
        &sample.package,
        &sample.name,
        lead,
        &invocation.sample_args,
        &invocation.features,
    );
    match runner.execute("cargo", &args) {
        // A replay's whole result is the digest the child printed, and in
        // JSON mode this process captured the child's stdout rather than
        // letting it through — so without lifting the line into the
        // envelope the caller would have to parse it back out of a string
        // field, or in plain mode would simply have seen it already.
        Ok(()) if invocation.command == Command::Replay => {
            runner.finish_with(vec![(
                "digest".to_string(),
                match samples::digest_line(runner.captured_stdout()) {
                    Some(line) => Value::String(line.to_string()),
                    // Null rather than an empty string: a replay that
                    // printed no digest did not produce one, which is a
                    // different fact from producing an empty one.
                    None => Value::Null,
                },
            )])
        }
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
    finish_json_with(
        command,
        status,
        child_code,
        started,
        stdout,
        stderr,
        Vec::new(),
    )
}

/// The same envelope with subcommand-specific fields appended, so a
/// reader finds the shared keys in the same order whatever ran.
fn finish_json_with(
    command: &str,
    status: &str,
    child_code: i32,
    started: Instant,
    stdout: &str,
    stderr: &str,
    extra: Vec<(String, Value)>,
) -> ExitCode {
    let document = json::result_envelope(
        command,
        status,
        i64::from(child_code),
        duration_ms(started),
        stdout,
        stderr,
        extra,
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
    let (rustup_found, active_toolchain, rustup_unavailable) =
        match probe("rustup", &["show", "active-toolchain"], root.as_deref()) {
            Ok((true, stdout)) => (true, doctor::first_token(&stdout), None),
            Ok((false, _)) => (true, None, None),
            Err(error) => (false, None, Some(doctor::probe_failure(error.kind()))),
        };
    let cargo_version = probe("cargo", &["--version"], root.as_deref())
        .ok()
        .filter(|(success, _)| *success)
        .and_then(|(_, stdout)| doctor::parse_cargo_version(&stdout));
    let (git_found, git_unavailable) = match probe("git", &["--version"], root.as_deref()) {
        Ok((success, _)) => (success, None),
        Err(error) => (false, Some(doctor::probe_failure(error.kind()))),
    };
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
        rustup_unavailable,
        active_toolchain,
        cargo_version,
        toolchain_file_channel,
        required_cargo,
        workspace_root_found: root.is_some(),
        git_found,
        git_unavailable,
    }
}

/// Run a probe command, from the workspace root when one exists so toolchain
/// overrides resolve consistently with the build steps.
///
/// The spawn error is returned rather than discarded: "could not run" and
/// "is not installed" are different answers, and a report that cannot tell
/// them apart sends its reader to fix the wrong thing.
fn probe(
    program: &str,
    args: &[&str],
    root: Option<&Path>,
) -> Result<(bool, String), std::io::Error> {
    let mut process = Process::new(program);
    process.args(args);
    if let Some(directory) = root {
        process.current_dir(directory);
    }
    process.output().map(|output| {
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
