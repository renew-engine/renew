//! Write the PLY reader's seed corpus.
//!
//! **Every input here is first-party**, so no licence question comes with
//! the corpus — the same rule the sample atlases and the other generated
//! corpora follow.
//!
//! A fuzzer reaches `end_header` eventually. It reaches a *coherent
//! header describing a body that is not there* essentially never, and
//! that is the shape this format's interesting failures live in: a count
//! of four billion, a list length that overruns the file, a property
//! whose width walks the cursor off the end on the last row. These seeds
//! put a mutation's starting point on the far side of the header parse,
//! where the arithmetic an attacker controls actually runs.
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-mesh --example make_ply_corpus
//! ```
//!
//! Existing files are left alone. The fuzzer adds its own finds to this
//! directory over time, and this program must never delete them.
//!
//! It exits non-zero if any seed it meant to write is missing afterwards.
//! Every bail prints its reason, because a generator that prints a
//! failure and then reports success is worse than one that crashes: the
//! caller sees a zero and believes the corpus is whole.

// The crate bans filesystem access because the library never touches a
// file -- a caller that reads one owns it, and owns the bound on reading
// it. This program is that caller: writing the corpus is its whole job.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes, never a path.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;
use std::process::ExitCode;

const SQUARE: [[f32; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 0.0],
];

/// The plain ASCII square: four shared corners, two faces.
fn ascii_square() -> Vec<u8> {
    use core::fmt::Write as _;
    let mut out = String::from(
        "ply\nformat ascii 1.0\ncomment a square\n\
         element vertex 4\nproperty float x\nproperty float y\nproperty float z\n\
         element face 2\nproperty list uchar int vertex_indices\nend_header\n",
    );
    for corner in SQUARE {
        let _ = writeln!(out, "{} {} {}", corner[0], corner[1], corner[2]);
    }
    out.push_str("3 0 1 2\n3 0 2 3\n");
    out.into_bytes()
}

/// The same square with a binary body, in either byte order.
fn binary_square(big: bool) -> Vec<u8> {
    let format = if big {
        "binary_big_endian"
    } else {
        "binary_little_endian"
    };
    let mut out = format!(
        "ply\nformat {format} 1.0\n\
         element vertex 4\nproperty float x\nproperty float y\nproperty float z\n\
         element face 2\nproperty list uchar int vertex_indices\nend_header\n"
    )
    .into_bytes();
    for corner in SQUARE {
        for value in corner {
            out.extend_from_slice(&if big {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            });
        }
    }
    for face in [[0u32, 1, 2], [0, 2, 3]] {
        out.push(3);
        for index in face {
            out.extend_from_slice(&if big {
                index.to_be_bytes()
            } else {
                index.to_le_bytes()
            });
        }
    }
    out
}

