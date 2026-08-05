//! Chess's command-line face.
//!
//! Three things it can do, and they are not the same kind of thing.
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
//!
//! **Showing** prints the position for a person, and is the mode that makes
//! this a game rather than a test fixture. It is stateless on purpose: a move
//! is applied to a position named on the command line and the result is
//! written back out in the same notation, so the next invocation can read it.
//! Nothing is stored between runs, which means a game is a shell variable and
//! a saved game is a text file — and every position is reachable directly,
//! without replaying the moves that led to it.

use renew_sample_chess_rules::{
    Board, Colour, Move, Outcome, Square, apply, legal, outcome, perft,
};

/// How a run can fail to start, or fail to finish.
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
    /// Text that is not a move at all, whatever the position.
    NotAMove(String),
    /// A readable move that the rules do not permit here.
    ///
    /// **Separate from [`Self::NotAMove`] because they are different things to
    /// tell a player.** One is a typo; the other is a misunderstanding of the
    /// position, and only the second is worth listing the alternatives for.
    IllegalMove(String),
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
            Self::NotAMove(text) => format!(
                "`{text}` is not a move; moves are the two squares, like `e2e4`, with a \
                 promotion letter for the fifth character, like `a7a8q`"
            ),
            Self::IllegalMove(text) => {
                format!("`{text}` is not legal in this position")
            }
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
    /// Print the position for a person.
    Show,
}

/// What to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Counting, playing or showing.
    pub mode: Mode,
    /// The position to start from.
    pub position: Board,
    /// How deep to count, or how many half-moves to play.
    pub depth: u32,
    /// Moves to apply to the position before the mode does anything.
    ///
    /// **Applied in every mode, not only when showing.** They name a position
    /// by the route to it, which is the form a player has and Forsyth-Edwards
    /// notation is not; counting or playing from there is then the same
    /// question asked somewhere else.
    pub moves: Vec<String>,
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
            moves: Vec::new(),
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
    /// How the position stood at the end.
    pub result: Outcome,
    /// How many half-moves were actually played, which is fewer than asked for
    /// if the game ended.
    pub played: u32,
    /// The position the run ended on.
    ///
    /// **Carried whole rather than as its notation**, so a caller can ask it
    /// further questions without parsing the answer back.
    pub board: Board,
}

/// The usage text. **Every flag the parser accepts appears here.**
#[must_use]
pub fn usage() -> &'static str {
    "chess — show a position, count the games from it, or play one out\n\
     \n\
     Usage: chess [--show | --count | --play] [--move MOVE]... [--fen POSITION]\n\
     \x20             [--depth N] [--json] [--help]\n\
     \n\
     --show          print the position, whose turn it is, and the moves available\n\
     --count         count the legal games of length N from the position (the default)\n\
     --play          play N half-moves, always taking the first legal move\n\
     --move MOVE     a move to apply first, like `e2e4` or `a7a8q`; may be repeated,\n\
     \x20               and implies --show unless a mode is named\n\
     --depth N       how deep to count, or how many half-moves to play (default 4)\n\
     --fen POSITION  the position, in Forsyth-Edwards notation (default the start)\n\
     --json          print the answer as JSON rather than as a sentence\n\
     --help          print this and stop\n\
     \n\
     A game is stateless: each run prints the position it reached, and that text is\n\
     what the next run reads.\n\
     \n\
       chess --show\n\
       chess --move e2e4 --move e7e5\n\
       chess --fen \"$position\" --move g1f3\n"
}

/// Parse a command line.
///
/// # Errors
///
/// Returns what was wrong with it. Moves are checked here for being *moves* and
/// not for being *legal* — legality is a question about a position, and the
/// position may still be named by a later flag.
pub fn parse<I: IntoIterator<Item = String>>(arguments: I) -> Result<Options, CliError> {
    let mut options = Options::default();
    let mut mode_named = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => options.help = true,
            "--json" => options.json = true,
            "--count" => {
                options.mode = Mode::Count;
                mode_named = true;
            }
            "--play" => {
                options.mode = Mode::Play;
                mode_named = true;
            }
            "--show" => {
                options.mode = Mode::Show;
                mode_named = true;
            }
            "--move" => {
                let text = arguments.next().ok_or(CliError::MissingValue("--move"))?;
                if Move::from_notation(&text).is_none() {
                    return Err(CliError::NotAMove(text));
                }
                options.moves.push(text);
            }
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

    // Someone who names a move and no mode is playing chess, not measuring it.
    if !mode_named && !options.moves.is_empty() {
        options.mode = Mode::Show;
    }

    // Checked after parsing rather than at the flag, because `--depth 9
    // --play` is fine and `--depth 9 --count` is not — the limit belongs to
    // the mode, and the mode may be named after the depth.
    if options.mode == Mode::Count && options.depth > MAX_DEPTH {
        return Err(CliError::DepthTooGreat(options.depth));
    }
    Ok(options)
}

