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
        // Every arm is named, and the last one is named rather than a
        // wildcard on purpose. A subcommand that fell through to
        // `run_steps` would run an empty step list and report `ok` in
        // exit code and envelope alike — for a gate whose whole purpose
        // is refusing to pass vacuously, being forgotten here is the
        // worst available bug. Spelled out, the compiler catches it: a
        // new variant fails to compile here exactly as it does in
        // `plan::steps`.
        Command::Determinism => {
            if invocation.compare.is_empty() {
                run_determinism_emit(
                    invocation.emit.as_deref().unwrap_or_default(),
                    invocation.target.as_deref(),
                    invocation.json,
                )
            } else {
                run_determinism_compare(&invocation.compare, invocation.json)
            }
        }
        Command::Configure | Command::Build | Command::Test | Command::Bench | Command::Lint => {
            run_steps(invocation)
        }
    }
}

/// The one wording for the tree-less refusal, shared by every subcommand
/// that anchors: since a standalone `[package]` manifest anchors as its
/// own workspace-of-one root, this says the walk ran and found no
/// manifest of either shape.
///
/// It is deliberately *not* the sentence for the two neighbouring
/// failures, which are different claims and carry their own words: a
/// manifest that is there and could not be read or named, and a working
/// directory the process could not read at all. Folding any of them into
/// this one would report a search that never happened.
const ROOTLESS_MESSAGE: &str =
    "no Cargo.toml declaring a workspace or a package was found above the current directory";

fn run_check(json_mode: bool) -> ExitCode {
    let started = Instant::now();
    let root = match workspace_root() {
        Ok(root) => root,
        Err(why) => {
            return check_error(
                json_mode,
                started,
                &why,
                None,
                vec![failure_entry("classification-failed", &why)],
            );
        }
    };
    if let Err((refusal, kind)) =
        require_engine_workspace(&root, "check runs the engine's own structure rules")
    {
        let entry = failure_entry(refusal.code, &refusal.message);
        let target = kind.map(|kind| target_field(kind, &root));
        return check_error(json_mode, started, &refusal.message, target, vec![entry]);
    }
    let outcome = gather_findings(&root);
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
                let mut fields = envelope_base(
                    "check",
                    if ok { "ok" } else { "failed" },
                    i64::from(!ok),
                    started,
                    "",
                );
                fields.push(("findings".to_string(), Value::Array(items)));
                // The guard above proved the tree is the engine's, on the
                // same root this run then checked — never re-derived.
                fields.push(target_field(workspace::TargetKind::EngineWorkspace, &root));
                fields.push(("failures".to_string(), Value::Array(Vec::new())));
                emit_stdout_line(&Value::Object(fields).render());
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
            // The failure here is the check's own machinery (metadata,
            // rule evaluation), past classification, so no refusal code
            // applies — but the invocation ended before a verdict, which
            // is what `aborted` names everywhere else, so it says so
            // here too rather than leaving a consumer that dispatches on
            // the code with nothing. The envelope keeps the
            // classification the guard proved.
            check_error(
                json_mode,
                started,
                &message,
                Some(target_field(workspace::TargetKind::EngineWorkspace, &root)),
                vec![failure_entry("aborted", &message)],
            )
        }
    }
}

/// The modules error envelope: same shape as the success path — `modules`
/// and `failures` always present, never conditional — with the reason in
/// `stderr`, any refusal class in `failures`, and whatever classification
/// the caller had established in `target`.
fn modules_error(
    json_mode: bool,
    started: Instant,
    message: &str,
    target: Option<(String, Value)>,
    failures: Vec<Value>,
) -> ExitCode {
    if json_mode {
        let mut fields = envelope_base("modules", "error", 1, started, &format!("{message}\n"));
        fields.push(("modules".to_string(), Value::Array(Vec::new())));
        fields.extend(target);
        fields.push(("failures".to_string(), Value::Array(failures)));
        emit_stdout_line(&Value::Object(fields).render());
        return ExitCode::FAILURE;
    }
    eprintln!("error: {message}");
    ExitCode::FAILURE
}

/// The check's error envelope: same shape as the success path — `findings`
/// and `failures` always present, never conditional — with the reason in
/// `stderr`, any refusal class in `failures`, and whatever classification
/// the caller had established in `target`.
fn check_error(
    json_mode: bool,
    started: Instant,
    message: &str,
    target: Option<(String, Value)>,
    failures: Vec<Value>,
) -> ExitCode {
    if json_mode {
        let mut fields = envelope_base("check", "error", 1, started, &format!("{message}\n"));
        fields.push(("findings".to_string(), Value::Array(Vec::new())));
        fields.extend(target);
        fields.push(("failures".to_string(), Value::Array(failures)));
        emit_stdout_line(&Value::Object(fields).render());
        return ExitCode::FAILURE;
    }
    eprintln!("error: {message}");
    ExitCode::FAILURE
}

fn gather_findings(root: &Path) -> Result<Vec<structure::Finding>, String> {
    let metadata = cargo_metadata(root)?;
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
                let mut fields = envelope_base("asset-pack", "ok", 0, started, "");
                fields.push(("entries".to_string(), Value::Number(count)));
                fields.push(("pack".to_string(), Value::String(pack_path.to_string())));
                emit_stdout_line(&Value::Object(fields).render());
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
        let mut fields = envelope_base(
            "asset-inspect",
            if ok { "ok" } else { "failed" },
            i64::from(!ok),
            started,
            "",
        );
        fields.push(("verified".to_string(), Value::Bool(verify)));
        fields.push((
            "mismatched".to_string(),
            Value::Array(bad.iter().map(|n| Value::String(n.clone())).collect()),
        ));
        fields.push(("entries".to_string(), Value::Array(items)));
        emit_stdout_line(&Value::Object(fields).render());
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
        let mut fields = envelope_base("ui-compile", "ok", 0, started, "");
        fields.push(("errors".to_string(), Value::Array(Vec::new())));
        fields.push(("nodes".to_string(), Value::Number(nodes)));
        fields.push(("bytes".to_string(), Value::Number(size)));
        fields.push(("out".to_string(), Value::String(out_path.to_string())));
        let document = Value::Object(fields);
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
        let mut fields = envelope_base("ui-compile", "error", 1, started, message);
        fields.push(("errors".to_string(), Value::Array(errors)));
        emit_stdout_line(&Value::Object(fields).render());
    } else {
        eprintln!("error: {message}");
    }
    ExitCode::FAILURE
}

