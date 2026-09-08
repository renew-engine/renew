//! Write the STL reader's seed corpus.
//!
//! **Every input here is first-party.** A model downloaded from a sample
//! repository would be a dependency with a licence and a directory of
//! them would be a dependency nobody recorded — the same rule the sample
//! atlases and the other six generated corpora follow.
//!
//! A fuzzer finds its own way past an eighty-byte header eventually, but
//! it wastes most of a budget doing it, and it will essentially never
//! stumble on a length that satisfies `84 + 50n` for the `n` written at
//! offset eighty. These seeds put it on the far side of that: valid
//! files of both dialects to mutate, and one malformed file per refusal
//! so a mutation of it starts from somewhere the random walk rarely
//! reaches.
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-mesh --example make_corpus
//! ```
//!
//! Existing files are left alone. The fuzzer adds its own finds to this
//! directory over time, and this program must never delete them.
//!
//! It exits non-zero if any seed it meant to write is missing afterwards.
//! Every bail prints its reason, because a generator that prints a
//! failure and then reports success is worse than one that crashes: the
//! caller sees a zero and believes the corpus is whole.
//!
//! **The seeds are named `.seed` rather than `.stl`, deliberately.** Most
//! of them are not STL files: they are byte strings written to be wrong,
//! and several are wrong in exactly the ways this repository's sweep over
//! source text exists to catch — one is not valid UTF-8, one is a run of
//! zero bytes. Calling them `.stl` would put a directory of deliberate
//! damage in front of a guard whose whole job is to find accidental
//! damage. The extension was never durable in any case: `cargo fuzz cmin`
//! renames what it keeps to a bare content hash.

// The crate bans filesystem access because the library never touches a
// file -- a caller that reads one owns it, and owns the bound on reading
// it. This program is that caller: writing the corpus is its whole job,
// and the same allowance sits on the replay test beside it.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes, never a path.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;
use std::process::ExitCode;

