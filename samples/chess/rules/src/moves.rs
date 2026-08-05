//! Which moves exist, which are legal, and what playing one does.

use crate::board::{Board, Castling, Colour, Kind, Piece, Square};

/// The most moves any legal chess position offers.
///
/// The known maximum is 218; 256 leaves room and keeps the list a power of
/// two. Fixed rather than grown, because generation runs inside a search and
/// a heap allocation per node is the difference between a usable perft and an
/// unusable one.
pub const MAX_MOVES: usize = 256;

/// A move: where from, where to, and what a pawn became.
///
/// **Castling, en passant and the double push are not tagged here.** They are
/// derivable from the position and the move — a king moving two files is a
/// castle, a pawn moving diagonally to an empty square is an en-passant
/// capture — and a tag would be a second source of truth that can disagree
/// with the first. A caller building a move by hand cannot get the tag wrong
/// if there is no tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Move {
    /// Where the piece started.
    pub from: Square,
    /// Where it ended.
    pub to: Square,
    /// What a pawn reaching the last rank became.
    pub promotion: Option<Kind>,
}

impl Move {
    /// A move with no promotion.
    #[must_use]
    pub const fn new(from: Square, to: Square) -> Self {
        Self {
            from,
            to,
            promotion: None,
        }
    }

    /// A pawn promotion.
    #[must_use]
    pub const fn promoting(from: Square, to: Square, kind: Kind) -> Self {
        Self {
            from,
            to,
            promotion: Some(kind),
        }
    }

    /// Long algebraic notation, like `e2e4` or `a7a8q`.
    #[must_use]
    pub fn notation(self) -> String {
        let from = self.from.name();
        let to = self.to.name();
        let mut text = String::with_capacity(5);
        text.push(from[0]);
        text.push(from[1]);
        text.push(to[0]);
        text.push(to[1]);
        if let Some(kind) = self.promotion {
            text.push(kind.letter());
        }
        text
    }
}

/// A fixed-capacity list of moves.
#[derive(Clone, Copy, Debug)]
pub struct MoveList {
    moves: [Move; MAX_MOVES],
    count: usize,
}

impl MoveList {
    /// Empty.
    #[must_use]
    pub fn new() -> Self {
        let filler = Move::new(
            Square::from_index(0).unwrap_or_else(|| unreachable!("zero is on the board")),
            Square::from_index(0).unwrap_or_else(|| unreachable!("zero is on the board")),
        );
        Self {
            moves: [filler; MAX_MOVES],
            count: 0,
        }
    }

    /// Add a move. Silently ignored past capacity, which no legal position
    /// reaches — the known maximum is 218 against a capacity of 256.
    pub fn push(&mut self, candidate: Move) {
        if let Some(slot) = self.moves.get_mut(self.count) {
            *slot = candidate;
            self.count += 1;
        }
    }

    /// The moves.
    #[must_use]
    pub fn as_slice(&self) -> &[Move] {
        self.moves.split_at(self.count.min(MAX_MOVES)).0
    }

    /// How many.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    /// Whether there are none — which for legal moves means the game is over.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
}

impl Default for MoveList {
    fn default() -> Self {
        Self::new()
    }
}

/// The eight squares a knight reaches.
const KNIGHT_STEPS: [(i32, i32); 8] = [
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
];

/// The eight squares a king reaches, and the eight directions a queen slides.
const KING_STEPS: [(i32, i32); 8] = [
    (0, 1),
    (1, 1),
    (1, 0),
    (1, -1),
    (0, -1),
    (-1, -1),
    (-1, 0),
    (-1, 1),
];

const ROOK_STEPS: [(i32, i32); 4] = [(0, 1), (1, 0), (0, -1), (-1, 0)];
const BISHOP_STEPS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, -1), (-1, 1)];

