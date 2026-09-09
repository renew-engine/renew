//! Write the OBJ reader's seed corpus.
//!
//! **Every input here is first-party**, so no licence question comes with
//! the corpus — the same rule the sample atlases and the other generated
//! corpora follow. It matters more for this format than for the others:
//! an OBJ is what a person exports out of a modelling tool, so the
//! obvious way to get one is to take somebody's model, and that is
//! exactly what this repository does not do.
//!
//! A fuzzer discovers `v` and `f` quickly. What it does not discover is a
//! *face whose indices are coherent against streams that are almost long
//! enough* — off by one at either end, a negative index reaching one past
//! the beginning, a corner naming a texture coordinate the file never
//! declared. Each is one character from a file that reads, and each is a
//! different branch of the resolver. These seeds put a mutation's
//! starting point there.
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-mesh --example make_obj_corpus
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

/// The three positions of a unit triangle, as OBJ spells them.
const POSITIONS: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\n";
/// Three texture coordinates and one normal to go with them.
const ATTRIBUTES: &str = "vt 0 0\nvt 1 0\nvt 0 1\nvn 0 0 1\n";

/// Seeds that read, so the fuzzer has somewhere to mutate *from*.
///
/// A corpus of nothing but refusals teaches the search that everything
/// is refused, and the branches past the first refusal never run.
fn readable_seeds() -> Vec<(&'static str, String)> {
    let mut seeds = Vec::new();

    seeds.push(("bare-triangle.seed", format!("{POSITIONS}f 1 2 3\n")));
    seeds.push((
        "textured-triangle.seed",
        format!("{POSITIONS}{ATTRIBUTES}f 1/1/1 2/2/1 3/3/1\n"),
    ));
    seeds.push((
        "normals-without-coordinates.seed",
        format!("{POSITIONS}{ATTRIBUTES}f 1//1 2//1 3//1\n"),
    ));
    seeds.push((
        "coordinates-without-normals.seed",
        format!("{POSITIONS}{ATTRIBUTES}f 1/1 2/2 3/3\n"),
    ));
    // The relative spelling, which is a whole branch of the resolver.
    seeds.push(("relative-indices.seed", format!("{POSITIONS}f -3 -2 -1\n")));
    // A quad, so the fan runs more than once.
    seeds.push((
        "quad-fan.seed",
        String::from("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n"),
    ));
    // A polygon large enough that the fan is the loop it looks like.
    //
    // Built by pushing pieces rather than by appending a `format!`, and
    // not by collecting `format!` either: both are refused here, one for
    // the allocation per append and one for the allocation per item.
    // Neither lint has anything to say about pushing a number's own text.
    let mut wheel = String::new();
    for step in 0..16 {
        let angle = f64::from(step) * core::f64::consts::TAU / 16.0;
        wheel.push_str("v ");
        wheel.push_str(&angle.cos().to_string());
        wheel.push(' ');
        wheel.push_str(&angle.sin().to_string());
        wheel.push_str(" 0\n");
    }
    wheel.push('f');
    for corner in 1..=16 {
        wheel.push(' ');
        wheel.push_str(&corner.to_string());
    }
    wheel.push('\n');
    seeds.push(("sixteen-sided-fan.seed", wheel));
    // Everything a real export puts around the geometry, none of which
    // this reader implements and all of which it must step over.
    seeds.push((
        "furnished-export.seed",
        format!(
            "# exported by something\nmtllib scene.mtl\no cube\ng default\ns off\n\
             usemtl steel\n{POSITIONS}cstype bezier\nf 1 2 3\n"
        ),
    ));
    // A one-component texture coordinate, which the format allows and
    // which means a position along a one-dimensional texture.
    seeds.push((
        "one-component-texcoord.seed",
        format!("{POSITIONS}vt 0\nvt 0.5\nvt 1\nf 1/1 2/2 3/3\n"),
    ));
    // A face whose corners disagree about their TEXTURE coordinates
    // rather than their normals, which is the other half of that check.
    seeds.push((
        "half-textured-face.seed",
        format!("{POSITIONS}vt 0 0\nf 1/1 2/1 3\n"),
    ));
    // Exponents, signs and a trailing fourth component on `v`, which the
    // format allows and this reader steps over.
    seeds.push((
        "exotic-numbers.seed",
        String::from("v -1e-3 +0.5 2E2 1.0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n"),
    ));
    // Blank lines and padding, which exporters emit and which reach the
    // one arm of the keyword match that has no keyword.
    seeds.push((
        "padded.seed",
        format!("\n\n{POSITIONS}\n   \n\nf 1 2 3\n\n"),
    ));

    seeds
}