/// Apply the named moves to a position, refusing the first one the rules do
/// not permit.
///
/// # Errors
///
/// Returns the move that could not be played, by the text the caller wrote.
fn advance(mut board: Board, moves: &[String]) -> Result<Board, CliError> {
    for text in moves {
        let Some(wanted) = Move::from_notation(text) else {
            return Err(CliError::NotAMove(text.clone()));
        };
        // **Matched against the generated list rather than applied
        // directly.** `apply` trusts its argument, and a move that merely
        // parses can name an empty square or the opponent's piece; asking the
        // rules whether this exact move is among the legal ones is the only
        // check that covers every way it could be wrong.
        if !legal(&board).as_slice().contains(&wanted) {
            return Err(CliError::IllegalMove(text.clone()));
        }
        board = apply(&board, wanted);
    }
    Ok(board)
}

/// Run, and answer.
///
/// # Errors
///
/// Returns the first named move the rules do not permit.
pub fn run(options: &Options) -> Result<Report, CliError> {
    let start = advance(options.position, &options.moves)?;
    Ok(match options.mode {
        Mode::Count => Report {
            mode: Mode::Count,
            depth: options.depth,
            nodes: perft(&start, options.depth),
            digest: start.digest(),
            result: outcome(&start),
            played: 0,
            board: start,
        },
        Mode::Show => Report {
            mode: Mode::Show,
            depth: 0,
            nodes: legal(&start).as_slice().len() as u64,
            digest: start.digest(),
            result: outcome(&start),
            played: 0,
            board: start,
        },
        Mode::Play => {
            let mut board = start;
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
                board,
            }
        }
    })
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

/// The letter a square is drawn with: upper case for White, lower for Black,
/// a dot for empty — the same convention as Forsyth-Edwards notation, so a
/// reader who can read one can read the other.
fn square_letter(board: &Board, file: i32, rank: i32) -> char {
    match Square::at(file, rank).and_then(|square| board.piece_at(square)) {
        Some(piece) if piece.colour == Colour::White => piece.kind.letter().to_ascii_uppercase(),
        Some(piece) => piece.kind.letter(),
        None => '.',
    }
}

/// The board, drawn for a person, always from White's side.
///
/// **Always White's side, never flipped to follow the side to move.** A board
/// that turns around between moves makes two consecutive positions
/// incomparable by eye, which is the one thing a printed board is for.
#[must_use]
pub fn board_text(board: &Board) -> String {
    let mut text = String::with_capacity(200);
    for rank in (0..8).rev() {
        text.push(char::from(b'1' + u8::try_from(rank).unwrap_or(0)));
        text.push(' ');
        for file in 0..8 {
            text.push(' ');
            text.push(square_letter(board, file, rank));
        }
        text.push('\n');
    }
    text.push_str("\n   a b c d e f g h");
    text
}

/// The one line — or, when showing, the several — a run answers with.
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
        Mode::Show => {
            let to_move = if report.board.to_move == Colour::White {
                "white"
            } else {
                "black"
            };
            format!(
                "{}\n\n{to_move} to move, {} legal {}, {}\n{}",
                board_text(&report.board),
                report.nodes,
                if report.nodes == 1 { "move" } else { "moves" },
                result_name(report.result),
                report.board.to_fen()
            )
        }
    }
}

/// The same answer, machine-readable, carrying its schema version from the
/// first release.
#[must_use]
pub fn describe_json(report: &Report) -> String {
    let mode = match report.mode {
        Mode::Count => "count",
        Mode::Play => "play",
        Mode::Show => "show",
    };
    format!(
        "{{\"schema_version\":1,\"sample\":\"chess\",\"mode\":\"{}\",\"depth\":{},\
         \"nodes\":{},\"moves\":{},\"digest\":\"0x{:016x}\",\"result\":\"{}\",\
         \"to_move\":\"{}\",\"fen\":\"{}\"}}",
        mode,
        report.depth,
        report.nodes,
        report.played,
        report.digest,
        result_name(report.result),
        if report.board.to_move == Colour::White {
            "white"
        } else {
            "black"
        },
        report.board.to_fen()
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
    let report = match run(&options) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("chess: {}", error.message());
            return 1;
        }
    };
    if options.json {
        println!("{}", describe_json(&report));
    } else {
        println!("{}", describe(&report));
    }
    0
}
