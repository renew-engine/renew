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
    "crates/render2d/tests/fault.rs",
    "crates/render3d/tests/fault.rs",
    "crates/rhi/src/lib.rs",
    "crates/rhi/tests/fault.rs",
    "crates/rhi/tests/fault_present.rs",
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

// --- Simulation crates and their lint files -----------------------------

/// Ambient sources a crate designated as simulation must not be able to
/// reach: reading them makes a run depend on when it happened, which is
/// the one thing such a crate promises not to do.
///
/// The manifest flag is a *declaration*. What actually stops the code
/// calling a clock is the crate's own lint file. Nothing compared the
/// two, so the declaration bought nothing on its own — a crate could
/// claim the property and permit the calls that break it.
/// What a `simulation = true` crate's lint file must forbid.
///
/// **All three applicable clauses, and it used to be one.** A simulation
/// crate may not read the wall clock, may not use unseeded randomness,
/// and may not depend on iteration order (fast-math has no stable-Rust
/// equivalent). Until
/// 2026-08-01 this list held only the clocks, so a crate could declare
/// itself simulation code and iterate a `HashMap` freely — which is
/// precisely the non-determinism the third clause exists to prevent.
///
/// Every declaring crate already banned the collections when that grew,
/// so it locked in a convention rather than demanding a change. **That
/// is the argument for adding an entry the moment it is free**: a list
/// that costs nothing to satisfy today costs a rewrite once a crate
/// forgets.
///
/// **The randomness clause is listed now, and the reason it was not is
/// worth keeping.** The account recorded here until 2026-08-01 was that
/// `std` ships no generator, so reaching one means taking a dependency,
/// which is separately gated. The premise is true and the inference is
/// not: what is banned is *unseeded randomness*, not generators, and
/// `RandomState::new` / `DefaultHasher::new` are stable Rust's
/// dependency-free road to operating-system entropy.
///
/// It stayed unlisted afterwards for a stated reason: every declaring
/// crate must already satisfy every entry, and the trigger recorded for
/// adding it was that a further declaring crate would not be surprised
/// by it.
///
/// **That trigger has fired**, several times over. Every crate declaring itself simulation
/// code now bans both types, so the entry is the same zero-cost lock-in
/// the collections were, and it is listed above rather than described
/// here as pending. A documented trigger that has fired and not been
/// acted on is worse than no trigger, because the prose keeps reading as
/// a plan.
///
/// No count appears in that sentence on purpose: one manifest quotes the
/// phrase in a comment while declaring the opposite, so a grep over them
/// answers a different question than the one being asked. The test reads
/// the manifests either way.
const BANNED_IN_SIMULATION: &[&str] = &[
    "std::time::Instant::now",
    "std::time::SystemTime::now",
    "std::collections::HashMap",
    "std::collections::HashSet",
    "std::hash::RandomState",
    "std::collections::hash_map::DefaultHasher",
];

