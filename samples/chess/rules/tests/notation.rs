//! The three round trips, and what each one catches that the other direction
//! alone cannot.
//!
//! Every pair here is a reader and a writer of the same notation. A test that
//! only reads checks the reader against the author's belief about the text; a
//! test that only writes checks the writer against the same belief. **Held
//! against each other they check a fact instead** — that whatever one produces,
//! the other accepts and returns unchanged — and that fact is false the moment
//! either side handles a field the other does not.

use renew_sample_chess_rules::{Board, Colour, Kind, Move, Square, apply, legal};

/// Positions with something awkward in them, deliberately chosen so the round
/// trip has fields to get wrong: castling rights partly gone, an en-passant
/// square live, clocks that are not zero, and an empty-heavy board that
/// exercises the run-length digits at both ends of a rank.
const POSITIONS: [&str; 8] = [
    "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    "rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR w KQkq f6 0 3",
    "4k3/8/8/8/8/8/8/4K2R w K - 99 60",
    "4k3/8/8/8/8/8/8/R3K3 b Q - 13 7",
    "8/8/8/8/8/8/8/K6k w - - 0 1",
    "r3k2r/8/8/8/8/8/8/R3K2R b KQkq - 4 12",
];

/// **A square's name and the square it names are inverses over the whole
/// board.** Sixty-four is small enough to check every one rather than sample,
/// so this is exhaustive rather than a property.
#[test]
fn every_square_name_round_trips() {
    for index in 0..64u8 {
        let square = Square::from_index(index).expect("0..64 is on the board");
        let name: String = square.name().iter().collect();
        assert_eq!(
            Square::from_name(&name),
            Some(square),
            "`{name}` did not read back as the square that wrote it"
        );
    }
}

/// The reader refuses what is not a square, rather than clamping into one.
///
/// **`i9` and `a0` are the cases worth having**: both are two characters in
/// the right shape, and an implementation that added offsets to an index
/// without a bounds check would map them onto real squares.
#[test]
fn a_name_that_names_no_square_is_refused() {
    for text in ["", "e", "e44", "i4", "a0", "a9", "z1", "44", "-", " e4"] {
        assert_eq!(
            Square::from_name(text),
            None,
            "`{text}` was read as a square"
        );
    }
}

/// **Every letter a kind writes reads back as that kind, in either case.**
/// The case-insensitivity is load-bearing: Forsyth-Edwards notation uses case
/// to carry colour, so the letter table must not.
#[test]
fn every_piece_letter_round_trips_in_both_cases() {
    for kind in [
        Kind::Pawn,
        Kind::Knight,
        Kind::Bishop,
        Kind::Rook,
        Kind::Queen,
        Kind::King,
    ] {
        let letter = kind.letter();
        assert_eq!(Kind::from_letter(letter), Some(kind));
        assert_eq!(Kind::from_letter(letter.to_ascii_uppercase()), Some(kind));
    }
    for letter in ['x', '1', ' ', '-'] {
        assert_eq!(Kind::from_letter(letter), None, "`{letter}` read as a kind");
    }
}

/// **The published positions survive a round trip through the writer.**
///
/// This is the test that catches a field which reads correctly and writes
/// wrongly — the half of the pair that the perft suite, which only ever reads,
/// is blind to. Castling rights and the en-passant square are the two most
/// likely to be dropped, and both appear here partly set rather than all or
/// nothing.
#[test]
fn every_position_survives_a_round_trip() {
    for fen in POSITIONS {
        let board = Board::from_fen(fen).expect("a well-formed position");
        assert_eq!(
            board.to_fen(),
            fen,
            "the position did not write back as the text it was read from"
        );
    }
}

