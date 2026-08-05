//! The platformer's command-line face.
//!
//! Small on purpose, and for the same reason as the voxel sample's: the world
//! is a pure function whose tests prove it reproduces **on one machine**, and
//! the obligation is across machines. The lane that checks that runs binaries,
//! so it needs something executable whose output can be compared.

use renew_fixed::{Fixed, Vec2};
use renew_sample_leap_world::{Intent, Leap, Platform, Tuning};

/// How a run can fail to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliError {
    /// A flag nobody knows.
    UnknownFlag(String),
    /// A flag that needs a value and did not get one.
    MissingValue(&'static str),
    /// A value that is not a number.
    NotANumber(String),
    /// A script name that names no script.
    UnknownScript(String),
}

impl CliError {
    /// What went wrong, for a person.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::UnknownFlag(flag) => format!("unknown flag `{flag}`; try --help"),
            Self::MissingValue(flag) => format!("`{flag}` needs a value"),
            Self::NotANumber(text) => format!("`{text}` is not a number"),
            Self::UnknownScript(name) => {
                format!("no script called `{name}`; try stand, dash, hop")
            }
        }
    }
}

/// A built-in input script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Fall, land, and stay put.
    Stand,
    /// Run right into the wall, then back.
    Dash,
    /// Run and jump repeatedly.
    Hop,
}

impl Script {
    /// What the player asks for on a given tick.
    #[must_use]
    pub fn intent(self, tick: u32) -> Intent {
        match self {
            Self::Stand => Intent::IDLE,
            Self::Dash => {
                if (tick / 120).is_multiple_of(2) {
                    Intent::running(1)
                } else {
                    Intent::running(-1)
                }
            }
            Self::Hop => match tick % 24 {
                0..=9 => Intent::running(1),
                10 => Intent::jumping(1),
                11..=20 => Intent::running(-1),
                21 => Intent::jumping(-1),
                _ => Intent::IDLE,
            },
        }
    }

    /// The name this script is asked for by.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stand => "stand",
            Self::Dash => "dash",
            Self::Hop => "hop",
        }
    }

    /// A script from its name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "stand" => Some(Self::Stand),
            "dash" => Some(Self::Dash),
            "hop" => Some(Self::Hop),
            _ => None,
        }
    }
}

/// What to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Which built-in script drives the character.
    pub script: Script,
    /// How many ticks to run.
    pub ticks: u32,
    /// Print the answer as JSON rather than as a sentence.
    pub json: bool,
    /// Print usage and stop.
    pub help: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            script: Script::Stand,
            ticks: 600,
            json: false,
            help: false,
        }
    }
}

/// What the run answers with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    /// Which script ran.
    pub script: Script,
    /// How many ticks it ran for.
    pub ticks: u64,
    /// The world's hash at the end.
    pub digest: u64,
    /// Whether the character finished on the ground.
    pub grounded: bool,
    /// Whether it finished against a wall.
    pub against_wall: bool,
}

/// The usage text. **Every flag the parser accepts appears here.**
#[must_use]
pub fn usage() -> &'static str {
    "leap — run the platformer headless and answer with a digest\n\
     \n\
     Usage: leap [--script NAME] [--ticks N] [--json] [--help]\n\
     \n\
     --script NAME   which built-in script drives the character: stand, dash, hop\n\
     --ticks N       how many ticks to run (default 600)\n\
     --json          print the answer as JSON rather than as a sentence\n\
     --help          print this and stop\n"
}

/// Parse a command line.
///
/// # Errors
///
/// Returns what was wrong with it.
pub fn parse<I: IntoIterator<Item = String>>(arguments: I) -> Result<Options, CliError> {
    let mut options = Options::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => options.help = true,
            "--json" => options.json = true,
            "--script" => {
                let name = arguments.next().ok_or(CliError::MissingValue("--script"))?;
                options.script =
                    Script::from_name(&name).ok_or(CliError::UnknownScript(name.clone()))?;
            }
            "--ticks" => {
                let text = arguments.next().ok_or(CliError::MissingValue("--ticks"))?;
                options.ticks = text
                    .parse()
                    .map_err(|_| CliError::NotANumber(text.clone()))?;
            }
            other => return Err(CliError::UnknownFlag(other.to_string())),
        }
    }
    Ok(options)
}

/// The level every script runs in: a floor, a wall to run into, and a ledge to
/// walk off.
#[must_use]
pub fn level() -> Vec<Platform> {
    vec![
        Platform::new(0, 0, 40, 1),
        Platform::new(10, 5, 1, 4),
        Platform::new(-14, 4, 3, 1),
    ]
}

/// Run a script and answer.
#[must_use]
pub fn run(options: &Options) -> Report {
    let start = Vec2::new(Fixed::ZERO, Fixed::from_int(6));
    let mut world = Leap::new(Tuning::default(), start, &level());
    for tick in 0..options.ticks {
        world.step(options.script.intent(tick));
    }
    Report {
        script: options.script,
        ticks: world.tick(),
        digest: world.digest(),
        grounded: world.footing().grounded,
        against_wall: world.footing().against_wall,
    }
}

/// The one line a run answers with.
#[must_use]
pub fn describe(report: &Report) -> String {
    format!(
        "leap script={} ticks={} digest=0x{:016x} grounded={} wall={}",
        report.script.name(),
        report.ticks,
        report.digest,
        report.grounded,
        report.against_wall
    )
}

/// The same answer, machine-readable, carrying its schema version from the
/// first release.
#[must_use]
pub fn describe_json(report: &Report) -> String {
    format!(
        "{{\"schema_version\":1,\"sample\":\"leap\",\"script\":\"{}\",\"ticks\":{},\
         \"digest\":\"0x{:016x}\",\"grounded\":{},\"against_wall\":{}}}",
        report.script.name(),
        report.ticks,
        report.digest,
        report.grounded,
        report.against_wall
    )
}

/// Parse, run, print, and answer with an exit code.
///
/// The whole binary, in the library, so a test can drive every refusal without
/// spawning a process to inspect.
pub fn run_cli<I: IntoIterator<Item = String>>(arguments: I) -> u8 {
    let options = match parse(arguments) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("leap: {}", error.message());
            return 1;
        }
    };
    if options.help {
        print!("{}", usage());
        return 0;
    }
    let report = run(&options);
    if options.json {
        println!("{}", describe_json(&report));
    } else {
        println!("{}", describe(&report));
    }
    0
}