/// Crates whose manifest sets `simulation = true`, with the text of the
/// lint file sitting beside it.
/// Every path a lint file actually bans, read as structure rather than text.
///
/// The check over these used to be `file.contains(banned)` across the raw
/// bytes, which passes on a commented-out entry and on a banned path
/// quoted inside another entry's `reason` prose. No file has that shape
/// today, so the guard was passing for the right reason -- but it was one
/// explanatory sentence away from passing for the wrong one, and the
/// reasons in these files do quote paths at each other.
///
/// Comment lines are dropped first, then each `path = "..."` value is
/// taken. That is the only position clippy reads, so it is the only
/// position this should accept.
fn declared_paths(lints: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in lints.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("path = \"") {
            rest = &rest[at + "path = \"".len()..];
            let Some(end) = rest.find('"') else { break };
            paths.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    paths
}

fn simulation_crates(root: &Path) -> Result<Vec<(String, Vec<String>)>, String> {
    let mut found = Vec::new();
    for member in workspace_members(root)? {
        let dir = root.join(&member);
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml"))
            .map_err(|error| format!("{member}/Cargo.toml unreadable: {error}"))?;
        if !manifest
            .lines()
            .any(|line| line.trim() == "simulation = true")
        {
            continue;
        }
        let lints = std::fs::read_to_string(dir.join("clippy.toml")).map_err(|error| {
            format!("{member} declares simulation but has no readable clippy.toml: {error}")
        })?;
        found.push((member, declared_paths(&lints)));
    }
    found.sort();
    Ok(found)
}

/// A crate that calls itself simulation must forbid reading a clock.
///
/// The designation exists so that a run is reproducible from its inputs.
/// A wall-clock read breaks that silently: the code compiles, the tests
/// pass, and two runs simply disagree. The lint file is what prevents
/// it, and until now nothing checked that a crate claiming the property
/// had the lint.
///
/// This does not prove such a crate is reproducible — no test here
/// could. It proves the one mechanical guard it relies on is present.
#[test]
fn every_simulation_crate_forbids_clocks_unordered_collections_and_entropy() {
    let root = workspace_root();
    let crates = simulation_crates(&root).expect("manifests and lint files should be readable");

    assert!(
        !crates.is_empty(),
        "no crate declares `simulation = true`, so this test would pass vacuously"
    );

    let mut faults = Vec::new();
    for (name, paths) in &crates {
        for banned in BANNED_IN_SIMULATION {
            if !paths.iter().any(|declared| declared == banned) {
                faults.push(format!("{name} does not disallow `{banned}`"));
            }
        }
    }
    assert!(
        faults.is_empty(),
        "these crates declare themselves simulation code but their lint files permit \
         a source of non-determinism, so the declaration is enforced by nothing: {faults:?}"
    );
}

/// Words a `reason` may not be followed by a number after. A reason that
/// cites a position rather than naming the code goes stale the first time
/// the file grows, and nothing notices.
const POSITION_WORDS: &[&str] = &["line", "lines"];

/// The number a `reason` cites, if it cites one.
///
/// Deliberately not a regex: the crate has no regex dependency and this
/// needs no backtracking. Scans a lowercased copy so `Line 12` is caught
/// as readily as `line 12`.
fn cited_position(reason: &str) -> Option<String> {
    let lowered = reason.to_ascii_lowercase();
    for word in POSITION_WORDS {
        let mut from = 0;
        while let Some(offset) = lowered[from..].find(word) {
            let start = from + offset;
            let after = start + word.len();
            let rest = &lowered[after..];
            let digits: String = rest
                .chars()
                .skip(1)
                .take_while(char::is_ascii_digit)
                .collect();
            // Both boundaries are checked. Without the left one `outlines
            // 7` matches on its tail; without the right one `linear` does.
            // Whole words only, and a single space before the number.
            let word_start = lowered[..start]
                .chars()
                .next_back()
                .is_none_or(|previous| !previous.is_alphanumeric());
            if word_start && rest.starts_with(' ') && !digits.is_empty() {
                return Some(format!("{word} {digits}"));
            }
            from = after;
        }
    }

    // A parenthesised group made of nothing but numbers and separators.
    // `(1321-1325)` carries exactly the information `lines 1321-1325`
    // does and goes stale exactly as fast, while matching nothing above
    // — the check was reading a spelling rather than a citation.
    //
    // The shape is narrow on purpose. Reasons legitimately talk about
    // widths, bounds and byte counts, so a run of digits anywhere in
    // the prose is not evidence of anything — an earlier, looser
    // version of this check flagged four honest sentences about packet
    // fields. This fires only when a reader put numbers in brackets
    // *instead of* naming the code, which is the habit the header
    // forbids.
    let mut rest = reason;
    while let Some(open) = rest.find('(') {
        let inside = &rest[open + 1..];
        let Some(close) = inside.find(')') else { break };
        let group = &inside[..close];
        let separators_only = !group.is_empty()
            && group
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '-' | ',' | ' '));
        let long_enough = group
            .split(|c: char| !c.is_ascii_digit())
            .any(|run| run.len() >= 3);
        if separators_only && long_enough {
            return Some(format!("the bracketed positions `({group})`"));
        }
        rest = &inside[close + 1..];
    }

    None
}

/// No exemption explains itself by citing a line number.
///
/// The gate reads each entry's `lines` array and never reads its `reason`,
/// so a number written into that prose has no guard at all. The array gets
/// corrected whenever the file moves, because the ratchet fails loudly in
/// both directions if it does not — and the sentence beside it silently
/// does not, which is precisely why the two drift apart.
///
/// This is not hypothetical. One reason said "line 390" long after the arm
/// it described had moved to 483, by which time 390 was error handling in
/// an unrelated function. A note telling a reader where to look, pointing
/// at the wrong place, is worse than no note.
///
/// The remedy the rule enforces is to name the code — "the keyboard arm" —
/// which cannot go stale when the file grows.
#[test]
fn no_coverage_exemption_explains_itself_with_a_line_number() {
    let root = workspace_root();
    let manifest = root.join(renew_cli::coverage::MANIFEST);
    let text = std::fs::read_to_string(&manifest).expect("the exemption manifest should be read");
    let exemptions = renew_cli::coverage::parse_manifest(&text).expect("it should parse");

    assert!(
        !exemptions.is_empty(),
        "no exemptions parsed, so this test would pass vacuously"
    );

    let mut faults = Vec::new();
    for exemption in &exemptions {
        if let Some(cited) = cited_position(&exemption.reason) {
            faults.push(format!(
                "{} {:?} cites `{cited}`",
                exemption.file, exemption.lines
            ));
        }
    }
    assert!(
        faults.is_empty(),
        "these exemption reasons cite a position instead of naming the code, and nothing \
         updates them when the file moves: {faults:?}"
    );
}

