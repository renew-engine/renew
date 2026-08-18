//! What tree an invocation runs in, and what the envelope says about it:
//! the engine by its marker, a game by its renew dependency, a loud coded
//! refusal for everything else — and a distinct code for the tree the
//! tool could not classify at all, because "could not tell" is not "told
//! and found nothing".

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use renew_cli::json::{self, Value};

fn scratch_directory(tag: &str) -> std::io::Result<PathBuf> {
    let directory =
        std::env::temp_dir().join(format!("renew-cli-target-{tag}-{}", std::process::id()));
    if directory.exists() {
        fs::remove_dir_all(&directory)?;
    }
    fs::create_dir_all(&directory)?;
    // A tree in the temp directory inherits none of this repository's
    // configuration, so the toolchain is pinned explicitly rather than
    // left to whatever the machine's default happens to be — the same
    // reasoning the sibling suite records.
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml"),
        directory.join("rust-toolchain.toml"),
    )?;
    Ok(directory)
}

fn write_member(root: &Path, name: &str, manifest_tail: &str) -> std::io::Result<()> {
    let member = root.join(name);
    fs::create_dir_all(member.join("src"))?;
    fs::write(
        member.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.0.1\"\nedition = \"2021\"\n{manifest_tail}"
        ),
    )?;
    fs::write(member.join("src").join("lib.rs"), "")?;
    Ok(())
}

/// A workspace with a `game` member depending on a local `renew-stub`:
/// the smallest tree that classifies as a project.
fn project_workspace(tag: &str) -> std::io::Result<PathBuf> {
    let directory = scratch_directory(tag)?;
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"game\", \"renew-stub\"]\n",
    )?;
    write_member(&directory, "renew-stub", "")?;
    write_member(
        &directory,
        "game",
        "\n[dependencies]\nrenew-stub = { path = \"../renew-stub\" }\n",
    )?;
    Ok(directory)
}

