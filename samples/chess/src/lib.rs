//! Chess's command-line face.
//!
//! Two things it can do, and they are not the same kind of thing.
//!
//! **Counting** runs perft — how many distinct legal games of a given length
//! exist from a position. That is a fact about chess with published values, so
//! this mode is the oracle made runnable: a wrong number here is a wrong rule,
//! not a wrong opinion.
//!
//! **Playing** walks a deterministic game and answers with a digest. That
//! proves reproduction rather than correctness, which is what the cross-target
//! lane needs and what perft cannot give it — perft's answer is the same on a
//! broken machine and a working one, because it is a count rather than a state.

use renew_sample_chess_rules::{Board, Move, Outcome, apply, legal, outcome, perft};

/// How a run can fail to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliError {
    /// A flag nobody knows.
    UnknownFlag(String),
    /// A flag that needs a value and did not get one.
    MissingValue(&'static str),
    /// A value that is not a number.
    NotANumber(String),
    /// A position that is not a position.
    BadPosition(String),
    /// A depth deep enough to outlive the caller.
    DepthTooGreat(u32),
}

/// The deepest count this binary will attempt.
///
/// Perft grows by roughly thirty per level, so depth seven from the start is
/// three billion positions and hours of work. A refusal with a number in it is
/// more useful than a command that appears to hang, and a caller who genuinely
/// wants that can call the library.
pub const MAX_DEPTH: u32 = 6;

impl CliError {
    /// What went wrong, for a person.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::UnknownFlag(flag) => format!("unknown flag `{flag}`; try --help"),
            Self::MissingValue(flag) => format!("`{flag}` needs a value"),
            Self::NotANumber(text) => format!("`{text}` is not a number"),
            Self::BadPosition(text) => {
                format!("`{text}` is not a position in Forsyth-Edwards notation")
            }
            Self::DepthTooGreat(depth) => format!(
                "depth {depth} is past the limit of {MAX_DEPTH}; each level is about thirty \
                 times the work of the one before"
            ),
        }
    }
}

/// What to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Count positions to a depth.
    Count,
    /// Play a deterministic game.
    Play,
}

/// What to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Counting or playing.
    pub mode: Mode,
    /// The position to start from.
    pub position: Board,
    /// How deep to count, or how many half-moves to play.
    pub depth: u32,
    /// Print the answer as JSON rather than as a sentence.
    pub json: bool,
    /// Print usage and stop.
    pub help: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::Count,
            position: Board::initial(),
            depth: 4,
            json: false,
            help: false,
        }
    }
}

/// What the run answers with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    /// Which mode ran.
    pub mode: Mode,
    /// The depth counted, or the half-moves played.
    pub depth: u32,
    /// Positions counted, when counting.
    pub nodes: u64,
    /// The final position's hash, when playing.
    pub digest: u64,
    /// How the played game stood at the end.
    pub result: Outcome,
    /// How many half-moves were actually played, which is fewer than asked for
    /// if the game ended.
    pub played: u32,
}

/// The usage text. **Every flag the parser accepts appears here.**
#[must_use]
pub fn usage() -> &'static str {
    "chess — count positions to a depth, or play a deterministic game\n\
     \n\
     Usage: chess [--count | --play] [--depth N] [--fen POSITION] [--json] [--help]\n\
     \n\
     --count         count the legal games of length N from the position (the default)\n\
     --play          play N half-moves, always taking the first legal move\n\
     --depth N       how deep to count, or how many half-moves to play (default 4)\n\
     --fen POSITION  the position, in Forsyth-Edwards notation (default the start)\n\
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
            "--count" => options.mode = Mode::Count,
            "--play" => options.mode = Mode::Play,
            "--depth" => {
                let text = arguments.next().ok_or(CliError::MissingValue("--depth"))?;
                options.depth = text
                    .parse()
                    .map_err(|_| CliError::NotANumber(text.clone()))?;
            }
            "--fen" => {
                let text = arguments.next().ok_or(CliError::MissingValue("--fen"))?;
                options.position =
                    Board::from_fen(&text).map_err(|_| CliError::BadPosition(text.clone()))?;
            }
            other => return Err(CliError::UnknownFlag(other.to_string())),
        }
    }
    // Checked after parsing rather than at the flag, because `--depth 9
    // --play` is fine and `--depth 9 --count` is not — the limit belongs to
    // the mode, and the mode may be named after the depth.
    if options.mode == Mode::Count && options.depth > MAX_DEPTH {
        return Err(CliError::DepthTooGreat(options.depth));
    }
    Ok(options)
}

/// Run, and answer.
#[must_use]
pub fn run(options: &Options) -> Report {
    match options.mode {
        Mode::Count => Report {
            mode: Mode::Count,
            depth: options.depth,
            nodes: perft(&options.position, options.depth),
            digest: options.position.digest(),
            result: outcome(&options.position),
            played: 0,
        },
        Mode::Play => {
            let mut board = options.position;
            let mut played = 0;
            for _ in 0..options.depth {
                // **Always the first legal move**, which is a deterministic
                // choice rather than a good one. A search would make this a
                // test of the search; taking the first makes it a test of the
                // rules and of nothing else.
                let moves = legal(&board);
                let Some(&chosen) = moves.as_slice().first() else {
                    break;
                };
                board = apply(&board, chosen);
                played += 1;
            }
            Report {
                mode: Mode::Play,
                depth: options.depth,
                nodes: 0,
                digest: board.digest(),
                result: outcome(&board),
                played,
            }
        }
    }
}

/// The name a result is printed under.
const fn result_name(result: Outcome) -> &'static str {
    match result {
        Outcome::Checkmate => "checkmate",
        Outcome::Stalemate => "stalemate",
        Outcome::FiftyMove => "fifty-move",
        Outcome::Ongoing => "ongoing",
    }
}

/// The one line a run answers with.
#[must_use]
pub fn describe(report: &Report) -> String {
    match report.mode {
        Mode::Count => format!(
            "chess count depth={} nodes={} result={}",
            report.depth,
            report.nodes,
            result_name(report.result)
        ),
        Mode::Play => format!(
            "chess play moves={} digest=0x{:016x} result={}",
            report.played,
            report.digest,
            result_name(report.result)
        ),
    }
}

/// The same answer, machine-readable, carrying its schema version from the
/// first release.
#[must_use]
pub fn describe_json(report: &Report) -> String {
    let mode = match report.mode {
        Mode::Count => "count",
        Mode::Play => "play",
    };
    format!(
        "{{\"schema_version\":1,\"sample\":\"chess\",\"mode\":\"{}\",\"depth\":{},\
         \"nodes\":{},\"moves\":{},\"digest\":\"0x{:016x}\",\"result\":\"{}\"}}",
        mode,
        report.depth,
        report.nodes,
        report.played,
        report.digest,
        result_name(report.result)
    )
}

/// The move a play would take next, for a caller that wants to follow along.
#[must_use]
pub fn next_move(board: &Board) -> Option<Move> {
    legal(board).as_slice().first().copied()
}

/// Parse, run, print, and answer with an exit code.
pub fn run_cli<I: IntoIterator<Item = String>>(arguments: I) -> u8 {
    let options = match parse(arguments) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("chess: {}", error.message());
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