/// One refusal shape for both asset subcommands.
fn asset_failure(command: &str, message: &str, json_mode: bool, started: Instant) -> ExitCode {
    if json_mode {
        let mut fields = envelope_base(command, "error", 1, started, message);
        fields.push(("entries".to_string(), Value::Array(Vec::new())));
        emit_stdout_line(&Value::Object(fields).render());
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
    let root = match workspace_root() {
        Ok(root) => root,
        Err(why) => {
            let entry = failure_entry("classification-failed", &why);
            return modules_error(json_mode, started, &why, None, vec![entry]);
        }
    };
    if let Err((refusal, kind)) =
        require_engine_workspace(&root, "modules reads the engine's own manifests")
    {
        let entry = failure_entry(refusal.code, &refusal.message);
        let target = kind.map(|kind| target_field(kind, &root));
        return modules_error(json_mode, started, &refusal.message, target, vec![entry]);
    }
    let rows = match cargo_metadata(&root).and_then(|text| module_rows(&text)) {
        Ok(rows) => rows,
        // Past classification: the failure is the listing's own machinery,
        // so no refusal code applies and the reason is prose — but the
        // envelope keeps the classification the guard proved.
        Err(message) => {
            return modules_error(
                json_mode,
                started,
                &message,
                Some(target_field(workspace::TargetKind::EngineWorkspace, &root)),
                vec![failure_entry("aborted", &message)],
            );
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
        let mut fields = envelope_base("modules", "ok", 0, started, "");
        fields.push(("modules".to_string(), Value::Array(items)));
        // The guard above proved the tree is the engine's, on the same
        // root this run then listed — never re-derived.
        fields.push(target_field(workspace::TargetKind::EngineWorkspace, &root));
        fields.push(("failures".to_string(), Value::Array(Vec::new())));
        emit_stdout_line(&Value::Object(fields).render());
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
    // The ratchet reads this repository's exemption manifest; holding a
    // game's report against the engine's exemptions would be a verdict
    // from the wrong ledger.
    let root = match workspace_root() {
        Ok(root) => root,
        Err(why) => {
            let entry = failure_entry("classification-failed", &why);
            return coverage_error(json_mode, started, &why, None, vec![entry]);
        }
    };
    if let Err((refusal, kind)) =
        require_engine_workspace(&root, "coverage holds this repository's own ratchet")
    {
        let entry = failure_entry(refusal.code, &refusal.message);
        let target = kind.map(|kind| target_field(kind, &root));
        return coverage_error(json_mode, started, &refusal.message, target, vec![entry]);
    }
    match evaluate_coverage(report_path, &root) {
        Ok(outcome) => {
            let ok = outcome.passes();
            if json_mode {
                emit_stdout_line(&coverage_envelope(&outcome, started, &root).render());
            } else {
                emit_stdout(&coverage_report(&outcome));
            }
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        // A gate that cannot read its inputs has failed — an empty
        // uncovered set is never a pass. Past classification, the failure
        // is the gate's own machinery, so no refusal code applies — but
        // the envelope keeps the classification the guard proved.
        Err(message) => coverage_error(
            json_mode,
            started,
            &message,
            Some(target_field(workspace::TargetKind::EngineWorkspace, &root)),
            vec![failure_entry("aborted", &message)],
        ),
    }
}

/// The coverage error envelope: same shape as the success path — every
/// coverage key present, so consumers never see a conditional one — with
/// the reason in `stderr`, any refusal class in `failures`, and whatever
/// classification the caller had established in `target`.
fn coverage_error(
    json_mode: bool,
    started: Instant,
    message: &str,
    target: Option<(String, Value)>,
    failures: Vec<Value>,
) -> ExitCode {
    if json_mode {
        let mut fields = envelope_base("coverage", "error", 1, started, &format!("{message}\n"));
        fields.push(("measured_files".to_string(), Value::Number(0)));
        fields.push(("exempt_lines".to_string(), Value::Number(0)));
        fields.push(("uncovered".to_string(), Value::Array(Vec::new())));
        fields.push(("stale".to_string(), Value::Array(Vec::new())));
        fields.extend(target);
        fields.push(("failures".to_string(), Value::Array(failures)));
        emit_stdout_line(&Value::Object(fields).render());
        return ExitCode::FAILURE;
    }
    eprintln!("error: {message}");
    ExitCode::FAILURE
}

/// The same envelope prefix as every other subcommand, plus one array per
/// direction of the ratchet. `root` is the engine root the caller's guard
/// already proved, named in the envelope's `target`.
fn coverage_envelope(outcome: &Outcome, started: Instant, root: &Path) -> Value {
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
    let mut fields = envelope_base(
        "coverage",
        if ok { "ok" } else { "failed" },
        i64::from(!ok),
        started,
        "",
    );
    fields.push((
        "measured_files".to_string(),
        Value::Number(count(outcome.measured_files)),
    ));
    fields.push((
        "exempt_lines".to_string(),
        Value::Number(count(outcome.exempt_lines)),
    ));
    fields.push(("uncovered".to_string(), Value::Array(uncovered)));
    fields.push(("stale".to_string(), Value::Array(stale)));
    fields.push(target_field(workspace::TargetKind::EngineWorkspace, root));
    fields.push(("failures".to_string(), Value::Array(Vec::new())));
    Value::Object(fields)
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
    // When the whole block slid, every finding in the file says so in a
    // few words and the pasteable list is printed once at the end. The
    // first version put the list on every finding: forty-one copies of a
    // forty-one-element line, correct and unreadable, and it did that on
    // the very change that added it.
    if let Some((offset, direction, _)) = relocation(outcome, file) {
        return Some(format!(
            " (the whole block moved {direction} {offset} lines — corrected pins below)"
        ));
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
fn relocation(outcome: &Outcome, file: &str) -> Option<(i64, &'static str, String)> {
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
    Some((offset.abs(), direction, listed))
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
    // One note per file, after the findings: the value of this line is
    // that it can be pasted, so it has to be whole — and a whole list
    // repeated once per finding is what made the first version useless.
    let mut noted: Vec<&str> = Vec::new();
    for stale in &outcome.stale {
        let file = stale.site.file.as_str();
        if stale.kind != coverage::StaleKind::NowCovered || noted.contains(&file) {
            continue;
        }
        if let Some((offset, direction, listed)) = relocation(outcome, file) {
            noted.push(file);
            let _ = writeln!(
                report,
                "MOVED {:<16} {file}: every pin is {offset} lines above a gap, so the block moved \
                 {direction}. Corrected: lines = [{listed}]. Check the content at those lines \
                 first — a block that moved and a block that changed look the same from here.",
                "relocation"
            );
        }
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
///
/// `root` is the tree the caller's guard already proved — passed in
/// rather than walked for again, so the exemption manifest is read from
/// the same tree the envelope's `target` names rather than from one that
/// merely resolves the same way a second time.
fn evaluate_coverage(report_path: &str, root: &Path) -> Result<Outcome, String> {
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
    /// Fields every envelope this runner emits carries — the `target`
    /// classification, and the `coverage` statement where the subcommand
    /// has one. Set right after anchoring, so the failure envelopes say
    /// where they failed just as the success envelope says where it ran.
    extra_base: Vec<(String, Value)>,
}

impl<'a> Runner<'a> {
    /// Anchor a runner at the enclosing workspace. `Err` carries the exit
    /// code of an invocation that has no tree to run in — a refusal,
    /// because a command that never found its workspace has not passed.
    ///
    /// `established` holds fields that depend only on the parsed
    /// invocation — the coverage statement, where the subcommand has one.
    /// They are handed in rather than pushed after anchoring so that even
    /// the rootless refusal carries them: what a run *would have* covered
    /// does not depend on finding a tree to run it in.
    fn anchored(
        name: &'a str,
        json: bool,
        established: Vec<(String, Value)>,
    ) -> Result<Self, ExitCode> {
        let started = Instant::now();
        let root = match workspace_root() {
            Ok(root) => root,
            // The same condition check and modules code the same way: the
            // tool could not establish what tree it stands in — whether
            // because there is no manifest above the caller or because
            // one is there and cannot be read.
            Err(message) => {
                let message = message.as_str();
                if json {
                    let mut extra = established;
                    extra.push((
                        "failures".to_string(),
                        Value::Array(vec![failure_entry("classification-failed", message)]),
                    ));
                    return Err(finish_json_with(
                        name,
                        "error",
                        1,
                        started,
                        "",
                        &format!("{message}\n"),
                        extra,
                    ));
                }
                eprintln!("error: {message}");
                return Err(ExitCode::FAILURE);
            }
        };
        Ok(Self {
            name,
            root,
            json,
            started,
            stdout_all: String::new(),
            stderr_all: String::new(),
            extra_base: established,
        })
    }

    /// The tree refused this invocation before any child ran: an envelope
    /// whose `failures` entry names the class, or the plain-mode line.
    /// The reason lands in `stderr` as well as the failure's summary —
    /// [`Runner::fail`] does the same, so a consumer that displays
    /// `stderr` on error sees every refusal shaped alike.
    fn refuse(&self, code: &str, reason: &str) -> ExitCode {
        if self.json {
            let mut extra = self.extra_base.clone();
            extra.push((
                "failures".to_string(),
                Value::Array(vec![failure_entry(code, reason)]),
            ));
            return finish_json_with(
                self.name,
                "error",
                1,
                self.started,
                &self.stdout_all,
                &format!("{}{reason}\n", self.stderr_all),
                extra,
            );
        }
        eprintln!("error: {reason}");
        ExitCode::FAILURE
    }

    /// A verdict was delivered and it is red: status `failed`, with the
    /// finding's own code. Distinct from [`Runner::fail`] because a
    /// consumer that retries aborts must never be handed a real finding
    /// wearing an abort's code. `exit_code` is the failing child's raw
    /// code where a child delivered the verdict, and `1` where the
    /// verdict is this tool's own (a determinism comparison).
    fn deliver_failed(&self, code: &str, report: &str, exit_code: i32) -> ExitCode {
        if self.json {
            let mut extra = self.extra_base.clone();
            extra.push((
                "failures".to_string(),
                Value::Array(vec![failure_entry(code, report)]),
            ));
            return finish_json_with(
                self.name,
                "failed",
                exit_code,
                self.started,
                &self.stdout_all,
                &format!("{}{report}\n", self.stderr_all),
                extra,
            );
        }
        // Whatever child output was captured on the way here (the
        // determinism emit path probes rather than inheriting) reaches
        // the caller ahead of the summary — a red whose only prose is
        // the summary sentence leaves nothing to diagnose with, in
        // either mode.
        emit_stdout(&self.stdout_all);
        eprint!("{}", self.stderr_all);
        eprintln!("{report}");
        ExitCode::FAILURE
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
                    let mut extra = self.extra_base.clone();
                    extra.push((
                        "failures".to_string(),
                        Value::Array(vec![failure_entry(
                            "step-failed",
                            &format!("{} exited with code {code}", step_name(program, args)),
                        )]),
                    ));
                    Err(finish_json_with(
                        self.name,
                        "failed",
                        code,
                        self.started,
                        &self.stdout_all,
                        &self.stderr_all,
                        extra,
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
    /// ran, or nothing could. The envelope keeps whatever target and
    /// coverage were already established and carries the abort as a
    /// structured failure, so this path claims no less than the others.
    fn fail(&self, message: &str) -> ExitCode {
        if self.json {
            let mut extra = self.extra_base.clone();
            extra.push((
                "failures".to_string(),
                Value::Array(vec![failure_entry("aborted", message)]),
            ));
            return finish_json_with(
                self.name,
                "error",
                1,
                self.started,
                &self.stdout_all,
                &format!("{}{message}\n", self.stderr_all),
                extra,
            );
        }
        // Whatever a child said before this abort reaches the caller
        // ahead of the reason, as [`Runner::deliver_failed`] does: the
        // paths that capture a child's output capture it for both modes,
        // and a plain-mode reader is the one most likely to be
        // diagnosing by eye.
        emit_stdout(&self.stdout_all);
        eprint!("{}", self.stderr_all);
        eprintln!("error: {message}");
        ExitCode::FAILURE
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
            let mut all = self.extra_base.clone();
            all.extend(extra);
            all.push(("failures".to_string(), Value::Array(Vec::new())));
            return finish_json_with(
                self.name,
                "ok",
                0,
                self.started,
                &self.stdout_all,
                &self.stderr_all,
                all,
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

/// The shared envelope prefix with this binary's timing plumbing filled
/// in. Delegates to [`json::envelope_fields`] — the single spelling of
/// the prefix — so no emission site here can drift from the one there.
fn envelope_base(
    command: &str,
    status: &str,
    exit_code: i64,
    started: Instant,
    stderr: &str,
) -> Vec<(String, Value)> {
    json::envelope_fields(command, status, exit_code, duration_ms(started), "", stderr)
}

/// The step a failure summary names: the program and the arguments that
/// say which of a subcommand's children it was.
///
/// `lint` runs two cargo children and only the second compiles anything,
/// so "cargo exited with code 1" leaves a reader unable to tell a
/// formatting drift from a clippy finding — and, where the envelope also
/// carries a coverage statement, unable to tell whether the compiling
/// child ran at all. Everything after a `--` belongs to the child's own
/// child and is left off; the verb and its flags are what identify the
/// step.
fn step_name<A: AsRef<OsStr>>(program: &str, args: &[A]) -> String {
    let mut named = program.to_string();
    for argument in args {
        let text = argument.as_ref().to_string_lossy().into_owned();
        if text == "--" {
            break;
        }
        named.push(' ');
        named.push_str(&text);
    }
    named
}

/// One structured failure for an envelope's `failures` array.
fn failure_entry(code: &str, summary: &str) -> Value {
    Value::Object(vec![
        ("code".to_string(), Value::String(code.to_string())),
        ("summary".to_string(), Value::String(summary.to_string())),
    ])
}

/// The envelope's `target` field: what tree this invocation ran in.
fn target_field(kind: workspace::TargetKind, root: &Path) -> (String, Value) {
    (
        "target".to_string(),
        Value::Object(vec![
            ("kind".to_string(), Value::String(kind.name().to_string())),
            (
                "root".to_string(),
                Value::String(root.display().to_string()),
            ),
            (
                "manifest".to_string(),
                Value::String(root.join("Cargo.toml").display().to_string()),
            ),
        ]),
    )
}

/// The envelope's `coverage` statement: what the cargo invocation this run
/// issued actually enabled. Descriptive, never aspirational — `features`
/// and `all_features` are the very values [`plan::workspace_steps`] hands
/// the child, so a verdict's reader can see which feature flags a green
/// was built with. `packages` and `profile` restate the scope and plan
/// the subcommand fixes rather than reading the child's argv back. The
/// `--workspace` pairing is asserted against the argv `plan.rs` builds;
/// the profile names are asserted only as the *values the envelope
/// carries* (`tests/targets.rs`), not against cargo's own choice, so
/// they are this table's claim about cargo rather than a reading of it.
fn coverage_field(
    command: Command,
    features: &[String],
    all_features: bool,
    packages: &str,
) -> (String, Value) {
    // Cargo's own profile names: `cargo test` compiles the `test`
    // profile, not `dev`, and the one field that promises to describe
    // the invocation exactly must not round that off.
    let profile = match command {
        Command::Bench => "bench",
        Command::Test => "test",
        _ => "dev",
    };
    (
        "coverage".to_string(),
        Value::Object(vec![
            (
                "features".to_string(),
                Value::Array(
                    features
                        .iter()
                        .map(|names| Value::String(names.clone()))
                        .collect(),
                ),
            ),
            ("all_features".to_string(), Value::Bool(all_features)),
            ("packages".to_string(), Value::String(packages.to_string())),
            ("profile".to_string(), Value::String(profile.to_string())),
        ]),
    )
}

/// A refusal with its structured class: what the `failures` entry carries
/// and what the prose says. Every refusal path emits both, so a consumer
/// can dispatch on the code and a person can read the reason.
struct Refusal {
    code: &'static str,
    message: String,
}

/// Why a tree could not be classified. The two cases are different claims
/// and carry different codes: `CannotTell` means the tool failed to
/// establish anything (unreadable manifest, broken metadata, a toolchain
/// that would not answer); `NotARenewProject` means the metadata was read
/// clean and no member depends on a renew crate. Folding them together
/// would stamp a verdict onto a question that was never answered.
enum ClassifyError {
    CannotTell(String),
    NotARenewProject,
}

impl ClassifyError {
    fn refusal(self) -> Refusal {
        match self {
            Self::CannotTell(detail) => Refusal {
                code: "classification-failed",
                message: format!("cannot tell what this workspace is: {detail}"),
            },
            Self::NotARenewProject => Refusal {
                code: "not-a-renew-project",
                message: "this workspace neither is the engine nor depends on a renew \
                          crate; there is nothing here this tool can honestly report on"
                    .to_string(),
            },
        }
    }
}

/// Refuse anything but the engine's own tree, naming what the subcommand
/// is for. The engine-only subcommands read manifests, samples, and rules
/// that exist nowhere else; running them in a game and reporting *anything*
/// would be a verdict about a question the tree never asked.
///
/// The refusal carries the classification where one succeeded — a
/// project tree's refusal knows the kind it established, and the caller
/// puts it in the envelope: an envelope keeps what it knows.
fn require_engine_workspace(
    root: &Path,
    purpose: &str,
) -> Result<(), (Refusal, Option<workspace::TargetKind>)> {
    match classify_target(root) {
        Ok(workspace::TargetKind::EngineWorkspace) => Ok(()),
        Ok(workspace::TargetKind::Project) => Err((
            Refusal {
                code: "engine-only-subcommand",
                message: format!(
                    "{purpose}; this workspace is a project that uses the engine, not the \
                     engine itself"
                ),
            },
            Some(workspace::TargetKind::Project),
        )),
        Err(error) => Err((error.refusal(), None)),
    }
}

/// Classify the tree at `root`. Engine detection is one manifest read;
/// only a foreign tree pays for `cargo metadata`. Refusing is the
/// contract; guessing would hand a verdict about somebody's unrelated
/// code to a reader who asked about a game.
fn classify_target(root: &Path) -> Result<workspace::TargetKind, ClassifyError> {
    let manifest = fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|error| ClassifyError::CannotTell(format!("cannot read the manifest: {error}")))?;
    if workspace::manifest_declares_engine(&manifest) {
        return Ok(workspace::TargetKind::EngineWorkspace);
    }
    let metadata = cargo_metadata(root)
        .and_then(|text| parsed_metadata(&text))
        .map_err(ClassifyError::CannotTell)?;
    if workspace::metadata_names_renew_dependency(&metadata) {
        Ok(workspace::TargetKind::Project)
    } else {
        Err(ClassifyError::NotARenewProject)
    }
}

fn run_steps(invocation: &Invocation) -> ExitCode {
    // The coverage statement depends only on the parsed invocation, so it
    // is established before anchoring, let alone classification: even the
    // rootless refusal says what the run would have covered.
    let mut established = Vec::new();
    if invocation.command.takes_workspace_features() {
        established.push(coverage_field(
            invocation.command,
            &invocation.features,
            invocation.all_features,
            "workspace",
        ));
    }
    let mut runner = match Runner::anchored(invocation.command.name(), invocation.json, established)
    {
        Ok(runner) => runner,
        Err(exit) => return exit,
    };
    // What tree is this? The engine and a game are both welcome here; a
    // workspace that is neither is refused before any child runs, so a
    // verdict about nothing is never emitted.
    let kind = match classify_target(&runner.root) {
        Ok(kind) => kind,
        Err(error) => {
            let refusal = error.refusal();
            return runner.refuse(refusal.code, &refusal.message);
        }
    };
    runner.extra_base.push(target_field(kind, &runner.root));
    for step in plan::workspace_steps(
        invocation.command,
        invocation.smoke,
        &invocation.features,
        invocation.all_features,
    ) {
        if let Err(exit) = runner.execute(&step.program, &step.args) {
            return exit;
        }
    }
    runner.finish()
}

/// Why a comparison that cannot name its compiler is not a passing one.
const TOOLCHAIN_UNREADABLE: &str = "could not read `rustc --version`, and a comparison that cannot name its compiler is \
     inconclusive rather than passing";

/// Run the pinned simulations and write this target's report.
///
/// Each run contributes whichever digests its own report carries — two for
/// the glide sample, which computes a frame-schedule hash beside its world
/// hash, and one apiece for every run added since. A lane that assumed one
/// shape would tell the others their report was missing a field, which is
/// true and useless.
fn run_determinism_emit(output_path: &str, target: Option<&str>, json_mode: bool) -> ExitCode {
    let mut runner = match Runner::anchored("determinism", json_mode, Vec::new()) {
        Ok(runner) => runner,
        Err(exit) => return exit,
    };
    match classify_target(&runner.root) {
        Ok(workspace::TargetKind::EngineWorkspace) => {
            let root = runner.root.clone();
            runner
                .extra_base
                .push(target_field(workspace::TargetKind::EngineWorkspace, &root));
        }
        Ok(workspace::TargetKind::Project) => {
            // The classification succeeded; the refusal keeps what it knows.
            let root = runner.root.clone();
            runner
                .extra_base
                .push(target_field(workspace::TargetKind::Project, &root));
            return runner.refuse(
                "engine-only-subcommand",
                "determinism runs the engine's pinned simulations and is not available \
                 for a project workspace",
            );
        }
        Err(error) => {
            let refusal = error.refusal();
            return runner.refuse(refusal.code, &refusal.message);
        }
    }

    // **Before a single build, because the answer cannot change and the
    // builds are expensive.** A triple the table has never been taught
    // would have to be labelled by reading its fields off by position,
    // which is wrong for more than half the targets rustc ships —
    // and a leg wearing the wrong platform is compared against rows it
    // does not belong to. Asked here, the refusal names the flag that
    // caused it; asked after the runs, it arrives eleven cargo
    // invocations later wearing the last one's error message.
    let platform = match target {
        Some(triple) => match determinism::platform_of_triple(triple) {
            Some((os, arch)) => Some((os.to_string(), arch.to_string())),
            None => {
                return runner.fail(&format!(
                    "`{triple}` is not a target this lane knows how to label, so a leg \
                     built for it could not say truthfully where it ran; add it to \
                     `platform_of_triple` beside the row that puts it in CI"
                ));
            }
        },
        None => None,
    };

    // The toolchain that built the binaries being compared. Read from
    // the compiler rather than from a file, because a file records an
    // intention and this has to record what actually ran.
    let toolchain = match probe("rustc", &["--version"], Some(&runner.root)) {
        Ok(probed) if probed.success => probed.stdout.trim().to_string(),
        Ok(probed) => {
            // Whatever the compiler said about its own failure is the
            // evidence for this abort; every other emit failure keeps
            // the child's words for the same reason.
            runner.stdout_all.push_str(&probed.stdout);
            runner.stderr_all.push_str(&probed.stderr);
            return runner.fail(TOOLCHAIN_UNREADABLE);
        }
        Err(_) => return runner.fail(TOOLCHAIN_UNREADABLE),
    };

    let digests = match collect_pinned_digests(&mut runner, target) {
        Ok(digests) => digests,
        Err(exit) => return exit,
    };

    // **Whose platform this leg describes.** With a target, the runs
    // executed somewhere this process is not, so reading this process's
    // own constants would label a device's digests with the desktop
    // that launched them. The triple is what the binaries were built
    // for and what the runner executed; what it cannot prove is that
    // the runner truly went there. Each lane answers that in the terms
    // its platform allows: the Android one asserts the attached
    // device's own architecture, and the iOS one leans on the loader,
    // which refuses a binary whose platform does not match the
    // simulator's - so digests coming back at all prove the match.
    let (os, arch) = match platform {
        Some(pair) => pair,
        None => (env::consts::OS.to_string(), env::consts::ARCH.to_string()),
    };
    let leg = determinism::Leg {
        origin: output_path.to_string(),
        os,
        arch,
        toolchain,
        digests,
    };
    let rendered = determinism::render_leg(&leg);
    if let Err(error) = fs::write(output_path, &rendered) {
        return runner.fail(&format!("could not write `{output_path}`: {error}"));
    }
    let note = format!(
        "wrote {} digests for {}/{} to {output_path}",
        leg.digests.len(),
        leg.os,
        leg.arch
    );
    emit_note(&note, runner.json, &mut runner.stdout_all);
    runner.finish()
}

/// Run every pinned simulation and gather what they reported.
///
/// Split from the emit half so each stays readable: this is the loop
/// that spawns children and reads their reports, and every way it can
/// end early is a finished envelope its caller returns unchanged.
fn collect_pinned_digests(
    runner: &mut Runner<'_>,
    target: Option<&str>,
) -> Result<BTreeMap<String, String>, ExitCode> {
    let mut digests = BTreeMap::new();
    for (name, package, args, fields) in determinism::PINNED_RUNS {
        // Cargo runs the built binary through whatever
        // `CARGO_TARGET_<TRIPLE>_RUNNER` names, so a device leg is this
        // same command with a runner that pushes and executes somewhere
        // else. Nothing here learns that a device exists. Built by a
        // function rather than inline so the pass-through is asserted by
        // tests instead of only by a lane that cannot fail the build.
        let invocation = determinism::pinned_invocation(package, args, target);
        let probed = match probe("cargo", &invocation, Some(&runner.root)) {
            Ok(probed) => probed,
            Err(error) => return Err(runner.fail(&format!("could not start `{name}`: {error}"))),
        };
        if !probed.success {
            // The child ran and failed: a step failure with a delivered
            // outcome, not an abort. Its output and raw exit code go into
            // the envelope — a red lane whose envelope carried only a
            // summary sentence would leave its reader nothing to
            // diagnose with.
            runner.stdout_all.push_str(&probed.stdout);
            runner.stderr_all.push_str(&probed.stderr);
            return Err(runner.deliver_failed(
                "step-failed",
                &format!(
                    "the pinned run `{name}` failed, so this target contributes nothing \
                     and the comparison must not be told otherwise"
                ),
                probed.code,
            ));
        }
        match determinism::digests_from_output(name, &probed.stdout, fields) {
            Ok(pairs) => {
                for (key, digest) in pairs {
                    // Inserting over an existing key would drop a digest
                    // and shrink what this leg proves — silently, and
                    // identically on every target, so the comparison
                    // would agree over the narrower set and report the
                    // breadth the list claims. Two rows sharing a name
                    // is the ordinary way that happens.
                    if let Some(previous) = digests.insert(key.clone(), digest) {
                        return Err(runner.fail(&format!(
                            "two pinned runs both report `{key}` (the earlier value was \
                             {previous}) — one would overwrite the other and this leg \
                             would prove less than the pinned list claims"
                        )));
                    }
                }
            }
            Err(message) => {
                // What the run printed is the evidence for why its report
                // could not be read; the step-failure branch above keeps
                // it for the same reason.
                runner.stdout_all.push_str(&probed.stdout);
                runner.stderr_all.push_str(&probed.stderr);
                return Err(runner.fail(&message));
            }
        }
    }
    Ok(digests)
}

/// Where a human-facing note goes: into the envelope's captured stdout
/// under `--json`, straight to stdout otherwise.
///
/// A function rather than an `if` at the call site so both halves are
/// reachable from a unit test: `--json` promises exactly one document on
/// stdout, and a loose line printed ahead of the envelope breaks that
/// promise in a way only a consumer notices.
fn emit_note(note: &str, json: bool, captured: &mut String) {
    if json {
        captured.push_str(note);
        captured.push('\n');
    } else {
        println!("{note}");
    }
}

/// Hold several targets' reports against each other.
///
/// No `target` parameter, and that is a rule rather than an omission:
/// the parser refuses `--target` alongside `--compare` before this is
/// reached, because comparison reads legs that were already emitted and
/// each one names the platform it ran on.
fn run_determinism_compare(paths: &[String], json_mode: bool) -> ExitCode {
    let mut runner = match Runner::anchored("determinism", json_mode, Vec::new()) {
        Ok(runner) => runner,
        Err(exit) => return exit,
    };
    // Compare holds legs against the engine's own expected-target list, so
    // it is as engine-only as emit: the same guard, the same refusals.
    match classify_target(&runner.root) {
        Ok(workspace::TargetKind::EngineWorkspace) => {
            let root = runner.root.clone();
            runner
                .extra_base
                .push(target_field(workspace::TargetKind::EngineWorkspace, &root));
        }
        Ok(workspace::TargetKind::Project) => {
            // The classification succeeded; the refusal keeps what it knows.
            let root = runner.root.clone();
            runner
                .extra_base
                .push(target_field(workspace::TargetKind::Project, &root));
            return runner.refuse(
                "engine-only-subcommand",
                "determinism compares the engine's pinned simulations and is not \
                 available for a project workspace",
            );
        }
        Err(error) => {
            let refusal = error.refusal();
            return runner.refuse(refusal.code, &refusal.message);
        }
    }

    let rows = determinism::expected_rows();
    let digests = determinism::expected_digest_names();

    let mut legs = Vec::new();
    for path in paths {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            // A leg file that cannot be read is an abort: the comparison
            // never received its inputs, so no verdict — not even the
            // inconclusive one, which is reserved for legs that were read
            // and judged — was delivered.
            Err(error) => return runner.fail(&format!("could not read `{path}`: {error}")),
        };
        match determinism::parse_leg(path, &text) {
            Ok(leg) => legs.push(leg),
            Err(message) => return runner.fail(&message),
        }
    }

    let verdict = determinism::compare(&legs, &rows, &digests);
    let report = determinism::describe(&verdict);
    // A delivered verdict is a finding, never an abort: divergence and
    // inconclusiveness each get their own code, because a consumer that
    // retries aborts must never retry away the one red this lane exists
    // to produce.
    match &verdict {
        determinism::Verdict::Agree { .. } => {
            // The agreeing verdict's prose rides inside the envelope in
            // JSON mode — stdout carries exactly one document, and a
            // consumer of that mode should not lose the report either.
            if runner.json {
                runner.stdout_all.push_str(&report);
                runner.stdout_all.push('\n');
            } else {
                println!("{report}");
            }
            runner.finish()
        }
        determinism::Verdict::Diverged(_) => {
            runner.deliver_failed("determinism-diverged", &report, 1)
        }
        determinism::Verdict::Inconclusive(_) => {
            runner.deliver_failed("determinism-inconclusive", &report, 1)
        }
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
    let mut runner = match Runner::anchored(invocation.command.name(), invocation.json, Vec::new())
    {
        Ok(runner) => runner,
        Err(exit) => return exit,
    };
    match classify_target(&runner.root) {
        Ok(workspace::TargetKind::EngineWorkspace) => {
            let root = runner.root.clone();
            runner
                .extra_base
                .push(target_field(workspace::TargetKind::EngineWorkspace, &root));
        }
        Ok(workspace::TargetKind::Project) => {
            // The classification succeeded; the refusal keeps what it knows.
            let root = runner.root.clone();
            runner
                .extra_base
                .push(target_field(workspace::TargetKind::Project, &root));
            // The reason names the subcommand that was refused — a
            // consumer renders the summary verbatim, and `record` being
            // told about `run` reads as an answer to a different ask.
            return runner.refuse(
                "engine-only-subcommand",
                &format!(
                    "{} starts the engine's own samples; it cannot run a project's binaries",
                    invocation.command.name()
                ),
            );
        }
        Err(error) => {
            let refusal = error.refusal();
            return runner.refuse(refusal.code, &refusal.message);
        }
    }
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
    // The sample is known now, and with it the package cargo will be
    // told to build — so the coverage statement can name it. Established
    // here rather than at anchoring for that reason: before the name
    // resolves there is no package to state. A replay's digest is a
    // machine-consumed artifact, and two digests from differently
    // featured builds must not look alike to the consumer comparing
    // them.
    runner.extra_base.push(coverage_field(
        invocation.command,
        &invocation.features,
        invocation.all_features,
        &sample.package,
    ));
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
        let mut fields = envelope_base(
            "doctor",
            if ok { "ok" } else { "failed" },
            i64::from(!ok),
            started,
            "",
        );
        fields.push(("checks".to_string(), Value::Array(items)));
        emit_stdout_line(&Value::Object(fields).render());
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
    // Doctor diagnoses environments that may have no workspace at all,
    // and a manifest it cannot read is, for its purposes, the same as
    // none: it probes tools rather than reporting on a tree.
    let root = workspace_root().ok();
    let (rustup_found, active_toolchain, rustup_unavailable) =
        match probe("rustup", &["show", "active-toolchain"], root.as_deref()) {
            Ok(probed) if probed.success => (true, doctor::first_token(&probed.stdout), None),
            Ok(_) => (true, None, None),
            Err(error) => (false, None, Some(doctor::probe_failure(error.kind()))),
        };
    let cargo_version = probe("cargo", &["--version"], root.as_deref())
        .ok()
        .filter(|probed| probed.success)
        .and_then(|probed| doctor::parse_cargo_version(&probed.stdout));
    let (git_found, git_unavailable) = match probe("git", &["--version"], root.as_deref()) {
        Ok(probed) => (probed.success, None),
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

/// What a probe's child reported: everything a failure envelope needs to
/// carry, so a red probe is diagnosable from the envelope alone.
struct Probed {
    success: bool,
    /// The child's raw exit code, `-1` for signal deaths.
    code: i32,
    stdout: String,
    stderr: String,
}

/// Run a probe command, from the workspace root when one exists so toolchain
/// overrides resolve consistently with the build steps.
///
/// The spawn error is returned rather than discarded: "could not run" and
/// "is not installed" are different answers, and a report that cannot tell
/// them apart sends its reader to fix the wrong thing.
fn probe(program: &str, args: &[&str], root: Option<&Path>) -> Result<Probed, std::io::Error> {
    let mut process = Process::new(program);
    process.args(args);
    if let Some(directory) = root {
        process.current_dir(directory);
    }
    process.output().map(|output| Probed {
        success: output.status.success(),
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// The tree to work in, or the reason there is none.
///
/// `Err` carries the sentence the caller reports: either no manifest of
/// any shape above the working directory, or one that is there and
/// could not be read. The two are different claims and are told apart
/// here rather than folded into "not found" — walking past a manifest
/// this tool cannot name would anchor somewhere the caller never asked
/// about, and report on that tree instead.
fn workspace_root() -> Result<PathBuf, String> {
    // Its own sentence, not the rootless one: nothing was looked for,
    // because the tool could not establish where it stands. Reusing
    // "no manifest was found above the current directory" would be a
    // verdict about a question never asked — the distinction this whole
    // surface is built on.
    let directory = match env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            return Err(format!(
                "the working directory could not be read, so no tree could be located: {error}"
            ));
        }
    };
    match workspace::find_root(&directory) {
        workspace::Anchor::Root(root) => Ok(root),
        workspace::Anchor::Unreadable(manifest) => Err(format!(
            "`{}` is not a manifest this tool can read: it declares neither a workspace nor \
             a package in a spelling the scan knows. Refusing rather than looking further up, \
             because a verdict from a tree above it would be about a different question",
            manifest.display()
        )),
        workspace::Anchor::None => Err(ROOTLESS_MESSAGE.to_string()),
    }
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

    /// Two rows sharing a name, or a field listed twice, would have one
    /// digest overwrite another: the leg would prove less than the list
    /// claims, identically on every target, so the comparison would
    /// agree over the narrower set and report the wider one. The emit
    /// path refuses the collision at runtime; this catches it at the
    /// table, where it is a one-line edit to make and to see.
    #[test]
    fn every_pinned_run_has_its_own_name_and_no_repeated_field() {
        let mut names: Vec<&str> = determinism::PINNED_RUNS
            .iter()
            .map(|(name, ..)| *name)
            .collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "two pinned runs share a name");
        for (name, _, _, fields) in determinism::PINNED_RUNS {
            // A row naming no fields contributes nothing to the leg and
            // says so nowhere: every target would narrow identically,
            // the anti-vacuity check would not fire because the other
            // rows still fill the map, and the comparison would agree
            // over the smaller set while reporting the list's breadth.
            assert!(
                !fields.is_empty(),
                "`{name}` names no digest field, so it would contribute nothing to the leg"
            );
            let mut seen: Vec<&str> = fields.to_vec();
            let listed = seen.len();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), listed, "`{name}` lists a digest field twice");
        }
    }

    /// The statement names the scope it was given, not a fixed one: the
    /// sample runners build one package, and a `replay` digest produced
    /// by a differently featured build of that package must not look
    /// like any other to the consumer comparing them.
    #[test]
    fn the_coverage_statement_names_the_scope_it_was_given() {
        let (_, workspace) = coverage_field(Command::Build, &[], false, "workspace");
        assert_eq!(
            workspace.get("packages").and_then(Value::as_str),
            Some("workspace")
        );
        let (_, sample) = coverage_field(
            Command::Replay,
            &["window".to_string()],
            false,
            "renew-sample-glide",
        );
        assert_eq!(
            sample.get("packages").and_then(Value::as_str),
            Some("renew-sample-glide")
        );
        assert_eq!(
            sample.get("profile").and_then(Value::as_str),
            Some("dev"),
            "a sample runner compiles the dev profile"
        );
        assert_eq!(
            sample
                .get("features")
                .and_then(Value::as_array)
                .map(<[Value]>::len),
            Some(1)
        );
    }

    /// A step is named by its verb and flags, and stops at the `--`:
    /// what follows belongs to the child's own child, and the two steps
    /// that carry one (clippy's lint level, bench's harness flag) would
    /// otherwise read as though those flags were the step's.
    #[test]
    fn a_step_is_named_by_its_verb_and_stops_at_the_separator() {
        assert_eq!(
            step_name("cargo", &["build", "--workspace"]),
            "cargo build --workspace"
        );
        assert_eq!(
            step_name("cargo", &["bench", "--workspace", "--", "--test"]),
            "cargo bench --workspace"
        );
        assert_eq!(
            step_name(
                "cargo",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings"
                ]
            ),
            "cargo clippy --workspace --all-targets"
        );
        // A program with no arguments is its own name.
        assert_eq!(step_name("rustup", &[] as &[&str]), "rustup");
    }

    /// The mode's promise: exactly one document on stdout. A note that
    /// went to stdout under `--json` would be a second one.
    #[test]
    fn a_note_rides_inside_the_envelope_in_json_mode() {
        let mut captured = String::new();
        emit_note("wrote 15 digests", true, &mut captured);
        assert_eq!(captured, "wrote 15 digests\n");

        // Outside that mode the note is the output, and nothing is
        // captured for an envelope that will not be written.
        let mut plain = String::new();
        emit_note("wrote 15 digests", false, &mut plain);
        assert!(plain.is_empty());
    }
}
