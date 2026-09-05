//! The sprite construction chain, at the rate a presenter runs it.
//!
//! `UiPresenter::emit` builds one sprite per quad per frame from a
//! compile-time-constant atlas region:
//!
//! ```ignore
//! sprites.push(&Sprite::new(atlas::white(), q.x, q.y).size(..).tint(..));
//! ```
//!
//! Everything in that line except `push` is device-free, so it can be
//! timed here while the packer itself — `pub(crate)`, reachable only
//! through a `SpriteRenderer` that needs a GPU device — cannot.
//!
//! **What this exists to catch.** A whole `Region` reaches the packer
//! through an integer-to-float widening. When the region is a constant,
//! that widening is loop-invariant over the entire frame and should cost
//! nothing — but it can only be hoisted if the constructor is visible to
//! the caller's optimiser, and nothing here is generic, the workspace
//! sets no LTO, so visibility means `#[inline]` and only `#[inline]`.
//! Without it the engine re-converts the same four constants once per
//! quad per frame, and once per *character* of every label.
//!
//! The region is deliberately **not** wrapped in `black_box`: its
//! constancy is the property under test, and hiding it would measure a
//! different program. Only the results are fenced.

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
            // scalar. `push` takes `&Sprite` from another crate, so all
            // 48 bytes must genuinely exist; a fold would let the
            // optimiser delete the struct and time a different program.
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
/// is the per-sprite cost of the fill minus one copy into the frame's
/// scratch.
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
                // Fenced by value: the record is forty-eight bytes the
                // renderer would copy into its scratch, and a fold would
                // let the optimiser skip lanes nothing reads.
                black_box(sprite.instance(canvas, atlas));
            }
        });
    });
}

criterion_group!(packing, sprite_pack);
criterion_main!(benches, packing);
