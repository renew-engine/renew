//! The position, and the vocabulary it is written in.

use renew_frame::StateHash;

/// Which side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Colour {
    /// Moves first.
    White,
    /// Moves second.
    Black,
}

impl Colour {
    /// The other one.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::White => Self::Black,
            Self::Black => Self::White,
        }
    }

    /// Which way this side's pawns move, in ranks.
    const fn pawn_step(self) -> i32 {
        match self {
            Self::White => 1,
            Self::Black => -1,
        }
    }

    /// The rank this side's pawns start on.
    const fn pawn_home_rank(self) -> i32 {
        match self {
            Self::White => 1,
            Self::Black => 6,
        }
    }

    /// The rank this side's pawns promote on.
    const fn promotion_rank(self) -> i32 {
        match self {
            Self::White => 7,
            Self::Black => 0,
        }
    }

    /// The rank this side's king and rooks start on.
    const fn back_rank(self) -> i32 {
        match self {
            Self::White => 0,
            Self::Black => 7,
        }
    }
}

/// What a piece is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// Moves forward, captures diagonally, and is the reason this game has
    /// three special rules.
    Pawn,
    /// The only piece that jumps.
    Knight,
    /// Diagonals.
    Bishop,
    /// Ranks and files.
    Rook,
    /// Both.
    Queen,
    /// One square, and the whole point.
    King,
}

impl Kind {
    /// The letter used in algebraic notation, lower case.
    #[must_use]
    pub const fn letter(self) -> char {
        match self {
            Self::Pawn => 'p',
            Self::Knight => 'n',
            Self::Bishop => 'b',
            Self::Rook => 'r',
            Self::Queen => 'q',
            Self::King => 'k',
        }
    }

    /// A kind from the letter [`Self::letter`] gives it, in either case.
    ///
    /// **The inverse exists so that only one table maps letters to kinds.**
    /// Reading Forsyth-Edwards notation and reading a promotion out of a
    /// move both need it, and a second copy of the mapping is a second
    /// chance to disagree with the first.
    #[must_use]
    pub fn from_letter(letter: char) -> Option<Self> {
        match letter.to_ascii_lowercase() {
            'p' => Some(Self::Pawn),
            'n' => Some(Self::Knight),
            'b' => Some(Self::Bishop),
            'r' => Some(Self::Rook),
            'q' => Some(Self::Queen),
            'k' => Some(Self::King),
            _ => None,
        }
    }

    /// A stable small number, so a digest does not depend on the enum's
    /// declaration order surviving an edit.
    const fn code(self) -> u32 {
        match self {
            Self::Pawn => 1,
            Self::Knight => 2,
            Self::Bishop => 3,
            Self::Rook => 4,
            Self::Queen => 5,
            Self::King => 6,
        }
    }
}

/// A piece on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Piece {
    /// Whose.
    pub colour: Colour,
    /// What.
    pub kind: Kind,
}

impl Piece {
    /// A piece.
    #[must_use]
    pub const fn new(colour: Colour, kind: Kind) -> Self {
        Self { colour, kind }
    }

    const fn code(self) -> u32 {
        match self.colour {
            Colour::White => self.kind.code(),
            Colour::Black => self.kind.code() + 8,
        }
    }
}

/// A square, 0 = a1 through 63 = h8.
///
/// One number rather than a file and a rank, because every table below is
/// indexed by it and a pair would need unpacking at each use. The file and
/// rank accessors are where the geometry lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Square(u8);