/// The matcher's own boundaries, because a guard whose detector is wrong
/// fails in whichever direction nobody checked. Both were wrong in the
/// first draft: without the right-hand boundary `linear` matched, and
/// without the left-hand one `outlines` did.
#[test]
fn the_position_matcher_reads_whole_words_only() {
    let cited = |text: &str| cited_position(text);

    assert_eq!(cited("cover line 390, which then"), Some("line 390".into()));
    assert_eq!(cited("Line 12 of the header"), Some("line 12".into()));
    assert_eq!(cited("lines 3 and 4 of the table"), Some("lines 3".into()));

    assert!(
        cited("the keyboard arm").is_none(),
        "names code, not a place"
    );
    assert!(cited("outlines 7 cases").is_none(), "left boundary");
    assert!(cited("a linear 3-way split").is_none(), "right boundary");
    assert!(cited("a multi-line refusal").is_none(), "no number follows");
    assert!(cited("the line above").is_none(), "no number follows");
}

/// The lints every engine crate is expected to carry itself.
///
/// Held as a list rather than one string so the message can say which of
/// them is missing, and so adding a third is a one-line change.
const ENGINE_CRATE_DENIES: &[&str] = &["clippy::print_stdout", "clippy::print_stderr"];

/// Every crate under the engine module root denies printing, at the crate
/// root, in its own source.
///
/// The workspace lint table cannot carry this: two non-engine crates —
/// the CLI and the samples — print by design, and a workspace-wide deny
/// would need an `allow` escape in each of them. So it is per crate, by
/// hand, which is exactly the arrangement that rots.
///
/// **And it did.** The convention held for nine crates and then three
/// landed on one day without it. Nothing noticed: the structure check
/// requires each crate's `clippy.toml` to exist but says nothing about
/// its contents or about crate-root attributes, and clippy cannot warn
/// about a deny that was never written. This was the predicted failure —
/// the convention holds until the crate that forgets, and nothing says
/// so — and it came true three times before this test existed.
#[test]
fn every_engine_crate_denies_printing_at_its_root() {
    let root = workspace_root();
    let engine = root.join("crates");
    let mut checked = Vec::new();
    let mut faults = Vec::new();

    let mut pending = vec![engine];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) != Some("Cargo.toml") {
                continue;
            }
            let Some(crate_dir) = path.parent() else {
                continue;
            };
            let lib = crate_dir.join("src").join("lib.rs");
            let shown = crate_dir
                .strip_prefix(&root)
                .unwrap_or(crate_dir)
                .display()
                .to_string();
            let Ok(source) = std::fs::read_to_string(&lib) else {
                // A crate under `crates/` with no lib.rs is a shape this
                // rule has no opinion about; reported rather than skipped
                // so a binary-only engine crate is a decision someone
                // makes rather than a silent exemption.
                faults.push(format!("{shown} has no src/lib.rs to carry the denies"));
                continue;
            };
            checked.push(shown.clone());
            for deny in ENGINE_CRATE_DENIES {
                if !source.contains(deny) {
                    faults.push(format!("{shown} does not deny `{deny}` at its crate root"));
                }
            }
        }
    }

    assert!(
        checked.len() >= 9,
        "only {} engine crates were found; the walk is not reaching the tree and this would pass \
         vacuously",
        checked.len()
    );
    assert!(
        faults.is_empty(),
        "these engine crates are missing a crate-root print deny, which the workspace lint table \
         cannot supply because the CLI and the samples print by design: {faults:?}"
    );
}

