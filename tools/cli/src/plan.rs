//! The canonical command table: each subcommand maps to a fixed sequence of
//! child processes with fixed arguments. This table is the single source of
//! truth for what the binary runs.

use crate::cli::Command;

/// One child process to run: a program plus its arguments.
#[derive(Debug)]
pub struct Step {
    pub program: &'static str,
    pub args: &'static [&'static str],
}

/// The steps a subcommand runs, in order. `Doctor` runs no external steps
/// (its probes are gathered separately) and returns an empty slice.
///
/// `smoke` selects bench's run-once mode (criterion test mode — every
/// bench executes once, no statistics). Argument parsing never produces
/// `smoke` together with any other subcommand; if a future caller passes
/// that pairing anyway, the flag is ignored and the normal plan applies.
#[must_use]
pub fn steps(command: Command, smoke: bool) -> &'static [Step] {
    match command {
        Command::Configure => &[
            Step {
                program: "rustup",
                args: &["show"],
            },
            Step {
                program: "cargo",
                args: &["--version"],
            },
        ],
        Command::Build => &[Step {
            program: "cargo",
            args: &["build", "--workspace"],
        }],
        Command::Test => &[Step {
            program: "cargo",
            args: &["test", "--workspace"],
        }],
        Command::Bench if smoke => &[Step {
            program: "cargo",
            args: &["bench", "--workspace", "--", "--test"],
        }],
        Command::Bench => &[Step {
            program: "cargo",
            args: &["bench", "--workspace"],
        }],
        Command::Lint => &[
            Step {
                program: "cargo",
                args: &["fmt", "--all", "--check"],
            },
            Step {
                program: "cargo",
                args: &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            },
        ],
        // Subcommands whose child cannot be a fixed table entry: check
        // spawns `cargo metadata` itself, coverage reads the export it is
        // handed, doctor's probes are gathered separately, and run,
        // record and replay have their child built by `sample_step` out
        // of the command line.
        Command::Check
        | Command::Coverage
        | Command::Modules
        | Command::AssetPack
        | Command::AssetInspect
        | Command::Doctor
        | Command::Run
        | Command::Record
        | Command::Replay
        | Command::Determinism
        | Command::UiCompile => &[],
    }
}

