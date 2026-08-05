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

pub mod camera;
pub mod mesh;
pub mod projection;
#[cfg(feature = "render")]
pub mod render;
#[cfg(feature = "window")]
pub mod windowed;

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
///
/// Not `Eq`: a free viewpoint carries coordinates, and float equality is
/// not the reflexive relation `Eq` promises.
#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "command-line switches are independent by nature; packing them into flags would make the parser and every reader translate"
)]
pub struct Options {
    /// Which built-in script drives the player.
    pub script: Script,
    /// How many ticks to run.
    pub ticks: u32,
    /// Draw the world as two slices through it.
    pub show: bool,
    /// Print the answer as JSON rather than as a sentence.
    pub json: bool,
    /// Print usage and stop.
    pub help: bool,
    /// Play it in a window, rather than running a script.
    pub window: bool,
    /// Which viewpoint `--render` draws from.
    pub view: View,
    /// Draw the world to a PNG at this path, if anywhere.
    ///
    /// **Needs the `render` feature**, which is off by default: the game
    /// a player runs carries no graphics crate. A build without it
    /// refuses the flag by name rather than ignoring it.
    pub render: Option<std::path::PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            script: Script::Stand,
            ticks: 600,
            show: false,
            json: false,
            help: false,
            window: false,
            view: View::Player,
            render: None,
        }
    }
}

/// Where `--render` draws from.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum View {
    /// The player's own eyes.
    ///
    /// **The default, because a picture of what the simulation believes
    /// is evidence about the simulation.** A view from outside shows the
    /// world; this shows what the player would see, so a wrong picture
    /// here means a wrong world rather than a wrong viewpoint.
    #[default]
    Player,
    /// A named point looking at a named point. Explicit rather than
    /// accumulated, so the same command line always draws the same frame.
    Free {
        /// Where the eye is.
        eye: [f32; 3],
        /// What it looks at.
        target: [f32; 3],
    },
    /// The whole world at once, drawn isometrically with the near walls
    /// cut away. Not a camera: there is no eye inside the world, and the
    /// faces turned away are dropped rather than clipped.
    Isometric,
}

