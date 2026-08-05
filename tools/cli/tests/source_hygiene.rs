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

/// The one-based line a byte offset falls on.
///
/// Clippy suggests the `bytecount` crate here. Declined: this runs once
/// per fault found, not once per byte scanned, and a fault list long
/// enough for the difference to matter is a tree in far worse trouble
/// than a slow test. A dependency is presumed rejected until it earns
/// itself, and counting newlines twice a year does not.
#[expect(
    clippy::naive_bytecount,
    reason = "runs once per fault, not per byte; a dependency would cost more than it saves"
)]
fn line_of(bytes: &[u8], offset: usize) -> usize {
    bytes[..offset]
        .iter()
        .filter(|byte| *byte == &b'\n')
        .count()
        + 1
}

/// A carriage return that no line feed follows, and a tab, in a tree that
/// uses neither.
///
/// **A second way to damage a file, with a different cause and the same
/// ending.** Where the checks above catch an encoding round-trip, this
/// catches a *collapsed escape*: a shell heredoc that eats one backslash
/// turns the two characters backslash-r into a carriage return, and
/// backslash-t into a tab, inside whatever literal was being written.
///
/// It happened here, twice in one session. A sample's README gained a
/// PowerShell block whose `$env:USERPROFILE\run.log` had become
/// `$env:USERPROFILE` + CR + `un.log`, and whose `.\target\debug\glide.exe`
/// had become `.` + TAB + `arget\debug\glide.exe`. It reached a merged
/// commit, because the file is prose: nothing compiled it, and both
/// characters are invisible in an editor and in rendered Markdown. The
/// first person to meet it would have been a Windows user copying the
/// block, which is the only reason that block exists. The second time,
/// the same shell mangled the source of this very test.
///
/// A *lone* carriage return is the signal rather than a carriage return as
/// such: this tree is checked out CRLF on Windows, so CR-LF pairs are
/// ordinary, and only an unpaired one means something went wrong.
#[test]
fn no_text_file_carries_a_stray_control_character() {
    const CARRIAGE_RETURN: u8 = b'\r';
    const LINE_FEED: u8 = b'\n';
    const TAB: u8 = b'\t';

    let root = workspace_root();
    let mut files = Vec::new();
    text_files(&root, &mut files).expect("the workspace should be walkable");
    assert!(
        files.len() > 50,
        "found only {} text files - the walk is not reaching the tree, and this would pass \
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
        let Ok(bytes) = std::fs::read(path) else {
            // Unreadable is the business of the check above, which
            // reports it; reporting it twice would say nothing new.
            continue;
        };

        for (offset, pair) in bytes.windows(2).enumerate() {
            if pair[0] == CARRIAGE_RETURN && pair[1] != LINE_FEED {
                faults.push(format!(
                    "{shown}:{}: a carriage return with no line feed after it",
                    line_of(&bytes, offset)
                ));
            }
        }
        // A file ending in a bare carriage return is the same fault, and
        // no two-byte window can see it.
        if bytes.last() == Some(&CARRIAGE_RETURN) {
            faults.push(format!("{shown}: ends with a bare carriage return"));
        }
        if let Some(offset) = bytes.iter().position(|b| *b == TAB) {
            faults.push(format!(
                "{shown}:{}: a tab, where this tree indents with spaces",
                line_of(&bytes, offset)
            ));
        }
    }

    assert!(
        faults.is_empty(),
        "{} stray control character(s):\n  {}\n\nUsually a shell heredoc that ate a backslash, \
         turning an escape into the character it names. Write the file with a file tool rather \
         than through a shell literal.",
        faults.len(),
        faults.join("\n  ")
    );
}