/// Every flag the parser accepts appears in the usage text.
///
/// **There was already a test called `usage_lists_every_command_and_every
/// _option`, and five flags were undocumented anyway.** It derived the
/// command half from `Command::ALL`, so that half could not rot — and
/// hardcoded the option half as three strings. The name made a claim
/// about twelve flags while the body checked a quarter of them, which is
/// the failure mode this file exists for: a test that keeps passing while
/// measuring less than it says.
///
/// `--pack`, `--from` and `--verify` were the casualties, all three
/// belonging to the two most recently added subcommands, and `--pack` is
/// *required* by both — so the tool told a user it needed a flag that its
/// own help never mentioned.
///
/// Derived from the parser's own match arms rather than a list kept
/// beside them. A source scan is cruder than a shared constant, but it
/// cannot be updated in one place and forgotten in the other, which is
/// exactly what happened.
#[test]
fn the_usage_text_documents_every_flag_the_parser_accepts() {
    let root = workspace_root();
    let source = std::fs::read_to_string(root.join("tools/cli/src/cli.rs"))
        .expect("the parser source is readable");
    // Only the parser, never its test module: test fixtures name sample
    // flags like `--frames` that renew passes through and does not own.
    let parser = source
        .split_once(
            "
mod tests",
        )
        .map_or(source.as_str(), |(before, _)| before);

    let mut flags: Vec<String> = Vec::new();
    let mut rest = parser;
    while let Some(at) = rest.find("\"--") {
        rest = &rest[at + 1..];
        if let Some(end) = rest.find('"') {
            let flag = &rest[..end];
            if flag.len() > 2
                && flag[2..]
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-')
                && !flags.iter().any(|seen| seen == flag)
            {
                flags.push(flag.to_string());
            }
        }
    }
    assert!(
        flags.len() >= 8,
        "the scan found only {flags:?}, which means it stopped matching the source"
    );

    let text = renew_cli::cli::usage();
    let missing: Vec<&String> = flags
        .iter()
        .filter(|f| !text.contains(f.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "the parser accepts {missing:?} and the usage text never mentions them;          a flag a user cannot discover may as well not exist"
    );
}

/// No Vulkan type reaches `renew-rhi`'s public API.
///
/// **The rule is documented intent with no enforcement, which is why this
/// exists.** `renew-rhi` wraps `ash` so that nothing above it names a
/// Vulkan type; the whole vocabulary argument for crate-owned enums —
/// `TargetFormat` listing two variants rather than re-exporting
/// `vk::Format` — rests on that boundary holding. Until now it held by
/// review alone, and the resource model is about to add several new
/// enums that each present the same temptation.
///
/// **A naive scan does not work and the shape of its failure is the
/// point.** Grepping the crate for `pub fn … vk::` finds two hits today,
/// `alloc::callbacks` and `debug::messenger_info` — both `pub` inside
/// private modules, neither reachable from outside. Every module in
/// `lib.rs` is private, so an item is public only if its owning type is
/// re-exported. The check therefore derives the exported set from the
/// `pub use` lines and looks only inside those types' own definitions
/// and impl blocks.
#[test]
fn no_vulkan_type_appears_in_the_rhi_public_api() {
    let root = workspace_root();
    let lib = std::fs::read_to_string(root.join("crates/rhi/src/lib.rs"))
        .expect("the rhi crate root is readable");

    let mut exported: Vec<String> = Vec::new();
    for line in lib.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub use ") else {
            continue;
        };
        let rest = rest.trim_end_matches(';');
        let names = match (rest.find('{'), rest.rfind('}')) {
            (Some(open), Some(close)) if open < close => rest[open + 1..close].to_string(),
            _ => rest.rsplit("::").next().unwrap_or_default().to_string(),
        };
        for name in names.split(',') {
            let name = name.trim();
            if !name.is_empty() && name.chars().next().is_some_and(char::is_uppercase) {
                exported.push(name.to_string());
            }
        }
    }
    assert!(
        exported.len() >= 10,
        "only {exported:?} parsed out of lib.rs; the scan stopped matching the re-export form"
    );

    let vulkan = |text: &str| text.contains("ash::") || text.contains("vk::");
    let mut faults = Vec::new();
    let mut files = Vec::new();
    collect_rust_files(&root.join("crates/rhi/src"), &mut files).expect("rhi sources readable");

    for path in files {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let shown = path.display().to_string();
        let mut owner: Option<String> = None;
        let mut depth = 0usize;
        for line in text.lines() {
            let trimmed = line.trim();
            if owner.is_none() {
                owner = exported
                    .iter()
                    .find(|name| {
                        let n = name.as_str();
                        trimmed.starts_with(&format!("impl {n} "))
                            || trimmed.starts_with(&format!("impl {n}<"))
                            || trimmed.starts_with(&format!("impl {n} {{"))
                            || trimmed.ends_with(&format!("for {n} {{"))
                            || trimmed.starts_with(&format!("pub struct {n}"))
                            || trimmed.starts_with(&format!("pub enum {n}"))
                    })
                    .cloned();
            }
            if owner.is_some() {
                depth += line.matches('{').count();
                depth = depth.saturating_sub(line.matches('}').count());
                let public_item = trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("pub const fn ")
                    || (trimmed.starts_with("pub ") && trimmed.contains(':'));
                if public_item && vulkan(trimmed) {
                    faults.push(format!(
                        "{shown}: `{}` exposes a Vulkan type on the public type `{}`",
                        trimmed.trim_end_matches('{').trim(),
                        owner.clone().unwrap_or_default()
                    ));
                }
                if depth == 0 {
                    owner = None;
                }
            }
        }
    }
    assert!(
        faults.is_empty(),
        "the rhi crate wraps Vulkan so nothing above it names a Vulkan type; these leak it: {faults:#?}"
    );
}

/// Every `.rs` under a directory, recursively.
fn collect_rust_files(dir: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect_rust_files(&path, found)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    Ok(())
}

/// Every committed golden image, paired with its provenance sidecar.
///
/// Walks the tree rather than reading a list, because a list of goldens
/// would be exactly the hand-maintained second copy this file exists to
/// distrust.
fn goldens_with_provenance(dir: &Path, found: &mut Vec<(PathBuf, PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("entry: {e}"))?.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name == "target" || name == ".git" {
                continue;
            }
            goldens_with_provenance(&path, found)?;
        } else if path.extension().is_some_and(|e| e == "rgba") {
            let sidecar = path.with_extension("provenance.txt");
            if sidecar.exists() {
                found.push((path, sidecar));
            }
        }
    }
    Ok(())
}