fn valid_seeds() -> Vec<(&'static str, Vec<u8>)> {
    use core::fmt::Write as _;
    let mut seeds: Vec<(&'static str, Vec<u8>)> = Vec::new();

    // --- Files that read, giving the fuzzer a shape to mutate. ------
    seeds.push(("ascii-square.seed", ascii_square()));
    seeds.push(("binary-le-square.seed", binary_square(false)));
    seeds.push(("binary-be-square.seed", binary_square(true)));

    // A schema full of columns this reader skips by width. **The seed
    // that keeps the schema-following honest**: coordinates are found by
    // name, and a mutation that broke that would still read the plain
    // square above.
    let mut wide = String::from(
        "ply\nformat ascii 1.0\nelement vertex 4\n\
         property uchar red\nproperty float x\nproperty double confidence\n\
         property float y\nproperty float nx\nproperty float z\nproperty uchar alpha\n\
         element face 2\nproperty list uchar int vertex_indices\n\
         property uchar flags\nend_header\n",
    );
    for corner in SQUARE {
        let _ = writeln!(
            wide,
            "255 {} 0.5 {} 0.0 {} 128",
            corner[0], corner[1], corner[2]
        );
    }
    wide.push_str("3 0 1 2 7\n3 0 2 3 7\n");
    seeds.push(("ascii-wide-schema.seed", wide.into_bytes()));

    // A quad, so the fan path is seeded rather than discovered.
    let quad = String::from_utf8(ascii_square())
        .unwrap_or_default()
        .replace("element face 2", "element face 1")
        .replace("3 0 1 2\n3 0 2 3\n", "4 0 1 2 3\n");
    seeds.push(("ascii-quad.seed", quad.into_bytes()));

    // Every scalar width the format defines, in one binary body, so a
    // mutation of a type name lands somewhere the widths matter.
    let mut widths = String::from(
        "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
         property char a\nproperty short b\nproperty double x\nproperty double y\n\
         property double z\nproperty uint c\n\
         element face 1\nproperty list uchar int vertex_indices\nend_header\n",
    )
    .into_bytes();
    for corner in [SQUARE[0], SQUARE[1], SQUARE[2]] {
        widths.push(0xFF);
        widths.extend_from_slice(&(-2i16).to_le_bytes());
        for value in corner {
            widths.extend_from_slice(&f64::from(value).to_le_bytes());
        }
        widths.extend_from_slice(&7u32.to_le_bytes());
    }
    widths.push(3);
    for index in [0u32, 1, 2] {
        widths.extend_from_slice(&index.to_le_bytes());
    }
    seeds.push(("binary-every-width.seed", widths));

    seeds
}

/// One malformed file per refusal the reader can make.
///
/// A fuzzer reaches a refusal eventually; it reaches *this* refusal
/// having spent most of a budget on the ones before it. Each of these
/// puts a mutation's starting point on the far side of a check the
/// random walk would rarely satisfy on its own.
#[expect(
    clippy::vec_init_then_push,
    reason = "each entry carries the comment explaining why it is here, and several are computed from a local; a vec literal would be either unreadable or impossible"
)]
fn malformed_seeds() -> Vec<(&'static str, Vec<u8>)> {
    use core::fmt::Write as _;
    let mut seeds: Vec<(&'static str, Vec<u8>)> = Vec::new();
    // --- One file per refusal. --------------------------------------
    seeds.push(("empty.seed", Vec::new()));
    seeds.push(("not-a-ply.seed", b"solid teapot\n".to_vec()));
    seeds.push((
        "no-end-header.seed",
        b"ply\nformat ascii 1.0\nelement vertex 1\n".to_vec(),
    ));
    seeds.push((
        "unknown-format.seed",
        b"ply\nformat ebcdic 1.0\nend_header\n".to_vec(),
    ));
    seeds.push((
        "wrong-version.seed",
        b"ply\nformat ascii 2.0\nend_header\n".to_vec(),
    ));
    seeds.push((
        "unknown-type.seed",
        b"ply\nformat ascii 1.0\nelement vertex 1\nproperty quadruple x\nend_header\n".to_vec(),
    ));
    // A point cloud: a valid PLY this reader cannot use, which is a
    // different answer from a malformed one.
    seeds.push((
        "point-cloud.seed",
        b"ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\nproperty float y\n\
          property float z\nend_header\n0 0 0\n1 1 1\n"
            .to_vec(),
    ));
    // Vertices with no coordinates on them.
    seeds.push((
        "no-coordinates.seed",
        b"ply\nformat ascii 1.0\nelement vertex 1\nproperty uchar red\nelement face 1\n\
          property list uchar int vertex_indices\nend_header\n255\n3 0 0 0\n"
            .to_vec(),
    ));
    // **The refusal this format adds over STL**: a face naming a vertex
    // that is not there, and the one-past-the-end case beside it.
    let past = String::from_utf8(ascii_square())
        .unwrap_or_default()
        .replace("3 0 2 3", "3 0 2 9");
    seeds.push(("index-out-of-range.seed", past.into_bytes()));
    let edge = String::from_utf8(ascii_square())
        .unwrap_or_default()
        .replace("3 0 1 2", "3 0 1 4");
    seeds.push(("index-one-past-the-end.seed", edge.into_bytes()));
    // A face that covers no area.
    let line = String::from_utf8(ascii_square())
        .unwrap_or_default()
        .replace("3 0 2 3", "2 0 2");
    seeds.push(("face-of-two-corners.seed", line.into_bytes()));
    // A coordinate that is not a number at all, which is a different
    // refusal from one that is a number and unusable.
    let words = String::from_utf8(ascii_square())
        .unwrap_or_default()
        .replace(
            "1 0 0
",
            "north 0 0
",
        );
    seeds.push(("not-a-number.seed", words.into_bytes()));
    // A coordinate that is not a number a bounding box can hold.
    let infinite = String::from_utf8(ascii_square())
        .unwrap_or_default()
        .replace("1 0 0\n", "inf 0 0\n");
    seeds.push(("not-finite.seed", infinite.into_bytes()));
    // **A coherent header describing a body that is not there** — the
    // shape a fuzzer essentially never reaches on its own, and the one
    // that separates a reader from a denial of service.
    seeds.push((
        "count-with-no-body.seed",
        b"ply\nformat ascii 1.0\nelement vertex 4000000000\nproperty float x\n\
          property float y\nproperty float z\nelement face 1\n\
          property list uchar int vertex_indices\nend_header\n"
            .to_vec(),
    ));
    // The same in binary, where the cursor rather than the word supply
    // runs out.
    seeds.push((
        "binary-count-with-no-body.seed",
        b"ply\nformat binary_little_endian 1.0\nelement vertex 4000000000\n\
          property float x\nproperty float y\nproperty float z\nelement face 1\n\
          property list uchar int vertex_indices\nend_header\n"
            .to_vec(),
    ));
    // A face claiming more corners than any real face has.
    let fan = String::from_utf8(ascii_square())
        .unwrap_or_default()
        .replace("3 0 1 2", "5000 0 1 2");
    seeds.push(("enormous-face.seed", fan.into_bytes()));
    // A schema wider than a schema.
    let mut many = String::from("ply\nformat ascii 1.0\nelement vertex 1\n");
    for index in 0..2000 {
        let _ = writeln!(many, "property float p{index}");
    }
    many.push_str("end_header\n");
    seeds.push(("enormous-schema.seed", many.into_bytes()));

    // **The refusal this corpus never reached.** The census in
    // `tests/ply.rs` says this reader can answer `NoGeometry`, and the
    // replay census said the committed seeds reached nine of its ten
    // outcomes — which is the hole the outcome floor exists to notice,
    // and did not, because the floor had been set high enough to pass
    // without it. A file declaring both elements and populating neither
    // is the shortest way to provoke it.
    seeds.push((
        "both-elements-empty.seed",
        b"ply\nformat ascii 1.0\nelement vertex 0\n\
          property float x\nproperty float y\nproperty float z\n\
          element face 0\nproperty list uchar int vertex_indices\n\
          end_header\n"
            .to_vec(),
    ));

    seeds
}

fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/ply_read");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {error}", dir.display());
        return ExitCode::FAILURE;
    }
    let mut seeds = valid_seeds();
    seeds.extend(malformed_seeds());

    let mut written = 0usize;
    for (name, bytes) in &seeds {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        if let Err(error) = std::fs::write(&path, bytes) {
            eprintln!("cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        written += 1;
    }

    // Every seed this program meant to produce must be there afterwards,
    // whether this run wrote it or a previous one did.
    let missing: Vec<&str> = seeds
        .iter()
        .filter(|(name, _)| !dir.join(name).exists())
        .map(|(name, _)| *name)
        .collect();
    if !missing.is_empty() {
        eprintln!("these seeds are missing after the run: {missing:?}");
        return ExitCode::FAILURE;
    }

    println!(
        "{} seeds, {written} written this run, in {}",
        seeds.len(),
        dir.display()
    );
    ExitCode::SUCCESS
}