/// Is `square` attacked by any piece of `by`?
///
/// Asked of the square a king stands on, this is the whole of check detection.
/// It walks outward from the square rather than over every enemy piece, which
/// is the same answer for a fraction of the work.
#[must_use]
pub fn is_attacked(board: &Board, square: Square, by: Colour) -> bool {
    // Pawns attack diagonally *forward*, so a square is attacked by a pawn
    // sitting diagonally *backward* from it — the direction is inverted here
    // and getting that wrong is the classic check-detection bug.
    let back = -by.forward();
    for files in [-1, 1] {
        if let Some(origin) = square.offset(files, back)
            && board.piece_at(origin) == Some(Piece::new(by, Kind::Pawn))
        {
            return true;
        }
    }

    for (files, ranks) in KNIGHT_STEPS {
        if let Some(origin) = square.offset(files, ranks)
            && board.piece_at(origin) == Some(Piece::new(by, Kind::Knight))
        {
            return true;
        }
    }

    for (files, ranks) in KING_STEPS {
        if let Some(origin) = square.offset(files, ranks)
            && board.piece_at(origin) == Some(Piece::new(by, Kind::King))
        {
            return true;
        }
    }

    for (steps, sliders) in [
        (ROOK_STEPS.as_slice(), [Kind::Rook, Kind::Queen]),
        (BISHOP_STEPS.as_slice(), [Kind::Bishop, Kind::Queen]),
    ] {
        for &(files, ranks) in steps {
            let mut at = square;
            while let Some(next) = at.offset(files, ranks) {
                at = next;
                if let Some(piece) = board.piece_at(at) {
                    if piece.colour == by && sliders.contains(&piece.kind) {
                        return true;
                    }
                    break;
                }
            }
        }
    }

    false
}

/// Every move the side to move could make ignoring whether it leaves its own
/// king attacked.
#[must_use]
pub fn pseudo_legal(board: &Board) -> MoveList {
    let mut list = MoveList::new();
    let us = board.to_move;
    for (from, occupant) in board.squares() {
        let Some(piece) = occupant else { continue };
        if piece.colour != us {
            continue;
        }
        match piece.kind {
            Kind::Pawn => pawn_moves(board, from, us, &mut list),
            Kind::Knight => step_moves(board, from, us, &KNIGHT_STEPS, &mut list),
            Kind::King => {
                step_moves(board, from, us, &KING_STEPS, &mut list);
                castles(board, from, us, &mut list);
            }
            Kind::Bishop => slide_moves(board, from, us, &BISHOP_STEPS, &mut list),
            Kind::Rook => slide_moves(board, from, us, &ROOK_STEPS, &mut list),
            Kind::Queen => {
                slide_moves(board, from, us, &ROOK_STEPS, &mut list);
                slide_moves(board, from, us, &BISHOP_STEPS, &mut list);
            }
        }
    }
    list
}

fn pawn_moves(board: &Board, from: Square, us: Colour, list: &mut MoveList) {
    let ahead = us.forward();

    // One forward, onto an empty square.
    if let Some(one) = from.offset(0, ahead)
        && board.piece_at(one).is_none()
    {
        push_pawn(from, one, us, list);
        // Two forward, only from home and only through an empty square.
        if from.rank() == us.home_rank()
            && let Some(two) = from.offset(0, ahead * 2)
            && board.piece_at(two).is_none()
        {
            list.push(Move::new(from, two));
        }
    }

    // Captures, including in passing.
    for files in [-1, 1] {
        let Some(target) = from.offset(files, ahead) else {
            continue;
        };
        let occupied_by_enemy = board
            .piece_at(target)
            .is_some_and(|piece| piece.colour != us);
        if occupied_by_enemy || board.en_passant == Some(target) {
            push_pawn(from, target, us, list);
        }
    }
}

/// Add a pawn move, expanding it into four if it reaches the last rank.
fn push_pawn(from: Square, to: Square, us: Colour, list: &mut MoveList) {
    if to.rank() == us.last_rank() {
        // The order is part of the contract: a caller taking the first move
        // gets a queen, which is what anybody means by "promote".
        for kind in [Kind::Queen, Kind::Rook, Kind::Bishop, Kind::Knight] {
            list.push(Move::promoting(from, to, kind));
        }
    } else {
        list.push(Move::new(from, to));
    }
}