impl Square {
    /// From a file and a rank, both 0..8, or nothing if either is off the
    /// board.
    ///
    /// **Returning an option rather than wrapping is the whole of the
    /// board-edge handling.** A knight on a1 offset by (−1, 2) lands on a
    /// file of −1, and an implementation that added to a square index instead
    /// would land on h2 — a legal-looking move that teleports across the
    /// board. Every offset in this crate goes through here.
    #[must_use]
    pub const fn at(file: i32, rank: i32) -> Option<Self> {
        if file < 0 || file > 7 || rank < 0 || rank > 7 {
            None
        } else {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the bounds check above puts the result in 0..64"
            )]
            let index = (rank * 8 + file) as u8;
            Some(Self(index))
        }
    }

    /// From a raw index, or nothing if past the board.
    #[must_use]
    pub const fn from_index(index: u8) -> Option<Self> {
        if index < 64 { Some(Self(index)) } else { None }
    }

    /// The raw index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// 0 for the a-file through 7 for the h-file.
    #[must_use]
    pub const fn file(self) -> i32 {
        (self.0 % 8) as i32
    }

    /// 0 for rank 1 through 7 for rank 8.
    #[must_use]
    pub const fn rank(self) -> i32 {
        (self.0 / 8) as i32
    }

    /// This square offset by a file and rank delta, if it stays on the board.
    #[must_use]
    pub const fn offset(self, files: i32, ranks: i32) -> Option<Self> {
        Self::at(self.file() + files, self.rank() + ranks)
    }

    /// The two-character name, like `e4`.
    #[must_use]
    pub fn name(self) -> [char; 2] {
        let file = char::from(b'a' + u8::try_from(self.file()).unwrap_or(0));
        let rank = char::from(b'1' + u8::try_from(self.rank()).unwrap_or(0));
        [file, rank]
    }

    /// The square a name like `e4` refers to, or nothing if it names none.
    ///
    /// **Exactly the inverse of [`Self::name`]**, and the pair is worth a
    /// property test rather than examples: sixty-four squares is small
    /// enough to check all of them.
    #[must_use]
    pub fn from_name(text: &str) -> Option<Self> {
        let mut characters = text.chars();
        let file = characters.next()?;
        let rank = characters.next()?;
        if characters.next().is_some() {
            return None;
        }
        let file = i32::from(u8::try_from(file).ok()?) - i32::from(b'a');
        let rank = i32::from(u8::try_from(rank).ok()?) - i32::from(b'1');
        Self::at(file, rank)
    }
}

/// Who may still castle where.
///
/// Four independent bits rather than a per-side pair: a rook that moves loses
/// that side's right alone, and a king that moves loses both. Collapsing them
/// makes the first case unrepresentable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the four rights are genuinely independent — a rook that moves loses one, a               king that moves loses two — and any packing into a smaller type would make               the single-right case harder to read rather than easier"
)]
pub struct Castling {
    /// White may castle toward the h-file.
    pub white_king_side: bool,
    /// White may castle toward the a-file.
    pub white_queen_side: bool,
    /// Black may castle toward the h-file.
    pub black_king_side: bool,
    /// Black may castle toward the a-file.
    pub black_queen_side: bool,
}

impl Castling {
    /// Everything still available, as at the start of a game.
    pub const ALL: Self = Self {
        white_king_side: true,
        white_queen_side: true,
        black_king_side: true,
        black_queen_side: true,
    };

    /// Nothing available.
    pub const NONE: Self = Self {
        white_king_side: false,
        white_queen_side: false,
        black_king_side: false,
        black_queen_side: false,
    };

    const fn code(self) -> u32 {
        (self.white_king_side as u32)
            | ((self.white_queen_side as u32) << 1)
            | ((self.black_king_side as u32) << 2)
            | ((self.black_queen_side as u32) << 3)
    }
}

/// A position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    squares: [Option<Piece>; 64],
    /// Whose turn.
    pub to_move: Colour,
    /// Who may still castle where.
    pub castling: Castling,
    /// The square a pawn just skipped over, capturable in passing.
    ///
    /// **The skipped square, not the pawn's.** That is what the capturing
    /// move's destination is, so storing the pawn instead would need a
    /// conversion at every use and would get it wrong once.
    pub en_passant: Option<Square>,
    /// Half-moves since the last capture or pawn move.
    pub halfmove_clock: u32,
    /// Full moves, from one, incremented after Black plays.
    pub fullmove_number: u32,
}

