//! Workspace-root discovery.

use std::fs;
use std::path::{Path, PathBuf};

/// Whether manifest text declares a workspace root (any `[workspace…]`
/// table). Member manifests opt into workspace lints with a plain
/// `workspace = true` key, which this deliberately does not match.
/// A leading UTF-8 BOM is ignored — Cargo accepts BOM'd manifests, so this
/// must too.
#[must_use]
pub fn manifest_declares_workspace(text: &str) -> bool {
    text.trim_start_matches('\u{feff}')
        .lines()
        .any(|line| line.trim_start().starts_with("[workspace"))
}

/// Find the nearest enclosing workspace root, walking up from `start`.
#[must_use]
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(directory) = current {
        let manifest = directory.join("Cargo.toml");
        if let Ok(text) = fs::read_to_string(&manifest)
            && manifest_declares_workspace(&text)
        {
            return Some(directory.to_path_buf());
        }
        current = directory.parent();
    }
    None
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
        assert_eq!(found.as_deref(), Some(base.as_path()));

        // Best-effort cleanup: a transient file lock must not fail the test.
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_walk_that_reaches_the_top_without_a_workspace_yields_none() {
        // The empty path is the one start whose ancestry is bounded on
        // every platform: `Path::new("").parent()` is `None`, so the walk
        // probes exactly one manifest — this crate's own, which cargo
        // makes the working directory and which is a member manifest, not
        // a root — and then runs out of parents.
        assert_eq!(find_root(Path::new("")), None);
    }
}
