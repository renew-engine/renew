//! Write the accessor layer's seed corpus.
//!
//! **Every byte here is built by this program**, which for once needs no
//! licensing argument at all: an accessor's input is six numbers and a
//! region of bytes, and no model was involved in making either.
//!
//! The encoding is `crates/mesh/tests/shared/accessor_seed.rs`, included
//! below rather than copied, because the fuzz target and the merge-time
//! replay gate read the same bytes.
//!
//! **What a random walk will not find here is a claim that is nearly
//! right.** The head is thirteen bytes and the fuzzer will vary all of
//! them quickly; what it will not stumble on is a count, a stride and a
//! region that agree to within one byte, which is where every
//! interesting fault in these layers lives. These seeds sit on both
//! sides of that line — one that fits exactly, one that is a byte short,
//! and one of each refusal besides.
//!
//! **Some seeds resolve a buffer view first**, which puts the claim that
//! a region is really inside its buffer in front of the claim that
//! elements are really inside the region. They are two of the three
//! claims an accessor makes, and only the second can be checked without
//! the first.
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-mesh --example make_accessor_corpus
//! ```
//!
//! Existing files are left alone. The fuzzer adds its own finds to this
//! directory over time, and this program must never delete them.

// The crate bans filesystem access because the library never touches a
// file -- a caller that reads one owns it, and owns the bound on reading
// it. This program is that caller: writing the corpus is its whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes, never a path.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;
use std::process::ExitCode;

use renew_mesh::accessor::{BufferView, Component, Shape};

#[path = "../tests/shared/accessor_seed.rs"]
mod seed;

use seed::{Seed, encode};

/// A seed with the usual defaults, so each case below states only what
/// makes it the case it is.
fn claim(component: Component, shape: Shape, count: usize, region: &[u8]) -> Seed<'_> {
    Seed {
        code: component.code(),
        shape,
        count,
        byte_offset: 0,
        byte_stride: None,
        view: None,
        assemble: None,
        index_offset: 0,
        normalized: false,
        as_indices: false,
        region,
    }
}

/// A buffer holding three positions and then six indices over them.
///
/// The layout a real file uses: attributes and the index stream in one
/// region, each located by its own offset.
fn positions_then_indices(indices: &[u16]) -> Vec<u8> {
    let mut out: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    for index in indices {
        out.extend_from_slice(&index.to_le_bytes());
    }
    out
}

/// Claims the reader accepts, so the search has somewhere to mutate
/// *from*.
fn readable_seeds() -> Vec<(String, Vec<u8>)> {
    let one_face = positions_then_indices(&[0, 1, 2]);
    let shared = positions_then_indices(&[0, 1, 2, 2, 1, 0]);
    let twelve = [0u8; 12];
    let thirty_six = [1u8; 36];
    let forty_four = [2u8; 44];
    let eight = [0xFFu8; 8];

    vec![
        (
            "packed-vec3-float".to_owned(),
            encode(&claim(Component::F32, Shape::Vec3, 3, &thirty_six)),
        ),
        (
            "one-element".to_owned(),
            encode(&claim(Component::F32, Shape::Vec3, 1, &twelve)),
        ),
        // **Exactly enough, by the bound that counts the last element's
        // size rather than a whole stride.** Deleting this seed leaves
        // the corpus unable to tell the two expressions apart.
        (
            "interleaved-exact".to_owned(),
            encode(&Seed {
                byte_stride: Some(32),
                ..claim(Component::F32, Shape::Vec3, 2, &forty_four)
            }),
        ),
        (
            "offset-into-region".to_owned(),
            encode(&Seed {
                byte_offset: 8,
                ..claim(Component::F32, Shape::Vec2, 2, &[3u8; 24])
            }),
        ),
        (
            "normalised-unsigned".to_owned(),
            encode(&Seed {
                normalized: true,
                ..claim(Component::U16, Shape::Scalar, 4, &eight)
            }),
        ),
        (
            "normalised-signed".to_owned(),
            encode(&Seed {
                normalized: true,
                ..claim(Component::I8, Shape::Vec4, 2, &eight)
            }),
        ),
        (
            "unsigned-indices".to_owned(),
            encode(&Seed {
                as_indices: true,
                ..claim(Component::U16, Shape::Scalar, 4, &eight)
            }),
        ),
        (
            "byte-indices".to_owned(),
            encode(&Seed {
                as_indices: true,
                ..claim(Component::U8, Shape::Scalar, 8, &eight)
            }),
        ),
        (
            "byte-components".to_owned(),
            encode(&claim(Component::U8, Shape::Vec3, 2, &[7u8; 6])),
        ),
        // **Assembled**, which is the layer above both: three positions
        // and three indices in one buffer, expanded into one triangle.
        (
            "assembled-one-face".to_owned(),
            encode(&Seed {
                assemble: Some(3),
                index_offset: 36,
                ..claim(Component::F32, Shape::Vec3, 3, &one_face)
            }),
        ),
        (
            "assembled-shared-vertex".to_owned(),
            encode(&Seed {
                assemble: Some(6),
                index_offset: 36,
                ..claim(Component::F32, Shape::Vec3, 3, &shared)
            }),
        ),
        // **Through a view**, which puts the claim that a region is
        // really inside its buffer in front of the claim that elements
        // are really inside the region.
        (
            "through-a-view".to_owned(),
            encode(&Seed {
                view: Some(BufferView {
                    byte_offset: 8,
                    byte_length: 24,
                    byte_stride: None,
                }),
                ..claim(Component::F32, Shape::Vec2, 3, &[0u8; 64])
            }),
        ),
    ]
}