fn step_moves(board: &Board, from: Square, us: Colour, steps: &[(i32, i32)], list: &mut MoveList) {
    for &(files, ranks) in steps {
        let Some(to) = from.offset(files, ranks) else {
            continue;
        };
        if board.piece_at(to).is_none_or(|piece| piece.colour != us) {
            list.push(Move::new(from, to));
        }
    }
}

fn slide_moves(board: &Board, from: Square, us: Colour, steps: &[(i32, i32)], list: &mut MoveList) {
    for &(files, ranks) in steps {
        let mut at = from;
        while let Some(to) = at.offset(files, ranks) {
            at = to;
            match board.piece_at(to) {
                None => list.push(Move::new(from, to)),
                Some(piece) => {
                    if piece.colour != us {
                        list.push(Move::new(from, to));
                    }
                    break;
                }
            }
        }
    }
}

/// Castling, with all four of its conditions.
///
/// The one people forget is the third: the king may not pass *through* an
/// attacked square, which is a different test from the two either side of it.
fn castles(board: &Board, from: Square, us: Colour, list: &mut MoveList) {
    let rank = us.castle_rank();
    if from.file() != 4 || from.rank() != rank {
        return;
    }
    // A king already in check may not castle out of it.
    if is_attacked(board, from, us.other()) {
        return;
    }

    let (king_side, queen_side) = match us {
        Colour::White => (
            board.castling.white_king_side,
            board.castling.white_queen_side,
        ),
        Colour::Black => (
            board.castling.black_king_side,
            board.castling.black_queen_side,
        ),
    };

    // (right, squares that must be empty, the square the king crosses, where
    // the king lands)
    let plans: [(bool, &[i32], i32, i32); 2] =
        [(king_side, &[5, 6], 5, 6), (queen_side, &[1, 2, 3], 3, 2)];

    for (allowed, empty_files, crossed, landing) in plans {
        if !allowed {
            continue;
        }
        if empty_files
            .iter()
            .any(|&file| Square::at(file, rank).is_some_and(|sq| board.piece_at(sq).is_some()))
        {
            continue;
        }
        // The crossed file is 3 or 5 and the rank is 0 or 7, so this is on the
        // board by construction — a `continue` here would be a branch nothing
        // could take.
        let crossed_square = Square::at(crossed, rank)
            .unwrap_or_else(|| unreachable!("a castling path stays on the board"));
        // Through check. The landing square is checked by the ordinary
        // legality filter, so it is deliberately not repeated here.
        if is_attacked(board, crossed_square, us.other()) {
            continue;
        }
        let to = Square::at(landing, rank)
            .unwrap_or_else(|| unreachable!("a castling destination stays on the board"));
        list.push(Move::new(from, to));
    }
}

/// Play a move, returning the position it leads to.
///
/// Copy-and-play rather than make-and-unmake: a position is 72 bytes, copying
/// one is cheaper than the bookkeeping an undo needs, and a function that
/// cannot corrupt the position it was given is worth more here than the
/// difference.
#[must_use]
pub fn apply(board: &Board, played: Move) -> Board {
    let mut next = *board;
    let us = board.to_move;
    let Some(piece) = board.piece_at(played.from) else {
        // Nothing to move. Returning the position unchanged except for the
        // turn would invent a null move, so the position is returned exactly
        // as given and the caller's own generation is what stops this arising.
        return next;
    };

    let captured = board.piece_at(played.to);
    let is_pawn = piece.kind == Kind::Pawn;

    // In passing: a pawn moving diagonally onto an empty square takes the
    // pawn beside it rather than the one it lands on.
    if is_pawn && played.from.file() != played.to.file() && captured.is_none() {
        let victim = Square::at(played.to.file(), played.from.rank());
        next.put(victim, None);
    }

    // Castling: the king has moved two files, so the rook jumps over it.
    if piece.kind == Kind::King && (played.to.file() - played.from.file()).abs() == 2 {
        let rank = played.from.rank();
        let (rook_from, rook_to) = if played.to.file() == 6 {
            (7, 5)
        } else {
            (0, 3)
        };
        let rook = Square::at(rook_from, rank).and_then(|sq| board.piece_at(sq));
        next.put(Square::at(rook_from, rank), None);
        next.put(Square::at(rook_to, rank), rook);
    }

    next.put(Some(played.from), None);
    next.put(
        Some(played.to),
        Some(Piece::new(us, played.promotion.unwrap_or(piece.kind))),
    );

    // A double push leaves a square capturable in passing, and only for one
    // move — which is why it is set from scratch every time rather than
    // cleared conditionally.
    next.en_passant = if is_pawn && (played.to.rank() - played.from.rank()).abs() == 2 {
        Square::at(
            played.from.file(),
            played.from.rank().midpoint(played.to.rank()),
        )
    } else {
        None
    };

    next.castling = rights_after(board.castling, played, piece);

    // The fifty-move rule counts half-moves since the last capture or pawn
    // move, so both reset it.
    next.halfmove_clock = if is_pawn || captured.is_some() {
        0
    } else {
        board.halfmove_clock.saturating_add(1)
    };
    if us == Colour::Black {
        next.fullmove_number = board.fullmove_number.saturating_add(1);
    }
    next.to_move = us.other();
    next
}

