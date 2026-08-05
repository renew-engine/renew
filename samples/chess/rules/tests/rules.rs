//! The rules perft cannot see: how a game ends, and what a position hashes to.
//!
//! Perft counts positions, so it proves move generation exhaustively and says
//! nothing about whether the game knows it is over. These are the parts a
//! player notices.

use renew_sample_chess_rules::{
    Board, Castling, Colour, Kind, Move, Outcome, Piece, Square, apply, in_check, legal, outcome,
};

/// A square from a name like `e4`.
///
/// The allowance is explicit because the crate's lint configuration exempts
/// `#[test]` bodies and this is a free helper beside them. Failing loudly on a
/// mistyped name is exactly what a fixture should do: a helper that quietly
/// returned a1 instead would make a test pass while checking the wrong square.
#[expect(
    clippy::expect_used,
    reason = "a test fixture, and a mistyped square must fail rather than default"
)]
fn square(name: &str) -> Square {
    let bytes = name.as_bytes();
    let file = i32::from(bytes[0]) - i32::from(b'a');
    let rank = i32::from(bytes[1]) - i32::from(b'1');
    Square::at(file, rank).expect("a name on the board")
}

fn play(board: &Board, from: &str, to: &str) -> Board {
    apply(board, Move::new(square(from), square(to)))
}

#[test]
fn the_initial_position_is_what_everyone_agrees_it_is() {
    let board = Board::initial();
    assert_eq!(board.to_move, Colour::White);
    assert_eq!(board.castling, Castling::ALL);
    assert_eq!(board.en_passant, None);
    assert_eq!(board.fullmove_number, 1);
    assert_eq!(
        board.piece_at(square("e1")),
        Some(Piece::new(Colour::White, Kind::King))
    );
    assert_eq!(
        board.piece_at(square("d8")),
        Some(Piece::new(Colour::Black, Kind::Queen))
    );
    assert_eq!(board.piece_at(square("e4")), None);
    assert_eq!(outcome(&board), Outcome::Ongoing);
    assert!(!in_check(&board));
}

/// The shortest possible checkmate, played move by move.
#[test]
fn the_shortest_checkmate_is_recognised() {
    let mut board = Board::initial();
    for (from, to) in [("f2", "f3"), ("e7", "e5"), ("g2", "g4"), ("d8", "h4")] {
        board = play(&board, from, to);
    }
    assert!(in_check(&board), "the king is attacked");
    assert!(legal(&board).is_empty(), "and cannot answer it");
    assert_eq!(outcome(&board), Outcome::Checkmate);
}

/// No legal move and no check is a draw, not a loss — the distinction the
/// outcome exists to make.
#[test]
fn a_king_with_nowhere_to_go_is_stalemate_not_checkmate() {
    let board = Board::from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1").expect("well-formed");
    assert!(!in_check(&board), "not attacked");
    assert!(legal(&board).is_empty(), "and with nowhere to go");
    assert_eq!(outcome(&board), Outcome::Stalemate);
}

#[test]
fn castling_moves_the_rook_too() {
    let board = Board::from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").expect("well-formed");

    let short = play(&board, "e1", "g1");
    assert_eq!(
        short.piece_at(square("f1")),
        Some(Piece::new(Colour::White, Kind::Rook)),
        "the rook jumped to f1"
    );
    assert_eq!(short.piece_at(square("h1")), None, "and left h1");
    assert!(!short.castling.white_king_side, "the right is spent");
    assert!(!short.castling.white_queen_side, "and so is the other");

    let long = play(&board, "e1", "c1");
    assert_eq!(
        long.piece_at(square("d1")),
        Some(Piece::new(Colour::White, Kind::Rook))
    );
    assert_eq!(long.piece_at(square("a1")), None);
}

