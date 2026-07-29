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
#[must_use]
pub fn steps(command: Command) -> &'static [Step] {
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
        Command::Doctor => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_verifies_toolchain_then_cargo() {
        let plan = steps(Command::Configure);
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
            let plan = steps(command);
            assert_eq!(plan.len(), 1, "{verb} should be a single step");
            assert_eq!(plan[0].program, "cargo");
            assert_eq!(plan[0].args, [verb, "--workspace"]);
        }
    }

    #[test]
    fn lint_runs_format_check_then_clippy_with_denied_warnings() {
        let plan = steps(Command::Lint);
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
    fn doctor_runs_no_external_steps() {
        assert!(steps(Command::Doctor).is_empty());
    }
}