/// The other direction of the same pair: what the writer produces, the reader
/// accepts, and it means the same position.
///
/// **Compared by digest rather than by text**, so this cannot pass by both
/// sides sharing a mistake in one field — the digest covers the whole state,
/// including the pieces the text round trip would also catch and the clocks
/// it might not.
#[test]
fn a_written_position_reads_back_as_the_same_position() {
    let mut board = Board::initial();
    for step in 0..40 {
        let written = board.to_fen();
        let read = Board::from_fen(&written).expect("what the writer produced");
        assert_eq!(
            read.digest(),
            board.digest(),
            "the position changed passing through its own notation at move {step}"
        );
        assert_eq!(read.to_fen(), written);

        let Some(&chosen) = legal(&board).as_slice().first() else {
            break;
        };
        board = apply(&board, chosen);
    }
}

/// The starting position writes the string every chess program agrees on.
///
/// **One literal, deliberately.** The round-trip tests above prove the pair is
/// self-consistent, which a pair that is wrong in the same way on both sides
/// would also satisfy. This anchors the pair to the outside world.
#[test]
fn the_starting_position_writes_the_published_string() {
    assert_eq!(
        Board::initial().to_fen(),
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"
    );
}

/// **Every legal move in a busy position round trips through its notation**,
/// including the promotions, which are the only moves carrying a fifth
/// character.
#[test]
fn every_legal_move_round_trips() {
    // Kiwipete plus a position whose only moves are promotions, so the
    // five-character form is not merely represented but forced.
    for fen in [
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "8/PPPk4/8/8/8/8/4Kppp/8 w - - 0 1",
    ] {
        let board = Board::from_fen(fen).expect("a well-formed position");
        let moves = legal(&board);
        assert!(!moves.as_slice().is_empty(), "nothing to check in {fen}");
        for &played in moves.as_slice() {
            let text = played.notation();
            assert_eq!(
                Move::from_notation(&text),
                Some(played),
                "`{text}` did not read back as the move that wrote it"
            );
        }
    }
}

/// Promotions specifically: all four kinds, both colours' promotion ranks.
#[test]
fn every_promotion_round_trips() {
    for kind in [Kind::Knight, Kind::Bishop, Kind::Rook, Kind::Queen] {
        for (from, to) in [("a7", "a8"), ("h2", "h1")] {
            let played = Move::promoting(
                Square::from_name(from).expect("a square"),
                Square::from_name(to).expect("a square"),
                kind,
            );
            assert_eq!(Move::from_notation(&played.notation()), Some(played));
        }
    }
}

/// **Text that is not a move is refused, and that is not the same as a move
/// that is not allowed.** `a1a1` is a perfectly readable move that no rules
/// permit; this reader's job is the first question only, so it accepts it.
#[test]
fn text_that_is_not_a_move_is_refused() {
    for text in [
        "", "e2", "e2e", "e2e4e", "e2e45", "i2i4", "e0e4", "  e2e4",
        // Longer than a move, with four good characters in front: the case
        // the two square reads and the promotion read both accept, leaving
        // the length itself as the only thing that can refuse it.
        "e2e4qq", "e2e4q1", "e2e4qqqq",
    ] {
        assert_eq!(
            Move::from_notation(text),
            None,
            "`{text}` was read as a move"
        );
    }

    // Promotion to pawn or king is unrepresentable, so it is refused by the
    // reader rather than by whatever would have had to cope with it later.
    for text in ["a7a8p", "a7a8k", "a7a8K"] {
        assert_eq!(
            Move::from_notation(text),
            None,
            "`{text}` was read as a promotion"
        );
    }

    // Legality is a different question and deliberately not asked here.
    assert!(
        Move::from_notation("a1a1").is_some(),
        "readable-but-illegal is the reader's business to accept"
    );
}

/// An empty board writes the all-empty ranks and reads back empty.
#[test]
fn an_empty_position_round_trips() {
    let board = Board::from_fen("8/8/8/8/8/8/8/8 w - - 0 1").expect("well formed");
    assert_eq!(board.to_fen(), "8/8/8/8/8/8/8/8 w - - 0 1");
    assert_eq!(
        board.piece_at(Square::from_name("e4").expect("a square")),
        None
    );
    assert_eq!(board.to_move, Colour::White);
}
