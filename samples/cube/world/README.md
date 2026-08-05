# renew-sample-cube-world

The voxel sample's simulation: a block grid, a player who walks and jumps in it, and the reach that
lets them break and place blocks.

**Status: bootstrap.** A sample, not an engine module. It exists to exercise `renew-physics3d`
against a shape of problem a real game has, and it is the reason that crate has a consumer at all.

## What it is

A pure function of its inputs. `Cube::new` takes tuning, a `Grid` and a start position; `step`
takes one `Intent` — walk, jump, dig, place — and advances one tick. No clock, no file, no
unseeded randomness. The same intents produce the same digest everywhere.

## What it demonstrates

**A finite world has to say it is finite.** `Grid::get` returns nothing outside its bounds rather
than guessing. Answering "air" would let a player walk off and fall forever with no way to tell
that from a deep hole; answering "solid" would trap them at the boundary with no explanation. The
caller decides, and the arena in `samples/cube` decides by being a closed box.

**The shell cannot be cleared, and that is the world having a bottom.** `Grid::set` refuses to turn
a boundary cell to air. This is not decoration: without it the building script dug through the
floor and spent most of its run below the world, and its digest described a fall. Two weaker rules
were tried first and are recorded where the rule lives — a thicker floor (anything that can dig
once can dig again) and an unbreakable bottom layer (which stopped the digging and not the
climbing, since a player who places blocks can build a tower over any wall).

**Reach is a ray, not a radius.** Breaking and placing use a pick along the look direction, so what
gets edited is what the player is looking at rather than whatever happens to be nearest.

## What it is not

Not a game: no inventory, no block types beyond stone, no world generation. The grid is whatever
the caller builds.

No lighting, meshing or rendering. `samples/cube` draws it as two slices of text.
