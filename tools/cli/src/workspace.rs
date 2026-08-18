//! Workspace-root discovery and target classification.

use crate::json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// What kind of tree a workspace-anchored subcommand is standing in.
///
/// `EngineWorkspace` is this repository, named by the explicit
/// `[workspace.metadata.renew]` marker in its root manifest — a marker
/// rather than a heuristic, so a reorganized tree cannot be misread.
/// `Project` is any other workspace in which at least one member depends
/// on a `renew-` crate: a game. A workspace that is neither is refused by
/// the callers, never guessed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    EngineWorkspace,
    Project,
}

impl TargetKind {
    /// The name the envelope carries.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::EngineWorkspace => "engine-workspace",
            Self::Project => "project",
        }
    }
}

/// The manifest's structure with comments and string contents removed,
/// by a character-level scan of TOML's lexical constructs: `#` comments
/// (only outside strings), `"…"` basic strings (backslash escapes),
/// `'…'` literal strings, and the `"""`/`'''` multi-line forms (closers
/// tolerate up to two adjacent extra quotes, as TOML does). String
/// contents are dropped — the quotes remain, so a string-valued key
/// never reads as a bare one — and newlines inside multi-line strings
/// are dropped with them, so a string collapses onto the line that
/// opened it.
///
/// Why a character scan rather than a per-line delimiter count: a count
/// taken before comments are stripped is inverted by an ordinary `#`
/// comment carrying an odd number of triple quotes, in both directions
/// — it can hide a real marker below it and it can open a phantom
/// string whose "close" admits a marker that was only ever quoted.
///
/// A raw newline inside a single-line string is illegal TOML; the scan
/// ends the string there, so a malformed manifest cannot swallow the
/// rest of the file into one phantom string.
fn structural_text(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut kept = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            quote @ (b'"' | b'\'') => {
                kept.push(quote);
                let delimiter = [quote; 3];
                if bytes[i..].starts_with(&delimiter) {
                    i += 3;
                    while i < bytes.len() && !bytes[i..].starts_with(&delimiter) {
                        // Only the basic form has escapes; a backslash
                        // in a literal string is a backslash.
                        i += if quote == b'"' && bytes[i] == b'\\' {
                            2
                        } else {
                            1
                        };
                    }
                    i += 3;
                    let mut extra = 0;
                    while extra < 2 && i < bytes.len() && bytes[i] == quote {
                        i += 1;
                        extra += 1;
                    }
                } else {
                    i += 1;
                    while i < bytes.len() && bytes[i] != quote && bytes[i] != b'\n' {
                        // An escape never carries the scan past a
                        // newline: a raw newline inside a single-line
                        // string is illegal TOML, and letting the skip
                        // step over one would let a malformed manifest
                        // swallow the line below — and the next, for
                        // every line that happens to end in a backslash.
                        i += if quote == b'"'
                            && bytes[i] == b'\\'
                            && bytes.get(i + 1).is_some_and(|next| *next != b'\n')
                        {
                            2
                        } else {
                            1
                        };
                    }
                    i += usize::from(i < bytes.len() && bytes[i] == quote);
                }
                kept.push(quote);
            }
            byte => {
                kept.push(byte);
                i += 1;
            }
        }
    }
    // Only whole ASCII delimiters were skipped or inserted around the
    // input's own bytes, so `kept` is valid UTF-8; lossy is a no-panic
    // formality, never a conversion.
    String::from_utf8_lossy(&kept).into_owned()
}