/// The hash a provenance sidecar records for its image.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn every_golden_matches_the_hash_its_provenance_records() {
    // The ritual is: render a candidate, have a human look at it, rename
    // it, and commit it beside a sidecar recording what was approved and
    // what rendered it. The sidecar is the only record that a human ever
    // saw those bytes.
    //
    // This has already failed once. A golden was refreshed with "the
    // bytes the comparison produces" and its sidecar was left behind, so
    // the file on disk was a capture nobody had approved while the
    // sidecar beside it described a different image entirely. Nothing
    // compared them, so nothing said so — and the mismatch was found
    // only because an unrelated branch went red on a different runner.
    //
    // A sidecar that disagrees with its image means one of two things,
    // and both are defects: the image was replaced without the ritual,
    // or the ritual ran and the record was not updated.
    let root = workspace_root();
    let mut goldens = Vec::new();
    goldens_with_provenance(&root, &mut goldens).expect("walk the tree for goldens");
    assert!(
        goldens.len() >= 5,
        "found {} goldens with sidecars, which is fewer than are known to exist — \
         the walk is broken and this test would pass vacuously",
        goldens.len()
    );

    let mut wrong = Vec::new();
    for (image, sidecar) in &goldens {
        let bytes = std::fs::read(image).expect("read the golden");
        let record = std::fs::read_to_string(sidecar).expect("read the sidecar");
        let claimed = record
            .lines()
            .find_map(|line| line.split_once("fnv1a-64 of the pixel bytes:"))
            .map(|(_, value)| value.trim().to_owned());
        let actual = format!("0x{:016x}", fnv1a(&bytes));
        match claimed {
            Some(claimed) if claimed == actual => {}
            Some(claimed) => wrong.push(format!(
                "{}: the file hashes to {actual} but its sidecar records {claimed}",
                image.display()
            )),
            None => wrong.push(format!(
                "{}: its sidecar records no hash at all",
                sidecar.display()
            )),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Engine crates, as cargo sees them: workspace members named `renew-*`
/// that are neither samples nor tools.
fn engine_crates(root: &Path) -> Result<Vec<String>, String> {
    let output = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("run cargo metadata: {e}"))?;
    let text = String::from_utf8(output.stdout).map_err(|e| format!("metadata utf8: {e}"))?;
    let Value::Object(root_object) =
        json::parse(&text).map_err(|e| format!("metadata json: {e}"))?
    else {
        return Err("metadata is not an object".into());
    };
    let Some((_, Value::Array(packages))) = root_object.iter().find(|(key, _)| key == "packages")
    else {
        return Err("metadata has no packages array".into());
    };
    let mut names = Vec::new();
    for package in packages {
        let Value::Object(fields) = package else {
            continue;
        };
        let Some((_, Value::String(name))) = fields.iter().find(|(key, _)| key == "name") else {
            continue;
        };
        let engine = name.starts_with("renew-")
            && !name.starts_with("renew-sample-")
            && name != "renew-cli"
            && name != "renew-bench";
        if engine {
            names.push(name.clone());
        }
    }
    names.sort();
    Ok(names)
}

#[test]
fn the_readme_counts_what_the_workspace_actually_holds() {
    // The front page states three numbers a reader is invited to trust:
    // how many removability configurations CI runs, which one builds the
    // minimal core, and how many engine crates there are. The first of
    // those is the row the repository nominates as its evidence that
    // every optional crate can be removed — its entire function is to let
    // a reader audit that claim without reading the workflow.
    //
    // All three had drifted, and one had drifted twice: a crate landed in
    // #217 without a table row and nothing said so, which is exactly the
    // failure mode this file exists to distrust — a document that keeps
    // reading plausibly while measuring nothing.
    let root = workspace_root();

    let workflow = std::fs::read_to_string(root.join(".github/workflows/ci.yml"))
        .expect("read the CI workflow");
    let cells = workflow
        .lines()
        .filter(|line| line.contains("sel=\"--workspace --exclude"))
        .count();

    let crates = engine_crates(&root).expect("list the engine crates");
    let readme = std::fs::read_to_string(root.join("README.md")).expect("read the README");

    // Written out, because the README writes them out.
    let spelled = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
        "twenty-one",
        "twenty-two",
        "twenty-three",
        "twenty-four",
        "twenty-five",
        "twenty-six",
        "twenty-seven",
        "twenty-eight",
        "twenty-nine",
        "thirty",
        "thirty-one",
        "thirty-two",
        "thirty-three",
        "thirty-four",
        "thirty-five",
    ];
    let word = |n: usize| -> String {
        spelled
            .get(n)
            .map_or_else(|| n.to_string(), |s| (*s).to_owned())
    };
    let capitalised = |n: usize| -> String {
        let w = word(n);
        let mut chars = w.chars();
        chars.next().map_or(w.clone(), |first| {
            first.to_uppercase().collect::<String>() + chars.as_str()
        })
    };

    let mut wrong = Vec::new();
    let cell_phrase = format!("*one crate at a time*: {}", word(cells));
    if !readme.contains(&cell_phrase) {
        wrong.push(format!(
            "the README does not say there are {cells} removability configurations \
             (expected the phrase {cell_phrase:?})"
        ));
    }
    let core_phrase = format!("A {} builds the minimal core alone", ordinal(cells + 1));
    if !readme.contains(&core_phrase) {
        wrong.push(format!(
            "the minimal-core cell is step {} of the removability job \
             (expected the phrase {core_phrase:?})",
            cells + 1
        ));
    }
    let count_phrase = format!("{} engine crates", capitalised(crates.len()));
    if !readme.contains(&count_phrase) {
        wrong.push(format!(
            "cargo reports {} engine crates (expected the phrase {count_phrase:?})",
            crates.len()
        ));
    }
    // **The name, not the row's shape.** The table used to be one row per
    // crate and is now grouped by what each crate is for, so a check
    // pinned to `**`renew-x`**` was pinning a layout rather than the
    // guarantee. The guarantee is that no crate is silently absent from
    // the page, and a crate is named there by its short form, since the
    // `renew-` prefix is on every one of them and carries nothing.
    for name in &crates {
        let short = name.strip_prefix("renew-").unwrap_or(name);
        if !readme.contains(&format!("`{short}`")) {
            wrong.push(format!(
                "{name} is named nowhere in the README's module table"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The ordinal the README spells, for the cell that follows the removals.
fn ordinal(n: usize) -> String {
    match n {
        21 => "twenty-first".to_owned(),
        22 => "twenty-second".to_owned(),
        23 => "twenty-third".to_owned(),
        24 => "twenty-fourth".to_owned(),
        25 => "twenty-fifth".to_owned(),
        26 => "twenty-sixth".to_owned(),
        27 => "twenty-seventh".to_owned(),
        28 => "twenty-eighth".to_owned(),
        other => format!("{other}th"),
    }
}

/// The part of a line to read as prose.
///
/// In Rust that is whatever follows the first `//`, because the code
/// half holds names that look like references and are not — the audio
/// module spells its PCM formats with a capital I and a bit width.
/// Everywhere else the whole line is prose: Markdown has no comment
/// syntax, and a reference inside a configuration value is read by
/// everyone that value speaks to.
///
/// Cuts at the first `//` wherever it appears, including inside a string
/// literal.
fn comment_part(line: &str, rust: bool) -> &str {
    if !rust {
        // **Outside Rust the whole line is prose.** Markdown has no
        // comment syntax, and a reference in a `reason = "..."` value is
        // read by anyone the lint speaks to — one of the references this
        // guard was written for lived in exactly that position, and
        // reading only what follows `//` would exclude every Markdown
        // paragraph and every configuration value in the tree.
        return line;
    }
    line.find("//").and_then(|at| line.get(at..)).unwrap_or("")
}

/// The half of a line the compiler reads: everything before the first
/// `//`, with the same string-literal caveat.
fn code_part(line: &str) -> &str {
    line.find("//")
        .and_then(|at| line.get(..at))
        .unwrap_or(line)
}

/// Code with its whitespace removed and its raw-identifier marks
/// dropped.
///
/// `r#net` and `net` are the same module to the compiler, so they are
/// the same module here.
fn squashed(code: &str) -> String {
    code.replace("r#", "")
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// Does `code` name `net` inside a group opened by `opening`?
///
/// Walks to the matching brace rather than the first one, so a nested
/// group does not end the region early, and splits on every brace and
/// comma so depth stops mattering once the region is right. `netas`
/// catches `net as n`, whose spacing the caller has removed.
///
/// `code` must already be [`squashed`].
fn names_net_in_group(code: &str, opening: &str) -> bool {
    for tail in code.split(opening).skip(1) {
        let mut depth = 1_usize;
        let mut region = String::new();
        for character in tail.chars() {
            match character {
                '{' => depth = depth.saturating_add(1),
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            region.push(character);
        }
        if region
            .split(['{', '}', ','])
            .any(|part| part == "net" || part.starts_with("net::") || part.starts_with("netas"))
        {
            return true;
        }
    }
    false
}

/// Every file this repository contains, as `(path, contents)`.
///
/// **Asked of git rather than of the filesystem.** A working tree's root
/// can hold files that are no part of the repository — reading those
/// makes a scan pass in one checkout and fail in another, and it cannot
/// tell a file it should never have opened from a fault.
///
/// Fails rather than shrinks: a listed file that will not read, and a
/// listing too small to be the tree, are both reported.
#[allow(
    clippy::expect_used,
    reason = "a guard that cannot enumerate the repository must fail loudly, and the message is the report"
)]
fn tracked_files(root: &std::path::Path, extensions: &[&str]) -> Vec<(String, String)> {
    let listing = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()
        .expect("`git ls-files` must run; this guard cannot otherwise tell what the tree holds");
    assert!(
        listing.status.success(),
        "`git ls-files` failed, so this guard does not know what the repository holds"
    );
    let text = String::from_utf8_lossy(&listing.stdout);

    let mut found = Vec::new();
    let mut unreadable = Vec::new();
    for name in text.split('\0').filter(|entry| !entry.is_empty()) {
        let path = root.join(name);
        let wanted = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                extensions
                    .iter()
                    .any(|wanted| wanted.eq_ignore_ascii_case(extension))
            });
        if !wanted {
            continue;
        }
        // Paths stay as git prints them, separators and all, so nothing
        // has to convert and nothing differs by platform.
        //
        // A listed file that will not read is reported, not skipped. It
        // is gone from disk, or not UTF-8, or named in bytes this could
        // not reproduce — and in every case the scan covered less than
        // it was asked to, which the caller must hear about.
        match std::fs::read_to_string(&path) {
            Ok(source) => found.push((name.to_owned(), source)),
            Err(error) => unreadable.push(format!("{name}: {error}")),
        }
    }
    assert!(
        unreadable.is_empty(),
        "git lists these but they could not be read, so the scan covered less than it claims:\n{}",
        unreadable.join("\n")
    );
    assert!(
        found.len() > 50,
        "git listed only {} scannable files, so this guard is looking at nothing",
        found.len()
    );
    found
}

/// An upper-case `I` followed by one or two digits, standing alone.
///
/// That is the shape of a reference naming a numbered rule. It is not
/// the shape of `Ipv4Addr` or `AXIS_I2`, both of which have an
/// alphanumeric neighbour, so both are skipped.
fn numbered_rule_citation(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    for index in 0..bytes.len() {
        if bytes.get(index) != Some(&b'I') {
            continue;
        }
        let before = index
            .checked_sub(1)
            .and_then(|earlier| bytes.get(earlier))
            .copied();
        if before.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
            continue;
        }
        let mut digits = 0;
        while index
            .checked_add(1 + digits)
            .and_then(|at| bytes.get(at))
            .is_some_and(u8::is_ascii_digit)
        {
            digits += 1;
        }
        if digits == 0 || digits > 2 {
            continue;
        }
        let after = index
            .checked_add(1 + digits)
            .and_then(|at| bytes.get(at))
            .copied();
        if after.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
            continue;
        }
        return index
            .checked_add(1 + digits)
            .and_then(|end| line.get(index..end))
            .map(str::to_owned);
    }
    None
}

/// A parenthesised rule reference: a bracketed letter and number.
///
/// The brackets are what separate a reference from a label. This tree
/// writes bare letter-and-number for its own things — fault scenarios
/// `D1` through `T18`, the Vulkan format `D32` — and none of them is
/// ever bracketed, so scanning bare ones would report followable labels
/// as unfollowable references.
fn parenthesised_rule_citation(line: &str) -> Option<String> {
    for (start, _) in line.match_indices('(') {
        let rest = line.get(start.saturating_add(1)..).unwrap_or("");
        let mut characters = rest.chars();
        let Some(letter) = characters.next() else {
            continue;
        };
        if !matches!(letter, 'D' | 'I' | 'T' | 'M') {
            continue;
        }
        let digits: String = characters.take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            continue;
        }
        if rest
            .chars()
            .nth(digits.len().saturating_add(1))
            .is_some_and(|next| next == ')')
        {
            return Some(format!("({letter}{digits})"));
        }
    }
    None
}