/// Assembly claims that are refused, which are a layer above the rest.
///
/// Their own function because they are about a different question — an
/// index addressing a vertex that is not there — and because the list
/// below hit the line limit, which is the linter noticing the same
/// thing.
fn refused_assemblies() -> Vec<(String, Vec<u8>)> {
    let past = positions_then_indices(&[0, 1, 7]);
    let four = positions_then_indices(&[0, 1, 2, 0]);
    vec![
        // **An index past the end of the positions it addresses**, which
        // is the fault this layer exists to catch and the one nothing
        // downstream could.
        (
            "assembled-index-past-the-end".to_owned(),
            encode(&Seed {
                assemble: Some(3),
                index_offset: 36,
                ..claim(Component::F32, Shape::Vec3, 3, &past)
            }),
        ),
        (
            "assembled-corners-do-not-divide".to_owned(),
            encode(&Seed {
                assemble: Some(4),
                index_offset: 36,
                ..claim(Component::F32, Shape::Vec3, 3, &four)
            }),
        ),
    ]
}

/// One claim per refusal, each wrong in exactly one way.
fn refused_seeds() -> Vec<(String, Vec<u8>)> {
    let room = [0u8; 64];

    vec![
        // Shorter than the head, so there are no parameters at all.
        ("no-head".to_owned(), vec![1, 2, 3]),
        // 5124 is `INT`, which the format leaves out on purpose.
        (
            "unknown-component".to_owned(),
            encode(&Seed {
                code: 5124,
                ..claim(Component::F32, Shape::Scalar, 1, &room)
            }),
        ),
        (
            "no-elements".to_owned(),
            encode(&claim(Component::F32, Shape::Vec3, 0, &room)),
        ),
        (
            "normalised-float".to_owned(),
            encode(&Seed {
                normalized: true,
                ..claim(Component::F32, Shape::Scalar, 1, &room)
            }),
        ),
        (
            "stride-unaligned".to_owned(),
            encode(&Seed {
                byte_stride: Some(13),
                ..claim(Component::F32, Shape::Vec3, 2, &room)
            }),
        ),
        (
            "stride-too-large".to_owned(),
            encode(&Seed {
                byte_stride: Some(256),
                ..claim(Component::F32, Shape::Vec3, 2, &room)
            }),
        ),
        (
            "stride-under-element".to_owned(),
            encode(&Seed {
                byte_stride: Some(8),
                ..claim(Component::F32, Shape::Vec3, 2, &room)
            }),
        ),
        (
            "offset-unaligned".to_owned(),
            encode(&Seed {
                byte_offset: 2,
                ..claim(Component::F32, Shape::Scalar, 1, &room)
            }),
        ),
        // **One byte short of the interleaved seed above**, which is the
        // pair that makes the bound testable.
        (
            "interleaved-one-short".to_owned(),
            encode(&Seed {
                byte_stride: Some(32),
                ..claim(Component::F32, Shape::Vec3, 2, &[2u8; 43])
            }),
        ),
        (
            "count-past-the-region".to_owned(),
            encode(&claim(Component::F32, Shape::Vec3, 999, &room)),
        ),
        // **The two the index entry point refuses and the attribute one
        // does not.** Both are perfectly good attribute accessors, which
        // is what makes them worth a seed: the accessor is not wrong,
        // the use of it is.
        (
            "signed-as-indices".to_owned(),
            encode(&Seed {
                as_indices: true,
                ..claim(Component::I16, Shape::Scalar, 2, &room)
            }),
        ),
        (
            "view-past-the-buffer".to_owned(),
            encode(&Seed {
                view: Some(BufferView {
                    byte_offset: 48,
                    byte_length: 32,
                    byte_stride: None,
                }),
                ..claim(Component::F32, Shape::Vec3, 1, &[0u8; 64])
            }),
        ),
        (
            "stride-wider-than-view".to_owned(),
            encode(&Seed {
                byte_stride: Some(64),
                view: Some(BufferView {
                    byte_offset: 0,
                    byte_length: 16,
                    byte_stride: Some(64),
                }),
                ..claim(Component::F32, Shape::Vec3, 2, &[0u8; 64])
            }),
        ),
        (
            "normalised-as-indices".to_owned(),
            encode(&Seed {
                as_indices: true,
                normalized: true,
                ..claim(Component::U8, Shape::Scalar, 2, &room)
            }),
        ),
    ]
}

fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/accessor_view");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {error}", dir.display());
        return ExitCode::FAILURE;
    }

    let mut written = 0usize;
    let mut kept = 0usize;
    for (name, bytes) in readable_seeds()
        .into_iter()
        .chain(refused_seeds())
        .chain(refused_assemblies())
    {
        let path = dir.join(format!("{name}.bin"));
        if path.exists() {
            kept += 1;
            continue;
        }
        // A generator that swallows a write failure and then reports
        // success is worse than one that crashes: the caller sees a
        // count and believes the corpus is whole.
        if let Err(error) = std::fs::write(&path, &bytes) {
            eprintln!("cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        written += 1;
    }

    println!(
        "{written} written, {kept} already present, in {}",
        dir.display()
    );
    ExitCode::SUCCESS
}
