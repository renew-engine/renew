//! Argument parsing for the fixed subcommand set.

use core::fmt;

/// The subcommands the binary accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Configure,
    Build,
    Test,
    Bench,
    Lint,
    Check,
    Doctor,
}

impl Command {
    /// Every subcommand, in the order `usage` lists them.
    pub const ALL: [Self; 7] = [
        Self::Configure,
        Self::Build,
        Self::Test,
        Self::Bench,
        Self::Lint,
        Self::Check,
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
            Self::Lint => "lint",
            Self::Check => "check",
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
            Self::Lint => "check formatting, then run clippy with warnings denied",
            Self::Check => "verify workspace crate manifests and dependencies",
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub json: bool,
}

/// What parsing decided: run a subcommand, or show usage on request.
/// Help carries the `--json` flag so usage can honor the output contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCommand => write!(f, "no command given"),
            Self::UnknownCommand(name) => write!(f, "unknown command `{name}`"),
            Self::UnexpectedArgument(argument) => {
                write!(f, "unexpected argument `{argument}`")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse command-line arguments (excluding the program name).
///
/// # Errors
///
/// Returns a [`ParseError`] when no subcommand is given, the subcommand is
/// unknown, or an argument other than `--json` is present.
pub fn parse(arguments: &[String]) -> Result<Parsed, ParseError> {
    let mut command = None;
    let mut json = false;
    let mut help = false;
    for argument in arguments {
        match argument.as_str() {
            "--json" => json = true,
            "help" | "--help" | "-h" => help = true,
            other => {
                if help {
                    // Help short-circuits; ignore anything after it.
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
    match command {
        Some(command) => Ok(Parsed::Run(Invocation { command, json })),
        None => Err(ParseError::NoCommand),
    }
}

/// The usage text printed for `help` and for usage errors.
#[must_use]
pub fn usage() -> String {
    use core::fmt::Write as _;

    let mut text = String::from("usage: renew <command> [--json]\n\ncommands:\n");
    for command in Command::ALL {
        let name = command.name();
        let summary = command.summary();
        let _ = writeln!(text, "  {name:<9}  {summary}");
    }
    text.push_str("\noptions:\n  --json     emit one machine-readable JSON document on stdout\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn every_command_parses_by_name() {
        for command in Command::ALL {
            let parsed = parse(&arguments(&[command.name()]));
            assert_eq!(
                parsed,
                Ok(Parsed::Run(Invocation {
                    command,
                    json: false
                })),
                "command `{}` did not round-trip",
                command.name()
            );
        }
    }

    #[test]
    fn json_flag_parses_in_either_position() {
        let before = parse(&arguments(&["--json", "build"]));
        let after = parse(&arguments(&["build", "--json"]));
        let expected = Ok(Parsed::Run(Invocation {
            command: Command::Build,
            json: true,
        }));
        assert_eq!(before, expected);
        assert_eq!(after, expected);
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
    fn usage_lists_every_command() {
        let text = usage();
        for command in Command::ALL {
            assert!(
                text.contains(command.name()),
                "usage text is missing `{}`",
                command.name()
            );
        }
    }
}
