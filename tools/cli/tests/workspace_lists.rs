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