impl Board {
    /// An empty board with White to move and no rights.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            squares: [None; 64],
            to_move: Colour::White,
            castling: Castling::NONE,
            en_passant: None,
            halfmove_clock: 0,
            fullmove_number: 1,
        }
    }

    /// The starting position.
    #[must_use]
    pub fn initial() -> Self {
        use Kind::{Bishop, King, Knight, Pawn, Queen, Rook};
        let back = [Rook, Knight, Bishop, Queen, King, Bishop, Knight, Rook];
        let mut board = Self::empty();
        for (file, kind) in back.into_iter().enumerate() {
            let file = i32::try_from(file).unwrap_or(0);
            board.put(Square::at(file, 0), Some(Piece::new(Colour::White, kind)));
            board.put(Square::at(file, 1), Some(Piece::new(Colour::White, Pawn)));
            board.put(Square::at(file, 6), Some(Piece::new(Colour::Black, Pawn)));
            board.put(Square::at(file, 7), Some(Piece::new(Colour::Black, kind)));
        }
        board.castling = Castling::ALL;
        board
    }

    /// What is on a square.
    #[must_use]
    pub fn piece_at(&self, square: Square) -> Option<Piece> {
        self.squares.get(square.index()).copied().flatten()
    }

    /// Put a piece on a square, or clear it. A square off the board is
    /// ignored, which only arises from a caller passing `None`.
    pub fn put(&mut self, square: Option<Square>, piece: Option<Piece>) {
        if let Some(square) = square
            && let Some(slot) = self.squares.get_mut(square.index())
        {
            *slot = piece;
        }
    }

    /// Where a side's king stands, if it has one.
    ///
    /// Optional because positions without kings are useful for testing a
    /// single piece's movement, and refusing to represent one would mean every
    /// such test had to build a legal game around it.
    #[must_use]
    pub fn king_square(&self, colour: Colour) -> Option<Square> {
        for index in 0..64u8 {
            let square = Square::from_index(index)?;
            if self.piece_at(square) == Some(Piece::new(colour, Kind::King)) {
                return Some(square);
            }
        }
        None
    }

    /// Every square, in ascending index order.
    ///
    /// The order is part of the contract rather than an accident: move
    /// generation walks it, and the order moves come out in is observable to
    /// anything that takes the first legal move.
    pub fn squares(&self) -> impl Iterator<Item = (Square, Option<Piece>)> + '_ {
        (0..64u8).filter_map(move |index| {
            let square = Square::from_index(index)?;
            Some((square, self.piece_at(square)))
        })
    }

    /// A hash over everything that can change future play.
    ///
    /// **Castling rights and the en-passant square are in here**, and leaving
    /// either out is the classic way to make two positions that look identical
    /// behave differently. The move clocks are in too: the fifty-move rule
    /// makes the halfmove clock decide a game.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hash = StateHash::new();
        for (_, piece) in self.squares() {
            hash = hash.absorb_u32(piece.map_or(0, Piece::code));
        }
        hash.absorb_u32(match self.to_move {
            Colour::White => 1,
            Colour::Black => 2,
        })
        .absorb_u32(self.castling.code())
        .absorb_u32(
            self.en_passant
                .map_or(64, |square| u32::try_from(square.index()).unwrap_or(64)),
        )
        .absorb_u32(self.halfmove_clock)
        .absorb_u32(self.fullmove_number)
        .finish()
    }
}

impl Colour {
    pub(crate) const fn forward(self) -> i32 {
        self.pawn_step()
    }
    pub(crate) const fn home_rank(self) -> i32 {
        self.pawn_home_rank()
    }
    pub(crate) const fn last_rank(self) -> i32 {
        self.promotion_rank()
    }
    pub(crate) const fn castle_rank(self) -> i32 {
        self.back_rank()
    }
}

/// Why a position could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenError {
    /// Fewer than four space-separated fields.
    TooFewFields,
    /// The piece placement did not describe eight ranks.
    WrongRankCount,
    /// A rank described more or fewer than eight files.
    WrongFileCount,
    /// A character that is not a piece or a digit.
    UnknownPiece,
    /// The side to move was not `w` or `b`.
    UnknownSideToMove,
    /// The en-passant field was neither `-` nor a square.
    UnknownSquare,
    /// A move counter was not a number.
    UnknownNumber,
}

