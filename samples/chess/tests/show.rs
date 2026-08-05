//! Showing a position — the mode that makes this a game rather than a fixture.
//!
//! The claim under test is that **a game survives being split across
//! invocations**. Nothing is stored between runs, so each one prints the
//! position it reached and the next one reads that text; if the pair of
//! notation functions were wrong anywhere, a game played in two halves would
//! end somewhere a game played in one would not. That is the test this file
//! exists for, and the rest support it.

use renew_sample_chess::{
    CliError, Mode, Options, Report, board_text, describe, describe_json, parse, run, run_cli,
    usage,
};
use renew_sample_chess_rules::{Board, Colour, Square};

fn args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

/// `run` for setups that are legal by construction, so the tests below are not
/// each obliged to say so. The ones about refusals call `run` itself.
#[expect(
    clippy::expect_used,
    reason = "a test helper: a panic here is the failure being reported"
)]
fn ran(options: &Options) -> Report {
    run(options).expect("a setup with no illegal move in it")
}

fn showing(moves: &[&str]) -> Report {
    ran(&Options {
        mode: Mode::Show,
        moves: moves.iter().map(|m| (*m).to_string()).collect(),
        ..Options::default()
    })
}

/// The drawn board agrees with the position it was drawn from, square by
/// square.
///
/// **Checked against `piece_at` rather than against a literal picture.** A
/// literal would be a second copy of the starting position that could drift
/// from the first; asking the board what is on each square makes this a test
/// of the drawing and of nothing else.
#[test]
fn the_drawn_board_agrees_with_the_position() {
    let board = Board::initial();
    let text = board_text(&board);
    let rows: Vec<&str> = text.lines().take(8).collect();
    assert_eq!(rows.len(), 8, "eight ranks");

    for (row, rank) in rows.iter().zip((0..8).rev()) {
        let drawn: Vec<char> = row.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(drawn.len(), 9, "a rank label and eight squares");
        assert_eq!(
            drawn[0],
            char::from(b'1' + u8::try_from(rank).unwrap_or(0)),
            "the rank is labelled"
        );
        for file in 0..8 {
            let square = Square::at(file, rank).expect("on the board");
            let expected = match board.piece_at(square) {
                Some(piece) if piece.colour == Colour::White => {
                    piece.kind.letter().to_ascii_uppercase()
                }
                Some(piece) => piece.kind.letter(),
                None => '.',
            };
            let at = usize::try_from(file).unwrap_or(0) + 1;
            assert_eq!(
                drawn[at], expected,
                "file {file} of rank {rank} was drawn as `{}`",
                drawn[at]
            );
        }
    }
    assert!(text.ends_with("a b c d e f g h"), "the files are labelled");
}

/// **The board is drawn from White's side whoever is to move**, so two
/// consecutive positions can be compared by eye.
#[test]
fn the_board_does_not_turn_around_between_moves() {
    let first_rank = |report: &Report| {
        board_text(&report.board)
            .lines()
            .next()
            .expect("eight ranks")
            .to_string()
    };
    assert!(first_rank(&showing(&[])).starts_with('8'));
    assert!(
        first_rank(&showing(&["e2e4"])).starts_with('8'),
        "rank eight is still drawn first with Black to move"
    );
}

/// **The claim this file exists for.** A game played across two invocations,
/// carried only by the text one printed and the other read, reaches the same
/// position as the same game played in one.
#[test]
fn a_game_continues_through_its_own_output() {
    let opening = showing(&["e2e4", "e7e5"]);
    let carried = opening.board.to_fen();

    let continued = ran(&Options {
        mode: Mode::Show,
        position: Board::from_fen(&carried).expect("what the last run wrote"),
        moves: vec!["g1f3".to_string()],
        ..Options::default()
    });
    let direct = showing(&["e2e4", "e7e5", "g1f3"]);

    assert_eq!(
        continued.digest, direct.digest,
        "a game split across two runs reached a different position than one run"
    );
    assert_eq!(continued.board.to_fen(), direct.board.to_fen());
}

