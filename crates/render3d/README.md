# renew-render3d

Indexed 3D geometry over the rendering crate: mesh pipelines,
depth-tested, indexed draws in submission order. Quads go in on the host, a
mesh comes out, and a draw item goes into a frame the caller composes.

- `Scene` — the pure half. Accumulates quads into packed vertex bytes
  and indices. No device, no adapter, no GPU call, so the packing and
  the numbering are testable on any machine.
- `MeshRenderer` / `TexturedMeshRenderer` — the clip-space device half.
  Each owns its pipeline, uploads a scene into a `Mesh`, and hands back
  an `Item`; the textured one samples an atlas through the one
  binding it holds.
- `CameraRenderer` / `TexturedCameraRenderer` — the world-space device
  half. Same shape, plus a `Camera`: sixty-four packed bytes of
  view-projection matrix that ride each item as push data, so the GPU
  performs the transform and the perspective divide.
- `Camera` — the pack type. Four column-major `[f32; 4]` columns in,
  sixty-four bytes out; whoever owns a camera owns the maths that built
  it, and what crosses this boundary is bytes with a stated order.
- `ShadowedCameraRenderer` / `ShadowMatrices` — the world-space half
  with a shadow. One type owns the whole story: a depth render image
  (the map), a depth-only caster pipeline that draws the scene from
  the light with no fragment stage at all, the lit pipeline sampling
  the atlas at slot 0 and the map at slot 1, and both bindings. A
  frame leads with `shadow_pass` (`caster_item`s pushing the light's
  sixty-four bytes — a light IS a camera), then draws `item`s pushing
  both matrices as one 128-byte `ShadowMatrices` block. The shadow
  test is reversed-Z like everything else: the map holds the depth
  nearest the light, and a fragment is lit exactly when its own light
  depth reaches it within a constant bias — constant because the
  light is orthographic, which makes light depth linear.
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
- **What a position means is the renderer's promise.** `MeshRenderer`
  and `TexturedMeshRenderer` take clip space and draw it straight;
  `CameraRenderer` and `TexturedCameraRenderer` take world space and
  multiply by the camera's matrix on the GPU. Two renderer families
  rather than a flag, because the meaning of a scene's positions must
  be decidable at the call site — the failure mode of guessing is a
  plausible wrong picture, not an error.
- **A camera costs nothing but its bytes.** The matrix is recorded as
  push data per draw: no buffer, no retention slot, and several camera
  items in one frame cost nothing extra. The camera pipelines fade
  distant fragments toward a horizon colour — a readability floor, not
  a look, and stated in their rustdoc because behaviour a caller cannot
  predict from a type's name is behaviour the type must name itself.
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

No window or presentation, no image writing, and no meshing — a voxel
mesher belongs to the sample that knows its world. The camera is a
matrix, not a viewpoint type: eye/target/projection maths belongs to the
caller — the engine's camera crate, ordinarily — and this crate takes
the sixty-four bytes that result. The vertex layout is a
position, a colour and a texture coordinate, packed to 36 bytes with no
padding: the rendering crate asserts at record time that a mesh's stride
matches the pipeline's, and a `#[repr(C)]` struct over the maths crate's
aligned vectors would not give 36.

## Testing note

The pure half's tests need no adapter and run everywhere, including the
sanitizer lanes. The pixel oracles compute their expected image rather
than committing one — the geometry is axis-aligned quads in flat colour,
so the only edge in play is the diagonal each quad's triangles share,
and the fill rule gives a sample on it to exactly one of them.

One of those oracles carries more than it looks. `at_equal_depth_the_later_push_wins`
draws two quads at the same depth in each order; under the fixed
`GREATER_OR_EQUAL` compare (depth is reversed engine-wide) the later
one wins, so it is the only test here that fails if submission order is
perturbed anywhere between a push and a draw. Verified by doing exactly that: reversing the index order makes
it the single failure, while every other test in the crate passes.

Those oracles skip where there is no adapter, which is why the software
rasterizer lane runs them with `RENEW_GOLDEN=1`, where a skip is a
failure. Without that lane they would be six green ticks proving nothing
on every runner without a GPU — including the depth-format assertion
that lets this crate translate the depth refusal instead of pre-flighting
it, which is a premise the design rests on rather than a detail.

`Scene` gets property tests as well as examples: it is index-and-offset
arithmetic over a byte container, and the two invariants the layer below
relies on — whole records, and every index inside this scene's own
corners — are statements about arbitrary quad counts that examples at one
and two quads cannot make.

**Fuzzing: N/A.** Nothing here parses external data. `Scene` serialises
first-party `f32`s handed in by a caller, and the bytes it produces are
consumed by the rendering crate in the same process. The obligation
attaches when geometry arrives from a file.

## Manifest

`Cargo.toml` is authoritative for maturity, core status, dependencies
and extension points. Contract lints live in `clippy.toml`: clock reads,
filesystem access and thread spawning are rejected at lint time — the
same fourteen paths the 2D sibling names, including the spellings that
would otherwise walk around the obvious ones (`Builder::spawn` for a
named thread, `Read::read_to_end` for a whole file).
