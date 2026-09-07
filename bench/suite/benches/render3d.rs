//! The 3D geometry path a renderer runs before any device sees it.
//!
//! `Scene` is the device-free half of `renew-render3d`: it holds no
//! device and can be built on a machine with no adapter, so everything
//! here runs anywhere the suite runs. What it does per face is index and
//! offset arithmetic over a byte container, which is the part a frame
//! pays for every visible face, every frame.
//!
//! Four lines. **Exactly one pair of them is controlled**, and saying
//! which is the whole point of this comment — a reader who assumes the
//! other differences are subtractions will draw a conclusion the
//! experiment cannot support:
//!
//! - `scene_build_4096` and `scene_build_4096_growing` **are** a pair.
//!   The same 4,096 faces into a scene that reserved up front and into
//!   one that grows; both allocate inside the timed region and both drop
//!   the scene there, so they differ in one thing and the difference is
//!   what the reservation is worth. It is measured rather than argued
//!   because the constructor's documentation argues for it while no
//!   caller in the tree uses it.
//! - `scene_bounds_4096` reads the extent of a scene built once outside
//!   the timed region — a walk over every vertex, which a caller that
//!   frames or culls a world does per rebuild.
//! - `scene_clear_rebuild_4096` is the frame-loop shape: keep the
//!   buffers, drop the contents, fill again. **It is not the third term
//!   of a subtraction.** It differs from both build lines in three ways
//!   at once — it allocates nothing inside the timed region, frees
//!   nothing there, and starts from warm buffers — so its distance from
//!   them isolates none of the three. Read it as its own number: what a
//!   steady-state frame pays to refill a scene it already owns.
//!
//! `quad_uv` is the widest of the three appenders — corners, per-corner
//! colours and per-corner texture coordinates — so it is what the build
//! lines time. The voxel sample calls it for every visible face and
//! calls the plain `quad` for its crosshair; both are exercised in the
//! tree, and this file times the wider one.
//!
//! **The per-face figure is not the cost of `quad_uv` alone.** Each
//! iteration streams the whole 4,096-face fixture — over half a mebibyte
//! read, more than a mebibyte touched with the output — which does not
//! fit in L2 on the machine these were recorded on. A real caller
//! generates its faces as it goes and pays no such stream. What the
//! lines compare against each other is sound, because every one of them
//! pays it; what a single absolute number means on its own is less.
//!
//! Nothing here measures the GPU. Upload and draw need a device, and no
//! lane can time one; the cost of a face on the other side of the
//! seam is unmeasured and stays that way until the RHI can be asked.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_render3d::Scene;

/// Faces per frame. The `with_capacity` documentation reasons about "a
/// four-thousand-face world", so the lines are sized at the scale that
/// argument is about rather than at a round number chosen here.
const QUADS: usize = 4096;

/// Corners, colours and texture coordinates that vary per face, so only
/// the shape of the call is invariant.
///
/// Counted in `f32` rather than cast from the index — the cast is denied
/// in this suite, and the counter is exact for every value this reaches.
type Face = ([[f32; 3]; 4], [[f32; 4]; 4], [[f32; 2]; 4]);

fn faces() -> Vec<Face> {
    let mut out = Vec::with_capacity(QUADS);
    let mut step = 0.0f32;
    while out.len() < QUADS {
        let left = step * 0.001;
        let top = step * 0.002;
        let depth = step * 0.0005;
        let corners = [
            [left, top, depth],
            [left + 0.05, top, depth],
            [left + 0.05, top + 0.05, depth],
            [left, top + 0.05, depth],
        ];
        let shade = 0.25 + step * 0.0001;
        let colours = [
            [shade, 0.5, 0.75, 1.0],
            [0.5, shade, 0.75, 1.0],
            [0.75, 0.5, shade, 1.0],
            [shade, shade, 0.75, 1.0],
        ];
        let texel = step * 0.0002;
        let uvs = [
            [texel, 0.0],
            [texel + 0.01, 0.0],
            [texel + 0.01, 0.01],
            [texel, 0.01],
        ];
        out.push((corners, colours, uvs));
        step += 1.0;
    }
    out
}

fn scene_geometry(c: &mut Criterion) {
    let faces = faces();

    c.bench_function("scene_build_4096", |b| {
        b.iter(|| {
            let mut scene = Scene::with_capacity(QUADS);
            for (corners, colours, uvs) in &faces {
                scene.quad_uv(*corners, *colours, *uvs);
            }
            black_box(scene.index_count());
            black_box(scene);
        });
    });

    // The same faces into a scene that reserved nothing — which is what
    // every caller outside this file does. Paired with the line above:
    // same work, same drop, one difference.
    c.bench_function("scene_build_4096_growing", |b| {
        b.iter(|| {
            let mut scene = Scene::new();
            for (corners, colours, uvs) in &faces {
                scene.quad_uv(*corners, *colours, *uvs);
            }
            black_box(scene.index_count());
            black_box(scene);
        });
    });

    // Reading the extent back: a walk over every vertex, which a caller
    // that frames or culls a world does once per rebuild.
    let mut built = Scene::with_capacity(QUADS);
    for (corners, colours, uvs) in &faces {
        built.quad_uv(*corners, *colours, *uvs);
    }
    c.bench_function("scene_bounds_4096", |b| {
        // The scene is fenced as well as the result: without it the walk
        // reads a value the optimiser can see is loop-invariant, and
        // nothing stops it being hoisted out of the iteration entirely.
        b.iter(|| black_box(black_box(&built).bounds()));
    });

    // The frame-loop shape: keep the buffers, drop the contents, fill
    // again. `Scene::clear` clears the vectors rather than dropping
    // them, so the second iteration onward starts warm — which is what
    // makes this a steady-state number and not a build number.
    c.bench_function("scene_clear_rebuild_4096", |b| {
        let mut scene = Scene::with_capacity(QUADS);
        b.iter(|| {
            scene.clear();
            for (corners, colours, uvs) in &faces {
                scene.quad_uv(*corners, *colours, *uvs);
            }
            black_box(scene.index_count());
        });
    });
}

criterion_group!(geometry, scene_geometry);
criterion_main!(geometry);
