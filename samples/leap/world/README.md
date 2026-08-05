# renew-sample-leap-world

The platformer's simulation: one character, some solid boxes, and the rules that connect them.

**Status: bootstrap.** A sample, not an engine module — nothing in the engine depends on it, and it
exists to exercise `renew-physics2d` against a shape of problem a real game has.

## What it is

A pure function of its inputs. `Leap::new` takes tuning, a start position and a list of platforms;
`step` takes one `Intent` — run left or right, jump or not — and advances the world by exactly one
tick. Nothing reads a clock, a file, or a random number. The same intents in the same order produce
the same digest on every machine, which is the property that lets three platforms compare one line
of output.

## What it demonstrates

**Footing comes from the slide, not from a contact query.** A body stopped by `move_and_slide`
rests a skin distance away from what stopped it — too far for a contact test to report. So
`grounded` and `against_wall` are derived from the surfaces the slide reported hitting and which
way each faced, which is the physics crate's intended use rather than a workaround.

**Coyote time is a counter, not a timer.** The character may still jump for a few ticks after
walking off a ledge. Counted in ticks because a fixed-timestep simulation has no other honest unit,
and a wall-clock version would not reproduce.

**The character's size is public.** `CHARACTER_HALF_EXTENTS` is exported because anything drawing
the world needs it — a drawing that guesses puts the character somewhere it is not, and a
one-cell-tall picture of a two-unit-tall body makes a character standing on the floor look like one
hovering above it.

## What it is not

Not a game: no scoring, no enemies, no level loading. The level is whatever list of platforms the
caller passes, and `samples/leap` supplies one with a floor, a wall and a ledge.

Not tuned for feel. The numbers in `Tuning` are chosen to make the interesting cases happen
quickly, not to be pleasant to play.
