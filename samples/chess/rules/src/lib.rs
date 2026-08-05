//! Chess, as a pure function of position and move.
//!
//! # Why this is here
//!
//! It is a second consumer for the engine, deliberately unlike the first. The
//! platformer is continuous, swept and geometric; this is discrete, exact and
//! combinatorial. Nothing in it is approximate, and nothing in it is a matter
//! of taste — a move is legal or it is not.
//!
//! # The oracle
//!
//! Chess has something most software does not: **published, independently
//! verified counts** of how many positions exist at each depth from a set of
//! standard starting points. A single missing en-passant case, an off-by-one
//! in castling rights, or a promotion that generates three pieces instead of
//! four shows up as a wrong number rather than as a game that feels slightly
//! odd.
//!
//! That is worth more than any amount of careful reading, and it is the whole
//! reason the rules are written as a separate crate with no rendering in it:
//! the thing that can be checked against an oracle is kept where the oracle
//! can reach it.
//!
//! ```
//! use renew_sample_chess_rules::{Board, perft};
//! assert_eq!(perft(&Board::initial(), 3), 8_902);
//! ```

// A rules crate is simulation: a game must replay identically from its move
// list, and a value that varied between runs would break that. The lint covers
// operators only, so it is necessary and not sufficient — but there is no
// floating point here at all, and this is what keeps that true under edits.
#![deny(clippy::float_arithmetic)]

pub mod board;
pub mod moves;

pub use board::{Board, Castling, Colour, FenError, Kind, Piece, Square};
pub use moves::{
    MAX_MOVES, Move, MoveList, Outcome, apply, in_check, is_attacked, legal, outcome, perft,
    pseudo_legal,
};