impl Board {
    /// Write this position in Forsyth-Edwards notation.
    ///
    /// **The inverse of [`Self::from_fen`], and the reason it exists is that
    /// a command-line game is stateless.** A player who makes one move per
    /// invocation needs the position handed back in the form the next
    /// invocation reads, or the game cannot continue past a single move.
    ///
    /// The pair is checked by round trip rather than against expected
    /// strings: every published position this crate is tested against
    /// survives `from_fen` then `to_fen` unchanged, which catches a field
    /// that reads correctly and writes wrongly — the failure a one-directional
    /// test cannot see.
    #[must_use]
    pub fn to_fen(&self) -> String {
        let mut text = String::with_capacity(90);
        for rank in (0..8).rev() {
            let mut empty = 0u32;
            for file in 0..8 {
                match Square::at(file, rank).and_then(|square| self.piece_at(square)) {
                    Some(piece) => {
                        if empty > 0 {
                            text.push_str(&empty.to_string());
                            empty = 0;
                        }
                        let letter = piece.kind.letter();
                        text.push(if piece.colour == Colour::White {
                            letter.to_ascii_uppercase()
                        } else {
                            letter
                        });
                    }
                    None => empty += 1,
                }
            }
            if empty > 0 {
                text.push_str(&empty.to_string());
            }
            if rank > 0 {
                text.push('/');
            }
        }

        text.push(' ');
        text.push(if self.to_move == Colour::White {
            'w'
        } else {
            'b'
        });

        text.push(' ');
        let before = text.len();
        for (held, letter) in [
            (self.castling.white_king_side, 'K'),
            (self.castling.white_queen_side, 'Q'),
            (self.castling.black_king_side, 'k'),
            (self.castling.black_queen_side, 'q'),
        ] {
            if held {
                text.push(letter);
            }
        }
        if text.len() == before {
            text.push('-');
        }

        text.push(' ');
        match self.en_passant {
            Some(square) => {
                let name = square.name();
                text.push(name[0]);
                text.push(name[1]);
            }
            None => text.push('-'),
        }

        text.push(' ');
        text.push_str(&self.halfmove_clock.to_string());
        text.push(' ');
        text.push_str(&self.fullmove_number.to_string());
        text
    }

    /// Read a position in Forsyth–Edwards notation.
    ///
    /// **The reason this exists is the test suite, not the user interface.**
    /// The published perft positions are distributed as FEN, and a rules crate
    /// that cannot read them cannot be checked against the one oracle chess
    /// has. Everything else it enables is a bonus.
    ///
    /// # Errors
    ///
    /// Returns why the text is not a position. Malformed input is an ordinary
    /// outcome here — the text usually comes from outside — so it is a result
    /// rather than an assertion.
    pub fn from_fen(text: &str) -> Result<Self, FenError> {
        let mut fields = text.split_whitespace();
        let placement = fields.next().ok_or(FenError::TooFewFields)?;
        let side = fields.next().ok_or(FenError::TooFewFields)?;
        let rights = fields.next().ok_or(FenError::TooFewFields)?;
        let passant = fields.next().ok_or(FenError::TooFewFields)?;
        // The clocks are optional: many published positions omit them, and
        // refusing those would make the oracle unreachable over a detail that
        // does not affect move generation.
        let halfmove = fields.next().unwrap_or("0");
        let fullmove = fields.next().unwrap_or("1");

        let mut board = Self::empty();
        let ranks: Vec<&str> = placement.split('/').collect();
        if ranks.len() != 8 {
            return Err(FenError::WrongRankCount);
        }
        // FEN lists rank 8 first, and this crate numbers rank 1 as zero.
        for (offset, row) in ranks.iter().enumerate() {
            let rank = 7 - i32::try_from(offset).unwrap_or(0);
            let mut file = 0;
            for symbol in row.chars() {
                if let Some(skip) = symbol.to_digit(10) {
                    file += i32::try_from(skip).unwrap_or(0);
                    continue;
                }
                let colour = if symbol.is_ascii_uppercase() {
                    Colour::White
                } else {
                    Colour::Black
                };
                let Some(kind) = Kind::from_letter(symbol) else {
                    return Err(FenError::UnknownPiece);
                };
                board.put(Square::at(file, rank), Some(Piece::new(colour, kind)));
                file += 1;
            }
            if file != 8 {
                return Err(FenError::WrongFileCount);
            }
        }

        board.to_move = match side {
            "w" => Colour::White,
            "b" => Colour::Black,
            _ => return Err(FenError::UnknownSideToMove),
        };
        board.castling = Castling {
            white_king_side: rights.contains('K'),
            white_queen_side: rights.contains('Q'),
            black_king_side: rights.contains('k'),
            black_queen_side: rights.contains('q'),
        };
        board.en_passant = if passant == "-" {
            None
        } else {
            Some(parse_square(passant).ok_or(FenError::UnknownSquare)?)
        };
        board.halfmove_clock = halfmove.parse().map_err(|_| FenError::UnknownNumber)?;
        board.fullmove_number = fullmove.parse().map_err(|_| FenError::UnknownNumber)?;
        Ok(board)
    }
}

/// A square from its name, like `e4`.
fn parse_square(text: &str) -> Option<Square> {
    Square::from_name(text)
}
