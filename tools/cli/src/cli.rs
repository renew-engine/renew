//! Argument parsing for the fixed subcommand set.

use core::fmt;

/// The subcommands the binary accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Configure,
    Build,
    Test,
    Bench,
    Run,
    Lint,
    Check,
    Coverage,
    Doctor,
}

impl Command {
    /// Every subcommand, in the order `usage` lists them.
    pub const ALL: [Self; 9] = [
        Self::Configure,
        Self::Build,
        Self::Test,
        Self::Bench,
        Self::Run,
        Self::Lint,
        Self::Check,
        Self::Coverage,
        Self::Doctor,
    ];

    /// The name the subcommand is invoked by.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Configure => "configure",
            Self::Build => "build",
            Self::Test => "test",
            Self::Bench => "bench",
            Self::Run => "run",
            Self::Lint => "lint",
            Self::Check => "check",
            Self::Coverage => "coverage",
            Self::Doctor => "doctor",
        }
    }

    /// One-line description shown in the usage text.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Self::Configure => "verify the toolchain and cargo are present and sane",
            Self::Build => "build the workspace",
            Self::Test => "run the workspace test suite",
            Self::Bench => "run the workspace benchmarks",
            Self::Run => "build and run a workspace sample",
            Self::Lint => "check formatting, then run clippy with warnings denied",
            Self::Check => "verify workspace crate manifests and dependencies",
            Self::Coverage => "hold a coverage report against the exemption manifest",
            Self::Doctor => "check the development environment",
        }
    }

    /// Look a subcommand up by its invocation name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|command| command.name() == name)
    }
}

/// A successfully parsed invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub json: bool,
    /// Bench only (parse enforces): run each benchmark once, without
    /// statistics — the fast run-proof mode CI's bench stage uses.
    pub smoke: bool,
    /// Coverage only (parse enforces, and requires): the `llvm-cov` JSON
    /// export to read.
    pub report: Option<String>,
    /// Run only (parse enforces, and requires): the sample to start,
    /// named by its binary.
    pub sample: Option<String>,
    /// Run only: the sample's own command line, taken verbatim.
    pub sample_args: Vec<String>,
}

/// What parsing decided: run a subcommand, or show usage on request.
/// Help carries the `--json` flag so usage can honor the output contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Invocation),
    Help { json: bool },
}

