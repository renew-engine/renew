# renew-sample-chess

Chess in a terminal, one run per move. It counts the legal games
from a position, plays a deterministic one out, or just prints the
board — and it stores nothing between runs, so a game is a shell
variable and a saved game is a text file.

## Running it

```
chess                            # count the games of length four: 197281
chess --show                     # the board, the turn, what is legal
chess --move e2e4 --move e7e5    # apply moves, then show
chess --count --depth 5          # 4865609
chess --play --depth 60          # 60 half-moves, answered with a digest
chess --show --json              # the same answer, machine-readable
```

From a fresh clone, `chess` above means
`cargo run --bin renew -- run chess -- --show`, or, going straight
at the package, `cargo run -p renew-sample-chess --bin chess --
--show`. Everything after the bare `--` is this binary's own
command line.

```console
$ chess --show
8  r n b q k b n r
7  p p p p p p p p
6  . . . . . . . .
5  . . . . . . . .
4  . . . . . . . .
3  . . . . . . . .
2  P P P P P P P P
1  R N B Q K B N R

   a b c d e f g h

white to move, 20 legal moves, ongoing
rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1
```

Upper case is White, lower case is Black, a dot is an empty square
— the same convention as the notation on the last line, so a reader
who can read one can read the other. The board is always drawn from
White's side, never turned around to follow the side to move: a
board that flips between moves makes two consecutive positions
impossible to compare by eye, which is the one thing a printed board
is for.

## A game is a shell variable

The last line of a shown position *is* the position, and `--fen`
reads exactly that text back. So the second run below consumes the
first run's output and the game continues, with nothing stored
anywhere in between:

```console
$ position=$(chess --move e2e4 --move e7e5 | tail -n 1)
$ echo "$position"
rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq e6 0 2
$ chess --fen "$position" --move g1f3 | tail -n 1
rnbqkbnr/pppp1ppp/8/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R b KQkq - 1 2
```

Redirect that line to a file instead of a variable and you have a
saved game; keep several files and you have branches. The property
that makes this work is that a position names itself completely —
castling rights, the en-passant square and both clocks included — so
no run needs to know which moves led to it. Every position is
reachable directly, and a game handed over at every single move ends
where the same game played in one run ends. The test suite holds
that over twenty half-moves, because the fields most likely to
survive one exchange and not twenty are exactly the ones a hand-
written position tends to drop.

`--move` is the other way to name a position: by the route to it,
which is the form a player has and the notation is not.

## The flags

| Flag | What it does |
|---|---|
| `--show` | Draw the board, name the side to move, count the moves available, say how the game stands, and print the position for the next run. |
| `--count` | Count the legal games of length `--depth` from the position. The mode with no flag. |
| `--play` | Play `--depth` half-moves, always taking the first legal move, and answer with a digest of where it stopped. |
| `--move MOVE` | A move to apply before anything else: the two squares, `e2e4`, with a promotion letter as a fifth character, `a7a8q`. Repeatable, and applied in every mode, not only when showing. |
| `--fen POSITION` | The position to start from, in Forsyth-Edwards notation. Defaults to the initial position. |
| `--depth N` | How deep to count, or how many half-moves to play. Default 4. |
| `--json` | Answer with one line of JSON instead of prose. |
| `--help` | Print the usage text and stop, successfully. |

Naming a move and no mode selects `--show`: someone who writes a
move is playing chess rather than measuring it. An explicit mode
wins over that, on whichever side of the move it appears. Naming two
modes is not an error — the last one wins.

`--show` accepts `--depth` and ignores it; there is nothing for a
depth to mean when the answer is one position, and the JSON reports
`"depth":0` for a shown run whatever was asked for.

Two ways a move can be refused, kept apart because they are
different mistakes. `--move e2e9` is not a move at all, and is
rejected while the command line is still being read. `--move e2e5`
is a perfectly well-formed move that the rules do not permit here,
which cannot be known until the position is — including the case
where the move belongs to the other side, so `--move e7e5` as
White's first move is refused rather than quietly taken.

Refusals go to the error stream and exit 1, printing nothing on the
output stream, so a script can tell without parsing anything. A
completed run and `--help` exit 0.

