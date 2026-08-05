# renew-render3d

Indexed 3D geometry over the rendering crate: one mesh pipeline,
depth-tested, one indexed draw per frame. Quads go in on the host, a
mesh comes out, and a draw item goes into a frame the caller composes.

- `Scene` — the pure half. Accumulates quads into packed vertex bytes
  and indices. No device, no adapter, no GPU call, so the packing and
  the numbering are testable on any machine.
- `MeshRenderer` — the device half. Owns the pipeline, uploads a scene
  into a `Mesh`, and hands back an `Item`.
- `attachment` / `depth_attachment` / `pass` — the frame pieces. `pass`
  always attaches depth; the parts stay public for frames it does not
  fit.

```rust,ignore
let renderer = MeshRenderer::new(&device, TargetFormat::Rgba8Unorm)?;
let mut scene = Scene::new();
scene.quad(corners, [0.0, 1.0, 0.0, 1.0]);
let mesh = renderer.upload(&device, &scene)?;

let color = [attachment(Color::new(0.0, 0.0, 0.0, 1.0))];
let items = [renderer.item(&mesh)];
target.render(&RenderDesc::new(&[pass(&color, &items)]))?;
```

## Contract

- **Push order is index order is draw order.** No sort, no batching, no
  depth pre-pass. Two scenes built by the same calls produce
  byte-identical buffers, and a frame drawn twice is the same image.
- **Depth is not optional.** The pipeline tests and writes depth, and
  `pass` attaches it. A 3D frame drawn without depth is a wrong picture
  that looks plausible, which is worse than one that refuses.
- **An adapter with no depth format is refused by name**, before
  anything is created, carrying the format chain that was tried.
- **Positions are clip space.** There is no camera; a caller drawing a
  world transforms on its own side.
- **Target-agnostic.** This crate never renders, never presents and
  never touches a window. It describes draws; the caller owns the
  target.
- **A mesh belongs to the caller.** `upload` hands it back rather than
  keeping it, so one mesh can be drawn by several items in a frame —
  which the rendering crate deliberately allows.

## Two decisions worth knowing

**The depth refusal is a translation, not a second detection.** The
obvious shape is to ask the device for a depth format and refuse if
there is none. That path has no reachable trigger: it needs a genuinely
depthless adapter, and every adapter the tests run on offers depth, so
the centrepiece of this crate would ship with its message tested and its
trigger never executed. Pipeline creation already refuses by name and
names the chain, so that refusal is translated instead — and the mapping
is driven by a unit test on every machine.

**An empty scene is refused here.** The rendering crate treats empty
geometry as a caller bug and asserts before returning its error, so an
empty scene passed through would panic in every build this repository
tests in. An all-air world or a fully culled mesh is ordinary data, so
it gets an ordinary refusal.

## Not in v0

No camera or projection, no textures or atlas, no window or
presentation, no image writing, and no meshing — a caller supplies quads
already in clip space. Each is a later step. The vertex layout is a
clip-space position and a colour, packed to 28 bytes with no padding:
the rendering crate asserts at record time that a mesh's stride matches
the pipeline's, and a `#[repr(C)]` struct over the maths crate's aligned
vectors would not give 28.

## Testing note

The pure half's tests need no adapter and run everywhere, including the
sanitizer lanes. The pixel oracles compute their expected image rather
than committing one — the geometry is axis-aligned quads in flat colour,
so the only edge in play is the diagonal each quad's triangles share,
and the fill rule gives a sample on it to exactly one of them.

One of those oracles carries more than it looks. `at_equal_depth_the_later_push_wins`
draws two quads at the same depth in each order; under the fixed
`LESS_OR_EQUAL` compare the later one wins, so it is the only test here
that fails if submission order is perturbed anywhere between a push and
a draw. Verified by doing exactly that: reversing the index order makes
it the single failure, while every other test in the crate passes.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies
and extension points. Contract lints live in `clippy.toml`: clock reads,
filesystem access and thread spawning are rejected at lint time.
