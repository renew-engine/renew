//! Every text file in the tree is plain UTF-8, without a byte-order mark
//! and without the signatures of a botched encoding round-trip.
//!
//! This exists because it happened. A source file was read as Windows-1252
//! and written back as UTF-8, which turned every em dash into three
//! characters and prefixed the file with a BOM. Nothing noticed: the code
//! compiled, the tests passed, clippy was happy, and the damage reached a
//! merged commit. One of the mangled strings was a sample's own help text,
//! so the first person to see it would have been a user.
//!
//! Tooling on Windows makes this easy to do by accident — PowerShell's
//! `Get-Content`/`Set-Content` pair will do it without a word — so the
//! guard is worth more here than the check itself suggests.
//!
//! Helpers return `Result` rather than unwrapping: the lint that forbids
//! `expect` and `panic` outside tests reaches helpers in a test file too,
//! because the exemption follows `#[test]` rather than the file.

use std::path::{Path, PathBuf};

/// Extensions worth checking. Deliberately a list of text formats rather
/// than "everything that is not binary": a guess about which unknown file
/// is text is how a guard starts failing on a PNG.
const TEXT_EXTENSIONS: &[&str] = &["rs", "toml", "yml", "yaml", "md", "trace", "txt", "json"];

/// Directories never descended into. `target` is build output and
/// enormous; `.git` is not source.
const SKIPPED: &[&str] = &["target", ".git"];

/// U+FEFF at the start of a file. Legal UTF-8, and a nuisance in every
/// format here: it is why a YAML key stops matching and why a golden
/// comparison fails on one platform only.
const BOM: &str = "\u{feff}";

/// U+00E2 followed by U+20AC: the signature of UTF-8 bytes read as
/// Windows-1252 and written back out as UTF-8. It is the first two
/// characters of every mangled em dash, quote and ellipsis, and it occurs
/// in no real text.
///
/// Written as escapes, never as the characters themselves — including in
/// this comment. The first draft spelled the pair out here and the test
/// failed on its own source, which is funny once and an obstacle
/// thereafter.
const CP1252_DOUBLE_ENCODED: &str = "\u{e2}\u{20ac}";

/// The replacement character: something already gave up decoding this.
const REPLACEMENT: char = '\u{fffd}';

fn workspace_root() -> PathBuf {
    let guess = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    guess.canonicalize().unwrap_or(guess)
}

/// Every text file under `dir`, recursively.
fn text_files(dir: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|error| format!("{} unreadable: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{} unreadable: {error}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !SKIPPED.contains(&name.as_ref()) {
                text_files(&path, found)?;
            }
            continue;
        }
        let is_text = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext));
        if is_text {
            found.push(path);
        }
    }
    Ok(())
}

#[test]
fn no_text_file_carries_a_byte_order_mark_or_a_mangled_encoding() {
    let root = workspace_root();
    let mut files = Vec::new();
    text_files(&root, &mut files).expect("the workspace should be walkable");
    assert!(
        files.len() > 50,
        "found only {} text files — the walk is not reaching the tree, and this would pass \
         vacuously",
        files.len()
    );

    let mut faults: Vec<String> = Vec::new();
    for path in &files {
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let Ok(text) = std::fs::read_to_string(path) else {
            // Not valid UTF-8 at all. Reported rather than skipped: a
            // `.rs` file that is not UTF-8 will not compile, and a `.md`
            // one renders as garbage.
            faults.push(format!("{shown}: not valid UTF-8"));
            continue;
        };
        if text.starts_with(BOM) {
            faults.push(format!("{shown}: starts with a byte-order mark"));
        }
        if text.contains(CP1252_DOUBLE_ENCODED) {
            faults.push(format!(
                "{shown}: contains `\u{e2}\u{20ac}`, the signature of UTF-8 read as Windows-1252 \
                 and written back as UTF-8"
            ));
        }
        if text.contains(REPLACEMENT) {
            faults.push(format!(
                "{shown}: contains U+FFFD, so something already lost characters"
            ));
        }
    }

    assert!(
        faults.is_empty(),
        "{} file(s) have encoding damage:\n  {}\n\nOn Windows this is usually a \
         `Get-Content`/`Set-Content` round-trip. Rewrite the file as UTF-8 without a BOM.",
        faults.len(),
        faults.join("\n  ")
    );
}