/// Runs the binary and hands back its envelope. A `Result` rather than a
/// panic, because helper code is not test code to the lint configuration —
/// each test unwraps at its own call site, where the exemption lives. The
/// child gets its own target directory (no lock contention with the outer
/// cargo) and none of the outer run's compiler flags.
fn renew_json(directory: &Path, arguments: &[&str]) -> Result<(Value, bool), String> {
    // The flag leads: everything after a sample name belongs to the
    // sample, so a trailing `--json` would be its argument, not ours.
    let mut command = Command::new(env!("CARGO_BIN_EXE_renew"));
    command
        .arg("--json")
        .args(arguments)
        .current_dir(directory)
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
    let line = lines.next().ok_or_else(|| {
        format!(
            "no output for {arguments:?}; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })?;
    // The mode promises exactly one document on stdout, and this helper
    // serves every runner path — so the promise is asserted everywhere,
    // not only on the subcommands whose suite happens to be strict.
    if let Some(extra) = lines.next() {
        return Err(format!(
            "{arguments:?} printed more than one line on stdout; the second is: {extra}"
        ));
    }
    let envelope =
        json::parse(line).map_err(|error| format!("unparseable envelope: {error}: {line}"))?;
    Ok((envelope, output.status.success()))
}

fn first_failure_code(envelope: &Value) -> Option<String> {
    envelope
        .get("failures")
        .and_then(Value::as_array)
        .and_then(<[Value]>::first)
        .and_then(|failure| failure.get("code"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

#[test]
fn a_workspace_that_is_nobodys_business_is_refused_with_a_code() {
    let directory = scratch_directory("stranger").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"plain\"]\n",
    )
    .expect("root manifest");
    write_member(&directory, "plain", "").expect("member");

    let (envelope, ok) = renew_json(&directory, &["build"]).expect("an envelope");
    assert!(!ok, "a refusal is not a success");
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("error")
    );
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("not-a-renew-project"),
        "the refusal names its class: {}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_tree_that_cannot_be_classified_says_so_instead_of_guessing() {
    // A workspace whose metadata cannot be read is not "not a project" —
    // the tool never established anything, and the code must say that.
    let directory = scratch_directory("untellable").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"missing\"]\n",
    )
    .expect("root manifest");

    // Every anchored subcommand answers the same way: each classifies at
    // its own call site, so each is driven rather than one standing in
    // for the rest.
    for arguments in [
        &["build"][..],
        &["check"][..],
        &["modules"][..],
        &["coverage", "--report", "report.json"][..],
        &["run", "anything"][..],
        &["determinism", "--emit", "out.json"][..],
        &["determinism", "--compare", "a.json", "--compare", "b.json"][..],
    ] {
        let (envelope, ok) = renew_json(&directory, arguments).expect("an envelope");
        assert!(!ok, "{arguments:?}");
        assert_eq!(
            first_failure_code(&envelope).as_deref(),
            Some("classification-failed"),
            "{arguments:?}: could-not-tell is its own claim: {}",
            envelope.render()
        );
    }
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_renew_dependent_workspace_is_a_project_and_builds_as_one() {
    let directory = project_workspace("project").expect("scratch");

    let (envelope, ok) = renew_json(&directory, &["build"]).expect("an envelope");
    assert!(ok, "a two-crate project builds: {}", envelope.render());
    assert_eq!(envelope.get("status").and_then(Value::as_str), Some("ok"));
    let target = envelope.get("target").expect("target field");
    assert_eq!(
        target.get("kind").and_then(Value::as_str),
        Some("project"),
        "a tree that depends on renew is a project: {}",
        envelope.render()
    );
    let coverage = envelope.get("coverage").expect("coverage field");
    assert_eq!(
        coverage.get("all_features").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        coverage.get("packages").and_then(Value::as_str),
        Some("workspace"),
        "the statement names its package scope"
    );
    assert_eq!(
        coverage.get("profile").and_then(Value::as_str),
        Some("dev"),
        "build compiles the dev profile and the statement says so"
    );
    assert_eq!(
        coverage
            .get("features")
            .and_then(Value::as_array)
            .map(<[Value]>::len),
        Some(0),
        "no features were asked for, and the statement says so"
    );
    assert_eq!(
        envelope
            .get("failures")
            .and_then(Value::as_array)
            .map(<[Value]>::len),
        Some(0),
        "a green envelope carries an empty failures array, not a missing one"
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn the_coverage_statement_repeats_what_was_asked_for() {
    let directory = scratch_directory("coverage").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"game\"]\n",
    )
    .expect("root manifest");
    write_member(
        &directory,
        "game",
        "\n[features]\nextra = []\nglossy = []\n\n[dependencies]\nrenew-game = { path = \"../renew-game\", optional = true }\n",
    )
    .expect("game");
    write_member(&directory, "renew-game", "").expect("stub");

    // Two occurrences: cargo unions them, and the statement must keep
    // both — a statement that kept only one would silently narrow what
    // the reader believes was covered.
    let (envelope, ok) = renew_json(
        &directory,
        &[
            "--features",
            "game/extra",
            "--features",
            "game/glossy",
            "build",
        ],
    )
    .expect("an envelope");
    assert!(ok, "{}", envelope.render());
    let features = envelope
        .get("coverage")
        .and_then(|coverage| coverage.get("features"))
        .and_then(Value::as_array)
        .expect("features list");
    let listed: Vec<&str> = features.iter().filter_map(Value::as_str).collect();
    assert_eq!(
        listed,
        ["game/extra", "game/glossy"],
        "every occurrence survives into the statement: {}",
        envelope.render()
    );

    // The everything switch is reported as fact, not defaulted: a
    // statement that always said false would be the exact lie the field
    // exists to prevent.
    let (envelope, ok) = renew_json(&directory, &["--all-features", "build"]).expect("an envelope");
    assert!(ok, "{}", envelope.render());
    assert_eq!(
        envelope
            .get("coverage")
            .and_then(|coverage| coverage.get("all_features"))
            .and_then(Value::as_bool),
        Some(true),
        "an all-features run says all_features true: {}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

/// Every verb the parser calls a workspace-feature verb carries the
/// statement, and carries the profile the registry's enum names for it.
///
/// Driven from `Command::ALL` filtered by the parser's own predicate
/// rather than from a hand-written list: the list is what goes stale
/// when a verb joins the set, and a contract row nothing drives is a
/// promise with no gate.
#[test]
fn every_workspace_feature_verb_states_its_coverage() {
    let directory = project_workspace("everyverb").expect("scratch");

    let mut driven = 0;
    for command in renew_cli::cli::Command::ALL {
        if !command.takes_workspace_features() {
            continue;
        }
        driven += 1;
        let name = command.name();
        let (envelope, _ok) = renew_json(&directory, &[name]).expect("an envelope");
        let coverage = envelope
            .get("coverage")
            .unwrap_or_else(|| panic!("{name} states its coverage: {}", envelope.render()));
        assert_eq!(
            coverage.get("packages").and_then(Value::as_str),
            Some("workspace"),
            "{name}: {}",
            envelope.render()
        );
        let profile = coverage
            .get("profile")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            ["dev", "test", "bench"].contains(&profile),
            "{name} names a profile the schema's enum knows: {profile}"
        );
        assert_eq!(
            envelope
                .get("target")
                .and_then(|target| target.get("kind"))
                .and_then(Value::as_str),
            Some("project"),
            "{name} classifies its tree: {}",
            envelope.render()
        );
    }
    assert!(
        driven >= 4,
        "the predicate named only {driven} verbs; the set has stopped being what it was"
    );

    // And the refusal direction, on the same verbs: a stranger's tree is
    // refused by every one of them, with the statement still saying what
    // the refused run would have covered.
    let stranger = scratch_directory("everyverb-stranger").expect("scratch");
    fs::write(
        stranger.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = []\n",
    )
    .expect("root manifest");
    for command in renew_cli::cli::Command::ALL {
        if !command.takes_workspace_features() {
            continue;
        }
        let name = command.name();
        let (envelope, ok) = renew_json(&stranger, &[name]).expect("an envelope");
        assert!(!ok, "{name} must refuse a stranger");
        assert_eq!(
            first_failure_code(&envelope).as_deref(),
            Some("not-a-renew-project"),
            "{name}: {}",
            envelope.render()
        );
        assert!(
            envelope.get("coverage").is_some(),
            "{name}: a refusal still states what the run would have covered: {}",
            envelope.render()
        );
    }

    let _ = fs::remove_dir_all(&stranger);
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn bench_and_test_state_their_own_profiles() {
    let directory = project_workspace("bench").expect("scratch");
    let (envelope, ok) = renew_json(&directory, &["bench"]).expect("an envelope");
    assert!(ok, "{}", envelope.render());
    assert_eq!(
        envelope
            .get("coverage")
            .and_then(|coverage| coverage.get("profile"))
            .and_then(Value::as_str),
        Some("bench"),
        "bench compiles the bench profile and the statement says so"
    );
    // `cargo test` compiles the `test` profile, not `dev` — the field
    // that promises to describe the invocation exactly must say so.
    let (envelope, ok) = renew_json(&directory, &["test"]).expect("an envelope");
    assert!(ok, "{}", envelope.render());
    assert_eq!(
        envelope
            .get("coverage")
            .and_then(|coverage| coverage.get("profile"))
            .and_then(Value::as_str),
        Some("test"),
        "test compiles the test profile and the statement says so"
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_failing_project_build_names_the_step_and_keeps_its_fields() {
    let directory = project_workspace("red").expect("scratch");
    fs::write(
        directory.join("game").join("src").join("lib.rs"),
        "fn deliberately_broken(",
    )
    .expect("break the game");

    let (envelope, ok) = renew_json(&directory, &["build"]).expect("an envelope");
    assert!(!ok);
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("step-failed"),
        "{}",
        envelope.render()
    );
    // And *which* step: a subcommand can run more than one child, and a
    // summary naming only the program leaves a reader unable to tell
    // which of them failed.
    let summary = envelope
        .get("failures")
        .and_then(Value::as_array)
        .and_then(<[Value]>::first)
        .and_then(|failure| failure.get("summary"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        summary.contains("cargo build --workspace"),
        "the summary names the step, not just the program: {summary}"
    );
    // The failure envelope says where it failed and what it covered,
    // exactly as the success envelope would.
    assert_eq!(
        envelope
            .get("target")
            .and_then(|target| target.get("kind"))
            .and_then(Value::as_str),
        Some("project"),
        "a red envelope still names its tree: {}",
        envelope.render()
    );
    assert!(
        envelope.get("coverage").is_some(),
        "a red envelope still states its coverage: {}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn engine_only_subcommands_refuse_a_project_tree() {
    let directory = project_workspace("engine-only").expect("scratch");

    for arguments in [
        &["modules"][..],
        &["check"][..],
        &["coverage", "--report", "report.json"][..],
        &["run", "anything"][..],
        &["record", "--output", "t.trace", "anything"][..],
        &["replay", "--input", "t.trace", "anything"][..],
        &["determinism", "--emit", "out.json"][..],
        &["determinism", "--compare", "a.json", "--compare", "b.json"][..],
    ] {
        let (envelope, ok) = renew_json(&directory, arguments).expect("an envelope");
        assert!(!ok, "{arguments:?} must refuse a project tree");
        assert_eq!(
            first_failure_code(&envelope).as_deref(),
            Some("engine-only-subcommand"),
            "{arguments:?}: {}",
            envelope.render()
        );
        // The reason names the subcommand that was refused: a consumer
        // renders the summary verbatim, and `record` being told about
        // `run` would read as an answer to a different ask.
        let summary = envelope
            .get("failures")
            .and_then(Value::as_array)
            .and_then(<[Value]>::first)
            .and_then(|failure| failure.get("summary"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            summary.contains(arguments[0]),
            "{arguments:?}: the summary must name the refused subcommand: {summary}"
        );
        // Every subcommand established the kind before refusing, and
        // every envelope keeps it — one rule, no per-subcommand shape.
        assert_eq!(
            envelope
                .get("target")
                .and_then(|target| target.get("kind"))
                .and_then(Value::as_str),
            Some("project"),
            "{arguments:?}: an engine-only refusal keeps the known kind: {}",
            envelope.render()
        );
        // The reason is in stderr as well as the failure's summary, so a
        // consumer that displays stderr on error is never shown nothing.
        assert!(
            envelope
                .get("stderr")
                .and_then(Value::as_str)
                .is_some_and(|stderr| !stderr.trim().is_empty()),
            "{arguments:?}: a refusal's reason belongs in stderr too: {}",
            envelope.render()
        );
    }
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_rootless_directory_is_a_coded_refusal_on_runner_subcommands_too() {
    // The system temp directory has no cargo workspace above it; the
    // refusal must carry the same code check and modules use for the
    // identical condition — an envelope with no failures key was the gap.
    let directory =
        std::env::temp_dir().join(format!("renew-cli-target-rootless-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("scratch");
    // Asserted, not assumed — and named, so a failure on a machine that
    // keeps a stray Cargo.toml above its temp directory reads as
    // environment, not regression. The package fallback widened what
    // anchors the walk, so the precondition is broader than it was.
    assert!(
        renew_cli::workspace::find_root(&directory) == renew_cli::workspace::Anchor::None,
        "precondition: no Cargo.toml declaring a workspace or a package may sit above {}",
        directory.display()
    );

    // Each subcommand anchors at its own call site, so each is driven.
    for arguments in [
        &["build"][..],
        &["run", "anything"][..],
        &["determinism", "--emit", "out.json"][..],
        &["determinism", "--compare", "a.json", "--compare", "b.json"][..],
    ] {
        let (envelope, ok) = renew_json(&directory, arguments).expect("an envelope");
        assert!(!ok, "{arguments:?}");
        assert_eq!(
            first_failure_code(&envelope).as_deref(),
            Some("classification-failed"),
            "{arguments:?}: {}",
            envelope.render()
        );
    }

    // The coverage statement depends only on the parsed invocation, so
    // even the refusal that found no tree at all states what the run
    // would have covered — "always" in the registry means always.
    let (envelope, _ok) = renew_json(&directory, &["build"]).expect("an envelope");
    assert!(
        envelope.get("coverage").is_some(),
        "a rootless refusal still states its coverage: {}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn an_abort_keeps_its_fields_and_names_its_class() {
    // An engine-marked tree whose metadata cannot be read: classification
    // succeeds on the marker alone, then the sample listing aborts. The
    // envelope must keep the established target and carry the abort code.
    let directory = scratch_directory("abort").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"missing\"]\n\n[workspace.metadata.renew]\nengine = true\n",
    )
    .expect("root manifest");

    let (envelope, ok) = renew_json(&directory, &["run", "anything"]).expect("an envelope");
    assert!(!ok);
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("error")
    );
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("aborted"),
        "{}",
        envelope.render()
    );
    assert_eq!(
        envelope
            .get("target")
            .and_then(|target| target.get("kind"))
            .and_then(Value::as_str),
        Some("engine-workspace"),
        "an abort envelope keeps the target it established: {}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

/// One determinism leg in the report shape `parse_leg` reads. Each leg
/// names its own (os, arch) row — a duplicated row is one target
/// reported twice and the comparison refuses it as inconclusive.
fn leg(os: &str, arch: &str, digest: &str) -> String {
    // Every name the pinned list binds: the comparison holds each leg
    // against that set, so a leg carrying one invented name would be
    // refused as having run a fraction of the claim — which is a
    // different test than these.
    let digests: Vec<String> = renew_cli::determinism::expected_digest_names()
        .iter()
        .map(|name| format!("\"{name}\": \"{digest}\""))
        .collect();
    format!(
        "{{\"schema_version\": 1, \"os\": \"{os}\", \"arch\": \"{arch}\", \
         \"toolchain\": \"rustc 1.97.1\", \"digests\": {{{}}}}}",
        digests.join(", ")
    )
}

#[test]
fn a_delivered_determinism_verdict_is_never_an_abort() {
    // The engine-marked tree satisfies the guard; the legs are supplied
    // as files, so no child runs and the verdict is entirely ours.
    let directory = scratch_directory("verdict").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = []\n\n[workspace.metadata.renew]\nengine = true\n",
    )
    .expect("root manifest");

    // One leg per expected target row, one digest differing: the
    // flagship red, and it must wear its own code rather than the
    // inconclusive one a short leg set would earn.
    fs::write(directory.join("a.json"), leg("linux", "x86_64", "0xaaaa")).expect("leg");
    fs::write(directory.join("b.json"), leg("windows", "x86_64", "0xaaaa")).expect("leg");
    fs::write(directory.join("c.json"), leg("macos", "aarch64", "0xbbbb")).expect("leg");
    fs::write(directory.join("g.json"), leg("android", "x86_64", "0xaaaa")).expect("leg");
    fs::write(
        directory.join("i.json"),
        leg("ios-simulator", "aarch64", "0xaaaa"),
    )
    .expect("leg");
    let (envelope, ok) = renew_json(
        &directory,
        &[
            "determinism",
            "--compare",
            "a.json",
            "--compare",
            "b.json",
            "--compare",
            "c.json",
            "--compare",
            "g.json",
            "--compare",
            "i.json",
        ],
    )
    .expect("an envelope");
    assert!(!ok);
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("failed"),
        "a delivered red is failed, not error: {}",
        envelope.render()
    );
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("determinism-diverged"),
        "{}",
        envelope.render()
    );
    // A delivered red keeps the target it established, exactly as the
    // registry promises for every post-classification outcome.
    assert_eq!(
        envelope
            .get("target")
            .and_then(|target| target.get("kind"))
            .and_then(Value::as_str),
        Some("engine-workspace"),
        "{}",
        envelope.render()
    );

    // Two legs where three targets are claimed: judged and found
    // incomplete — its own code, not an abort either.
    let (envelope, ok) = renew_json(
        &directory,
        &["determinism", "--compare", "a.json", "--compare", "c.json"],
    )
    .expect("an envelope");
    assert!(!ok);
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("determinism-inconclusive"),
        "{}",
        envelope.render()
    );
    assert_eq!(
        envelope
            .get("target")
            .and_then(|target| target.get("kind"))
            .and_then(Value::as_str),
        Some("engine-workspace"),
        "an inconclusive verdict keeps the target too: {}",
        envelope.render()
    );

    let _ = fs::remove_dir_all(&directory);
}

/// Legs that all ran the same *fraction* of the pinned list agree with
/// each other perfectly, so the comparison has to know what the list
/// binds. Driven end to end because the wiring is what this pins: the
/// binary hands the comparison those names, and a comparison told
/// nothing would report this very fixture as full agreement.
#[test]
fn legs_that_all_ran_less_than_the_pinned_list_are_inconclusive() {
    let directory = scratch_directory("narrowed").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = []\n\n[workspace.metadata.renew]\nengine = true\n",
    )
    .expect("root manifest");

    let one_name = renew_cli::determinism::expected_digest_names()
        .first()
        .cloned()
        .unwrap_or_default();
    for (name, os, arch) in [
        ("n1.json", "linux", "x86_64"),
        ("n2.json", "windows", "x86_64"),
        ("n3.json", "macos", "aarch64"),
        ("n4.json", "android", "x86_64"),
        ("n5.json", "ios-simulator", "aarch64"),
    ] {
        fs::write(
            directory.join(name),
            format!(
                "{{\"schema_version\": 1, \"os\": \"{os}\", \"arch\": \"{arch}\", \
                 \"toolchain\": \"rustc 1.97.1\", \"digests\": {{\"{one_name}\": \"0xdddd\"}}}}"
            ),
        )
        .expect("leg");
    }
    let (envelope, ok) = renew_json(
        &directory,
        &[
            "determinism",
            "--compare",
            "n1.json",
            "--compare",
            "n2.json",
            "--compare",
            "n3.json",
            "--compare",
            "n4.json",
            "--compare",
            "n5.json",
        ],
    )
    .expect("an envelope");
    assert!(!ok, "agreeing over a fraction of the claim is not a pass");
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("determinism-inconclusive"),
        "{}",
        envelope.render()
    );
    assert!(
        envelope
            .get("stderr")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("digests the pinned list binds")),
        "the reason names what went unrun: {}",
        envelope.render()
    );

    let _ = fs::remove_dir_all(&directory);
}

/// The agreeing verdict, in JSON: the mode promises exactly one document
/// on stdout (`renew_json` asserts that shape), and the report a
/// plain-mode caller would read rides inside the envelope rather than
/// leaking as a second line ahead of it.
#[test]
fn an_agreeing_comparison_carries_its_report_inside_the_envelope() {
    let directory = scratch_directory("agree").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = []\n\n[workspace.metadata.renew]\nengine = true\n",
    )
    .expect("root manifest");

    for (name, os, arch) in [
        ("d.json", "linux", "x86_64"),
        ("e.json", "windows", "x86_64"),
        ("f.json", "macos", "aarch64"),
        ("h.json", "android", "x86_64"),
        ("j.json", "ios-simulator", "aarch64"),
    ] {
        fs::write(directory.join(name), leg(os, arch, "0xcccc")).expect("leg");
    }
    let (envelope, ok) = renew_json(
        &directory,
        &[
            "determinism",
            "--compare",
            "d.json",
            "--compare",
            "e.json",
            "--compare",
            "f.json",
            "--compare",
            "h.json",
            "--compare",
            "j.json",
        ],
    )
    .expect("an envelope");
    assert!(ok, "every bound row agreeing agrees: {}", envelope.render());
    assert_eq!(envelope.get("status").and_then(Value::as_str), Some("ok"));
    assert!(
        envelope
            .get("stdout")
            .and_then(Value::as_str)
            .is_some_and(|stdout| stdout.contains("agree")),
        "the verdict a person reads is inside the envelope: {}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_lookalike_table_in_a_member_does_not_anchor_the_walk() {
    // A member manifest carrying a table whose name merely begins with
    // "workspace" must not become the root: anchored there, the marker
    // would be read from the member manifest while cargo answered for
    // the enclosing workspace — mixed anchors, and a wrong code out of
    // the mismatch.
    let directory = scratch_directory("lookalike").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"one\"]\n\n[workspace.metadata.renew]\nengine = true\n",
    )
    .expect("root manifest");
    write_member(&directory, "one", "\n[workspacex]\nkey = 1\n").expect("member");

    let member = directory.join("one");
    let (envelope, _ok) = renew_json(&member, &["configure"]).expect("an envelope");
    assert_eq!(
        envelope
            .get("target")
            .and_then(|target| target.get("kind"))
            .and_then(Value::as_str),
        Some("engine-workspace"),
        "the walk must pass the lookalike and anchor at the real root: {}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

/// A manifest the tool cannot read is refused, not walked past.
///
/// End to end, because the failure this pins is a *green*: with an
/// anchorable ancestor above it, a walk that stepped over the broken
/// file would compile the ancestor and report `ok` — a verdict about a
/// tree the caller never asked about, over code the caller's own tree
/// never compiled.
#[test]
fn a_manifest_the_scan_cannot_name_is_refused_rather_than_stepped_over() {
    let directory = scratch_directory("unreadable").expect("scratch");
    write_member(&directory, "renew-stub", "").expect("stub");
    fs::write(
        directory.join("Cargo.toml"),
        "[package]\nname = \"outer\"\nversion = \"0.0.1\"\nedition = \"2021\"\n\n\
         [dependencies]\nrenew-stub = { path = \"renew-stub\" }\n",
    )
    .expect("outer manifest");
    let inner = directory.join("inner");
    fs::create_dir_all(inner.join("src")).expect("scratch");
    fs::write(inner.join("src").join("lib.rs"), "").expect("inner lib");
    // The commonest possible typo — cargo refuses this file outright.
    fs::write(
        inner.join("Cargo.toml"),
        "[package\nname = \"inner-game\"\nversion = \"0.1.0\"\n",
    )
    .expect("inner manifest");

    let (envelope, ok) = renew_json(&inner, &["build"]).expect("an envelope");
    assert!(
        !ok,
        "a tree whose own manifest cannot be read is not a green: {}",
        envelope.render()
    );
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("classification-failed"),
        "{}",
        envelope.render()
    );
    // The rootless message also contains "Cargo.toml", so naming the
    // file is not enough to tell the two refusals apart — the reason has
    // to say a manifest was found and could not be read, not that none
    // was found.
    let stderr = envelope
        .get("stderr")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        stderr.contains("not a manifest this tool can read"),
        "the refusal says a manifest is here and unreadable: {stderr}"
    );
    assert!(
        !stderr.contains("was found above the current directory"),
        "and not that none was found: {stderr}"
    );
    assert!(
        envelope.get("target").is_none(),
        "nothing was classified, so nothing is named: {}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

/// The quoted table spelling, end to end.
///
/// Legal TOML that cargo reads as the workspace table, and a spelling
/// this scan deliberately does not name. It is refused — the walk stops
/// at the manifest rather than anchoring at a nested package and
/// compiling somebody else's tree. The registry says so; until this
/// existed, only the *predicate* was pinned and the walk's behaviour on
/// it was not.
#[test]
fn a_quoted_table_header_is_refused_rather_than_walked_past() {
    let directory = scratch_directory("quoted-header").expect("scratch");
    write_member(&directory, "renew-stub", "").expect("stub");
    fs::write(
        directory.join("Cargo.toml"),
        "[\"workspace\"]\nresolver = \"3\"\nmembers = [\"game\", \"renew-stub\"]\n",
    )
    .expect("root manifest");
    write_member(
        &directory,
        "game",
        "\n[dependencies]\nrenew-stub = { path = \"../renew-stub\" }\n",
    )
    .expect("game");

    let (envelope, ok) = renew_json(&directory.join("game"), &["build"]).expect("an envelope");
    assert!(!ok, "{}", envelope.render());
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("classification-failed"),
        "{}",
        envelope.render()
    );
    assert!(
        envelope.get("target").is_none(),
        "nothing was classified, so nothing is named: {}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_standalone_package_with_a_renew_dependency_is_a_project() {
    // The default `cargo new` game: a single `[package]` manifest, no
    // `[workspace]` table. Cargo treats it as a workspace of one, and so
    // must the classification — this is the most common tree a game
    // developer will ever stand in.
    let directory = scratch_directory("standalone").expect("scratch");
    let game = directory.join("game");
    fs::create_dir_all(game.join("src")).expect("scratch");
    fs::write(
        game.join("Cargo.toml"),
        "[package]\nname = \"game\"\nversion = \"0.0.1\"\nedition = \"2021\"\n\n\
         [dependencies]\nrenew-stub = { path = \"../renew-stub\" }\n",
    )
    .expect("game manifest");
    fs::write(game.join("src").join("lib.rs"), "").expect("game lib");
    write_member(&directory, "renew-stub", "").expect("stub");

    let (envelope, ok) = renew_json(&game, &["build"]).expect("an envelope");
    assert!(ok, "a standalone game builds: {}", envelope.render());
    let target = envelope.get("target").expect("target field");
    assert_eq!(
        target.get("kind").and_then(Value::as_str),
        Some("project"),
        "{}",
        envelope.render()
    );
    // Anchored at the package itself, not somewhere above it.
    let root = target.get("root").and_then(Value::as_str).expect("root");
    assert!(
        Path::new(root).ends_with("game"),
        "the package is its own root: {}",
        envelope.render()
    );
    // And the manifest names that root's own Cargo.toml — a field a
    // consumer reads to find the tree, so its content is pinned, not
    // merely its type.
    assert_eq!(
        target.get("manifest").and_then(Value::as_str),
        Some(
            Path::new(root)
                .join("Cargo.toml")
                .to_string_lossy()
                .as_ref()
        ),
        "{}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn the_stated_coverage_is_the_coverage_the_child_was_handed() {
    // The vacuous-green regression, pinned end to end with a
    // feature-gated compile error. If the child cargo stopped receiving
    // the flags while the statement still claimed them, the gated build
    // below could not fail — so this asserts the wiring in both
    // directions, not just the echo.
    let directory = scratch_directory("gated").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"game\", \"renew-stub\"]\n",
    )
    .expect("root manifest");
    write_member(&directory, "renew-stub", "").expect("stub");
    write_member(
        &directory,
        "game",
        "\n[features]\nbroken = []\n\n[dependencies]\nrenew-stub = { path = \"../renew-stub\" }\n",
    )
    .expect("game");
    fs::write(
        directory.join("game").join("src").join("lib.rs"),
        "#[cfg(feature = \"broken\")]\ncompile_error!(\"the gated half was compiled\");\n",
    )
    .expect("gated lib");

    // Default features: the gated half is not compiled, and the green is
    // exactly the vacuous kind the coverage statement exists to expose.
    let (envelope, ok) = renew_json(&directory, &["build"]).expect("an envelope");
    assert!(ok, "{}", envelope.render());

    // The same tree with the feature on must fail — proof the flag
    // reached the child and the statement describes a real invocation.
    let (envelope, ok) =
        renew_json(&directory, &["--features", "game/broken", "build"]).expect("an envelope");
    assert!(
        !ok,
        "the gated error must be compiled: {}",
        envelope.render()
    );
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("step-failed"),
        "{}",
        envelope.render()
    );
    let features: Vec<String> = envelope
        .get("coverage")
        .and_then(|coverage| coverage.get("features"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(features, ["game/broken"], "{}", envelope.render());

    // And the everything switch, symmetrically.
    let (envelope, ok) = renew_json(&directory, &["--all-features", "build"]).expect("an envelope");
    assert!(!ok, "{}", envelope.render());
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("step-failed"),
        "{}",
        envelope.render()
    );
    assert_eq!(
        envelope
            .get("coverage")
            .and_then(|coverage| coverage.get("all_features"))
            .and_then(Value::as_bool),
        Some(true),
        "{}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_machinery_failure_past_classification_keeps_the_target() {
    // An engine-marked tree whose metadata cannot be read: the guard
    // passes on the marker alone, then the subcommand's own machinery
    // fails. Check and modules build their envelopes in separate
    // functions, so both are driven rather than one standing in for the
    // other.
    let directory = scratch_directory("machinery").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"missing\"]\n\n[workspace.metadata.renew]\nengine = true\n",
    )
    .expect("root manifest");

    for arguments in [
        &["check"][..],
        &["modules"][..],
        &["coverage", "--report", "report.json"][..],
    ] {
        let subcommand = arguments[0];
        let (envelope, ok) = renew_json(&directory, arguments).expect("an envelope");
        assert!(!ok, "{subcommand}");
        assert_eq!(
            envelope.get("status").and_then(Value::as_str),
            Some("error"),
            "{subcommand}"
        );
        // The invocation ended before a verdict, which is what `aborted`
        // names everywhere else — a consumer dispatching on the code is
        // never handed a red with nothing to dispatch on.
        assert_eq!(
            first_failure_code(&envelope).as_deref(),
            Some("aborted"),
            "{subcommand}: {}",
            envelope.render()
        );
        assert_eq!(
            envelope
                .get("target")
                .and_then(|target| target.get("kind"))
                .and_then(Value::as_str),
            Some("engine-workspace"),
            "{subcommand}: a machinery error past classification keeps the target: {}",
            envelope.render()
        );
    }
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_refusal_reaches_a_plain_mode_caller_too() {
    // Every other refusal test drives `--json`. The plain path is what a
    // person at a terminal sees, and it must still say why rather than
    // failing silently.
    let directory = project_workspace("plain-refusal").expect("scratch");

    // `run` refuses through the runner, `modules` through its own
    // envelope builder — two plain-mode paths, both a person's first
    // encounter with a refusal.
    for arguments in [&["run", "anything"][..], &["modules"][..]] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_renew"));
        command
            .args(arguments)
            .current_dir(&directory)
            .env("CARGO_TARGET_DIR", directory.join("target"));
        let output = command.output().expect("binary runs");
        assert!(!output.status.success(), "{arguments:?}");
        assert!(
            output.stdout.is_empty(),
            "{arguments:?}: the plain path reports on stderr only"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("engine"),
            "{arguments:?}: the refusal says why: {stderr}"
        );
    }
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_failing_pinned_run_delivers_the_childs_own_output_and_code() {
    // An engine-marked workspace that lacks the pinned packages: emit's
    // first pinned run fails inside cargo. The red envelope must carry
    // the child's own stderr and raw exit code — a lane whose red said
    // only "it failed" would leave its reader nothing to diagnose with.
    let directory = scratch_directory("emit-red").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = []\n\n[workspace.metadata.renew]\nengine = true\n",
    )
    .expect("root manifest");

    let (envelope, ok) =
        renew_json(&directory, &["determinism", "--emit", "out.json"]).expect("an envelope");
    assert!(!ok);
    assert_eq!(
        envelope.get("status").and_then(Value::as_str),
        Some("failed"),
        "a failing child is a delivered outcome, not an abort: {}",
        envelope.render()
    );
    assert_eq!(
        first_failure_code(&envelope).as_deref(),
        Some("step-failed"),
        "{}",
        envelope.render()
    );
    // The child's own words, not just this tool's summary sentence.
    assert!(
        envelope
            .get("stderr")
            .and_then(Value::as_str)
            .is_some_and(|stderr| stderr.contains("renew-ui")),
        "the failing child's stderr is in the envelope: {}",
        envelope.render()
    );
    // The child's raw code, not a hardcoded 1.
    let exit_code = envelope.get("exit_code").and_then(|value| match value {
        Value::Number(number) => Some(*number),
        _ => None,
    });
    assert!(
        exit_code.is_some_and(|code| code != 0 && code != 1),
        "the envelope reports the child's raw exit code: {}",
        envelope.render()
    );
    assert_eq!(
        envelope
            .get("target")
            .and_then(|target| target.get("kind"))
            .and_then(Value::as_str),
        Some("engine-workspace"),
        "{}",
        envelope.render()
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_failing_pinned_run_reaches_the_caller_in_plain_mode_too() {
    // The same red as the sibling test, without --json: the captured
    // child output must reach the caller ahead of the summary — a red
    // whose only prose is the summary sentence leaves nothing to
    // diagnose with, in either mode.
    let directory = scratch_directory("emit-red-plain").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = []\n\n[workspace.metadata.renew]\nengine = true\n",
    )
    .expect("root manifest");

    let mut command = Command::new(env!("CARGO_BIN_EXE_renew"));
    command
        .args(["determinism", "--emit", "out.json"])
        .current_dir(&directory)
        .env("CARGO_TARGET_DIR", directory.join("target"));
    for inherited in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTDOCFLAGS",
        "CARGO_ENCODED_RUSTDOCFLAGS",
    ] {
        command.env_remove(inherited);
    }
    let output = command.output().expect("binary runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("renew-ui"),
        "plain mode carries the failing child's own words: {stderr}"
    );
    assert!(
        stderr.contains("ui/menu-16"),
        "and the summary naming the run: {stderr}"
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn an_engine_marked_tree_names_itself_in_the_envelope() {
    let directory = scratch_directory("marked").expect("scratch");
    fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"one\"]\n\n[workspace.metadata.renew]\nengine = true\n",
    )
    .expect("root manifest");
    write_member(&directory, "one", "").expect("member");

    // The environment may legitimately lack rustup, so configure's exit
    // is not asserted — the sibling suite records the same tolerance.
    // What is asserted is the classification, which needs no tool at all.
    let (envelope, _ok) = renew_json(&directory, &["configure"]).expect("an envelope");
    assert_eq!(
        envelope
            .get("target")
            .and_then(|target| target.get("kind"))
            .and_then(Value::as_str),
        Some("engine-workspace")
    );
    assert!(
        envelope.get("coverage").is_none(),
        "configure compiles nothing; a coverage statement there would be a lie"
    );
    let _ = fs::remove_dir_all(&directory);
}
