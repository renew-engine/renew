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
        // Internal subcommands: their work happens in-process (check spawns
        // `cargo metadata` itself; coverage reads the export it is handed),
        // not through this table.
        Command::Check | Command::Coverage | Command::Doctor => &[],
    }
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
    fn internal_subcommands_run_no_external_steps() {
        assert!(steps(Command::Doctor, false).is_empty());
        assert!(steps(Command::Check, false).is_empty());
        assert!(steps(Command::Coverage, false).is_empty());
    }
}
