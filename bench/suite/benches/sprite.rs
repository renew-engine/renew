//! The sprite construction chain and the packer beneath it, at the rate
//! a presenter runs them.
//!
//! `UiPresenter::emit` builds one sprite per quad per frame from a
//! compile-time-constant atlas region:
//!
//! ```ignore
//! sprites.push(&Sprite::new(atlas::white(), q.x, q.y).size(..).tint(..));
//! ```
//!
//! Everything in that line except `push` is device-free and is timed
//! here as `sprite_build_2048`; the packer `push` runs afterwards is
//! reachable without a device through `Sprite::instance` and is timed
//! as `sprite_pack_2048` for the untransformed sprite and as
//! `sprite_pack_transformed_2048` with every sprite turned, scaled
//! and smeared.
//! All three exist so that a change to `Sprite`'s layout, to its
//! constructor or to the packer's arithmetic has a before.
//!
//! The region is deliberately **not** wrapped in `black_box`: a
//! presenter's region is a constant at the call site, and hiding it
//! would measure a different program. Whether that constant's widening
//! hoists out of the loop was measured with and without `#[inline]` on
//! the constructor and builders; the ranges overlapped, so no attribute
//! is carried. Only the results are fenced.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use renew_render2d::{Canvas, Region, Sprite};
use renew_rhi::Extent;

/// Quads per frame, matching the presenter's own ceiling at the tree
/// size the `ui` suite benches: `max_quads` is twice the node count,
/// and that suite stands at 1,024 nodes.
const QUADS: usize = 2048;

/// The white texel every plain background quad samples — a constant at
/// the real call site, and a constant here.
const WHITE: Region = Region {
    x: 2,
    y: 2,
    width: 4,
    height: 4,
};

/// Destinations that vary per quad, so only the source is invariant.
fn destinations() -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(QUADS);
    // Counted in `f32` rather than cast from the index: the cast is
    // denied here and the counter is exact for every value this reaches.
    let mut n = 0.0f32;
    while out.len() < QUADS {
        out.push([n * 0.5, n * 0.25, 8.0 + n * 0.125, 6.0 + n * 0.0625]);
        n += 1.0;
    }
    out
}

fn sprite_chain(c: &mut Criterion) {
    c.bench_function("sprite_build_2048", |b| {
        let rects = destinations();
        b.iter(|| {
            // Each sprite is fenced by reference, not folded into a
            // scalar. `push` takes `&Sprite` from another crate, so the
            // whole struct must genuinely exist; a fold would let the
            // optimiser delete it and time a different program.
            for rect in &rects {
                let sprite = Sprite::new(WHITE, rect[0], rect[1])
                    .size(rect[2], rect[3])
                    .tint([1.0, 1.0, 1.0, 1.0]);
                black_box(&sprite);
            }
        });
    });
}

criterion_group!(benches, sprite_chain);

/// The packer itself: every sprite the chain above builds, turned into
/// its instance record without a device. `Sprite::instance` is what
/// `SpriteRenderer::push` runs after applying the batch state, so this
/// is the per-sprite cost of the fill minus the batch placement, the
/// capacity assertion and the copy into the frame's scratch.
fn sprite_pack(c: &mut Criterion) {
    c.bench_function("sprite_pack_2048", |b| {
        let Some(canvas) = Canvas::new(320, 240) else {
            unreachable!("320 by 240 is nonzero");
        };
        let atlas = Extent {
            width: 8,
            height: 8,
        };
        let sprites: Vec<Sprite> = destinations()
            .iter()
            .map(|rect| Sprite::new(WHITE, rect[0], rect[1]).size(rect[2], rect[3]))
            .collect();
        b.iter(|| {
            for sprite in &sprites {
                // Fenced by value: the record is the bytes the renderer
                // would copy into its scratch, and a fold would let the
                // optimiser skip lanes nothing reads.
                black_box(sprite.instance(canvas, atlas));
            }
        });
    });
    c.bench_function("sprite_pack_transformed_2048", |b| {
        // The same sprites, every one turned, scaled and smeared: the
        // pivot arithmetic, the crate's own sine and cosine, and the
        // smear extension, per sprite — the everything-on cost.
        let Some(canvas) = Canvas::new(320, 240) else {
            unreachable!("320 by 240 is nonzero");
        };
        let atlas = Extent {
            width: 8,
            height: 8,
        };
        let sprites: Vec<Sprite> = destinations()
            .iter()
            .map(|rect| {
                Sprite::new(WHITE, rect[0], rect[1])
                    .size(rect[2], rect[3])
                    .rotation(0.05)
                    .saturation(0.5)
                    .flash(0.1)
                    .scale(1.5, 0.75)
                    .smear(2.0, 1.0)
            })
            .collect();
        b.iter(|| {
            for sprite in &sprites {
                black_box(sprite.instance(canvas, atlas));
            }
        });
    });
}

criterion_group!(packing, sprite_pack);
criterion_main!(benches, packing);
