//! Write the MTL reader's seed corpus.
//!
//! **Every input here is first-party**, so no licence question comes with
//! the corpus — the same rule the sample atlases and the other generated
//! corpora follow. A material library travels beside somebody's model,
//! which makes borrowing one exactly as much of a licence question as
//! borrowing the model.
//!
//! A fuzzer finds `newmtl` and a number quickly. What it does not find is
//! the *structure*: a property stated before the first material opened, a
//! map line whose options run to the end, a name declared twice so the
//! later definition wins, the two reciprocal spellings of opacity in one
//! file. Those are the branches worth starting a mutation from.
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-mesh --example make_mtl_corpus
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

/// Seeds that read, so the fuzzer has somewhere to mutate *from*.
///
/// A corpus of nothing but refusals teaches the search that everything
/// is refused, and the branches past the first refusal never run.
fn readable_seeds() -> Vec<(&'static str, &'static str)> {
    vec![
        ("one-material.seed", "newmtl steel\nKd 0.4 0.4 0.45\n"),
        (
            "every-factor.seed",
            "newmtl steel\nKa 0.1 0.1 0.1\nKd 0.4 0.4 0.45\nKs 0.9 0.9 0.9\n\
             Ke 0 0 0\nNs 250\nd 1\n",
        ),
        // A grey, which is the format's one-component spelling.
        ("grey-colour.seed", "newmtl paper\nKd 0.5\n"),
        // Both reciprocal spellings of opacity in one file.
        ("both-opacities.seed", "newmtl glass\nTr 0.75\nd 0.9\n"),
        // Every map slot, with options on one of them.
        (
            "every-map.seed",
            "newmtl wall\nmap_Ka ao.png\nmap_Kd -s 1 1 1 brick.png\nmap_Ks spec.png\n\
             map_Ns gloss.png\nmap_d cutout.png\nmap_bump height.png\nnorm normal.png\n",
        ),
        // A map line whose options run to the very end, so the last word
        // is not a file name at all.
        ("map-all-options.seed", "newmtl wall\nmap_Kd -bm 0.2\n"),
        // And one with nothing after the keyword, which names no file
        // and adds no map.
        ("bare-map.seed", "newmtl wall\nmap_Kd\n"),
        // A name declared twice, where the format says the later wins.
        (
            "redefined-name.seed",
            "newmtl steel\nKd 1 0 0\nnewmtl steel\nKd 0 1 0\n",
        ),
        // A `newmtl` with no name: legal, and nothing can refer to it.
        ("nameless-material.seed", "newmtl\nKd 0.5 0.5 0.5\n"),
        // Everything a real exporter puts in that this reader steps over.
        (
            "extended.seed",
            "# exported by something\nnewmtl modern\nillum 2\nPr 0.4\nPm 0\n\
             aniso 0.1\nKd 0.5 0.5 0.5\n",
        ),
        // Exponents and signs, which the number parser has to take.
        (
            "exotic-numbers.seed",
            "newmtl odd\nKd -1e-3 +0.5 2E2\nNs 1e3\n",
        ),
        // Blank lines and padding, reaching the arm with no keyword.
        ("padded.seed", "\n\n   \nnewmtl steel\n\nKd 0.5\n\n"),
        // Many materials, so the "belongs to the last one" walk runs.
        (
            "several-materials.seed",
            "newmtl a\nKd 1 0 0\nnewmtl b\nKd 0 1 0\nnewmtl c\nKd 0 0 1\n",
        ),
    ]
}

/// Seeds that are refused, each for a different reason.
///
/// **Each sits one character away from a file that reads**, which is
/// where a mutator is useful: the branch is already reached, and the
/// search only has to find the neighbours.
fn refused_seeds() -> Vec<(&'static str, &'static str)> {
    vec![
        // A property with no material to belong to.
        ("property-first.seed", "# a library\nKd 0.5 0.5 0.5\n"),
        // A library declaring nothing this reader can use.
        ("no-material.seed", "# nothing here\nillum 2\n"),
        // A value that is not a number, and one that is not finite.
        ("not-a-number.seed", "newmtl steel\nNs shiny\n"),
        ("infinite-colour.seed", "newmtl steel\nKd 1 1 inf\n"),
        ("nan-factor.seed", "newmtl steel\nd nan\n"),
        // Two components: neither a grey nor a colour.
        ("two-component-colour.seed", "newmtl steel\nKd 1 1\n"),
        // A factor keyword with nothing after it at all.
        ("bare-factor.seed", "newmtl steel\nNs\n"),
    ]
}

fn main() -> ExitCode {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/mtl_read");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {error}", dir.display());
        return ExitCode::FAILURE;
    }
    let mut seeds = readable_seeds();
    seeds.extend(refused_seeds());

    // A file that is not text at all, which is the one refusal above the
    // grammar and cannot be written as a `&str`.
    let mut not_text = b"newmtl steel\n".to_vec();
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