/// The same claim over a longer game, so a field that only differs after
/// castling or a capture cannot hide.
///
/// **Twenty half-moves, handing the position over at every one.** The clocks
/// and the castling rights are the fields most likely to survive one exchange
/// and not twenty.
#[test]
fn a_long_game_survives_being_handed_over_at_every_move() {
    let mut carried = Board::initial();
    let mut in_one = Board::initial();
    for step in 0..20 {
        let Some(chosen) = renew_sample_chess::next_move(&in_one) else {
            break;
        };
        in_one = renew_sample_chess_rules::apply(&in_one, chosen);

        // The same move, but through the notation and the text both ways.
        let report = ran(&Options {
            mode: Mode::Show,
            position: carried,
            moves: vec![chosen.notation()],
            ..Options::default()
        });
        carried = Board::from_fen(&report.board.to_fen()).expect("what the run wrote");

        assert_eq!(
            carried.digest(),
            in_one.digest(),
            "the games diverged at half-move {step}"
        );
    }
    assert_eq!(carried.to_fen(), in_one.to_fen());
}

/// **Moves apply in every mode, not only when showing** — they name a position
/// by the route to it, and counting from there is the same question asked
/// elsewhere.
#[test]
fn moves_apply_before_counting_too() {
    let after_e4 = ran(&Options {
        mode: Mode::Count,
        depth: 2,
        moves: vec!["e2e4".to_string()],
        ..Options::default()
    });
    let same_by_fen = ran(&Options {
        mode: Mode::Count,
        depth: 2,
        position: Board::from_fen("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1")
            .expect("well formed"),
        ..Options::default()
    });
    assert_eq!(after_e4.nodes, same_by_fen.nodes);
    assert_eq!(after_e4.digest, same_by_fen.digest);
}

/// Naming a move and no mode means the caller is playing chess, not measuring
/// it.
#[test]
fn a_move_with_no_mode_named_shows_the_board() {
    assert_eq!(
        parse(args("--move e2e4")).expect("well formed").mode,
        Mode::Show
    );
    assert_eq!(
        parse(args("--count --move e2e4"))
            .expect("well formed")
            .mode,
        Mode::Count,
        "a named mode wins"
    );
    assert_eq!(
        parse(args("--move e2e4 --count"))
            .expect("well formed")
            .mode,
        Mode::Count,
        "and it wins on whichever side of the move it is named"
    );
    assert_eq!(
        parse(Vec::new()).expect("well formed").mode,
        Mode::Count,
        "with no move and no mode, counting is still the default"
    );
}

/// **Text that is not a move and a move that is not legal are different
/// refusals**, because they are different mistakes: one is a typo, the other a
/// misreading of the position. Only the second needs a position to detect,
/// which is why they are refused at different times.
#[test]
fn the_two_kinds_of_bad_move_are_refused_differently() {
    assert_eq!(
        parse(args("--move wat")),
        Err(CliError::NotAMove("wat".to_string()))
    );
    assert_eq!(
        parse(args("--move e2e9")),
        Err(CliError::NotAMove("e2e9".to_string()))
    );
    assert_eq!(parse(args("--move")), Err(CliError::MissingValue("--move")));

    let options = parse(args("--move e2e5")).expect("it parses as a move");
    assert_eq!(
        run(&options),
        Err(CliError::IllegalMove("e2e5".to_string()))
    );

    // The third of three, so the refusal is not merely about the first.
    let options = parse(args("--move e2e4 --move e7e6 --move e2e4")).expect("all parse");
    assert_eq!(
        run(&options),
        Err(CliError::IllegalMove("e2e4".to_string())),
        "the pawn has already moved, so playing it again is illegal"
    );

    for (error, subject) in [
        (CliError::NotAMove("wat".to_string()), "wat"),
        (CliError::IllegalMove("e2e5".to_string()), "e2e5"),
    ] {
        assert!(
            error.message().contains(subject),
            "the refusal does not name `{subject}`: {}",
            error.message()
        );
    }
}

