//! Guards for lists that are maintained by hand in one file while the
//! facts they track live in another.
//!
//! The check here exists because its list has actually rotted, in this
//! repository, twice — and both times silently, because nothing compared
//! the two halves. The failure mode is the bad one: a test that keeps
//! passing while measuring nothing.
//!
//! This is a test rather than a `renew check` rule on purpose. A rule
//! would change what gates `main`, which is a heavier change than the
//! problem needs; a test rides the suite that already runs on every push
//! and fails in the same place as everything else.
//!
//! Helpers return `Result` rather than unwrapping: the lint that forbids
//! `expect` and `panic` outside tests reaches helpers in a test file too,
//! because the exemption follows `#[test]` rather than the file.

use std::path::{Path, PathBuf};
use std::process::Command;

use renew_cli::json::{self, Value};

/// The workspace root, from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    let guess = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    guess.canonicalize().unwrap_or(guess)
}

/// Crates whose manifest declares a `sanitized` feature.
///
/// Read from cargo rather than by scraping TOML: the feature table is
/// exactly what cargo already computes, and a hand-rolled parser here
/// would be a third copy of a fact, in a test whose whole subject is
/// duplicated facts.
fn crates_declaring_sanitized(root: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("cargo metadata could not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let document = json::parse(&text).map_err(|error| format!("metadata is not JSON: {error}"))?;
    let packages = document
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata carried no packages array".to_string())?;

    let mut found: Vec<String> = packages
        .iter()
        .filter(|package| {
            package
                .get("features")
                .and_then(Value::as_object)
                .is_some_and(|features| features.iter().any(|(name, _)| name == "sanitized"))
        })
        .filter_map(|package| {
            package
                .get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect();
    found.sort();
    Ok(found)
}

/// Every `<crate>/sanitized` token in the sanitizer workflow, and
/// separately the ones on its workspace-wide test step.
///
/// The file carries more than one `--features`: the workspace step that
/// runs everything, and a targeted step that reruns the job system's
/// stress tier alone. Only the first can be checked for *completeness* —
/// the targeted one legitimately names a single crate. Every token in
/// either is still checked for *existence*, because a typo anywhere fails
/// the whole job on an unknown feature.
///
/// That distinction is not hypothetical tidiness: the first draft of this
/// test assumed one `--features` in the file, and said so as an
/// assertion. It failed on its first run, which is the only reason it
/// does not now silently check the wrong line.
fn workflow_features(root: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    let path = root.join(".github/workflows/nightly-checks.yml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("{} is unreadable: {error}", path.display()))?;

    let mut all = Vec::new();
    let mut workspace_wide: Option<Vec<String>> = None;
    for line in text.lines() {
        let Some((_, after)) = line.split_once("--features ") else {
            continue;
        };
        let Some(argument) = after.split_whitespace().next() else {
            return Err(format!(
                "`--features` with no argument in {}",
                path.display()
            ));
        };
        let named: Vec<String> = argument
            .split(',')
            .filter_map(|entry| entry.strip_suffix("/sanitized"))
            .map(ToString::to_string)
            .collect();
        all.extend(named.iter().cloned());
        if line.contains("--workspace") {
            if workspace_wide.is_some() {
                return Err(format!(
                    "two workspace-wide `--features` lines in {}; this test would check only one",
                    path.display()
                ));
            }
            workspace_wide = Some(named);
        }
    }
    let mut wide = workspace_wide.ok_or_else(|| {
        format!(
            "no workspace-wide `--features` line in {} — the completeness check would have \
             nothing to read, and would pass vacuously",
            path.display()
        )
    })?;
    all.sort();
    all.dedup();
    wide.sort();
    Ok((all, wide))
}

/// The sanitizer workflow must enable `sanitized` for every crate that
/// declares it.
///
/// **This has gone wrong twice.** Once `renew-frame` was missing, and its
/// allocation-counting test ran under instrumented allocators — passing,
/// while asserting on counts that meant nothing. Once the
/// `hello_triangle` sample was missing, and that one surfaced only
/// because a person went looking on purpose. Both times the list was
/// correct when written and rotted when a crate was added somewhere else.
///
/// The correspondence is *declares the feature*, not *has an
/// allocation-counting test*: `renew-memory` declares it and has no such
/// test, so a check written the other way round would report it as a
/// false positive forever.
#[test]
fn the_sanitizer_workflow_names_every_crate_that_declares_the_feature() {
    let root = workspace_root();
    let declared = crates_declaring_sanitized(&root).expect("the workspace should describe itself");
    let (every_token, workspace_step) =
        workflow_features(&root).expect("the sanitizer workflow should be readable");

    assert!(
        !declared.is_empty(),
        "no crate declares a `sanitized` feature — this test would pass vacuously"
    );

    let missing: Vec<&String> = declared
        .iter()
        .filter(|crate_name| !workspace_step.contains(crate_name))
        .collect();
    assert!(
        missing.is_empty(),
        "these crates declare a `sanitized` feature but the workspace-wide sanitizer step does \
         not enable it, so their allocation-counting tests run under instrumented allocators and \
         assert on counts that mean nothing: {missing:?}. Add `<crate>/sanitized` to the \
         `--features` list in .github/workflows/nightly-checks.yml."
    );

    let unknown: Vec<&String> = every_token
        .iter()
        .filter(|crate_name| !declared.contains(crate_name))
        .collect();
    assert!(
        unknown.is_empty(),
        "the sanitizer workflow enables `sanitized` for crates that do not declare it: \
         {unknown:?}. cargo fails on an unknown feature, so this is a job that cannot start."
    );
}

// --- The unsafe surface -------------------------------------------------

/// Every file that opts itself out of the workspace's `unsafe_code` denial
/// with a crate-root allow.
///
/// Kept here so that adding one **fails this test** rather than passing
/// quietly. Carrying unsafe is a reviewed, per-crate decision with a
/// written grant behind it; the grant records exactly where the unsafe
/// territory is, and a file that appears here without appearing there
/// makes that record under-report. Both have happened: one grant was
/// written naming three of four items, and another naming one of three
/// test files. Neither was caught by anything mechanical.
const ALLOWS_UNSAFE: &[&str] = &[
    "crates/core/diag/tests/zero_alloc.rs",
    "crates/core/memory/src/lib.rs",
    "crates/jobs/src/lib.rs",
    "crates/rhi/src/lib.rs",
    "crates/rhi/tests/fault.rs",
    "crates/rhi/tests/fault_present.rs",
    "crates/rhi/tests/zero_alloc.rs",
];

/// Crates that do not inherit the workspace lint table, and so are not
/// subject to its `unsafe_code` denial at all.
///
/// The second, quieter route to carrying unsafe: no allow appears
/// anywhere in the source, because the lint was never in force. A guard
/// that only looked for allows would miss it entirely.
const OPTS_OUT_OF_WORKSPACE_LINTS: &[&str] = &["tools/vk-fault-layer"];

/// The attribute this walk searches for, assembled at runtime so that
/// **this file does not match its own search**. Spelled as one literal it
/// would, and the test would report itself forever.
///
/// Second time today a guard has matched its own source; the encoding
/// check learned it first. A guard that scans the tree is part of the
/// tree.
fn root_allow_needle() -> String {
    format!("#![{}(unsafe_code)]", "allow")
}

fn files_with_root_allow(dir: &Path, found: &mut Vec<String>, root: &Path) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|error| format!("{} unreadable: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{} unreadable: {error}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // `target` is build output and enormous. Nothing else is
            // skipped by name: the walk is rooted at the workspace's own
            // members, so it never sees anything that is not source.
            if name != "target" {
                files_with_root_allow(&path, found, root)?;
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{} unreadable: {error}", path.display()))?;
        if text.contains(&root_allow_needle()) {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            found.push(
                rel.to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
            );
        }
    }
    Ok(())
}

/// Crates whose manifest does not inherit the workspace lint table.
/// The workspace's member directories, as its own manifest lists them.
fn workspace_members(root: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|error| format!("workspace manifest unreadable: {error}"))?;
    let list = text
        .split_once("members = [")
        .ok_or_else(|| "no members list in the workspace manifest".to_string())?
        .1
        .split_once(']')
        .ok_or_else(|| "unterminated members list".to_string())?
        .0;
    let members: Vec<String> = list
        .split(',')
        .map(|member| member.trim().trim_matches('"').to_string())
        .filter(|member| !member.is_empty())
        .collect();
    if members.is_empty() {
        return Err(
            "the workspace lists no members; every check here would be vacuous".to_string(),
        );
    }
    Ok(members)
}

fn crates_not_inheriting_lints(root: &Path) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    for member in workspace_members(root)? {
        let manifest = std::fs::read_to_string(root.join(&member).join("Cargo.toml"))
            .map_err(|error| format!("{member}/Cargo.toml unreadable: {error}"))?;
        let inherits = manifest
            .split_once("[lints]")
            .is_some_and(|(_, rest)| rest.trim_start().starts_with("workspace = true"));
        if !inherits {
            found.push(member.clone());
        }
    }
    found.sort();
    Ok(found)
}

/// The unsafe surface is exactly what is recorded, by both routes.
///
/// Unsafe is granted per crate, in writing, and the grant is what a
/// reader consults to learn where unsafe lives. Nothing mechanical has
/// ever checked that the grant matches the code, and twice the grant has
/// been written incomplete. This does not read the grant — it cannot —
/// but it makes the code side impossible to change silently, which is the
/// half that moves.
#[test]
fn the_unsafe_surface_is_exactly_what_is_recorded() {
    let root = workspace_root();

    // Rooted at each workspace member rather than at the repository, so
    // the guard's scope is exactly the crates the workspace builds and it
    // needs no knowledge of anything else that may sit beside them.
    let mut allows = Vec::new();
    for member in workspace_members(&root).expect("the workspace should list its members") {
        files_with_root_allow(&root.join(&member), &mut allows, &root)
            .expect("every member directory should be walkable");
    }
    allows.sort();
    let expected: Vec<String> = ALLOWS_UNSAFE.iter().map(ToString::to_string).collect();
    assert_eq!(
        allows, expected,
        "the set of files carrying a crate-root `allow(unsafe_code)` has changed. \
         Adding one is a reviewed decision: update this list, and update the written \
         grant that records where the unsafe territory is: a file allowed here but \
         absent there makes that record under-report, which has happened twice."
    );

    let opted_out = crates_not_inheriting_lints(&root).expect("manifests should be readable");
    let expected_out: Vec<String> = OPTS_OUT_OF_WORKSPACE_LINTS
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        opted_out, expected_out,
        "the set of crates not inheriting the workspace lint table has changed. This \
         is the quieter way to carry unsafe: no allow appears anywhere, because the \
         denial was never in force. It needs the same written grant and the same \
         scrutiny."
    );
}