/// Whether root-manifest text carries the engine marker:
/// a `[workspace.metadata.renew]` table with `engine = true`.
///
/// Matched by line shape over [`structural_text`], which has already
/// removed comments and string contents — so a `#` comment cannot open
/// or close a phantom string, and a marker quoted inside a string never
/// stands in for the marker. The key is required *after* the table
/// header, a new header ends the table, whitespace and tabs are
/// tolerated exactly where TOML tolerates them, and the lines of a
/// multi-line array value are tracked by bracket depth so a
/// continuation row — even one that closes the array — never reads as
/// a header. This is still not a TOML parser: only the bracketed-table
/// spelling counts (no dotted or inline-table forms) — the registry
/// documents the spelling.
#[must_use]
pub fn manifest_declares_engine(text: &str) -> bool {
    let mut in_marker_table = false;
    let mut array_depth: usize = 0;
    for raw in structural_text(text.trim_start_matches('\u{feff}')).lines() {
        let line = raw.trim();
        if array_depth > 0 {
            // Inside a multi-line array value every line is the value's
            // continuation until the brackets balance — never a header,
            // never a key, whatever shape the row takes.
            array_depth = array_depth
                .saturating_add(line.matches('[').count())
                .saturating_sub(line.matches(']').count());
            continue;
        }
        if let Some(header) = table_header(line) {
            in_marker_table = header == "[workspace.metadata.renew]";
            continue;
        }
        if in_marker_table
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "engine"
            && value.trim() == "true"
        {
            return true;
        }
        if line.starts_with('[') && line.ends_with(']') {
            // A closed bracketed line that is not a bare-key header: a
            // foreign header shape (a quoted key, say). It still ends
            // the table — the keys after it belong to it, whatever it
            // is called.
            in_marker_table = false;
            continue;
        }
        // A key line that opens more brackets than it closes starts a
        // multi-line array value.
        let opens = line.matches('[').count();
        let closes = line.matches(']').count();
        if opens > closes {
            array_depth = opens - closes;
        }
    }
    false
}

/// A structural line's table-header spelling, with the whitespace TOML
/// allows *around* the segments removed — `[ workspace . lints ]` and
/// `[workspace.lints]` name the same table, and cargo accepts both —
/// but not whitespace inside a segment: `[work space]` is not a table
/// name TOML accepts, and matching it would recognize a manifest cargo
/// refuses. `None` for lines that are not headers: a header is exactly
/// bare dotted keys inside one bracket pair, so a nested-array
/// continuation line like `[1, 2],` is a value, not a header. Quoted
/// keys (`["workspace"]`) and array-of-tables headers (`[[bench]]`) are
/// legal TOML this deliberately does not name: the scan keeps only
/// bare-key table spellings, and the registry says so. Both still *end*
/// a table — the caller treats any other closed bracketed line that
/// way — so an unnamed header never leaves a foreign key reading as
/// though it belonged to the marker table.
fn table_header(line: &str) -> Option<String> {
    let inner = line
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))?;
    let mut segments = Vec::new();
    for segment in inner.split('.') {
        let bare = segment.trim();
        if bare.is_empty()
            || !bare
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return None;
        }
        segments.push(bare);
    }
    Some(format!("[{}]", segments.join(".")))
}

/// Whether `cargo metadata` output names a dependency on any `renew-`
/// crate in any workspace member. This is what makes a foreign workspace
/// a *project* rather than a stranger.
#[must_use]
pub fn metadata_names_renew_dependency(metadata: &Value) -> bool {
    let Some(packages) = metadata.get("packages").and_then(Value::as_array) else {
        return false;
    };
    packages.iter().any(|package| {
        package
            .get("dependencies")
            .and_then(Value::as_array)
            .is_some_and(|dependencies| {
                dependencies.iter().any(|dependency| {
                    dependency
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.starts_with("renew-"))
                })
            })
    })
}

/// Whether manifest text declares a workspace root: a `[workspace]`
/// table or any of its subtables (`[workspace.…]`). Exactly those — a
/// table whose name merely *begins* with the word (`[workspacex]`, a
/// `workspace-hack`) is somebody else's table, and matching it would
/// anchor the walk at a member. Member manifests opt into workspace
/// lints with a plain `workspace = true` key, which this deliberately
/// does not match. A leading UTF-8 BOM is ignored — Cargo accepts BOM'd
/// manifests, so this must too. Comments and string contents are
/// removed first, so a header quoted inside a string is never read as
/// one.
#[must_use]
pub fn manifest_declares_workspace(text: &str) -> bool {
    structural_text(text.trim_start_matches('\u{feff}'))
        .lines()
        .any(|line| {
            table_header(line.trim())
                .is_some_and(|header| header == "[workspace]" || header.starts_with("[workspace."))
        })
}

/// Whether manifest text declares a package (a `[package]` table).
/// The same structural scan as [`manifest_declares_workspace`], for the
/// same reason.
#[must_use]
pub fn manifest_declares_package(text: &str) -> bool {
    structural_text(text.trim_start_matches('\u{feff}'))
        .lines()
        .any(|line| table_header(line.trim()).is_some_and(|header| header == "[package]"))
}

