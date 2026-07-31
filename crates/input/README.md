# renew-input

Raw window events to named actions. A game asks "is the player jumping",
not "is the space bar down"; this is the layer between.

## Contract

- **Edges live on the tick, not on the event.** `just_pressed` is true for
  the whole tick in which an action became active and false in the next,
  whatever order events arrived in or how many there were. **A key pressed
  and released inside one tick still reports both edges** — a game that
  misses a fast tap is worse than one that sees it a tick late.
- **Nothing here reads a clock.** `advance` is called by the caller's
  fixed-timestep loop. The crate cannot know whether a frame was slow and
  so cannot behave differently on one.
- **Binding order does not reach behaviour.** The binding table is a
  sorted vector, searched rather than hashed, so two runs of the same
  events always agree.
- **Unbound input is ignored, not refused.** A keyboard has far more keys
  than any game binds.

## Architecture

Three sorted vectors: bindings by physical input, one state per distinct
action, and the set of inputs currently down. Lookup is a binary search;
at the size a binding table actually reaches, that beats a map and cannot
surprise anyone with its iteration order.

`HashMap` is banned by the crate's [clippy.toml](clippy.toml), and here it
is a real temptation rather than a theoretical one — a binding table is
the textbook use for one. Its hasher is seeded per process, so resolving
which action an input drives could differ between runs.

## Public API

`Binding::key` and `Binding::pointer` to name a physical input; `bind` to
attach it to an action; `handle` to feed a window event; `advance` to end
a tick; `held`, `just_pressed`, `just_released`, and `state` to read.

`release_all` exists for focus loss: the OS stops delivering key-up for
keys released while another window has focus, so without it a player who
alt-tabs mid-jump comes back still jumping. It reports the releases as
edges, so a system watching for them sees them.

Actions are a caller-defined type, not strings — a typo is a compile
error rather than an action that silently never fires. The bound is
`Copy + Eq`, deliberately not `Hash`.

## Thread safety and ownership

No shared state, no interior mutability, no globals. `InputMap<A>` owns
its tables and is `Send` and `Sync` when `A` is. Nothing synchronises,
because nothing is shared.

## Testing

The state machine's edges, including the cases that justify the layer: a
tap inside one tick, a key repeat that must not re-fire the press, two
bindings OR-ing into one action, rebinding, focus loss, and redundant
events that a real event stream produces.

Two of them are about the absence of hidden inputs rather than about the
machine — the same events produce the same state twice over, and binding
order does not change behaviour.

## Status

`bootstrap`. Bindings and edges are settled; anything above them — axes,
chords, gamepads, runtime rebinding UI, serialised binding sets — is not,
and none of it has a consumer asking yet.

## Key decisions

- **Edges on the tick, not the event.** The alternative is to report a
  press the instant it arrives, which loses a tap that completes inside
  one tick and makes behaviour depend on how many events a frame happened
  to deliver.
- **Repeats ignored.** A repeat is the OS restating that a key is still
  down, which the map already knows.
- **Rebinding replaces.** One physical input means one thing at a time;
  keeping both would make the result depend on binding order.
- **Sorted vectors, not maps.** Determinism first, and faster at this
  size anyway.
- **No axes yet.** A stick or a mouse delta is a different shape from a
  button and deserves its own design, not a bool with a magnitude bolted
  on.