## Counting

`--count` runs perft: how many distinct legal games of a given
length exist from the position. That is the reason chess is in this
repository at all, and the rules crate's README
([`rules/README.md`](rules/README.md)) explains why at length. The
short version: these are published numbers that other people
computed independently, so a wrong one here is a wrong rule rather
than a wrong opinion — 20, 400, 8902 and 197281 from the initial
position for depths one to four, and 97862 at depth three from the
position known as Kiwipete:

```console
$ chess --count --depth 3 --fen "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1"
chess count depth=3 nodes=97862 result=ongoing
```

**Six is as deep as this binary will go.** Each level is roughly
thirty times the work of the one before, so depth seven from the
start is billions of positions and hours of waiting; a refusal that
names the limit is more use than a command that appears to hang. A
caller who genuinely wants depth seven can call the library, which
imposes no limit. The refusal applies to counting only — `--play
--depth 5000` is a trivial amount of work and is allowed.

The search is plain recursion with no transposition table and no
memoisation, and the difference between build profiles is
therefore large. On one desktop machine, depth five took 10.9 s
built with `cargo build` and 0.30 s built with `--release`, where
depth six took 15.9 s in release. Deep counts want `--release`.

`result` in a counted run describes the *starting* position, not
anything discovered during the count, and the `digest` field
identifies that same position.

## Playing

`--play` is the mode with a digest, and the digest is the point.
Perft cannot serve here: a count is a fact about chess, so it comes
out the same on a machine that is working and one that is not.
A position hash is a fact about *this* run, and comparing it across
processes, build profiles and platforms is what shows the state
itself survived the journey. `--play --depth 60 --json` is one of
the pinned runs whose digests are compared that way; it answers
`digest=0x6bf0be22d95711ee` here in both a debug and a release
build.

The move chosen is always the first legal one — a deterministic
choice, not a good one. There is no search and no evaluation
anywhere in this sample, deliberately: a search would turn the
digest into a test of the search instead of a test of the rules.
The games it produces are therefore not chess anybody would want to
watch.

Two honest limits on playing:

- **The loop stops only when there is no legal move.** Reaching a
  checkmate or a stalemate ends it early, and `moves=` in the answer
  says how many half-moves it actually managed. The fifty-move rule
  is *reported* in `result` but does not stop the loop, so
  `chess --play --depth 5000` really does play five thousand
  half-moves and then say `result=fifty-move`.
- **Draws by repetition do not exist here**, so a long run wanders
  rather than ending. The rules crate says what else it leaves out.

## JSON

`--json` replaces the whole answer with one line carrying its schema
version, in every mode:

```console
$ chess --show --json
{"schema_version":1,"sample":"chess","mode":"show","depth":0,"nodes":20,"moves":0,"digest":"0x1afd057861571eda","result":"ongoing","to_move":"white","fen":"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"}
```

The fields are shared across the modes, and each mode leaves some of
them at zero rather than omitting them, so a reader never has to
handle three shapes:

- `nodes` — positions counted when counting, moves available when
  showing, `0` when playing.
- `moves` — half-moves actually played, `0` in the other two modes.
- `depth` — the depth asked for; `0` when showing.
- `digest` and `fen` — the position the run ended on, as a hash and
  as text. Every mode carries it, so a caller can ask that position
  another question without parsing the prose answer back.

## Shape

Two crates. [`rules/`](rules/README.md) is the game: positions,
legal moves, and how a game ends. It knows nothing about a command
line, a terminal or a file, which is what lets the published perft
counts be pointed straight at it.

This crate is the face: it parses the command line, applies the
named moves, chooses what to run, draws the board, writes both
answers, and owns the exit code. It has no window, no renderer and
no clock — the position on the command line is the entire input, and
a printed position is the entire output. That is also what makes it
a useful second witness for reproduction: there is no geometry and
no floating point here at all, so a difference between two runs
would have to be in the integer state itself rather than in
arithmetic shared with the other samples.

## Manifest

`Cargo.toml` is authoritative for maturity, core status,
dependencies and extension points.
