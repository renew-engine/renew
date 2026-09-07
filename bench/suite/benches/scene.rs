//! The 3D geometry path a renderer runs before any device sees it.
//!
//! `Scene` is the device-free half of `renew-render3d`: it holds no
//! device and can be built on a machine with no adapter, so everything
//! here runs anywhere the suite runs. What it does per face is index and
//! offset arithmetic over a byte container, which is the part a frame
//! pays for every visible face, every frame.
//!
//! Four lines, and they exist as two pairs so a difference is a
//! subtraction rather than an edit-and-rebuild:
//!
//! - `scene_build_4096` and `scene_build_4096_growing` are the same
//!   4,096 faces built into a scene that reserved up front and into one
//!   that grows. **The difference is what `Scene::with_capacity` is
//!   worth**, and it is measured rather than argued because the
//!   constructor's own documentation argues for it — "an allocation per
//!   quad on a four-thousand-face world is a cost with no reason" —
//!   while the only consumer in the tree calls `Scene::new`.
//! - `scene_bounds_4096` and `scene_clear_rebuild_4096` are the two
//!   things a caller does with a scene after building it: read its
//!   extent, and reuse its buffers for the next frame.
//!
//! `quad_uv` is the widest of the three appenders — corners, per-corner
//! colours and per-corner texture coordinates — and is the one the
//! sample actually calls, so it is what the build lines time.
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
    // the one consumer in the tree does. Paired with the line above so
    // the reservation's worth is a subtraction.
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
        b.iter(|| black_box(built.bounds()));
    });

    // The frame-loop shape: keep the buffers, drop the contents, fill
    // again. This is the path a moving world takes every frame, and it
    // is the one `with_capacity` stops mattering for after frame one.
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
