# renew-sample-chess-rules

The rules of chess: positions, legal moves, and the outcome of a game.

**Status: bootstrap.** A sample, not an engine module — no engine crate depends on it.

## Why it exists here

Every other sample in this repository is checked against itself: run it twice, compare the digest,
and reproduction is proved while correctness is assumed. **Chess is the exception, and that is the
point of having it.**

Move generation has a published oracle. Perft counts — how many distinct legal games of a given
length exist from a position — are known numbers that other people computed independently: 20, 400,
8902, 197281 from the start; 97862 for Kiwipete at depth three. A wrong count here is a wrong rule,
not a wrong opinion, and no amount of internal consistency can hide it.

That makes this crate a check on the *testing* approach used everywhere else, not just on itself.

## What it does

The whole game: castling both sides, en passant, promotion to any of four pieces, check, checkmate,
stalemate, and the fifty-move rule. Positions read and write Forsyth-Edwards notation, and moves
read and write their algebraic form.

**Both notations are pairs, and both are tested by round trip.** A reader checked alone is only
checked against its author's belief about the text — and the author of the test is the author of
the code. Held against its writer it checks a fact instead. The pair caught a hand-written position
in its own tests that omitted the en-passant square, which is exactly the field that matters one
move later and only for one capture.

One literal is kept deliberately: the starting position's published string. Self-consistency is
satisfied by a pair that is wrong the same way on both sides, so something has to anchor it to the
outside world.

## What it is not

**No search and no evaluation.** There is no opinion here about which move is good. `samples/chess`
plays by taking the first legal move, which is deterministic rather than strong — a search would
make the digest a test of the search instead of the rules.

No clocks, no draw-by-repetition, no notation beyond the two above.
