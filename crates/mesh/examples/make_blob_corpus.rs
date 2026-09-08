//! Write the blob reader's seed corpus.
//!
//! **These seeds are the only ones in the tree that need no licensing
//! argument at all**, because the format is this repository's own and
//! every byte of it comes out of `blob::write`. The other four
//! generators build their inputs by hand to avoid borrowing somebody's
//! model; this one builds meshes and asks the writer for the bytes,
//! which is both simpler and a stronger guarantee.
//!
//! A fuzzer finds the magic quickly, because eight fixed bytes are what
//! a coverage-guided search is best at. What it does not find is a
//! *coherent header describing a body that is not there*: the arrays are
//! located by arithmetic on a four-byte corner count, so the inputs that
//! matter are the ones where that count is plausible and wrong. These
//! seeds put a mutation's starting point on both sides of that line.
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-mesh --example make_blob_corpus
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

use renew_mesh::{Mesh, blob};

/// Two triangles, with every optional array filled.
fn furnished() -> Mesh {
    Mesh {
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        face_normals: vec![[0.0, 0.0, 1.0], [0.0, 0.0, -1.0]],
        corner_normals: vec![
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0],
            [0.0, -1.0, 0.0],
            [-1.0, 0.0, 0.0],
        ],
        corner_texcoords: vec![
            [0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0],
            [1.0, 1.0],
            [0.25, 0.5],
            [-2.0, 3.5],
        ],
    }
}

/// One triangle and nothing optional, which is what the PLY reader
/// produces.
fn bare() -> Mesh {
    Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        ..Mesh::default()
    }
}

/// Blobs that read, so the fuzzer has somewhere to mutate *from*.
///
/// **All eight presence shapes**, because the flags are independent and
/// a search that only ever saw "all" and "none" would never learn that
/// the bits mean different things.
fn readable_seeds() -> Vec<(String, Vec<u8>)> {
    let full = furnished();
    // No `furnished.seed`: shape 7 below IS the furnished mesh, and the
    // same bytes under two names is a corpus that says one number and
    // means another. `bare` stays, because three positions with no
    // optional arrays is a different blob from six with none.
    let mut seeds = vec![("bare.seed".to_owned(), blob::write(&bare()))];
    for shape in 0..8u8 {
        let mesh = Mesh {
            positions: full.positions.clone(),
            face_normals: if shape & 1 != 0 {
                full.face_normals.clone()
            } else {
                Vec::new()
            },
            corner_normals: if shape & 2 != 0 {
                full.corner_normals.clone()
            } else {
                Vec::new()
            },
            corner_texcoords: if shape & 4 != 0 {
                full.corner_texcoords.clone()
            } else {
                Vec::new()
            },
        };
        seeds.push((format!("shape-{shape}.seed"), blob::write(&mesh)));
    }
    // Coordinates a mutator will not invent: subnormals, a very large
    // finite value, and a negative zero, all of which are legal and all
    // of which the reader must hand back unchanged.
    seeds.push((
        "extreme-coordinates.seed".to_owned(),
        blob::write(&Mesh {
            positions: vec![
                [f32::MIN_POSITIVE, -0.0, 1e30],
                [-1e30, f32::EPSILON, 0.0],
                [0.0, 1.0, -f32::MIN_POSITIVE],
            ],
            ..Mesh::default()
        }),
    ));
    seeds
}

/// Blobs that are refused, each for a different reason.
///
/// **Each is a real blob with one thing changed**, which is where a
/// mutator is useful: the branch is already reached and the search only
/// has to find the neighbours.
fn refused_seeds() -> Vec<(String, Vec<u8>)> {
    let full = blob::write(&furnished());
    let mut seeds = Vec::new();

    let mut edit = |name: &str, change: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = full.clone();
        change(&mut bytes);
        seeds.push((format!("{name}.seed"), bytes));
    };

    edit("wrong-magic", &|bytes| bytes[0] = b'X');
    edit("later-version", &|bytes| bytes[8] = 2);
    edit("unknown-flag", &|bytes| bytes[16] |= 0b0000_1000);
    edit("ragged-corners", &|bytes| bytes[12] = 7);
    edit("no-corners", &|bytes| bytes[12] = 0);
    edit("truncated", &|bytes| {
        let keep = bytes.len() - 4;
        bytes.truncate(keep);
    });
    edit("trailing-byte", &|bytes| bytes.push(0));
    edit("infinite-position", &|bytes| {
        bytes[20..24].copy_from_slice(&f32::INFINITY.to_le_bytes());
    });
    edit("nan-position", &|bytes| {
        bytes[20..24].copy_from_slice(&f32::NAN.to_le_bytes());
    });
    // Past the positions, the face normals and the corner normals: the
    // arrays a fuzzer reaches last and the suite reached not at all.
    edit("nan-texcoord", &|bytes| {
        let at = 20 + 72 + 24 + 72;
        bytes[at..at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    });
    edit("infinite-face-normal", &|bytes| {
        let at = 20 + 72;
        bytes[at..at + 4].copy_from_slice(&f32::NEG_INFINITY.to_le_bytes());
    });
    // A four-byte count that would reserve a quarter of a gigabyte from
    // a twenty-four-byte file: the amplification the ceiling exists for.
    edit("enormous-count", &|bytes| {
        bytes[12..16].copy_from_slice(&0xFFFF_FFF0_u32.to_le_bytes());
    });
    // A count that is plausible and wrong by exactly one triangle, which
    // is the shape a truncated transfer actually has.
    edit("count-three-too-many", &|bytes| bytes[12] = 9);

    // And the header alone, with no body at all.
    seeds.push(("header-only.seed".to_owned(), full[..20].to_vec()));
    // Shorter than a header can be.
    seeds.push(("too-short.seed".to_owned(), full[..8].to_vec()));

    seeds
}

fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/blob_read");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {error}", dir.display());
        return ExitCode::FAILURE;
    }
    let mut seeds = readable_seeds();
    seeds.extend(refused_seeds());

    let mut written = 0usize;
    for (name, bytes) in &seeds {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        if let Err(error) = std::fs::write(&path, bytes) {
            eprintln!("cannot write {name}: {error}");
            return ExitCode::FAILURE;
        }
        written += 1;
    }

    // Every seed this program meant to produce must be there afterwards,
    // whether this run wrote it or a previous one did.
    let missing: Vec<&str> = seeds
        .iter()
        .filter(|(name, _)| !dir.join(name).exists())
        .map(|(name, _)| name.as_str())
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
