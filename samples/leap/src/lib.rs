//! The platformer's command-line face.
//!
//! Small on purpose, and for the same reason as the voxel sample's: the world
//! is a pure function whose tests prove it reproduces **on one machine**, and
//! the obligation is across machines. The lane that checks that runs binaries,
//! so it needs something executable whose output can be compared.

use renew_fixed::{Fixed, Vec2};
use renew_sample_leap_world::{CHARACTER_HALF_EXTENTS, Intent, Leap, Platform, Tuning};

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
    /// Draw the level and the character where it ended up.
    pub show: bool,
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
            show: false,
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
    /// Where the character ended up.
    ///
    /// **Carried so that drawing needs no second run.** The alternative is a
    /// caller stepping the world again to find out, which is the same
    /// simulation done twice and one chance for the two to disagree.
    pub position: Vec2,
}

/// The usage text. **Every flag the parser accepts appears here.**
#[must_use]
pub fn usage() -> &'static str {
    "leap — run the platformer headless and answer with a digest\n\
     \n\
     Usage: leap [--script NAME] [--ticks N] [--json] [--help]\n\
     \n\
     --script NAME   which built-in script drives the character: stand, dash, hop\n\
     --show          draw the level and where the character ended up\n\
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
            "--show" => options.show = true,
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
        position: world.position(),
    }
}

/// How wide and tall a drawn view is, in world units — one character each.
///
/// Odd on both axes so the character sits in the exact middle rather than
/// half a cell off it, and small enough that a view fits an ordinary terminal
/// without wrapping. The level is wider than this, which is the point: the
/// view follows the character rather than showing everything.
const VIEW_WIDTH: i64 = 61;
/// How tall a drawn view is. See [`VIEW_WIDTH`].
const VIEW_HEIGHT: i64 = 17;

/// The whole part of a coordinate, rounded toward negative infinity.
///
/// **Not [`Fixed::trunc_int`], which rounds toward zero.** Truncation maps
/// everything in (-1, 1) onto the cell `0`, so the two cells either side of
/// the origin become one and every drawn thing left of it is off by one. That
/// is invisible in a level built to the right of the origin and wrong the
/// moment one is not — this level has a ledge at x = -14.
fn floor_int(value: Fixed) -> i64 {
    let whole = value.trunc_int();
    if value.fract() < Fixed::ZERO {
        whole - 1
    } else {
        whole
    }
}

/// Whether a box covers the unit cell whose lower-left corner is `(x, y)`.
///
/// Cells are half-open — `[x, x + 1)` — so a box ending exactly on a cell
/// boundary does not bleed into the cell beyond it, and two boxes meeting at a
/// boundary do not both claim the same cell.
///
/// **One test for the character and the platforms alike.** They are the same
/// kind of thing to the simulation, and a drawing that treated them
/// differently would be able to disagree with it about which of them is
/// touching what.
fn covers(centre: Vec2, half_extents: Vec2, x: i64, y: i64) -> bool {
    let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
        return false;
    };
    let left = Fixed::from_int(x);
    let bottom = Fixed::from_int(y);
    let right = left + Fixed::ONE;
    let top = bottom + Fixed::ONE;
    centre.x - half_extents.x < right
        && centre.x + half_extents.x > left
        && centre.y - half_extents.y < top
        && centre.y + half_extents.y > bottom
}

/// The level drawn around the character: `#` is solid, `@` is the character,
/// `.` is air.
///
/// **The view follows the character rather than framing the level**, because
/// a level is allowed to be larger than a terminal and a character that walks
/// off the drawing tells the reader nothing. The consequence is that two
/// pictures from different ticks are not directly comparable by eye — the
/// coordinates on the left edge are what makes them comparable, so they are
/// printed.
#[must_use]
pub fn world_text(platforms: &[Platform], position: Vec2) -> String {
    let centre_x = floor_int(position.x);
    let centre_y = floor_int(position.y);
    let mut text =
        String::with_capacity(usize::try_from((VIEW_WIDTH + 8) * VIEW_HEIGHT).unwrap_or(1024));
    for row in 0..VIEW_HEIGHT {
        let y = centre_y + VIEW_HEIGHT / 2 - row;
        // Right-aligned in a four-character gutter, built by hand rather than
        // formatted: the rows must line up, and this is the whole of that
        // requirement.
        let label = y.to_string();
        for _ in label.len()..4 {
            text.push(' ');
        }
        text.push_str(&label);
        text.push(' ');
        for column in 0..VIEW_WIDTH {
            let x = centre_x - VIEW_WIDTH / 2 + column;
            // The character is drawn over the level rather than under it, so
            // it is never hidden by the floor it is standing on.
            if covers(position, CHARACTER_HALF_EXTENTS, x, y) {
                text.push('@');
            } else if platforms
                .iter()
                .any(|platform| covers(platform.centre, platform.half_extents, x, y))
            {
                text.push('#');
            } else {
                text.push('.');
            }
        }
        text.push('\n');
    }
    text.push_str("     x=");
    text.push_str(&floor_int(position.x).to_string());
    text.push_str(" y=");
    text.push_str(&floor_int(position.y).to_string());
    text
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
        // The picture first, then the line: a reader scanning a terminal
        // wants the summary nearest the prompt.
        if options.show {
            println!("{}", world_text(&level(), report.position));
        }
        println!("{}", describe(&report));
    }
    0
}