/// Find the enclosing workspace root, walking up from `start`.
///
/// A manifest with a `[workspace…]` table wins, nearest first — that is
/// how a member directory resolves to its workspace. When the walk
/// reaches the top without finding one, the nearest `[package]`
/// manifest is the root: cargo treats a standalone package as a
/// workspace of one, and the default `cargo new` game is exactly that
/// shape. A package nested under an *unrelated* workspace manifest
/// still resolves to that ancestor, unconditionally — including where
/// the ancestor `exclude`s the package and cargo's own resolution would
/// therefore treat it as standalone. There is no override today; the
/// registry documents the divergence.
#[must_use]
pub fn find_root(start: &Path) -> Anchor {
    let mut nearest_package: Option<PathBuf> = None;
    let mut current = Some(start);
    while let Some(directory) = current {
        let manifest = directory.join("Cargo.toml");
        match fs::read_to_string(&manifest) {
            Ok(text) => {
                if manifest_declares_workspace(&text) {
                    return Anchor::Root(directory.to_path_buf());
                }
                if manifest_declares_package(&text) {
                    if nearest_package.is_none() {
                        nearest_package = Some(directory.to_path_buf());
                    }
                } else {
                    // A manifest is *here* and this scan cannot name it —
                    // a syntax error, or a spelling the scan does not
                    // read (see the registry). Stepping over it would let
                    // the walk anchor somewhere above and report a
                    // verdict about a tree the caller never asked about;
                    // with the package fallback below, that verdict could
                    // be a green over code the caller's own tree never
                    // compiled. "Could not tell" is not "not here".
                    return Anchor::Unreadable(manifest);
                }
            }
            // Only "there is no file here" lets the walk continue. Every
            // other way a read can fail — bytes that are not UTF-8, a
            // directory wearing the name, a permission or lock error —
            // means a manifest *is* here and this process could not see
            // it, which is the same claim as one it could not name and
            // must not become "keep looking upward".
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Anchor::Unreadable(manifest),
        }
        current = directory.parent();
    }
    nearest_package.map_or(Anchor::None, Anchor::Root)
}