/// No source file may cite material this repository does not contain.
///
/// **A comment naming a document a reader cannot open is worse than a
/// comment naming nothing.** It reads as sourced. A reader who goes
/// looking finds no such file and cannot tell whether the evidence is
/// missing or they are.
///
/// A sweep by eye searches for the notations it already knows and
/// reports the tree clean when it runs out of them, which is why this is
/// a test — and why the needle list is the thing to extend when a new
/// spelling turns up.
///
/// **The needles are assembled at run time**, halves joined rather than
/// written whole, so the file defining them is scanned like every other.
///
/// What it does not catch: a reference inside an identifier, since the
/// rule scanners read only prose; a bare letter-and-number, for the
/// reason [`parenthesised_rule_citation`] gives; and any phrasing that
/// avoids the literal needles, including a different case of one.
#[test]
fn no_source_cites_material_this_repository_does_not_contain() {
    let root = workspace_root();
    let join = |left: &str, right: &str| format!("{left}{right}");
    let needles = [
        join("DEBT", "-"),
        join("A", "DR"),
        join("mile", "stone"),
        join("consti", "tution"),
        join("STATE", ".md"),
        join("ROADMAP", ".md"),
        join("CLAUDE", ".md"),
        join("DEVELOPMENT", ".md"),
        join("DEBT", ".md"),
        join("docs", "/"),
        join("spikes", "/"),
        join("threading", ".md"),
        join("targets", ".md"),
        join("lifecycle", ".md"),
        join("design ", "note"),
        join("decision ", "journal"),
        join("decision ", "record"),
        join("review ", "pass"),
    ];

    let files = tracked_files(&root, &["rs", "toml", "yml", "yaml", "md", "txt"]);

    let mut faults = Vec::new();
    for (shown, source) in &files {
        let rust = std::path::Path::new(shown)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"));

        // **The prose of a file, joined, before it is read line by
        // line.** A citation is a phrase and a wrap is a line break, so
        // a two-word needle straddling one is invisible to a per-line
        // scan — which is how a live citation of a document sat in this
        // tree while this guard reported clean. Joined first, then
        // scanned; the per-line pass below survives only to give a line
        // number when the phrase happens to fit on one.
        let flowed = source
            .lines()
            .map(|line| comment_part(line, rust))
            // The marker goes too. Joining `// the design` to
            // `// note's` with the slashes still on puts `//` between
            // the two words and the phrase never matches — which is the
            // shape of the miss this pass exists to close, and it went
            // wrong that way once before it went right.
            .map(|prose| prose.trim_start().trim_start_matches(['/', '!', '#', ' ']))
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for needle in &needles {
            if flowed.contains(needle.as_str()) && !source.contains(needle.as_str()) {
                faults.push(format!("{shown} names `{needle}` across a line break"));
            }
        }

        for (offset, line) in source.lines().enumerate() {
            let number = offset.saturating_add(1);
            for needle in &needles {
                if line.contains(needle.as_str()) {
                    faults.push(format!("{shown}:{number} names `{needle}`"));
                }
            }
            // Only the comment half of a line. A reference is prose; the
            // collisions are code — the audio module names its PCM
            // sample formats with a capital I and a bit width, and a
            // scanner that reads those as references is a scanner
            // nobody keeps.
            let prose = comment_part(line, rust);
            if let Some(found) = numbered_rule_citation(prose) {
                faults.push(format!("{shown}:{number} cites the rule `{found}`"));
            }
            if let Some(found) = parenthesised_rule_citation(prose) {
                faults.push(format!("{shown}:{number} cites the rule `{found}`"));
            }
        }
    }

    assert!(
        faults.is_empty(),
        "these cite material that is not in this repository, so a reader cannot follow \
         them. Say the thing itself, or name a file that is actually here:\n{}",
        faults.join("\n")
    );
}