/// One binary triangle: a normal and three corners, then the attribute
/// word nobody agrees about.
fn record(normal: [f32; 3], corners: [[f32; 3]; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(50);
    for value in normal.iter().chain(corners.iter().flatten()) {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A binary file with `header` as its eighty bytes and a count that
/// matches its body, unless `count` overrides it.
fn binary(header: &[u8], triangles: &[([f32; 3], [[f32; 3]; 3])], count: Option<u32>) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    let cut = header.len().min(80);
    out[..cut].copy_from_slice(&header[..cut]);
    let declared = count.unwrap_or_else(|| u32::try_from(triangles.len()).unwrap_or(u32::MAX));
    out.extend_from_slice(&declared.to_le_bytes());
    for (normal, corners) in triangles {
        out.extend_from_slice(&record(*normal, *corners));
    }
    out
}

const UNIT: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
const UP: [f32; 3] = [0.0, 0.0, 1.0];
const DOWN: [f32; 3] = [0.0, 0.0, -1.0];

/// Every seed, in the order they were reasoned about: the valid files
/// that give a fuzzer a shape to mutate, then one malformed file per
/// refusal, so a mutation starts from somewhere the random walk rarely
/// reaches.
///
/// Separate from `main` because it is data and `main` is a procedure,
/// and because a function that both builds two dozen files and writes
/// them is two jobs sharing a scope.
#[expect(
    clippy::vec_init_then_push,
    reason = "each entry carries the comment explaining why it is here, and several are computed from a local; a vec literal would be either unreadable or impossible"
)]
fn valid_seeds() -> Vec<(&'static str, Vec<u8>)> {
    let mut seeds: Vec<(&'static str, Vec<u8>)> = Vec::new();

    // --- Valid files, which give the fuzzer a shape to mutate. ------
    seeds.push((
        "binary-one.seed",
        binary(b"one triangle", &[(UP, UNIT)], None),
    ));
    seeds.push((
        "binary-many.seed",
        binary(b"a fan of eight", &[(UP, UNIT); 8], None),
    ));
    // **A binary file whose header begins with the word `solid`.** The
    // classic way to get this format wrong, and the seed that keeps a
    // mutation of the dispatch honest.
    seeds.push((
        "binary-header-says-solid.seed",
        binary(b"solid exported by something", &[(UP, UNIT)], None),
    ));
    // Normals that disagree with their winding: legal, and the most
    // common thing wrong with a real file.
    seeds.push((
        "binary-wound-against-normals.seed",
        binary(b"inverted", &[(DOWN, UNIT), (DOWN, UNIT)], None),
    ));
    // Coordinates spanning the exponent range, so a mutation of a float
    // starts somewhere other than near one.
    seeds.push((
        "binary-wide-range.seed",
        binary(
            b"tiny and huge",
            &[(
                UP,
                [
                    [1e-30, -1e-30, 0.0],
                    [3.4e38, 0.0, 0.0],
                    [0.0, -3.4e38, 1e-45],
                ],
            )],
            None,
        ),
    ));

    seeds.push((
        "text-one.seed",
        b"solid unit\n  facet normal 0 0 1\n    outer loop\n      vertex 0 0 0\n      \
          vertex 1 0 0\n      vertex 0 1 0\n    endloop\n  endfacet\nendsolid unit\n"
            .to_vec(),
    ));
    // Tabs and CRLF, which the format's writers disagree about.
    seeds.push((
        "text-crlf-tabs.seed",
        b"solid x\r\n\tfacet normal 0 0 1\r\n\t\touter loop\r\n\t\t\tvertex 0 0 0\r\n\
          \t\t\tvertex 1 0 0\r\n\t\t\tvertex 0 1 0\r\n\t\tendloop\r\n\tendfacet\r\nendsolid x\r\n"
            .to_vec(),
    ));
    // Exponents and signs, so the number grammar is seeded rather than
    // discovered.
    seeds.push((
        "text-exponents.seed",
        b"solid x\nfacet normal 0.0e0 -0.0 +1\nouter loop\nvertex -1.5e-3 0 0\n\
          vertex 1E+2 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid x\n"
            .to_vec(),
    ));

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
    let mut seeds: Vec<(&'static str, Vec<u8>)> = Vec::new();
    // Too short to be anything.
    seeds.push(("short-empty.seed", Vec::new()));
    seeds.push(("short-three-bytes.seed", vec![1, 2, 3]));
    // A run of zero bytes: valid UTF-8, opens like nothing, and short.
    seeds.push(("short-zeroes.seed", vec![0u8; 82]));
    // Not UTF-8 at all, so the text reader never sees it. **This seed
    // is the reason the dispatch is worth a seed**: it goes to the
    // binary reader, which is the right place for bytes that are not
    // text, and gets an answer about headers rather than about words.
    seeds.push(("not-utf8.seed", vec![0xFF, 0xFE, 0xFD, 0xFC]));
    // A header, a count, and nothing behind it: the hostile-count shape.
    seeds.push((
        "binary-count-with-no-body.seed",
        binary(b"lying", &[], Some(u32::MAX)),
    ));
    // A count of zero, which is a file declaring no geometry.
    seeds.push(("binary-no-triangles.seed", binary(b"empty", &[], None)));
    seeds.push((
        "text-no-facets.seed",
        b"solid empty\nendsolid empty\n".to_vec(),
    ));
    // A count that disagrees with the body by exactly one triangle.
    seeds.push((
        "binary-count-too-high.seed",
        binary(b"off by one", &[(UP, UNIT)], Some(2)),
    ));
    // A coordinate that is not a number a bounding box can hold, in
    // each of the two fields that can carry one.
    seeds.push((
        "binary-nan-position.seed",
        binary(
            b"nan",
            &[(UP, [[f32::NAN, 0.0, 0.0], UNIT[1], UNIT[2]])],
            None,
        ),
    ));
    seeds.push((
        "binary-infinite-normal.seed",
        binary(b"inf", &[([f32::INFINITY, 0.0, 0.0], UNIT)], None),
    ));
    // A word the grammar wanted, and a number that is not one.
    seeds.push((
        "text-wrong-keyword.seed",
        b"solid x\nfacet wombat 0 0 1\n".to_vec(),
    ));
    seeds.push((
        "text-not-a-number.seed",
        b"solid x\nfacet normal 0 0 1\nouter loop\nvertex 1.0.0 0 0\n".to_vec(),
    ));
    // A file that stops in the middle of a facet, and one that never
    // says `endsolid`.
    seeds.push((
        "text-truncated-loop.seed",
        b"solid x\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\n".to_vec(),
    ));
    seeds.push((
        "text-no-endsolid.seed",
        b"solid x\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\n\
          vertex 0 1 0\nendloop\nendfacet\n"
            .to_vec(),
    ));
    // A binary file cut one byte short: the commonest way a real STL
    // is wrong, and now the seed that reaches `CountMismatch`.
    let mut cut = binary(b"cut short", &[(UP, UNIT)], None);
    cut.truncate(cut.len() - 1);
    seeds.push(("binary-cut-short.seed", cut));
    // A text file with a run of control bytes where a keyword belongs,
    // which is where the escaping in a refusal message is exercised.
    let mut controls = b"solid x
"
    .to_vec();
    controls.extend(std::iter::repeat_n(0u8, 82));
    seeds.push(("text-control-bytes.seed", controls));
    // A word so long it exercises the bound on what a refusal quotes.
    let mut long_word = b"solid x\nfacet ".to_vec();
    long_word.extend(std::iter::repeat_n(b'q', 4096));
    seeds.push(("text-enormous-word.seed", long_word));

    seeds
}

fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/stl_read");
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
    let mut missing = Vec::new();
    for (name, _) in &seeds {
        if !dir.join(name).exists() {
            missing.push(*name);
        }
    }
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