/// **The condition people forget**: a king may not pass through an attacked
/// square, which is a different test from the squares either side of it.
#[test]
fn a_king_may_not_castle_through_check() {
    // A black rook on f8 attacks f1, the square the white king crosses.
    let board = Board::from_fen("5r2/8/8/8/8/8/8/R3K2R w KQ - 0 1").expect("well-formed");
    let castles: Vec<String> = legal(&board)
        .as_slice()
        .iter()
        .filter(|m| m.from == square("e1") && (m.to == square("g1") || m.to == square("c1")))
        .map(|m| m.notation())
        .collect();
    assert_eq!(
        castles,
        vec!["e1c1".to_string()],
        "only the queen-side castle survives"
    );
}

#[test]
fn a_king_in_check_may_not_castle_out_of_it() {
    // A black rook on e8 gives check down the e-file.
    let board = Board::from_fen("4r3/8/8/8/8/8/8/R3K2R w KQ - 0 1").expect("well-formed");
    assert!(in_check(&board));
    let castles = legal(&board)
        .as_slice()
        .iter()
        .filter(|m| m.from == square("e1") && (m.to.file() - 4).abs() == 2)
        .count();
    assert_eq!(castles, 0, "castling out of check is never legal");
}

/// A rook captured on its home square ends that side's right, even though
/// neither the king nor the rook moved of its own accord.
#[test]
fn capturing_a_rook_on_its_home_square_ends_that_right() {
    let board = Board::from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").expect("well-formed");
    let after = play(&board, "a1", "a8");
    assert!(
        !after.castling.black_queen_side,
        "black lost the queen-side right with the rook"
    );
    assert!(
        after.castling.black_king_side,
        "and kept the other one, which is the point of four bits"
    );
    assert!(
        !after.castling.white_queen_side,
        "the mover lost its own too"
    );
}

#[test]
fn a_double_push_offers_an_en_passant_square_for_exactly_one_move() {
    let board = Board::initial();
    let after = play(&board, "e2", "e4");
    assert_eq!(
        after.en_passant,
        Some(square("e3")),
        "the square skipped over, not the pawn's"
    );

    let later = play(&after, "b8", "c6");
    assert_eq!(later.en_passant, None, "the chance lasts one move");
}

#[test]
fn an_en_passant_capture_removes_the_pawn_beside_it() {
    let board = Board::from_fen("4k3/3p4/8/4P3/8/8/8/4K3 b - - 0 1").expect("well-formed");
    let pushed = play(&board, "d7", "d5");
    assert_eq!(pushed.en_passant, Some(square("d6")));

    let captured = play(&pushed, "e5", "d6");
    assert_eq!(
        captured.piece_at(square("d6")),
        Some(Piece::new(Colour::White, Kind::Pawn)),
        "the capturer landed behind it"
    );
    assert_eq!(
        captured.piece_at(square("d5")),
        None,
        "and the pawn it passed is gone"
    );
}

#[test]
fn a_pawn_reaching_the_last_rank_may_become_any_of_four_pieces() {
    let board = Board::from_fen("4k3/P7/8/8/8/8/8/4K3 w - - 0 1").expect("well-formed");
    let promotions: Vec<Option<Kind>> = legal(&board)
        .as_slice()
        .iter()
        .filter(|m| m.from == square("a7"))
        .map(|m| m.promotion)
        .collect();
    assert_eq!(promotions.len(), 4, "queen, rook, bishop, knight");
    assert_eq!(
        promotions.first().copied().flatten(),
        Some(Kind::Queen),
        "the queen comes first, so taking the first move promotes to one"
    );
    assert!(
        promotions.contains(&Some(Kind::Knight)),
        "and the knight is there"
    );

    let under = apply(
        &board,
        Move::promoting(square("a7"), square("a8"), Kind::Knight),
    );
    assert_eq!(
        under.piece_at(square("a8")),
        Some(Piece::new(Colour::White, Kind::Knight)),
        "under-promotion is honoured"
    );
}

