//! The voxel game's command-line face.
//!
//! # Why a driver at all, when the world is already testable
//!
//! The world is a pure function and its tests assert its digest, which proves
//! it reproduces **on one machine**. The determinism obligation is across
//! machines, and the lane that checks that runs binaries: it needs something
//! it can execute on three platforms and compare the output of.
//!
//! So this is small on purpose. It parses a script name and a tick count, runs
//! the world headless, and prints one line. Everything interesting is in the
//! world; everything here is the seam that lets a machine ask it a question.

use renew_fixed::{Fixed, Vec3};
use renew_sample_cube_world::{Cell, Cube, Grid, Intent, STONE, Tuning};

/// How a run can fail to start.
///
/// Malformed input is an ordinary outcome — it comes from a command line — so
/// it is a value rather than an assertion.
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
                format!("no script called `{name}`; try {}", script_names())
            }
        }
    }
}

/// What to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Which built-in script drives the player.
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

/// A built-in input script.
///
/// **Named rather than supplied as a file**, because the point of this binary
/// is to be run identically on three platforms and compared. A file is one
/// more thing that can differ between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Do nothing but fall and land.
    Stand,
    /// Walk a loop around the arena.
    Patrol,
    /// Walk, jump, dig and place.
    Build,
}

impl Script {
    /// What the player asks for on a given tick.
    #[must_use]
    pub fn intent(self, tick: u32) -> Intent {
        match self {
            Self::Stand => Intent::IDLE,
            Self::Patrol => match (tick / 40) % 4 {
                0 => Intent::walking(1, 0),
                1 => Intent::walking(0, 1),
                2 => Intent::walking(-1, 0),
                _ => Intent::walking(0, -1),
            },
            Self::Build => match tick % 17 {
                0..=4 => Intent::walking(1, 0),
                5 => Intent {
                    jump: true,
                    ..Intent::IDLE
                },
                6..=9 => Intent::walking(0, 1),
                10 => Intent {
                    dig: true,
                    ..Intent::IDLE
                },
                11 => Intent {
                    place: true,
                    ..Intent::IDLE
                },
                _ => Intent::IDLE,
            },
        }
    }

    /// The name this script is asked for by.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stand => "stand",
            Self::Patrol => "patrol",
            Self::Build => "build",
        }
    }

    /// A script from its name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "stand" => Some(Self::Stand),
            "patrol" => Some(Self::Patrol),
            "build" => Some(Self::Build),
            _ => None,
        }
    }
}

/// Every script's name, for a message.
fn script_names() -> String {
    "stand, patrol, build".to_string()
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
    /// How many blocks are left.
    pub solids: usize,
    /// Broken and placed.
    pub edits: (u32, u32),
    /// Whether the player finished standing on something.
    pub grounded: bool,
}

/// The usage text.
///
/// **Every flag the parser accepts appears here**, which the repository checks
/// mechanically — a flag a reader cannot discover is a flag that does not
/// exist as far as anyone but its author is concerned.
#[must_use]
pub fn usage() -> &'static str {
    "cube — run the voxel world headless and answer with a digest\n\
     \n\
     Usage: cube [--script NAME] [--ticks N] [--json] [--help]\n\
     \n\
     --script NAME   which built-in script drives the player: stand, patrol, build\n\
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

/// The arena every script runs in.
///
/// Walled, because outside the grid is not solid and a player who walks off
/// the floor falls forever — which would make `patrol` answer with a digest
/// about falling rather than about walking.
#[must_use]
pub fn arena() -> Grid {
    let mut grid = Grid::new(Cell::new(-20, -2, -20), (41, 14, 41));
    grid.fill(Cell::new(-20, 0, -20), Cell::new(20, 0, 20), STONE);
    grid.fill(Cell::new(-20, 1, -20), Cell::new(-20, 3, 20), STONE);
    grid.fill(Cell::new(20, 1, -20), Cell::new(20, 3, 20), STONE);
    grid.fill(Cell::new(-20, 1, -20), Cell::new(20, 3, -20), STONE);
    grid.fill(Cell::new(-20, 1, 20), Cell::new(20, 3, 20), STONE);
    grid
}

/// Run a script and answer.
#[must_use]
pub fn run(options: &Options) -> Report {
    let start = Vec3::new(Fixed::ZERO, Fixed::from_int(4), Fixed::ZERO);
    let mut world = Cube::new(Tuning::default(), arena(), start);
    // Looking down and forward, so `build` has something to dig at.
    world.look_at(Vec3::new(
        Fixed::ONE,
        Fixed::from_int(-1),
        Fixed::from_ratio(1, 2),
    ));

    for tick in 0..options.ticks {
        world.step(options.script.intent(tick));
    }

    Report {
        script: options.script,
        ticks: world.tick(),
        digest: world.digest(),
        solids: world.grid().solid_count(),
        edits: world.edits(),
        grounded: world.grounded(),
    }
}

/// The one line a run answers with.
#[must_use]
pub fn describe(report: &Report) -> String {
    let (broken, placed) = report.edits;
    format!(
        "cube script={} ticks={} digest={:016x} solids={} broken={} placed={} grounded={}",
        report.script.name(),
        report.ticks,
        report.digest,
        report.solids,
        broken,
        placed,
        report.grounded
    )
}

/// The same answer, machine-readable.
///
/// Carries a `schema_version` from its first release, so a consumer can tell a
/// shape it understands from one it does not.
#[must_use]
pub fn describe_json(report: &Report) -> String {
    let (broken, placed) = report.edits;
    format!(
        "{{\"schema_version\":1,\"sample\":\"cube\",\"script\":\"{}\",\"ticks\":{},\
         \"digest\":\"{:016x}\",\"solids\":{},\"broken\":{},\"placed\":{},\"grounded\":{}}}",
        report.script.name(),
        report.ticks,
        report.digest,
        report.solids,
        broken,
        placed,
        report.grounded
    )
}
