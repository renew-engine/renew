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
/// **Two of I3's three applicable clauses, and it used to be one.** I3
/// bans wall-clock reads, unseeded randomness, and iteration-order
/// dependent state (fast-math has no stable-Rust equivalent). Until
/// 2026-08-01 this list held only the clocks, so a crate could declare
/// itself simulation code and iterate a `HashMap` freely — which is
/// precisely the non-determinism the third clause exists to prevent.
///
/// All four declaring crates already banned the collections when this
/// grew, so it locks in a convention rather than demanding a change.
/// **That is the argument for adding it now**: a list that costs nothing
/// to satisfy today costs a rewrite once a crate forgets.
///
/// The randomness clause stays unlisted deliberately — but **the reason
/// recorded here until 2026-08-01 was wrong, and the correction is worth
/// keeping.** It said `std` ships no generator, so reaching one means
/// taking a dependency caught at the `docs/deps/` gate. The premise is
/// true and the inference is not: what is banned is *unseeded
/// randomness*, not generators, and `RandomState::new` /
/// `DefaultHasher::new` are stable Rust's dependency-free road to
/// operating-system entropy.
///
/// It stays unlisted because adding it here is **not** the zero-cost
/// lock-in the collections were. Every declaring crate must already
/// satisfy every entry, and until 2026-08-01 only `renew-rng` banned
/// those two types — the other three carried the marker with no
/// randomness guard at all. That gap is now closed crate-by-crate, which
/// is the prerequisite for listing it here rather than an alternative to
/// it. Add it once a fifth crate would not be surprised by it.
const BANNED_IN_SIMULATION: &[&str] = &[
    "std::time::Instant::now",
    "std::time::SystemTime::now",
    "std::collections::HashMap",
    "std::collections::HashSet",
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
fn every_simulation_crate_forbids_the_nondeterminism_i3_names() {
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
    // those is the row the repository nominates as its evidence for I12,
    // an INVARIANT — its entire function is to let a session audit the
    // invariant without reading the workflow.
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
    for name in &crates {
        if !readme.contains(&format!("**`{name}`**")) {
            wrong.push(format!("{name} has no row in the README's module table"));
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