/// A pinned piece has no moves, and this falls out of the legality filter
/// rather than being a special case anywhere.
#[test]
fn a_pinned_piece_cannot_move() {
    // White knight on e2 pinned to the king on e1 by a black rook on e8.
    let board = Board::from_fen("4r3/8/8/8/8/8/4N3/4K3 w - - 0 1").expect("well-formed");
    let knight_moves = legal(&board)
        .as_slice()
        .iter()
        .filter(|m| m.from == square("e2"))
        .count();
    assert_eq!(knight_moves, 0, "moving it would expose the king");
}

/// The fifty-move rule counts half-moves since the last capture or pawn move,
/// and both reset it.
#[test]
fn the_halfmove_clock_resets_on_a_capture_or_a_pawn_move() {
    let board = Board::from_fen("4k3/8/8/3p4/4P3/8/8/4K3 w - - 40 60").expect("well-formed");
    let quiet = play(&board, "e1", "d1");
    assert_eq!(quiet.halfmove_clock, 41, "a quiet move advances it");

    let pawn = play(&board, "e4", "e5");
    assert_eq!(pawn.halfmove_clock, 0, "a pawn move resets it");

    let capture = play(&board, "e4", "d5");
    assert_eq!(capture.halfmove_clock, 0, "and so does a capture");
}

#[test]
fn a_hundred_half_moves_without_progress_ends_the_game() {
    let board = Board::from_fen("4k3/8/8/8/8/8/8/4K2R w - - 100 60").expect("well-formed");
    assert_eq!(outcome(&board), Outcome::FiftyMove);
}

/// **Two positions that look identical must not hash the same when they play
/// differently.** Castling rights and the en-passant square are the classic
/// pair to forget, and both change what is legal next.
#[test]
fn the_digest_separates_positions_that_play_differently() {
    let base = Board::from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").expect("well-formed");

    let mut fewer_rights = base;
    fewer_rights.castling = Castling {
        white_king_side: false,
        ..base.castling
    };
    assert_ne!(
        base.digest(),
        fewer_rights.digest(),
        "the same pieces with different rights are different positions"
    );

    let mut passant = base;
    passant.en_passant = Some(square("e3"));
    assert_ne!(base.digest(), passant.digest());

    let mut later = base;
    later.halfmove_clock = 99;
    assert_ne!(
        base.digest(),
        later.digest(),
        "the fifty-move clock decides games, so it is state"
    );

    // And the same position hashes the same, or none of the above means
    // anything.
    let twin = Board::from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").expect("well-formed");
    assert_eq!(base.digest(), twin.digest());
}

#[test]
fn a_malformed_position_is_refused_rather_than_guessed() {
    assert!(Board::from_fen("").is_err());
    assert!(
        Board::from_fen("8/8/8/8 w - -").is_err(),
        "four ranks is not eight"
    );
    assert!(
        Board::from_fen("8/8/8/8/8/8/8/9 w - -").is_err(),
        "nine files is not eight"
    );
    assert!(
        Board::from_fen("8/8/8/8/8/8/8/X7 w - -").is_err(),
        "X is not a piece"
    );
    assert!(
        Board::from_fen("8/8/8/8/8/8/8/8 x - -").is_err(),
        "x is not a side"
    );
    assert!(
        Board::from_fen("8/8/8/8/8/8/8/8 w - z9").is_err(),
        "z9 is not a square"
    );
    // And the clocks are optional, because published positions often omit them.
    assert!(Board::from_fen("8/8/8/8/8/8/8/8 w - -").is_ok());
}

/// Notation round-trips through the move list, which is what a recorded game
/// is written in.
#[test]
fn moves_have_readable_names() {
    let board = Board::initial();
    let names: Vec<String> = legal(&board)
        .as_slice()
        .iter()
        .map(|m| m.notation())
        .collect();
    assert_eq!(names.len(), 20);
    assert!(names.contains(&"e2e4".to_string()));
    assert!(names.contains(&"g1f3".to_string()));

    let promotion = Move::promoting(square("a7"), square("a8"), Kind::Knight);
    assert_eq!(promotion.notation(), "a7a8n");
}