/// Why parsing failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    NoCommand,
    UnknownCommand(String),
    UnexpectedArgument(String),
    /// An option that takes a value was given without one.
    MissingValue(&'static str),
    /// A subcommand was given without an option it requires.
    MissingOption {
        command: &'static str,
        option: &'static str,
    },
    /// `run` was given without naming a sample.
    MissingSample,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCommand => write!(f, "no command given"),
            Self::UnknownCommand(name) => write!(f, "unknown command `{name}`"),
            Self::UnexpectedArgument(argument) => {
                write!(f, "unexpected argument `{argument}`")
            }
            Self::MissingValue(option) => write!(f, "`{option}` needs a value"),
            Self::MissingOption { command, option } => {
                write!(f, "`{command}` needs `{option} <path>`")
            }
            Self::MissingSample => write!(f, "`run` needs a sample to run"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse command-line arguments (excluding the program name).
///
/// Everything after `run <sample>` is the sample's own command line and
/// is taken verbatim, so a flag this binary also understands still
/// reaches the sample. Flags meant for `renew` itself therefore go
/// *before* the sample name. A single `--` may stand between the two
/// halves for readers; it is the marker, not an argument, so only the
/// first one is dropped and a sample that wants a literal `--` gets it
/// by writing two.
///
/// # Errors
///
/// Returns a [`ParseError`] when no subcommand is given, the subcommand is
/// unknown, or an argument other than the known flags is present —
/// including `--smoke` with any subcommand other than `bench`, `--report`
/// with any subcommand other than `coverage`, `--report` without a path,
/// `coverage` without `--report`, and `run` without a sample.
pub fn parse(arguments: &[String]) -> Result<Parsed, ParseError> {
    let mut command = None;
    let mut json = false;
    let mut smoke = false;
    let mut help = false;
    let mut report = None;
    let mut sample: Option<String> = None;
    let mut sample_args: Vec<String> = Vec::new();
    // The separator, if it comes at all, comes immediately after the
    // sample name. Tracked rather than inferred from an empty tail, so
    // that a second `--` is an argument even when the first was consumed.
    let mut separator_due = false;
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        // Past the sample name nothing is this binary's business, so this
        // arm comes before every flag the binary knows.
        if sample.is_some() {
            let was_due = separator_due;
            separator_due = false;
            if was_due && argument == "--" {
                continue;
            }
            sample_args.push(argument.clone());
            continue;
        }
        match argument.as_str() {
            "--json" => json = true,
            "--smoke" => smoke = true,
            // Consumes its value even under `help`, so the path can never
            // be mistaken for a subcommand.
            "--report" => {
                let path = rest.next().ok_or(ParseError::MissingValue("--report"))?;
                report = Some(path.clone());
            }
            "help" | "--help" | "-h" => help = true,
            other => {
                if help {
                    // Help short-circuits; ignore anything after it.
                    continue;
                }
                if command == Some(Command::Run) {
                    // `run`'s first free argument names the sample; the
                    // rest of the line is the sample's, taken above.
                    sample = Some(other.to_string());
                    separator_due = true;
                    continue;
                }
                if command.is_some() {
                    return Err(ParseError::UnexpectedArgument(other.to_string()));
                }
                match Command::from_name(other) {
                    Some(found) => command = Some(found),
                    None => return Err(ParseError::UnknownCommand(other.to_string())),
                }
            }
        }
    }
    if help {
        return Ok(Parsed::Help { json });
    }
    if smoke && command != Some(Command::Bench) {
        // The flag belongs to exactly one subcommand; anywhere else it is
        // as unexpected as any stray argument.
        return Err(ParseError::UnexpectedArgument("--smoke".to_string()));
    }
    if report.is_some() && command != Some(Command::Coverage) {
        return Err(ParseError::UnexpectedArgument("--report".to_string()));
    }
    if command == Some(Command::Coverage) && report.is_none() {
        // The report is the whole input: coverage has nothing to read
        // without it, and guessing a path would be worse than refusing.
        return Err(ParseError::MissingOption {
            command: "coverage",
            option: "--report",
        });
    }
    if command == Some(Command::Run) && sample.is_none() {
        // Guessing a sample would be worse than refusing: which one runs
        // is the entire content of the command.
        return Err(ParseError::MissingSample);
    }
    match command {
        Some(command) => Ok(Parsed::Run(Invocation {
            command,
            json,
            smoke,
            report,
            sample,
            sample_args,
        })),
        None => Err(ParseError::NoCommand),
    }
}

/// The usage text printed for `help` and for usage errors.
#[must_use]
pub fn usage() -> String {
    use core::fmt::Write as _;

    let mut text = String::from(concat!(
        "usage: renew <command> [options]\n",
        "       renew [options] run <sample> [--] [sample arguments...]\n",
        "\ncommands:\n",
    ));
    for command in Command::ALL {
        let name = command.name();
        let summary = command.summary();
        let _ = writeln!(text, "  {name:<9}  {summary}");
    }
    text.push_str(concat!(
        "\noptions:\n",
        "  --json            emit one machine-readable JSON document on stdout\n",
        "  --report <path>   (coverage only, required) the llvm-cov JSON export to read\n",
        "  --smoke           (bench only) run each benchmark once, without statistics\n",
        "\nEverything after `run <sample>` goes to the sample untouched, including\n",
        "flags renew itself knows: `renew run hello_triangle --json` gives the sample\n",
        "`--json`, while `renew --json run hello_triangle` gives it to renew. One `--`\n",
        "after the sample name is an optional separator and is not passed on.\n",
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    /// The invocation a bare subcommand parses to.
    fn plain(command: Command) -> Invocation {
        Invocation {
            command,
            json: false,
            smoke: false,
            report: None,
            sample: None,
            sample_args: Vec::new(),
        }
    }

    /// What `run <sample>` with a given tail must parse to.
    fn running(sample: &str, sample_args: &[&str]) -> Parsed {
        Parsed::Run(Invocation {
            sample: Some(sample.to_string()),
            sample_args: sample_args.iter().map(ToString::to_string).collect(),
            ..plain(Command::Run)
        })
    }

    #[test]
    fn every_command_parses_by_name() {
        for command in Command::ALL {
            let name = command.name();
            // Two subcommands need more than their name: coverage takes a
            // required option, run takes the sample. Both still have to
            // round-trip by name.
            let (line, expected) = match command {
                Command::Coverage => (
                    vec![name, "--report", "cov.json"],
                    Invocation {
                        report: Some("cov.json".to_string()),
                        ..plain(command)
                    },
                ),
                Command::Run => (
                    vec![name, "hello_triangle"],
                    Invocation {
                        sample: Some("hello_triangle".to_string()),
                        ..plain(command)
                    },
                ),
                _ => (vec![name], plain(command)),
            };
            assert_eq!(
                parse(&arguments(&line)),
                Ok(Parsed::Run(expected)),
                "command `{name}` did not round-trip"
            );
        }
    }

    #[test]
    fn json_flag_parses_in_either_position() {
        let before = parse(&arguments(&["--json", "build"]));
        let after = parse(&arguments(&["build", "--json"]));
        let expected = Ok(Parsed::Run(Invocation {
            json: true,
            ..plain(Command::Build)
        }));
        assert_eq!(before, expected);
        assert_eq!(after, expected);
    }

    #[test]
    fn smoke_parses_with_bench_in_either_position_and_with_json() {
        let expected = Ok(Parsed::Run(Invocation {
            smoke: true,
            ..plain(Command::Bench)
        }));
        assert_eq!(parse(&arguments(&["bench", "--smoke"])), expected);
        assert_eq!(parse(&arguments(&["--smoke", "bench"])), expected);
        assert_eq!(
            parse(&arguments(&["bench", "--smoke", "--json"])),
            Ok(Parsed::Run(Invocation {
                json: true,
                smoke: true,
                ..plain(Command::Bench)
            }))
        );
    }

    #[test]
    fn smoke_with_any_other_subcommand_is_rejected() {
        for name in [
            "configure",
            "build",
            "test",
            "run",
            "lint",
            "check",
            "doctor",
        ] {
            assert_eq!(
                parse(&arguments(&[name, "--smoke"])),
                Err(ParseError::UnexpectedArgument("--smoke".to_string())),
                "`{name} --smoke` must be rejected"
            );
        }
        assert_eq!(
            parse(&arguments(&["--smoke"])),
            Err(ParseError::UnexpectedArgument("--smoke".to_string()))
        );
    }

    #[test]
    fn report_parses_with_coverage_in_either_position_and_with_json() {
        let expected = Ok(Parsed::Run(Invocation {
            report: Some("target/cov.json".to_string()),
            ..plain(Command::Coverage)
        }));
        assert_eq!(
            parse(&arguments(&["coverage", "--report", "target/cov.json"])),
            expected
        );
        assert_eq!(
            parse(&arguments(&["--report", "target/cov.json", "coverage"])),
            expected
        );
        assert_eq!(
            parse(&arguments(&["coverage", "--report", "cov.json", "--json"])),
            Ok(Parsed::Run(Invocation {
                json: true,
                report: Some("cov.json".to_string()),
                ..plain(Command::Coverage)
            }))
        );
    }

    #[test]
    fn report_with_any_other_subcommand_is_rejected() {
        for name in [
            "configure",
            "build",
            "test",
            "bench",
            "run",
            "lint",
            "check",
        ] {
            assert_eq!(
                parse(&arguments(&[name, "--report", "cov.json"])),
                Err(ParseError::UnexpectedArgument("--report".to_string())),
                "`{name} --report` must be rejected"
            );
        }
        assert_eq!(
            parse(&arguments(&["--report", "cov.json"])),
            Err(ParseError::UnexpectedArgument("--report".to_string()))
        );
    }

    #[test]
    fn coverage_without_a_report_is_rejected() {
        assert_eq!(
            parse(&arguments(&["coverage"])),
            Err(ParseError::MissingOption {
                command: "coverage",
                option: "--report",
            })
        );
    }

    #[test]
    fn a_report_flag_without_a_path_is_rejected() {
        assert_eq!(
            parse(&arguments(&["coverage", "--report"])),
            Err(ParseError::MissingValue("--report"))
        );
    }

    #[test]
    fn a_report_path_is_never_read_as_a_subcommand() {
        // `--report` consumes the token after it, so a path that happens to
        // spell a subcommand stays a path.
        assert_eq!(
            parse(&arguments(&["coverage", "--report", "build"])),
            Ok(Parsed::Run(Invocation {
                report: Some("build".to_string()),
                ..plain(Command::Coverage)
            }))
        );
    }

    #[test]
    fn run_without_a_sample_is_rejected() {
        assert_eq!(parse(&arguments(&["run"])), Err(ParseError::MissingSample));
        assert_eq!(
            parse(&arguments(&["--json", "run"])),
            Err(ParseError::MissingSample)
        );
    }

    #[test]
    fn the_sample_command_line_is_taken_verbatim_with_or_without_a_separator() {
        // The two spellings CI and a person respectively use; the sample
        // must not be able to tell them apart.
        let expected = Ok(running(
            "hello_triangle",
            &["--headless", "--frames", "600"],
        ));
        assert_eq!(
            parse(&arguments(&[
                "run",
                "hello_triangle",
                "--headless",
                "--frames",
                "600"
            ])),
            expected
        );
        assert_eq!(
            parse(&arguments(&[
                "run",
                "hello_triangle",
                "--",
                "--headless",
                "--frames",
                "600"
            ])),
            expected
        );
    }

    #[test]
    fn flags_this_binary_knows_still_reach_the_sample() {
        // Nothing after the sample name is claimed here — otherwise a
        // sample could never own a flag whose name renew also uses.
        assert_eq!(
            parse(&arguments(&[
                "run",
                "hello_triangle",
                "--json",
                "--smoke",
                "help"
            ])),
            Ok(running("hello_triangle", &["--json", "--smoke", "help"]))
        );
        // Before the sample name, the same flag is renew's.
        assert_eq!(
            parse(&arguments(&[
                "run",
                "--json",
                "hello_triangle",
                "--headless"
            ])),
            Ok(Parsed::Run(Invocation {
                json: true,
                sample: Some("hello_triangle".to_string()),
                sample_args: vec!["--headless".to_string()],
                ..plain(Command::Run)
            }))
        );
    }

    #[test]
    fn only_the_first_separator_is_the_separator() {
        // A sample wanting a literal `--` writes two, exactly as it would
        // through `cargo run`.
        assert_eq!(
            parse(&arguments(&["run", "sample", "--", "--", "x"])),
            Ok(running("sample", &["--", "x"]))
        );
        // Later ones are ordinary arguments, in place.
        assert_eq!(
            parse(&arguments(&["run", "sample", "-a", "--", "b"])),
            Ok(running("sample", &["-a", "--", "b"]))
        );
        // A sample can also be run with nothing at all.
        assert_eq!(
            parse(&arguments(&["run", "sample"])),
            Ok(running("sample", &[]))
        );
        assert_eq!(
            parse(&arguments(&["run", "sample", "--"])),
            Ok(running("sample", &[]))
        );
    }

    #[test]
    fn a_sample_name_is_never_read_as_a_subcommand() {
        // A sample called `build` is still the sample, because the first
        // free argument after `run` is a name, not a command.
        assert_eq!(
            parse(&arguments(&["run", "build"])),
            Ok(Parsed::Run(Invocation {
                sample: Some("build".to_string()),
                ..plain(Command::Run)
            }))
        );
    }

    #[test]
    fn unknown_command_is_rejected() {
        assert_eq!(
            parse(&arguments(&["deploy"])),
            Err(ParseError::UnknownCommand("deploy".to_string()))
        );
    }

    #[test]
    fn trailing_argument_is_rejected() {
        assert_eq!(
            parse(&arguments(&["build", "extra"])),
            Err(ParseError::UnexpectedArgument("extra".to_string()))
        );
    }

    #[test]
    fn missing_command_is_rejected() {
        assert_eq!(parse(&arguments(&[])), Err(ParseError::NoCommand));
        assert_eq!(parse(&arguments(&["--json"])), Err(ParseError::NoCommand));
    }

    #[test]
    fn help_short_circuits_in_any_position() {
        for list in [&["help"][..], &["--help"], &["-h"], &["build", "--help"]] {
            assert_eq!(parse(&arguments(list)), Ok(Parsed::Help { json: false }));
        }
    }

    #[test]
    fn help_sees_the_json_flag_in_either_order() {
        for list in [&["--json", "help"][..], &["help", "--json"]] {
            assert_eq!(parse(&arguments(list)), Ok(Parsed::Help { json: true }));
        }
    }

    #[test]
    fn arguments_after_help_are_ignored() {
        assert_eq!(
            parse(&arguments(&["help", "nonsense"])),
            Ok(Parsed::Help { json: false })
        );
    }

    #[test]
    fn help_swallows_the_smoke_and_report_flags_like_any_other_argument() {
        // Deliberate: help short-circuits everything except `--json`
        // (same rule as `help nonsense`), so the subcommand-specific
        // validation never runs and help still prints. `--report` still
        // eats its value, which is why the path never reaches the
        // subcommand slot.
        for list in [
            &["help", "--smoke"][..],
            &["--smoke", "--help"],
            &["help", "--report", "cov.json"],
            &["--report", "cov.json", "--help"],
        ] {
            assert_eq!(parse(&arguments(list)), Ok(Parsed::Help { json: false }));
        }
    }

    #[test]
    fn usage_lists_every_command_and_every_option() {
        let text = usage();
        for command in Command::ALL {
            let name = command.name();
            assert!(text.contains(name), "usage text is missing `{name}`");
        }
        for option in ["--json", "--report", "--smoke"] {
            assert!(text.contains(option), "usage text is missing `{option}`");
        }
        // The pass-through rule is the one thing about this command line
        // a reader cannot guess, so it is spelled out rather than implied.
        assert!(
            text.contains("run <sample> [--] [sample arguments...]"),
            "usage text does not show run's shape"
        );
        assert!(
            text.contains("goes to the sample untouched"),
            "usage text does not explain the pass-through"
        );
    }

    #[test]
    fn every_parse_error_says_what_went_wrong() {
        for error in [
            ParseError::NoCommand,
            ParseError::UnknownCommand("deploy".to_string()),
            ParseError::UnexpectedArgument("extra".to_string()),
            ParseError::MissingValue("--report"),
            ParseError::MissingOption {
                command: "coverage",
                option: "--report",
            },
            ParseError::MissingSample,
        ] {
            assert!(!error.to_string().is_empty(), "{error:?}");
        }
    }
}
