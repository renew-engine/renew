//! The schema registry is a contract, so something must hold it to the
//! binary: every failure code the source can emit is listed in the
//! schema, and probe envelopes conform to the shapes the schema declares.
//!
//! Helpers return `Result` — helper code is not test code to the lint
//! configuration, so each test unwraps at its own call site.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use renew_cli::json::{self, Value};

fn schema() -> Result<Value, String> {
    let text =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/envelope.v2.json"))
            .map_err(|error| format!("cannot read the schema: {error}"))?;
    json::parse(&text).map_err(|error| format!("the schema is not valid JSON: {error}"))
}

/// Every `.rs` file under `root`, recursively. A walk rather than a
/// listing: a module moved into a subdirectory would leave a
/// single-level scan quietly reading less than the tree holds.
fn source_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let entries =
        fs::read_dir(root).map_err(|error| format!("cannot read {}: {error}", root.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("cannot read an entry of {}: {error}", root.display()))?
            .path();
        if path.is_dir() {
            found.extend(source_files(&path)?);
        } else if path.extension().is_some_and(|kind| kind == "rs") {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

fn known_codes(schema: &Value) -> Result<Vec<String>, String> {
    Ok(schema
        .get("x-known-codes")
        .and_then(Value::as_array)
        .ok_or("the schema lists no known codes")?
        .iter()
        .filter_map(|code| code.as_str().map(ToString::to_string))
        .collect())
}

#[test]
fn the_source_and_the_schema_agree_on_the_code_set_exactly() {
    // The literals are the arguments of `failure_entry(`, `refuse(` and
    // `deliver_failed(` calls and the `code:` fields of the refusal
    // constructors. Whitespace is stripped from the whole source before
    // matching, so a formatter wrapping a call across lines cannot hide
    // a site from the scan. The text
    // comes from walking `src/` at test time rather than from a list
    // beside an `include_str!` block: two lists drift, and a scan that
    // silently narrows as modules are added is the failure this test
    // exists to prevent.
    let files = source_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src")).expect("src");
    assert!(
        files.len() >= 12,
        "the walk found only {} source files; it has stopped seeing the tree",
        files.len()
    );
    let mut source = String::new();
    for file in &files {
        source.push_str(&fs::read_to_string(file).expect("a source file"));
    }
    let source: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    let known = known_codes(&schema().expect("schema")).expect("codes");
    let mut emitted: Vec<String> = Vec::new();
    // The refuse/deliver markers carry the receiver name: `ui_compile`'s
    // scanner has its own `refuse` whose argument is a diagnostic
    // sentence, not a code, and the bare `.refuse("` marker read it as
    // one. A future envelope site whose runner binding is named
    // something else must be added here — the schema-to-source
    // direction below catches the drift as soon as its code reaches the
    // schema.
    for marker in [
        "failure_entry(\"",
        "runner.refuse(\"",
        "runner.deliver_failed(\"",
        "code:\"",
    ] {
        for (index, _) in source.match_indices(marker) {
            let rest = &source[index + marker.len()..];
            let code: String = rest.chars().take_while(|c| *c != '"').collect();
            // Format strings and non-literal arguments are not codes.
            if !code.is_empty() && !code.contains(['{', '}']) && !emitted.contains(&code) {
                emitted.push(code);
            }
        }
    }
    // Both directions: a code the schema never learned fails, and a
    // listed code the source can no longer emit fails too — a registry
    // is stale in either direction.
    for code in &emitted {
        assert!(
            known.contains(code),
            "the source emits `{code}` but schema/envelope.v2.json does not list it"
        );
    }
    for code in &known {
        assert!(
            emitted.contains(code),
            "the schema lists `{code}` but no source site emits it"
        );
    }
    assert!(
        emitted.len() >= 6,
        "the scan found only {} distinct codes; the markers have drifted from the source",
        emitted.len()
    );
}

/// The rollout table calls itself exhaustive over the subcommands, and
/// it is the contract consumers read. Nothing else holds it to the
/// parser's own list, so a subcommand added later would turn that claim
/// into a lie with no gate to catch it — the same drift the usage-text
/// parity gate exists to prevent.
#[test]
fn the_rollout_table_names_every_subcommand() {
    let registry =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/README.md"))
            .expect("the registry README");
    // The table as a *table*: the contiguous run of rows following the
    // header's delimiter line, ending at the first line that is not one.
    // Scanning every `|`-leading line anywhere in the file would accept a
    // row that had drifted out of the table — into a following
    // paragraph, where a Markdown reader renders it as prose and a
    // consumer never sees it as a row at all. A `|`-leading-line scan
    // cannot notice that, because it matches the stranded row wherever
    // the row happens to sit.
    let mut rows: Vec<&str> = Vec::new();
    let mut lines = registry.lines();
    while let Some(line) = lines.next() {
        if !line.starts_with("| Subcommands") {
            continue;
        }
        let delimiter = lines.next().unwrap_or_default();
        assert!(
            delimiter.starts_with("|---"),
            "the rollout table's header is not followed by a delimiter row: {delimiter}"
        );
        for row in lines.by_ref() {
            if !row.starts_with('|') {
                break;
            }
            rows.push(row);
        }
        break;
    }
    assert!(
        rows.len() >= 8,
        "the rollout table has only {} contiguous rows; part of it has drifted out of the \
         table, where a Markdown reader renders it as prose rather than as contract",
        rows.len()
    );
    // And nothing that looks like a row may sit after the block: a line
    // stranded past the break reads as contract to a grep and as prose to
    // every renderer.
    let after: usize = registry
        .lines()
        .skip_while(|line| !line.starts_with("| Subcommands"))
        .skip(2 + rows.len())
        .filter(|line| line.starts_with('|'))
        .count();
    assert_eq!(
        after, 0,
        "a `|`-leading line sits outside the rollout table's contiguous block"
    );

    // The table's first column, cell by cell: every name written in
    // backticks.
    let listed: Vec<String> = rows
        .iter()
        .flat_map(|line| {
            line.split('|')
                .nth(1)
                .unwrap_or_default()
                .split('`')
                .skip(1)
                .step_by(2)
                .map(ToString::to_string)
                .collect::<Vec<String>>()
        })
        .collect();
    for command in renew_cli::cli::Command::ALL {
        let name = command.name();
        assert!(
            listed.iter().any(|cell| cell == name),
            "`{name}` is a subcommand and the rollout table does not name it"
        );
    }
    assert!(
        listed.iter().any(|cell| cell == "help"),
        "the table covers `help` too, which is not a Command"
    );
}

/// The registry's prose introduces its bullet list as the documentation
/// of `x-known-codes`, so a code that joins the schema and the source
/// while the prose stays short would satisfy both existing gates and
/// leave the most consumer-facing paragraph under-describing the enum it
/// claims to enumerate.
#[test]
fn the_registry_prose_documents_every_known_code() {
    let registry =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/README.md"))
            .expect("the registry README");
    let known = known_codes(&schema().expect("schema")).expect("codes");
    assert!(!known.is_empty(), "the schema lists no codes at all");
    for code in &known {
        assert!(
            registry.contains(&format!("- `{code}` —")),
            "`{code}` is a known code and schema/README.md carries no bullet for it"
        );
    }
}

/// The rollout table's **cells**, not only its first column.
///
/// The cells are the per-subcommand contract a consumer implements
/// against, and until this existed nothing held them to the binary: a
/// cell could say `no` where the envelope carries the field, or `yes`
/// where it does not, and every suite stayed green. Only the rows whose
/// envelope is cheap to obtain are driven — no tree, no compile — which
/// is enough to hold the negative claims that were unheld.
#[test]
fn the_rollout_tables_cells_match_the_envelopes() {
    let registry =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/README.md"))
            .expect("the registry README");
    let base = std::env::temp_dir().join(format!("renew-cli-cells-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("scratch");

    // Subcommands whose envelope needs neither a tree nor a child.
    let cheap: [(&str, &[&str]); 4] = [
        ("help", &["help"]),
        ("doctor", &["doctor"]),
        ("asset-inspect", &["asset-inspect", "--pack", "none.rpk"]),
        (
            "ui-compile",
            &["ui-compile", "--from", "n.ui", "--out", "n.bin"],
        ),
    ];
    let mut checked = 0;
    for (name, arguments) in cheap {
        let Some(row) = registry
            .lines()
            .find(|line| line.starts_with('|') && line.contains(&format!("`{name}`")))
        else {
            panic!("the rollout table names no row for `{name}`");
        };
        let cells: Vec<&str> = row.split('|').map(str::trim).collect();
        // | subcommands | target | coverage | failures |
        let claims = [
            ("target", cells.get(2).copied().unwrap_or_default()),
            ("coverage", cells.get(3).copied().unwrap_or_default()),
            ("failures", cells.get(4).copied().unwrap_or_default()),
        ];
        let envelope = probe(&base, arguments).expect("an envelope");
        for (field, cell) in claims {
            // Both halves. A cell that says `no` must not have the field;
            // a cell that claims one must have it. Gating only the
            // negatives would let the positive half of the contract lie.
            if cell.starts_with("no") {
                assert!(
                    envelope.get(field).is_none(),
                    "the table says `{name}` carries no `{field}`, and the envelope has one: {}",
                    envelope.render()
                );
            } else {
                assert!(
                    envelope.get(field).is_some(),
                    "the table says `{name}` carries `{field}` ({cell}), and the envelope has \
                     none: {}",
                    envelope.render()
                );
            }
            checked += 1;
        }
    }
    assert!(
        checked >= 9,
        "only {checked} negative cells were driven; the table's shape has changed"
    );
    let _ = fs::remove_dir_all(&base);
}

/// One envelope against the schema's declared shapes: the leading fields
/// and their types, the status enum, and — where present — the target
/// kinds, coverage keys, and failure codes. `Err` names the violation.
fn conforms(envelope: &Value, schema: &Value) -> Result<(), String> {
    let rendered = envelope.render();
    // Read from the schema rather than restated here: a constant written
    // twice is a constant that can disagree with itself, and the file is
    // the one consumers validate against.
    let version = schema
        .get("properties")
        .and_then(|properties| properties.get("schema_version"))
        .and_then(|field| field.get("const"))
        .ok_or("the schema declares no schema_version const")?;
    if envelope.get("schema_version") != Some(version) {
        return Err(format!(
            "schema_version is not {}: {rendered}",
            version.render()
        ));
    }
    // The leading fields come from the schema's own `required` array, so
    // an edit to the schema and this check cannot drift apart — a field
    // added there is checked here without anyone remembering to.
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .ok_or("the schema declares no required array")?
        .iter()
        .filter_map(|name| name.as_str().map(ToString::to_string))
        .collect();
    if required.len() < 7 {
        return Err(format!(
            "the schema requires only {} fields; the leading seven have gone missing",
            required.len()
        ));
    }
    // Order too, not merely membership: the registry publishes the
    // leading fields "in a fixed order" and scopes the read-by-name rule
    // to the tail after them, so the order is contractual and something
    // has to hold it.
    let leading: Vec<String> = envelope
        .as_object()
        .ok_or_else(|| format!("the envelope is not an object: {rendered}"))?
        .iter()
        .take(required.len())
        .map(|(key, _)| key.clone())
        .collect();
    if leading != required {
        return Err(format!(
            "the leading fields are {leading:?}, and the schema fixes {required:?}: {rendered}"
        ));
    }
    for name in &required {
        let Some(value) = envelope.get(name) else {
            return Err(format!("required field `{name}` missing: {rendered}"));
        };
        let declared = schema
            .get("properties")
            .and_then(|properties| properties.get(name))
            .and_then(|property| property.get("type"))
            .and_then(Value::as_str);
        let type_holds = match declared {
            Some("string") => value.as_str().is_some(),
            Some("integer") => matches!(value, Value::Number(_)),
            // `schema_version` is a const, already checked above; any
            // future untyped property is not this check's to judge.
            _ => true,
        };
        if !type_holds {
            return Err(format!(
                "required field `{name}` has the wrong type: {rendered}"
            ));
        }
    }
    let statuses: Vec<String> = schema
        .get("properties")
        .and_then(|p| p.get("status"))
        .and_then(|s| s.get("enum"))
        .and_then(Value::as_array)
        .ok_or("the schema declares no status enum")?
        .iter()
        .filter_map(|s| s.as_str().map(ToString::to_string))
        .collect();
    let status = envelope
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("status missing: {rendered}"))?;
    if !statuses.contains(&status.to_string()) {
        return Err(format!("status `{status}` not in the enum: {rendered}"));
    }
    if let Some(target) = envelope.get("target") {
        target_conforms(target, schema, &rendered)?;
    }
    if let Some(coverage) = envelope.get("coverage") {
        coverage_conforms(coverage, schema, &rendered)?;
    }
    if let Some(failures) = envelope.get("failures").and_then(Value::as_array) {
        let known = known_codes(schema)?;
        for failure in failures {
            let code = failure
                .get("code")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("a failure without a code: {rendered}"))?;
            if !known.contains(&code.to_string()) {
                return Err(format!("unknown failure code `{code}`: {rendered}"));
            }
            if failure.get("summary").and_then(Value::as_str).is_none() {
                return Err(format!("a failure without a summary: {rendered}"));
            }
        }
    }
    Ok(())
}

/// The target field's shape, split from [`conforms`] so each half stays
/// readable: the kind drawn from the schema's own enum, and the two path
/// fields the registry says accompany it.
fn target_conforms(target: &Value, schema: &Value, rendered: &str) -> Result<(), String> {
    let kinds: Vec<String> = schema
        .get("properties")
        .and_then(|properties| properties.get("target"))
        .and_then(|field| field.get("properties"))
        .and_then(|properties| properties.get("kind"))
        .and_then(|field| field.get("enum"))
        .and_then(Value::as_array)
        .ok_or("the schema declares no target kind enum")?
        .iter()
        .filter_map(|kind| kind.as_str().map(ToString::to_string))
        .collect();
    let kind = target
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("target without kind: {rendered}"))?;
    if !kinds.contains(&kind.to_string()) {
        return Err(format!("target kind `{kind}` not in the enum: {rendered}"));
    }
    if target.get("root").and_then(Value::as_str).is_none()
        || target.get("manifest").and_then(Value::as_str).is_none()
    {
        return Err(format!("target without root/manifest: {rendered}"));
    }
    Ok(())
}

/// The coverage statement's shape, split from [`conforms`] so each half
/// stays readable: every key present, and the profile drawn from the
/// schema's own enum so an edit there bites here.
fn coverage_conforms(coverage: &Value, schema: &Value, rendered: &str) -> Result<(), String> {
    if coverage.get("features").and_then(Value::as_array).is_none()
        || coverage
            .get("all_features")
            .and_then(Value::as_bool)
            .is_none()
        || coverage.get("packages").and_then(Value::as_str).is_none()
    {
        return Err(format!("coverage missing a required key: {rendered}"));
    }
    let profiles: Vec<String> = schema
        .get("properties")
        .and_then(|p| p.get("coverage"))
        .and_then(|c| c.get("properties"))
        .and_then(|p| p.get("profile"))
        .and_then(|p| p.get("enum"))
        .and_then(Value::as_array)
        .ok_or("the schema declares no profile enum")?
        .iter()
        .filter_map(|p| p.as_str().map(ToString::to_string))
        .collect();
    let profile = coverage
        .get("profile")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("coverage without profile: {rendered}"))?;
    if !profiles.contains(&profile.to_string()) {
        return Err(format!("profile `{profile}` not in the enum: {rendered}"));
    }
    Ok(())
}