/// Seeds that are refused, each for a different reason.
///
/// **Each sits one character away from a file that reads**, which is
/// where a mutator is useful: the branch is already reached, and the
/// search only has to find the neighbours.
fn refused_seeds() -> Vec<(&'static str, String)> {
    vec![
        // Zero, in a format that numbers from one.
        ("index-zero.seed", format!("{POSITIONS}f 0 1 2\n")),
        // One past the end, and one past the beginning.
        ("index-past-end.seed", format!("{POSITIONS}f 1 2 4\n")),
        (
            "relative-past-beginning.seed",
            format!("{POSITIONS}f -4 -2 -1\n"),
        ),
        // A corner naming a stream the file never declared.
        (
            "coordinate-never-declared.seed",
            format!("{POSITIONS}f 1/1 2/2 3/3\n"),
        ),
        // A face whose corners disagree about their own shape.
        (
            "half-normalled-face.seed",
            format!("{POSITIONS}vn 0 0 1\nf 1//1 2//1 3\n"),
        ),
        // Two corners is a line.
        ("two-corners.seed", format!("{POSITIONS}f 1 2\n")),
        // A position short of a component, and one that is not a number.
        ("short-position.seed", String::from("v 0 0\nf 1 1 1\n")),
        (
            "position-not-a-number.seed",
            format!("v 0 0 zero\n{POSITIONS}f 1 2 3\n"),
        ),
        // A coordinate that parses and is not a number anything can
        // bound.
        (
            "infinite-position.seed",
            format!("v 0 0 inf\n{POSITIONS}f 1 2 3\n"),
        ),
        // Vertices and no face: a point cloud, which is a legal file and
        // not a surface.
        ("no-face.seed", String::from(POSITIONS)),
        // An index that is not a number at all, and an empty one.
        ("index-not-a-number.seed", format!("{POSITIONS}f a b c\n")),
        ("index-empty.seed", format!("{POSITIONS}f / / /\n")),
    ]
}

fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/obj_read");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {error}", dir.display());
        return ExitCode::FAILURE;
    }
    let mut seeds = readable_seeds();
    seeds.extend(refused_seeds());

    // A file that is not text at all, which is the one refusal above the
    // grammar and cannot be written as a `String`.
    let mut not_text = POSITIONS.as_bytes().to_vec();
    not_text.extend_from_slice(&[0xFF, 0xFE]);

    let mut written = 0usize;
    let mut write = |name: &str, bytes: &[u8]| -> Result<(), String> {
        let path = dir.join(name);
        if path.exists() {
            return Ok(());
        }
        std::fs::write(&path, bytes).map_err(|error| format!("cannot write {name}: {error}"))?;
        written += 1;
        Ok(())
    };

    for (name, text) in &seeds {
        if let Err(reason) = write(name, text.as_bytes()) {
            eprintln!("{reason}");
            return ExitCode::FAILURE;
        }
    }
    if let Err(reason) = write("not-text.seed", &not_text) {
        eprintln!("{reason}");
        return ExitCode::FAILURE;
    }

    // Every seed this program meant to produce must be there afterwards,
    // whether this run wrote it or a previous one did.
    let mut names: Vec<&str> = seeds.iter().map(|(name, _)| *name).collect();
    names.push("not-text.seed");
    let missing: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| !dir.join(name).exists())
        .collect();
    if !missing.is_empty() {
        eprintln!("these seeds are missing after the run: {missing:?}");
        return ExitCode::FAILURE;
    }

    println!(
        "{} seeds, {written} written this run, in {}",
        names.len(),
        dir.display()
    );
    ExitCode::SUCCESS
}