/// What the upward walk found: a tree to work in, a manifest it could
/// not read, or nothing at all.
///
/// Three outcomes rather than two, because the middle one is a
/// different claim from the last: a `Cargo.toml` this scan cannot name
/// is a question it failed to answer, and answering it anyway — by
/// walking past and anchoring somewhere else — is how a tool reports
/// on a tree nobody asked about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// The workspace root to run children from.
    Root(PathBuf),
    /// A manifest sits here and could not be named; the path is its.
    Unreadable(PathBuf),
    /// No `Cargo.toml` of any shape above the starting directory.
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_tables_are_recognized() {
        assert!(manifest_declares_workspace("[workspace]\nmembers = []\n"));
        assert!(manifest_declares_workspace(
            "[workspace.package]\nversion = \"1\"\n"
        ));
        assert!(manifest_declares_workspace("  [workspace.lints.rust]\n"));
    }

    #[test]
    fn utf8_bom_does_not_defeat_detection() {
        assert!(manifest_declares_workspace(
            "\u{feff}[workspace]\nmembers = []\n"
        ));
    }

    #[test]
    fn member_manifests_are_not_mistaken_for_roots() {
        let member = "[package]\nname = \"x\"\n\n[lints]\nworkspace = true\n";
        assert!(!manifest_declares_workspace(member));
        assert!(!manifest_declares_workspace(""));
    }

    #[test]
    fn find_root_walks_up_to_the_workspace_manifest() {
        let base = std::env::temp_dir().join(format!("renew-cli-ws-test-{}", std::process::id()));
        let nested = base.join("member").join("src");
        fs::create_dir_all(&nested).expect("create nested dirs");
        fs::write(base.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write root manifest");
        fs::write(
            base.join("member").join("Cargo.toml"),
            "[package]\nname = \"member\"\n",
        )
        .expect("write member manifest");

        let found = find_root(&nested);
        assert_eq!(found, Anchor::Root(base.clone()));

        // Best-effort cleanup: a transient file lock must not fail the test.
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_engine_marker_is_the_table_plus_the_key_not_either_alone() {
        assert!(manifest_declares_engine(
            "[workspace]\nmembers = []\n\n[workspace.metadata.renew]\nengine = true\n"
        ));
        assert!(manifest_declares_engine(
            "\u{feff}[workspace.metadata.renew]\n  engine = true\n"
        ));
        // The key outside the table is not the marker.
        assert!(!manifest_declares_engine("[workspace]\nengine = true\n"));
        // The table without the key is not the marker.
        assert!(!manifest_declares_engine("[workspace.metadata.renew]\n"));
        // The key in a table that merely *follows* the marker table is not
        // the marker either: a new header ends the previous table.
        assert!(!manifest_declares_engine(
            "[workspace.metadata.renew]\n[workspace.metadata.other]\nengine = true\n"
        ));
        assert!(!manifest_declares_engine(""));
    }

    #[test]
    fn the_marker_survives_what_toml_allows_around_it() {
        // Comments and tabs are legal TOML on both lines; a one-character
        // comment edit must not silently turn the engine into somebody's
        // game.
        assert!(manifest_declares_engine(
            "[workspace.metadata.renew] # the marker\nengine = true # yes\n"
        ));
        assert!(manifest_declares_engine(
            "[workspace.metadata.renew]\n\tengine\t=\ttrue\n"
        ));
        assert!(manifest_declares_engine(
            "  [workspace.metadata.renew]  \n  engine = true  \n"
        ));
        // A commented-out marker is not a marker.
        assert!(!manifest_declares_engine(
            "# [workspace.metadata.renew]\n# engine = true\n"
        ));
        // The documented limitation, pinned: only the bracketed-table
        // spelling counts.
        assert!(!manifest_declares_engine(
            "[workspace.metadata]\nrenew = { engine = true }\n"
        ));
        assert!(!manifest_declares_engine(
            "[workspace]\nmetadata.renew.engine = true\n"
        ));
    }

    #[test]
    fn a_marker_quoted_inside_a_string_is_not_the_marker() {
        // A stranger's manifest can legally carry the marker's text inside
        // a multi-line string; classifying that tree as the engine is the
        // vacuous-verdict failure the classification exists to prevent.
        assert!(!manifest_declares_engine(concat!(
            "[workspace]\nmembers = []\n\n[workspace.metadata.notes]\n",
            "text = \"\"\"\n[workspace.metadata.renew]\nengine = true\n\"\"\"\n"
        )));
        assert!(!manifest_declares_engine(concat!(
            "[workspace.metadata.notes]\n",
            "text = '''\n[workspace.metadata.renew]\nengine = true\n'''\n"
        )));
        // A string that opens and closes on one line does not swallow the
        // real marker after it.
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.notes]\ntext = \"\"\"quoted\"\"\"\n\n",
            "[workspace.metadata.renew]\nengine = true\n"
        )));
    }

    #[test]
    fn only_the_exact_true_value_is_the_marker() {
        // An explicit opt-out, junk, or the string spelling of true must
        // never classify a tree as the engine.
        assert!(!manifest_declares_engine(
            "[workspace.metadata.renew]\nengine = false\n"
        ));
        assert!(!manifest_declares_engine(
            "[workspace.metadata.renew]\nengine = \"true\"\n"
        ));
        assert!(!manifest_declares_engine(
            "[workspace.metadata.renew]\nengine = truely\n"
        ));
        // Nor a different key that merely begins with the marker's name:
        // a foreign workspace using this metadata namespace must not be
        // handed the engine's own verdicts.
        assert!(!manifest_declares_engine(
            "[workspace.metadata.renew]\nengineering = true\n"
        ));
        // The table name is matched whole, both ways. A neighbouring
        // third-party namespace and a subtable of the marker's own are
        // each a different table, and the sibling predicate below
        // deliberately *does* match by prefix — so nothing but a test
        // stops that idiom migrating here.
        assert!(!manifest_declares_engine(
            "[workspace.metadata.renewal]\nengine = true\n"
        ));
        assert!(!manifest_declares_engine(
            "[workspace.metadata.renew.extra]\nengine = true\n"
        ));
    }

    #[test]
    fn this_repositorys_own_root_manifest_carries_the_marker() {
        // Compile-time include of the real file: formatter or refactor
        // drift in the root manifest fails here, by name, rather than
        // only in a distant integration suite.
        let root_manifest = include_str!("../../../Cargo.toml");
        assert!(
            manifest_declares_engine(root_manifest),
            "the engine's own root manifest must classify as the engine"
        );
    }

    #[test]
    fn a_renew_dependency_anywhere_in_the_workspace_marks_a_project() {
        let with = crate::json::parse(
            r#"{"packages":[
                {"name":"a","dependencies":[{"name":"serde"}]},
                {"name":"b","dependencies":[{"name":"renew-fixed"}]}
            ]}"#,
        )
        .expect("valid metadata");
        assert!(metadata_names_renew_dependency(&with));

        let without = crate::json::parse(
            r#"{"packages":[{"name":"a","dependencies":[{"name":"serde"},{"name":"renewal-notquite"}]}]}"#,
        )
        .expect("valid metadata");
        assert!(
            !metadata_names_renew_dependency(&without),
            "the prefix is renew- with the hyphen, not any word starting renew"
        );

        let malformed = crate::json::parse(r#"{"answer":42}"#).expect("valid json");
        assert!(!metadata_names_renew_dependency(&malformed));
    }

    #[test]
    fn a_walk_that_reaches_the_top_without_a_workspace_stops_at_a_package() {
        // The empty path is the one start whose ancestry is bounded on
        // every platform: `Path::new("").parent()` is `None`, so the walk
        // probes exactly one manifest — this crate's own, a `[package]`
        // manifest with no `[workspace]` table. With no workspace found
        // above it, that package is its own root: the standalone
        // `cargo new` shape, which cargo treats as a workspace of one.
        assert_eq!(find_root(Path::new("")), Anchor::Root(PathBuf::from("")));
    }

    /// A manifest that is *there* and cannot be **read at all** stops
    /// the walk too — not only one that was read and could not be
    /// named.
    ///
    /// The two are one claim ("a manifest is here and this process
    /// cannot see it") and only one of them was closed at first: bytes
    /// that are not UTF-8, a directory wearing the name, a permission
    /// or lock error all fell out of the read and let the walk carry on
    /// upward. The sibling test's *name* is what hid that, by promising
    /// "unreadable" while pinning "unnameable".
    #[test]
    fn a_manifest_that_cannot_be_read_at_all_stops_the_walk() {
        let base = std::env::temp_dir().join(format!("renew-cli-ws-bytes-{}", std::process::id()));
        let inner = base.join("inner");
        fs::create_dir_all(&inner).expect("scratch");
        fs::write(base.join("Cargo.toml"), "[package]\nname = \"outer\"\n").expect("outer");

        // Bytes no UTF-8 reader can accept. `read_to_string` fails with
        // InvalidData rather than NotFound, and the walk must treat that
        // as a manifest it could not see rather than as no manifest.
        fs::write(inner.join("Cargo.toml"), [0x5b, 0x70, 0xff, 0xfe, 0x5d]).expect("inner");
        assert_eq!(
            find_root(&inner),
            Anchor::Unreadable(inner.join("Cargo.toml")),
            "bytes that are not UTF-8 are a manifest this process cannot see, not an absent one"
        );

        // A directory wearing the manifest's name: the read fails for a
        // different reason and the answer is the same.
        fs::remove_file(inner.join("Cargo.toml")).expect("clear");
        fs::create_dir(inner.join("Cargo.toml")).expect("directory in its place");
        assert_eq!(
            find_root(&inner),
            Anchor::Unreadable(inner.join("Cargo.toml"))
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// A manifest that is *there* and cannot be named stops the walk.
    ///
    /// Stepping over it would anchor somewhere above and report on a
    /// tree the caller never asked about — and with the standalone
    /// package fallback below, that report could be a green over code
    /// the caller's own tree never compiled.
    #[test]
    fn a_manifest_this_scan_cannot_name_stops_the_walk() {
        let base = std::env::temp_dir().join(format!("renew-cli-ws-unread-{}", std::process::id()));
        let inner = base.join("inner");
        fs::create_dir_all(&inner).expect("scratch");
        // An anchorable ancestor, so a walk that stepped over the broken
        // manifest would find something and call it the answer.
        fs::write(base.join("Cargo.toml"), "[package]\nname = \"outer\"\n").expect("outer");
        // The commonest possible typo: an unclosed table header. Cargo
        // refuses this file outright.
        fs::write(inner.join("Cargo.toml"), "[package\nname = \"inner\"\n").expect("inner");

        assert_eq!(
            find_root(&inner),
            Anchor::Unreadable(inner.join("Cargo.toml")),
            "a manifest that is here and unreadable is not the same as no manifest"
        );
        // The ancestor is still perfectly anchorable from its own side,
        // so the refusal is about the file in the way, not about the tree.
        assert_eq!(find_root(&base), Anchor::Root(base.clone()));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_walk_that_finds_no_manifest_at_all_yields_none() {
        let base =
            std::env::temp_dir().join(format!("renew-cli-ws-rootless-{}", std::process::id()));
        fs::create_dir_all(&base).expect("scratch");
        assert_eq!(find_root(&base), Anchor::None);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_standalone_package_manifest_is_its_own_root() {
        let base =
            std::env::temp_dir().join(format!("renew-cli-ws-standalone-{}", std::process::id()));
        let game = base.join("game");
        fs::create_dir_all(game.join("src")).expect("scratch");
        fs::write(
            game.join("Cargo.toml"),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
        )
        .expect("manifest");
        assert_eq!(find_root(&game.join("src")), Anchor::Root(game.clone()));
        // A workspace manifest above it still wins: that is how a member
        // resolves to its workspace rather than to itself.
        fs::write(
            base.join("Cargo.toml"),
            "[workspace]\nmembers = [\"game\"]\n",
        )
        .expect("root manifest");
        assert_eq!(find_root(&game.join("src")), Anchor::Root(base.clone()));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_comment_containing_string_delimiters_does_not_confuse_the_scan() {
        // The comment-inversion defect, pinned in both directions: a
        // legal `#` comment carrying an odd number of triple-quote
        // delimiters must neither hide the real marker below it…
        assert!(manifest_declares_engine(concat!(
            "# note: docstrings use \"\"\" in some languages\n",
            "[workspace.metadata.renew]\nengine = true\n"
        )));
        assert!(manifest_declares_engine(concat!(
            "# ''' one literal delimiter\n",
            "[workspace.metadata.renew]\nengine = true\n"
        )));
        // …nor open a phantom string whose "close" admits a marker that
        // is really quoted inside a stranger's string.
        assert!(!manifest_declares_engine(concat!(
            "[workspace]\nmembers = []\n# \"\"\"\n",
            "[workspace.metadata.notes]\n",
            "text = \"\"\"\n[workspace.metadata.renew]\nengine = true\n\"\"\"\n"
        )));
    }

    #[test]
    fn lookalike_tables_are_not_the_workspace() {
        // A table whose name merely begins with the word is somebody
        // else's table; matching it would anchor the walk at a member.
        assert!(!manifest_declares_workspace("[workspacex]\nkey = 1\n"));
        assert!(!manifest_declares_workspace("[workspaces]\n"));
        assert!(!manifest_declares_workspace("[workspace-hack]\n"));
        // Whitespace inside the brackets and around dots is TOML's to
        // give, and cargo accepts it.
        assert!(manifest_declares_workspace("[ workspace ]\nmembers = []\n"));
        assert!(manifest_declares_workspace(
            "[ workspace . lints . rust ]\n"
        ));
        assert!(manifest_declares_engine(
            "[ workspace . metadata . renew ]\nengine = true\n"
        ));
        // A quoted key is legal TOML this scan deliberately does not
        // recognize — it classifies as nothing, never as something else.
        assert!(!manifest_declares_workspace("[\"workspace\"]\n"));
        // Whitespace *inside* a segment is a spelling TOML rejects, and
        // recognizing it would classify a manifest cargo refuses.
        assert!(!manifest_declares_workspace("[work space]\nmembers = []\n"));
        assert!(!manifest_declares_engine(
            "[workspace.metadata.renew]\nen gine = true\n"
        ));
        // Neither is an empty segment, nor an invalid one in a later
        // position — a subtable spelling cargo refuses must not anchor
        // the walk at whatever manifest carries it.
        assert!(!manifest_declares_workspace("[workspace.]\n"));
        assert!(!manifest_declares_workspace("[.workspace]\n"));
        assert!(!manifest_declares_workspace("[workspace.foo bar]\n"));
        // The valid subtable spelling still counts, so the guard is
        // narrow rather than merely strict.
        assert!(manifest_declares_workspace("[workspace.lints.rust]\n"));
        // An array-of-tables header is not a table name this scan
        // reports, but it still ends the table it follows: a key under
        // `[[bench]]` must not read as though the marker table were
        // still open.
        assert!(!manifest_declares_workspace("[[workspace]]\n"));
        assert!(!manifest_declares_engine(
            "[workspace.metadata.renew]\n[[bench]]\nengine = true\n"
        ));
    }

    #[test]
    fn a_backslash_in_a_literal_string_is_a_backslash() {
        // TOML literal strings have no escapes: a Windows path ending
        // in a backslash right before the closer must not swallow the
        // closer — under an escape-honoring scan the string runs to the
        // end of the file and hides the marker below it.
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.notes]\n",
            "path = '''C:\\'''\n",
            "[workspace.metadata.renew]\nengine = true\n"
        )));
        // The single-line branch, observed on the scan directly: the
        // backslash does not consume the closing quote, so the space
        // before the (stripped) comment survives as structure.
        assert_eq!(
            structural_text("a = 'C:\\' # x\nb = true\n"),
            "a = '' \nb = true\n"
        );
    }

    #[test]
    fn an_unterminated_string_does_not_swallow_the_rest_of_the_file() {
        // Malformed TOML — cargo refuses it — but the scan must stay
        // line-local: the unterminated string ends at its newline
        // rather than running on to swallow the marker below.
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.notes]\n",
            "text = \"oops\n",
            "[workspace.metadata.renew]\nengine = true\n"
        )));
        // Including when the line ends in a backslash: the escape skip
        // must not step over the newline, or each such line swallows
        // the next and the marker disappears an arbitrary distance
        // below.
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.notes]\n",
            "text = \"x\\\n",
            "[workspace.metadata.renew]\nengine = true\n"
        )));
    }

    #[test]
    fn an_escaped_quote_does_not_close_a_multiline_string() {
        // An escaped quote immediately before what would otherwise be
        // the closing delimiter must not end the string early — early
        // closure turns the string's remaining contents into structure,
        // and a quoted marker inside would classify a stranger's tree.
        assert!(!manifest_declares_engine(concat!(
            "[workspace]\nmembers = []\n\n[workspace.metadata.notes]\n",
            "text = \"\"\"docs say \\\"\"\"\n",
            "[workspace.metadata.renew]\nengine = true\n",
            "\"\"\"\n"
        )));
        // The mirrored positive: an escaped quote inside a properly
        // closed string swallows nothing that follows it.
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.notes]\n",
            "text = \"\"\"docs say \\\" and close\"\"\"\n",
            "[workspace.metadata.renew]\nengine = true\n"
        )));
    }

    #[test]
    fn a_multiline_close_keeps_its_extra_quotes_out_of_the_structure() {
        // TOML lets a multi-line string end in up to two quotes of its
        // own. Observed on the scan directly, because the predicates
        // cannot see the difference: the leftover quote is consumed as
        // string content, never left behind to open a phantom string
        // over the rest of the line.
        assert_eq!(
            structural_text("a = \"\"\"ends in a quote\"\"\"\"\nb = true\n"),
            "a = \"\"\nb = true\n"
        );
        assert_eq!(
            structural_text("a = '''ends in a quote''''\nb = true\n"),
            "a = ''\nb = true\n"
        );
        // Both of TOML's allowed extra quotes, so the tolerance's full
        // width is what the test holds, not just its first step.
        assert_eq!(
            structural_text("a = \"\"\"ends in two\"\"\"\"\"\nb = true\n"),
            "a = \"\"\nb = true\n"
        );
        assert_eq!(
            structural_text("a = '''ends in two'''''\nb = true\n"),
            "a = ''\nb = true\n"
        );
        // And the bound holds from above: TOML allows two extra quotes,
        // not three, so a third is content the scan leaves standing
        // rather than swallowing.
        assert_eq!(
            structural_text("a = \"\"\"ends in three\"\"\"\"\"\"\nb = true\n"),
            "a = \"\"\"\"\nb = true\n"
        );
    }

    #[test]
    fn the_nearest_package_wins_when_no_workspace_encloses() {
        // Two nested standalone packages (an examples/ or vendored
        // layout): the walk must anchor at the inner one — the tree the
        // caller is standing in — not the farthest ancestor.
        let base = std::env::temp_dir().join(format!("renew-cli-ws-nested-{}", std::process::id()));
        let inner = base.join("outer").join("examples").join("game");
        fs::create_dir_all(inner.join("src")).expect("scratch");
        fs::write(
            base.join("outer").join("Cargo.toml"),
            "[package]\nname = \"outer\"\n",
        )
        .expect("outer manifest");
        fs::write(inner.join("Cargo.toml"), "[package]\nname = \"game\"\n")
            .expect("inner manifest");
        assert_eq!(find_root(&inner.join("src")), Anchor::Root(inner.clone()));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_nested_array_inside_the_marker_table_does_not_hide_the_key() {
        // A multi-line nested array is legal TOML; its continuation
        // lines start with `[` but are values, and must not read as
        // headers that end the marker table.
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.renew]\n",
            "grid = [\n    [1, 2],\n    [3, 4],\n]\n",
            "engine = true\n"
        )));
        // The closing row can carry the array's own close bracket; that
        // shape is a value too, not a header that ends the table.
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.renew]\n",
            "grid = [\n    [1, 2],\n    [3, 4]]\n",
            "engine = true\n"
        )));
        // The key line can open more than one bracket, and a row can
        // both open and close: the depth is arithmetic over the whole
        // line, not a flag set by its first character. Counting either
        // wrong leaves the tracker stuck and swallows the marker.
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.renew]\n",
            "grid = [[1, 2],\n    [3, 4]]\n",
            "engine = true\n"
        )));
        // And an array whose rows are balanced among themselves still
        // ends where its own bracket closes.
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.renew]\n",
            "grid = [\n    [1, [2, 3]],\n]\n",
            "engine = true\n"
        )));
        // A key line that opens two brackets and closes none, whose
        // array then closes across two rows: the depth has to be
        // arithmetic to survive this, since a row that closes more than
        // it opens would drop a flag to zero and let the next row read
        // as a header that ends the table.
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.renew]\n",
            "grid = [[\n    1, 2],\n    [3]]\n",
            "engine = true\n"
        )));
        // A quoted-key header, though unrecognized as a header this
        // scan can name, still ends the table: the key after it belongs
        // to that table, whatever it is called.
        assert!(!manifest_declares_engine(concat!(
            "[workspace.metadata.renew]\n",
            "[\"other\"]\n",
            "engine = true\n"
        )));
    }

    #[test]
    fn escapes_and_single_line_strings_do_not_leak_structure() {
        // An escaped quote does not end a basic string, and a `#` inside
        // one does not start a comment that swallows the line. Observed
        // on the scan directly: through a predicate the two behaviours
        // agree, because the `#` hides the difference either way.
        assert_eq!(
            structural_text("a = \"quote \\\" here\" b = 1\n"),
            "a = \"\" b = 1\n"
        );
        // An empty string is a string, not the opening of a multi-line
        // one — `description = ""` is ordinary, and reading it as an
        // opener swallows the rest of the file.
        assert_eq!(
            structural_text("a = \"\"\nb = true\n"),
            "a = \"\"\nb = true\n"
        );
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.notes]\ndescription = \"\"\n\n",
            "[workspace.metadata.renew]\nengine = true\n"
        )));
        assert!(manifest_declares_engine(concat!(
            "[workspace.metadata.notes]\n",
            "text = \"quote \\\" then # not a comment\"\n",
            "[workspace.metadata.renew]\nengine = true\n"
        )));
        // Marker text inside a one-line literal string is content, not
        // structure.
        assert!(!manifest_declares_engine(
            "text = '[workspace.metadata.renew]'\nengine = true\n"
        ));
        // A workspace header quoted inside a string is not a root
        // declaration.
        assert!(!manifest_declares_workspace(
            "[package]\nname = \"x\"\ndescription = \"\"\"\n[workspace]\n\"\"\"\n"
        ));
        assert!(manifest_declares_package("[package]\nname = \"x\"\n"));
        assert!(!manifest_declares_package(
            "# [package]\ntext = \"[package]\"\n"
        ));
        // The package predicate is held to the whole name too. Anchoring
        // the walk at a `[packagex]` manifest would name a directory
        // cargo refuses as a root, and the sibling predicate's prefix
        // idiom is one refactor away from arriving here.
        assert!(!manifest_declares_package("[packagex]\nname = \"x\"\n"));
        assert!(!manifest_declares_package("[packages]\n"));
        assert!(!manifest_declares_package("[package.metadata]\n"));
    }
}