fn probe(directory: &Path, arguments: &[&str]) -> Result<Value, String> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_renew"));
    command
        .arg("--json")
        .args(arguments)
        .current_dir(directory)
        // The same isolation the sibling suite records: a target dir of
        // the fixture's own, and none of the outer run's compiler flags.
        .env("CARGO_TARGET_DIR", directory.join("target"));
    for inherited in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTDOCFLAGS",
        "CARGO_ENCODED_RUSTDOCFLAGS",
    ] {
        command.env_remove(inherited);
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot run the binary: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
    let line = lines.next().ok_or("no envelope line")?;
    // Exactly one document on stdout — asserted here too, so a stray
    // line ahead of the envelope cannot hide behind a last-line read.
    if let Some(extra) = lines.next() {
        return Err(format!("more than one stdout line; the second is: {extra}"));
    }
    json::parse(line).map_err(|error| format!("unparseable envelope: {error}: {line}"))
}

#[test]
fn probe_envelopes_conform_to_the_schema() {
    let schema = schema().expect("schema");
    let base = std::env::temp_dir().join(format!("renew-cli-registry-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);

    // A refusal envelope: a stranger workspace, refused before any child
    // runs — carries a coded failure, no compile needed. (On a machine
    // whose toolchain cannot even answer `cargo metadata`, the code is
    // `classification-failed` instead — also conformant, also honest.)
    let stranger = base.join("stranger");
    fs::create_dir_all(&stranger).expect("scratch");
    fs::write(
        stranger.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = []\n",
    )
    .expect("manifest");
    let refusal = probe(&stranger, &["build"]).expect("an envelope");
    conforms(&refusal, &schema).expect("the refusal envelope conforms");

    // A target-carrying envelope: an engine-marked tree, no external tool
    // needed for the classification itself.
    let marked = base.join("marked");
    fs::create_dir_all(&marked).expect("scratch");
    fs::write(
        marked.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = []\n\n[workspace.metadata.renew]\nengine = true\n",
    )
    .expect("manifest");
    let anchored = probe(&marked, &["configure"]).expect("an envelope");
    conforms(&anchored, &schema).expect("the anchored envelope conforms");

    // A payload envelope with neither target nor coverage: help.
    let help = probe(&marked, &["help"]).expect("an envelope");
    conforms(&help, &schema).expect("the help envelope conforms");

    // An engine-only refusal from a project tree: carries the code and,
    // for runner subcommands, the established kind.
    let project = base.join("project");
    fs::create_dir_all(project.join("game").join("src")).expect("scratch");
    fs::create_dir_all(project.join("renew-stub").join("src")).expect("scratch");
    fs::write(
        project.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"game\", \"renew-stub\"]\n",
    )
    .expect("manifest");
    // A tree in the temp directory inherits none of this repository's
    // configuration; the toolchain is pinned explicitly.
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml"),
        project.join("rust-toolchain.toml"),
    )
    .expect("toolchain pin");
    for (name, tail) in [
        ("renew-stub", ""),
        (
            "game",
            "\n[dependencies]\nrenew-stub = { path = \"../renew-stub\" }\n",
        ),
    ] {
        fs::write(
            project.join(name).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.0.1\"\nedition = \"2021\"\n{tail}"
            ),
        )
        .expect("member manifest");
        fs::write(project.join(name).join("src").join("lib.rs"), "").expect("member lib");
    }
    let refused = probe(&project, &["modules"]).expect("an envelope");
    conforms(&refused, &schema).expect("the engine-only refusal conforms");

    // A coverage-carrying envelope, so the schema's coverage shape checks
    // are exercised by a real emission rather than never at all.
    let built = probe(&project, &["build"]).expect("an envelope");
    conforms(&built, &schema).expect("the project build envelope conforms");
    assert!(
        built.get("coverage").is_some(),
        "the probe set must include a coverage-carrying envelope: {}",
        built.render()
    );

    // A `failed` envelope, so the status enum's third value and the
    // delivered-red shape are exercised by a real emission rather than
    // only by the two that never fail. Breaking the game's source is the
    // cheapest way to reach one.
    fs::write(
        project.join("game").join("src").join("lib.rs"),
        "fn deliberately_broken(",
    )
    .expect("break the game");
    let red = probe(&project, &["build"]).expect("an envelope");
    conforms(&red, &schema).expect("the failed envelope conforms");
    assert_eq!(
        red.get("status").and_then(Value::as_str),
        Some("failed"),
        "the probe set must include a failed envelope: {}",
        red.render()
    );

    // And a non-`dev` profile, so the coverage enum is exercised past its
    // first value.
    let benched = probe(&project, &["bench"]).expect("an envelope");
    conforms(&benched, &schema).expect("the bench envelope conforms");
    assert_eq!(
        benched
            .get("coverage")
            .and_then(|coverage| coverage.get("profile"))
            .and_then(Value::as_str),
        Some("bench"),
        "{}",
        benched.render()
    );

    let _ = fs::remove_dir_all(&base);
}