/// Only the platform's socket module may name the standard networking
/// types.
///
/// **A zoning rule that was written down and enforced by nothing.** One
/// module owns the socket so everything above it stays testable without
/// one, and so a crate carrying a determinism obligation cannot reach a
/// wire by accident. Any crate could have added the import with every
/// gate staying green.
///
/// **A lexical check, not a proof, and the difference matters.** It
/// reads text; Rust has more ways to name a path than text can
/// enumerate, and each round of use has turned up another. What it
/// catches is the spellings anyone actually writes: `std::net` and
/// `core::net`; a brace group naming `net`, nested or not, on one line
/// or split as the formatter splits a long import; `net as something`;
/// raw identifiers; and `use std as` / `extern crate std as`, refused
/// outright because no file here needs a second name for the standard
/// library.
///
/// What it does not catch is unbounded, and these are the known ones: a
/// crate-root rename spelled `use std::{self as s}`; an import on a line
/// whose earlier text contains `//` inside a string literal, which the
/// comment split truncates; and any file whose extension is outside the
/// scanned set. The graph rule in the structure check is the load-bearing
/// half — it denies a simulation crate any dependency path here at all —
/// and this is a speed bump in front of it.
///
/// Comments are exempt, since the rule is discussed in them, which also
/// means a string literal quoting the path is a false positive.
#[test]
fn only_the_platform_socket_module_names_the_standard_network_types() {
    let root = workspace_root();
    let allowed = "crates/core/platform/src/net.rs";

    let files = tracked_files(&root, &["rs"]);
    assert!(
        files.iter().any(|(shown, _)| shown == allowed),
        "the one module allowed to name them was not walked, so this guards nothing"
    );

    let direct = [
        format!("{}{}", "std::", "net"),
        format!("{}{}", "core::", "net"),
    ];
    let groups = [
        format!("{}{}", "std::", "{"),
        format!("{}{}", "core::", "{"),
    ];
    // Renaming the crate root reaches the socket without either pair
    // ever appearing: `use std as s` then `s::net::UdpSocket`. Tracking
    // the alias would mean parsing; refusing the rename costs nothing,
    // because no file here has a reason to give the standard library a
    // second name.
    let renames = [
        format!("{}{}", "usestd", "as"),
        format!("{}{}", "usecore", "as"),
        format!("{}{}", "externcratestd", "as"),
        format!("{}{}", "externcratecore", "as"),
    ];

    let mut faults = Vec::new();
    for (shown, source) in &files {
        if shown == allowed {
            continue;
        }

        // The direct spellings, per line, so the report can point at one.
        for (offset, line) in source.lines().enumerate() {
            let code = squashed(code_part(line));
            let named = direct.iter().any(|needle| code.contains(needle.as_str()))
                || renames.iter().any(|needle| code.contains(needle.as_str()));
            if named {
                faults.push(format!("{shown}:{}", offset.saturating_add(1)));
            }
        }

        // **Brace groups against the whole file, not line by line.** The
        // formatter splits an import list wider than the line limit, so
        // `use std::{` and `net::UdpSocket` land on different lines and a
        // per-line scan sees unrelated text. The formatter runs as a
        // gate, so that is where long imports live. Squashing the file
        // costs the line number, which is why the direct spellings keep
        // their own pass above.
        let code = squashed(&source.lines().map(code_part).collect::<Vec<_>>().join("\n"));
        let grouped = groups
            .iter()
            .any(|opening| names_net_in_group(&code, opening));
        if grouped {
            faults.push(format!("{shown} (in a brace group)"));
        }
    }

    assert!(
        faults.is_empty(),
        "the socket belongs to one module and these reach around it:\n{}",
        faults.join("\n")
    );
}