/// A move that is legal for the other side is refused, not silently taken.
///
/// **The case a naive check misses**: `e7e5` is a real move by a real piece,
/// and only the side to move makes it wrong.
#[test]
fn a_move_by_the_wrong_side_is_refused() {
    let options = parse(args("--move e7e5")).expect("it parses as a move");
    assert_eq!(
        run(&options),
        Err(CliError::IllegalMove("e7e5".to_string()))
    );
}

/// Showing reports how many moves are available, and it agrees with the rules.
#[test]
fn showing_counts_the_moves_available() {
    let start = showing(&[]);
    assert_eq!(start.nodes, 20, "twenty first moves");
    assert!(describe(&start).contains("white to move, 20 legal moves"));

    let mated = ran(&Options {
        mode: Mode::Show,
        position: Board::from_fen("rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3")
            .expect("well formed"),
        ..Options::default()
    });
    assert_eq!(mated.nodes, 0);
    assert!(describe(&mated).contains("checkmate"));

    // The singular is not "1 legal moves".
    let one = ran(&Options {
        mode: Mode::Show,
        position: Board::from_fen("7k/8/5K2/8/8/8/8/7R b - - 0 1").expect("well formed"),
        ..Options::default()
    });
    assert_eq!(one.nodes, 1);
    assert!(
        describe(&one).contains("1 legal move,"),
        "expected the singular: {}",
        describe(&one)
    );
}

/// What showing prints is what a person needs and what the next run reads.
#[test]
fn the_shown_answer_reads_and_parses() {
    let report = showing(&["d2d4"]);

    let text = describe(&report);
    assert!(text.contains("black to move"));
    assert!(
        Board::from_fen(text.lines().last().expect("a last line")).is_ok(),
        "the last line is the position, ready for the next run"
    );

    let json = describe_json(&report);
    assert!(json.contains("\"schema_version\":1"));
    assert!(json.contains("\"mode\":\"show\""));
    assert!(json.contains("\"to_move\":\"black\""));
    assert!(json.contains(&format!("\"fen\":\"{}\"", report.board.to_fen())));
    assert!(json.starts_with('{') && json.ends_with('}'));
}

/// Every mode carries the position it ended on, so a caller need not parse the
/// answer back to ask another question.
#[test]
fn every_mode_reports_the_position_it_ended_on() {
    let counted = ran(&Options {
        depth: 1,
        ..Options::default()
    });
    assert_eq!(counted.board.to_fen(), Board::initial().to_fen());

    let played = ran(&Options {
        mode: Mode::Play,
        depth: 4,
        ..Options::default()
    });
    assert_ne!(played.board.to_fen(), Board::initial().to_fen());
    assert_eq!(played.board.digest(), played.digest);
}

/// **Every flag the parser accepts appears in the usage text**, including the
/// ones added with this mode.
#[test]
fn the_usage_text_documents_the_new_flags() {
    let text = usage();
    for flag in ["--show", "--move", "--count", "--play", "--fen"] {
        assert!(text.contains(flag), "usage does not mention {flag}");
    }
}

/// The whole binary, driven without spawning one, over the new surface.
#[test]
fn the_command_line_shows_and_refuses() {
    assert_eq!(run_cli(args("--show")), 0);
    assert_eq!(run_cli(args("--move e2e4")), 0);
    assert_eq!(run_cli(args("--move e2e4 --json")), 0);
    assert_eq!(run_cli(args("--move wat")), 1, "not a move");
    assert_eq!(run_cli(args("--move e2e5")), 1, "not legal");
    assert_eq!(run_cli(args("--move")), 1, "no value");
}

/// **`run` checks the moves it is given rather than trusting that `parse`
/// checked them.**
///
/// The two are separate public entry points, and a caller building `Options`
/// directly — which is what every test in this file does — never goes through
/// the parser. If `run` assumed otherwise, `apply` would be handed text that
/// names no move at all.
#[test]
fn run_refuses_a_non_move_that_never_passed_the_parser() {
    let options = Options {
        mode: Mode::Show,
        moves: vec!["e2e4".to_string(), "wat".to_string()],
        ..Options::default()
    };
    assert_eq!(run(&options), Err(CliError::NotAMove("wat".to_string())));
}