/// Parse three comma-separated numbers.
///
/// Public so its refusals can be tested directly: a viewpoint typed
/// wrongly should say so rather than silently becoming the origin.
///
/// # Errors
///
/// [`CliError::NotANumber`] when there are not exactly three parts, or
/// when one of them is not a number.
pub fn triple(text: &str) -> Result<[f32; 3], CliError> {
    let mut out = [0.0f32; 3];
    let mut parts = text.split(',');
    for slot in &mut out {
        let part = parts.next().ok_or(CliError::NotANumber(text.to_string()))?;
        *slot = part
            .trim()
            .parse()
            .map_err(|_| CliError::NotANumber(part.to_string()))?;
    }
    if parts.next().is_some() {
        return Err(CliError::NotANumber(text.to_string()));
    }
    Ok(out)
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
    /// What drove the run: `script` for a scripted one, `window` for a
    /// played one.
    ///
    /// **In the digest line, and that is the point.** A played run is
    /// driven by a person against a wall clock; a scripted one is a pure
    /// function of its inputs. The two digests are not comparable, and a
    /// line that did not say which it was would invite exactly that
    /// comparison. The platformer carries the same field for the same
    /// reason.
    pub source: &'static str,
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
     --show          draw two slices through the world: the plan and the elevation\n\
     --window        play it: WASD walks, arrows look, space jumps\n\
     --render PATH   draw the world to a PNG there (needs --features render)\n\
     --view NAME     player (default) or iso, for --render\n\
     --eye X,Y,Z     draw from here instead, looking at --look-at\n\
     --look-at X,Y,Z where a free view points\n\
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
            "--view" => {
                let name = arguments.next().ok_or(CliError::MissingValue("--view"))?;
                options.view = match name.as_str() {
                    "player" => View::Player,
                    "iso" | "isometric" => View::Isometric,
                    other => return Err(CliError::UnknownFlag(other.to_string())),
                };
            }
            "--eye" => {
                let value = arguments.next().ok_or(CliError::MissingValue("--eye"))?;
                let eye = triple(&value)?;
                options.view = match options.view {
                    View::Free { target, .. } => View::Free { eye, target },
                    _ => View::Free {
                        eye,
                        target: [0.0, 1.0, 0.0],
                    },
                };
            }
            "--look-at" => {
                let value = arguments
                    .next()
                    .ok_or(CliError::MissingValue("--look-at"))?;
                let target = triple(&value)?;
                options.view = match options.view {
                    View::Free { eye, .. } => View::Free { eye, target },
                    _ => View::Free {
                        eye: [0.0, 2.0, -8.0],
                        target,
                    },
                };
            }
            "--render" => {
                let path = arguments.next().ok_or(CliError::MissingValue("--render"))?;
                options.render = Some(std::path::PathBuf::from(path));
            }
            "--window" => options.window = true,
            "--show" => options.show = true,
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
///
/// **A closed box, filled on every face**, because outside the grid is neither
/// solid nor air and a player who leaves falls forever with nothing to land on.
///
/// The shell coincides with the boundary `Grid::set` refuses to clear, so the
/// box cannot be opened from inside. That took three tries. Walls three blocks
/// high let `build` dig through the floor; an unbreakable floor let it build a
/// tower and step over those walls. A player that can place blocks can reach
/// any height, so the only enclosure that holds is one with no face missing.
#[must_use]
pub fn arena() -> Grid {
    let mut grid = Grid::new(Cell::new(-20, 0, -20), (41, 12, 41));
    let (low, high) = (Cell::new(-20, 0, -20), Cell::new(20, 11, 20));
    grid.fill(low, Cell::new(high.x, low.y, high.z), STONE);
    grid.fill(Cell::new(low.x, high.y, low.z), high, STONE);
    grid.fill(low, Cell::new(low.x, high.y, high.z), STONE);
    grid.fill(Cell::new(high.x, low.y, low.z), high, STONE);
    grid.fill(low, Cell::new(high.x, high.y, low.z), STONE);
    grid.fill(Cell::new(low.x, low.y, high.z), high, STONE);

    // A mound inside the box, because the shell cannot be cleared and a
    // digging script with only the shell in reach digs nothing. It sits in
    // front of the start, along the direction the player looks.
    grid.fill(Cell::new(2, 1, -2), Cell::new(6, 2, 2), STONE);
    grid
}

/// Run a script and answer with the world it left behind.
///
/// **Separate from [`run`] because drawing needs the world and a report is not
/// one.** The grid is edited as the script runs — blocks are broken and placed
/// — so a drawing made from `arena()` would show the world as it started
/// rather than as it ended, which is the one thing a picture of it is for.
#[must_use]
pub fn run_world(options: &Options) -> Cube {
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
    world
}

/// Run a script and answer.
#[must_use]
pub fn run(options: &Options) -> Report {
    report_of(options.script, &run_world(options))
}

/// What a finished world answers with.
fn report_of(script: Script, world: &Cube) -> Report {
    report_from(script, world, "script")
}

/// The same report, with the source named.
fn report_from(script: Script, world: &Cube, source: &'static str) -> Report {
    Report {
        script,
        ticks: world.tick(),
        digest: world.digest(),
        solids: world.grid().solid_count(),
        edits: world.edits(),
        grounded: world.grounded(),
        source,
    }
}

/// What a cell is drawn as: the player over the blocks, solid over air.
fn cell_char(world: &Cube, here: Cell, player: Cell) -> char {
    if here == player {
        '@'
    } else if world.grid().is_solid(here) {
        '#'
    } else {
        '.'
    }
}

/// A row label, right-aligned in a four-character gutter so rows line up.
fn gutter(value: i32, into: &mut String) {
    let label = value.to_string();
    for _ in label.len()..4 {
        into.push(' ');
    }
    into.push_str(&label);
    into.push(' ');
}

/// The world seen from above, sliced at the height the player is standing in.
///
/// **The whole grid, not a window onto it.** The arena is forty-one cells
/// across and fits a terminal, so every picture has the same frame and two
/// runs can be compared column for column — which the platformer's view
/// deliberately gives up, its level being wider than a terminal is.
#[must_use]
pub fn plan_text(world: &Cube) -> String {
    let player = Cell::containing(world.position());
    let min = world.grid().min();
    let (width, _, depth) = world.grid().size();
    let mut text = String::new();
    text.push_str("plan, looking down, at y=");
    text.push_str(&player.y.to_string());
    text.push('\n');
    for step in 0..depth {
        let z = min.z + step;
        gutter(z, &mut text);
        for column in 0..width {
            let here = Cell::new(min.x + column, player.y, z);
            text.push(cell_char(world, here, player));
        }
        text.push('\n');
    }
    text.push_str("     x from ");
    text.push_str(&min.x.to_string());
    text.push_str(", z down the side");
    text
}

/// The world seen from the side, sliced at the depth the player is standing
/// at, with height increasing upward the way a reader expects.
#[must_use]
pub fn elevation_text(world: &Cube) -> String {
    let player = Cell::containing(world.position());
    let min = world.grid().min();
    let (width, height, _) = world.grid().size();
    let mut text = String::new();
    text.push_str("elevation, looking along z, at z=");
    text.push_str(&player.z.to_string());
    text.push('\n');
    for step in (0..height).rev() {
        let y = min.y + step;
        gutter(y, &mut text);
        for column in 0..width {
            let here = Cell::new(min.x + column, y, player.z);
            text.push(cell_char(world, here, player));
        }
        text.push('\n');
    }
    text.push_str("     x from ");
    text.push_str(&min.x.to_string());
    text.push_str(", y up the side");
    text
}

/// The one line a run answers with.
#[must_use]
pub fn describe(report: &Report) -> String {
    let (broken, placed) = report.edits;
    format!(
        "cube script={} source={} ticks={} digest=0x{:016x} solids={} broken={} placed={} \
         grounded={}",
        report.script.name(),
        report.source,
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
         \"digest\":\"0x{:016x}\",\"solids\":{},\"broken\":{},\"placed\":{},\"grounded\":{}}}",
        report.script.name(),
        report.ticks,
        report.digest,
        report.solids,
        broken,
        placed,
        report.grounded
    )
}

/// Draw the world to `path`, or say why not.
///
/// # Errors
///
/// A refusal from the renderer, or the message a build without the
/// feature answers with.
#[cfg(feature = "render")]
fn render_to(world: &Cube, view: View, path: &std::path::Path) -> Result<(), String> {
    match view {
        View::Isometric => render::to_png(world.grid(), path),
        View::Player => {
            let camera = camera::player_view(world, 1.0);
            render::to_png_through(world.grid(), &camera, path)
        }
        View::Free { eye, target } => {
            let camera = camera::free_view(eye, target, 1.0);
            render::to_png_through(world.grid(), &camera, path)
        }
    }
    .map_err(|error| error.to_string())
}

/// The honest answer in a build with the rendering stack compiled out.
///
/// Named rather than ignored, and it names both roads: a reader who
/// typed a `renew` command has no use for a cargo flag on its own.
#[cfg(not(feature = "render"))]
fn render_to(_world: &Cube, _view: View, _path: &std::path::Path) -> Result<(), String> {
    Err(
        "this build cannot draw. Run `renew --features render run cube -- --render out.png`, \
         or build it directly with `cargo run -p renew-sample-cube --features render --bin cube \
         -- --render out.png`"
            .to_string(),
    )
}

/// Play it in a window.
///
/// # Errors
///
/// The message a build without the feature answers with, or the reason no
/// window could be opened.
#[cfg(feature = "window")]
fn play(options: &Options) -> Result<Report, String> {
    windowed::run(options).map_err(|error| error.to_string())
}

/// The honest answer in a build with the windowing stack compiled out.
///
/// Names both roads, the tool's first: a reader who typed a `renew`
/// command has no use for a cargo flag on its own.
#[cfg(not(feature = "window"))]
fn play(_options: &Options) -> Result<Report, String> {
    Err(
        "this build has no window. Run `renew --features window run cube -- --window`, or \
         build it directly with `cargo run -p renew-sample-cube --features window --bin cube \
         -- --window`"
            .to_string(),
    )
}

/// Parse, run, print, and answer with an exit code.
///
/// **The whole binary, in the library.** A process shell that did any of this
/// itself would be a piece of the program no test could drive without spawning
/// one — and the parts most worth testing, the refusals, are exactly the parts
/// a spawned process makes hardest to inspect.
pub fn run_cli<I: IntoIterator<Item = String>>(arguments: I) -> u8 {
    // Diagnostics, before anything can fail. A path that cannot be
    // written is said out loud once: silence there would look exactly
    // like a run with nothing to report.
    if let Err(error) = renew_platform::diag::log_to_file(diagnostics_path(), None) {
        eprintln!("RENEW_LOG: {error}");
    }

    let options = match parse(arguments) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("cube: {}", error.message());
            return 1;
        }
    };
    if options.help {
        print!("{}", usage());
        return 0;
    }
    if options.window {
        return match play(&options) {
            Ok(report) => {
                println!("{}", describe(&report));
                0
            }
            Err(message) => {
                eprintln!("usage: {message}");
                2
            }
        };
    }
    let world = run_world(&options);
    let report = report_of(options.script, &world);
    if options.json {
        println!("{}", describe_json(&report));
    } else {
        // The pictures first, then the line: a reader scanning a terminal
        // wants the summary nearest the prompt.
        if options.show {
            println!("{}", plan_text(&world));
            println!();
            println!("{}", elevation_text(&world));
        }
        if let Some(path) = &options.render
            && let Err(error) = render_to(&world, options.view, path)
        {
            eprintln!("usage: {error}");
            return 2;
        }
        println!("{}", describe(&report));
    }
    0
}

/// The file `RENEW_LOG` names, if it names one.
///
/// An environment variable rather than a flag, so a panic that happens
/// before the command line is parsed still has somewhere to go. Absent
/// and empty both mean off.
///
/// There is no validation switch beside it here: this sample touches no
/// device, so what a log carries is a panic and nothing a graphics layer
/// could add to.
#[must_use]
pub fn diagnostics_path() -> Option<std::path::PathBuf> {
    renew_platform::diag::path_from_value(std::env::var_os("RENEW_LOG"))
}
