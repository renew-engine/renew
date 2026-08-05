//! The one oracle chess has.
//!
//! These counts are published and independently verified: from a given
//! position, the number of distinct legal games of a given length is a fact
//! about chess, not about this implementation. A missing en-passant case, an
//! off-by-one in castling rights, a promotion that generates three pieces
//! instead of four, a check-evasion that misses a pin — each shows up as a
//! wrong number rather than as a game that feels slightly odd.
//!
//! The positions below are the standard set, chosen because between them they
//! exercise every rule that is easy to get wrong. Position 3 is almost empty
//! and is about pawns and rooks; position 4 is dense with promotions;
//! Kiwipete is the classic castling-and-en-passant torture test.

use renew_sample_chess_rules::{Board, perft};

/// The starting position.
#[test]
fn perft_from_the_initial_position() {
    let board = Board::initial();
    assert_eq!(perft(&board, 0), 1, "depth zero is the position itself");
    assert_eq!(perft(&board, 1), 20);
    assert_eq!(perft(&board, 2), 400);
    assert_eq!(perft(&board, 3), 8_902);
    assert_eq!(perft(&board, 4), 197_281);
}

/// **Kiwipete**, the standard torture test: castling both ways for both sides,
/// pins, and a position where a careless generator invents moves.
#[test]
fn perft_from_kiwipete() {
    let board =
        Board::from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1")
            .expect("a well-formed position");
    assert_eq!(perft(&board, 1), 48);
    assert_eq!(perft(&board, 2), 2_039);
    assert_eq!(perft(&board, 3), 97_862);
}

/// A sparse endgame: pawns, rooks and a king on each side. This is the
/// position that catches en passant discovering a check along a rank — the
/// capture removes two pawns from the same rank at once, which a legality
/// filter that only re-checks the moved piece will miss.
#[test]
fn perft_from_the_sparse_endgame() {
    let board = Board::from_fen("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1")
        .expect("a well-formed position");
    assert_eq!(perft(&board, 1), 14);
    assert_eq!(perft(&board, 2), 191);
    assert_eq!(perft(&board, 3), 2_812);
    assert_eq!(perft(&board, 4), 43_238);
    assert_eq!(perft(&board, 5), 674_624);
}

/// Dense with promotions, including under-promotions that matter: a generator
/// producing only queens gets this badly wrong.
#[test]
fn perft_from_the_promotion_position() {
    let board = Board::from_fen("r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1")
        .expect("a well-formed position");
    assert_eq!(perft(&board, 1), 6);
    assert_eq!(perft(&board, 2), 264);
    assert_eq!(perft(&board, 3), 9_467);
    assert_eq!(perft(&board, 4), 422_333);
}

/// The mirror of the promotion position, with Black to move. A generator with
/// a colour-dependent bug passes one of these and fails the other.
#[test]
fn perft_from_the_mirrored_promotion_position() {
    let board = Board::from_fen("r2q1rk1/pP1p2pp/Q4n2/bbp1p3/Np6/1B3NBn/pPPP1PPP/R3K2R b KQ - 0 1")
        .expect("a well-formed position");
    assert_eq!(perft(&board, 1), 6);
    assert_eq!(perft(&board, 2), 264);
    assert_eq!(perft(&board, 3), 9_467);
}

/// A middlegame with no castling rights left, which isolates ordinary piece
/// movement from the special cases.
#[test]
fn perft_from_a_quiet_middlegame() {
    let board = Board::from_fen("rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8")
        .expect("a well-formed position");
    assert_eq!(perft(&board, 1), 44);
    assert_eq!(perft(&board, 2), 1_486);
    assert_eq!(perft(&board, 3), 62_379);
}

/// One more, from a symmetric position, because a bug that cancels itself in
/// symmetric play still shows up in the counts.
#[test]
fn perft_from_a_symmetric_position() {
    let board =
        Board::from_fen("r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10")
            .expect("a well-formed position");
    assert_eq!(perft(&board, 1), 46);
    assert_eq!(perft(&board, 2), 2_079);
    assert_eq!(perft(&board, 3), 89_890);
}

/// Depth five from the start is four and a half million positions. Too slow
/// for every run, and worth having: it is the depth at which the counts stop
/// being reachable by accident.
#[test]
#[ignore = "four and a half million positions; run with --ignored"]
fn perft_five_from_the_initial_position() {
    assert_eq!(perft(&Board::initial(), 5), 4_865_609);
}