/// Castling rights after a move.
///
/// Three ways to lose one, and the third is the one that gets missed: a rook
/// **captured on its home square** ends that side's right even though neither
/// the king nor the rook moved of its own accord.
fn rights_after(mut rights: Castling, played: Move, piece: Piece) -> Castling {
    if piece.kind == Kind::King {
        match piece.colour {
            Colour::White => {
                rights.white_king_side = false;
                rights.white_queen_side = false;
            }
            Colour::Black => {
                rights.black_king_side = false;
                rights.black_queen_side = false;
            }
        }
    }
    for square in [played.from, played.to] {
        match (square.file(), square.rank()) {
            (0, 0) => rights.white_queen_side = false,
            (7, 0) => rights.white_king_side = false,
            (0, 7) => rights.black_queen_side = false,
            (7, 7) => rights.black_king_side = false,
            _ => {}
        }
    }
    rights
}

/// Every move the side to move may legally make.
#[must_use]
pub fn legal(board: &Board) -> MoveList {
    let mut list = MoveList::new();
    let us = board.to_move;
    for &candidate in pseudo_legal(board).as_slice() {
        let after = apply(board, candidate);
        // A move is legal exactly when it does not leave one's own king
        // attacked — which covers pins, moving into check, and failing to
        // address an existing check, without any of them being a special case.
        let safe = after
            .king_square(us)
            .is_none_or(|king| !is_attacked(&after, king, us.other()));
        if safe {
            list.push(candidate);
        }
    }
    list
}

/// Whether the side to move is in check.
#[must_use]
pub fn in_check(board: &Board) -> bool {
    board
        .king_square(board.to_move)
        .is_some_and(|king| is_attacked(board, king, board.to_move.other()))
}

/// How a game ended, if it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The side to move is checkmated.
    Checkmate,
    /// The side to move has no legal move and is not in check.
    Stalemate,
    /// Fifty moves by each side without a capture or a pawn move.
    FiftyMove,
    /// Still playing.
    Ongoing,
}

/// Whether the game is over, and how.
#[must_use]
pub fn outcome(board: &Board) -> Outcome {
    if legal(board).is_empty() {
        if in_check(board) {
            Outcome::Checkmate
        } else {
            Outcome::Stalemate
        }
    } else if board.halfmove_clock >= 100 {
        Outcome::FiftyMove
    } else {
        Outcome::Ongoing
    }
}

/// Count the leaf nodes of the legal-move tree at a given depth.
///
/// **This is the oracle chess has and most software does not.** The counts
/// from the standard positions are published, independently verified, and
/// unforgiving: a single missing en-passant case, an off-by-one in castling
/// rights, or a promotion that generates three pieces instead of four shows up
/// as a wrong number rather than as a game that feels slightly odd.
#[must_use]
pub fn perft(board: &Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let moves = legal(board);
    if depth == 1 {
        return moves.len() as u64;
    }
    let mut nodes = 0;
    for &candidate in moves.as_slice() {
        nodes += perft(&apply(board, candidate), depth - 1);
    }
    nodes
}