/// The cargo arguments `run`, `record` and `replay` spawn for one
/// sample: build and start that binary, then hand it the rest of the
/// command line.
///
/// Owned rather than a table entry, because which sample runs and what it
/// is told are not knowable until the command line is read — that is the
/// whole point of the subcommand. `--package` as well as `--bin` because
/// a bare `--bin` does not select a package in a virtual workspace.
///
/// The trailing `--` is always present, so a sample argument that looks
/// like a cargo flag reaches the sample instead of cargo.
///
/// `lead` is the flag a subcommand translates to — `--record-trace` or
/// `--replay-trace` and its path — and goes at the **front** of the
/// sample's line, ahead of anything the caller wrote. Front rather than
/// back because the caller's half is verbatim and may end in a flag
/// still waiting for its value, which would otherwise swallow this one.
/// `features` are cargo's, and so go **before** the `--`: everything
/// after that separator belongs to the sample, and a feature landing
/// there would be handed to the sample as an argument it never declared.
#[must_use]
pub fn sample_step(
    package: &str,
    binary: &str,
    lead: Option<(&str, &str)>,
    sample_args: &[String],
    features: &[String],
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--package".to_string(),
        package.to_string(),
        "--bin".to_string(),
        binary.to_string(),
    ];
    for names in features {
        args.push("--features".to_string());
        args.push(names.clone());
    }
    args.push("--".to_string());
    if let Some((flag, value)) = lead {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
    args.extend(sample_args.iter().cloned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_verifies_toolchain_then_cargo() {
        let plan = steps(Command::Configure, false);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].program, "rustup");
        assert_eq!(plan[0].args, ["show"]);
        assert_eq!(plan[1].program, "cargo");
        assert_eq!(plan[1].args, ["--version"]);
    }

    #[test]
    fn build_test_bench_run_workspace_wide() {
        for (command, verb) in [
            (Command::Build, "build"),
            (Command::Test, "test"),
            (Command::Bench, "bench"),
        ] {
            let plan = steps(command, false);
            assert_eq!(plan.len(), 1, "{verb} should be a single step");
            assert_eq!(plan[0].program, "cargo");
            assert_eq!(plan[0].args, [verb, "--workspace"]);
        }
    }

    #[test]
    fn bench_smoke_runs_every_bench_once() {
        let plan = steps(Command::Bench, true);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].program, "cargo");
        assert_eq!(plan[0].args, ["bench", "--workspace", "--", "--test"]);
    }

    #[test]
    fn lint_runs_format_check_then_clippy_with_denied_warnings() {
        let plan = steps(Command::Lint, false);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].program, "cargo");
        assert_eq!(plan[0].args, ["fmt", "--all", "--check"]);
        assert_eq!(plan[1].program, "cargo");
        assert_eq!(
            plan[1].args,
            [
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings"
            ]
        );
    }

    #[test]
    fn subcommands_outside_the_table_run_no_step_from_it() {
        assert!(steps(Command::Doctor, false).is_empty());
        assert!(steps(Command::Check, false).is_empty());
        assert!(steps(Command::Coverage, false).is_empty());
        assert!(steps(Command::Modules, false).is_empty());
        assert!(steps(Command::AssetPack, false).is_empty());
        assert!(steps(Command::AssetInspect, false).is_empty());
        assert!(steps(Command::Run, false).is_empty());
        assert!(steps(Command::Record, false).is_empty());
        assert!(steps(Command::Replay, false).is_empty());
    }

    /// Features are cargo's and land before the separator; the sample's
    /// own arguments stay after it, untouched.
    ///
    /// The position is the whole point: everything after `--` is handed
    /// to the sample, so a feature written there would arrive as an
    /// argument the sample never declared and be refused by it, naming a
    /// flag the caller did type but in a role they did not intend.
    #[test]
    fn features_go_to_cargo_and_the_separator_still_divides_the_line() {
        let args = sample_step(
            "renew-sample-glide",
            "glide",
            None,
            &["--window".to_string()],
            &["window".to_string(), "audio".to_string()],
        );
        assert_eq!(
            args,
            [
                "run",
                "--package",
                "renew-sample-glide",
                "--bin",
                "glide",
                "--features",
                "window",
                "--features",
                "audio",
                "--",
                "--window",
            ]
        );
        // Each occurrence kept: cargo unions them, and dropping one is
        // how a caller silently loses a capability they asked for.
        let separator = args.iter().position(|arg| arg == "--").expect("separator");
        assert_eq!(
            args.iter().filter(|arg| *arg == "--features").count(),
            2,
            "both occurrences must survive"
        );
        assert!(
            args.iter().take(separator).any(|arg| arg == "audio"),
            "features belong to cargo, ahead of the separator"
        );
    }

    #[test]
    fn a_sample_step_names_its_package_and_binary_then_hands_over() {
        let args = sample_step(
            "renew-sample-hello-triangle",
            "hello_triangle",
            None,
            &["--headless".to_string(), "--frames".to_string()],
            &[],
        );
        assert_eq!(
            args,
            [
                "run",
                "--package",
                "renew-sample-hello-triangle",
                "--bin",
                "hello_triangle",
                "--",
                "--headless",
                "--frames",
            ]
        );
    }

    #[test]
    fn a_sample_step_with_nothing_to_hand_over_still_ends_in_the_separator() {
        // Uniform shape: the separator is not conditional, so nothing
        // downstream has to know whether the sample was given arguments.
        let args = sample_step("renew-sample-input-echo", "input_echo", None, &[], &[]);
        assert_eq!(
            args,
            [
                "run",
                "--package",
                "renew-sample-input-echo",
                "--bin",
                "input_echo",
                "--",
            ]
        );
    }

    /// The translated flag leads the sample's line, ahead of everything
    /// the caller wrote. Ordering is the point: the caller's half is
    /// verbatim and may end in a flag still waiting for a value, which
    /// would swallow this one if it came last.
    #[test]
    fn a_trace_flag_leads_the_samples_line_ahead_of_the_callers_own() {
        let args = sample_step(
            "renew-sample-input-echo",
            "input_echo",
            Some(("--replay-trace", "walk.trace")),
            &["--headless".to_string(), "--seed".to_string()],
            &[],
        );
        assert_eq!(
            args,
            [
                "run",
                "--package",
                "renew-sample-input-echo",
                "--bin",
                "input_echo",
                "--",
                "--replay-trace",
                "walk.trace",
                "--headless",
                "--seed",
            ]
        );
    }
}
