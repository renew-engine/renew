# renew-camera

The presentation-side camera: where the eye stands, what it looks at,
and the matrix that follows from it. Pure functions over the maths
crate's value types — no device, no window, no clock — so every claim is
testable on any machine. The manifest in `Cargo.toml` is authoritative
for maturity, dependencies and core status.

- `View` — eye and target, as two points rather than accumulated angles,
  so the same values always produce the same matrix. `View::blend` is
  the display-rate smoothing seam: a lerp between two ticks' views by
  the frame loop's interpolation factor, bit-exactly the previous view
  at zero, never an input to simulation.
- `Projection` — a perspective under the engine conventions.
- `Camera` — the pair, with `view_projection()` and the `columns()`
  boundary shape a GPU-facing pack type consumes.
- `aspect_of` — width over height with a degenerate-extent fallback, so
  no infinity ever reaches a projection.

## Contract

The engine's clip conventions, each pinned by a test because each
produces a *plausible* wrong picture when violated:

- **Clip `y` points down.** World up is screen `-y`; the projection
  carries an explicit minus.
- **Clip `z` runs `[0, 1]` REVERSED: near is one, far is zero.** Depth
  clears to zero and the compare keeps the larger value — the engine's
  single depth convention, chosen so the perspective hyperbola's dense
  end and the float format's dense end coincide and the far field keeps
  distinct depth values.
- **The matrix goes to the GPU; the divide happens there.** A caller
  transforming its own vertices would have to clip polygons against the
  near plane itself.
- **No roll.** World up is fixed until a consumer needs otherwise.
- **Floats, by design.** This crate is presentation-side. Simulation
  state never passes through it, and the workspace structure check is
  what enforces that direction mechanically.

## What this deliberately is not

Not a controller: how aim is stored, clamped and advanced is game
policy (the voxel sample keeps its fixed-point yaw and clamped pitch on
its own side of the boundary). Not a scene graph: transforms belong
elsewhere. Not a viewport manager: one camera is one matrix, and a
caller with two viewports calls it twice.
