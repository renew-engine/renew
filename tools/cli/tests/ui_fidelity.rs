//! The compiler-fidelity gate: every committed document blob in the
//! tree is byte-identical to compiling its committed source. A drift
//! between the two — an edited source without a recompile, a compiler
//! change that alters output — reddens here, on every push, before
//! any sample embeds a stale picture of its own menu.

use std::path::PathBuf;

/// Every (source, blob) pair the tree commits. Grows a line per
/// document; a missing file is a failure, not a skip.
const DOCUMENTS: [(&str, &str); 1] = [("samples/glide/menu.ui", "samples/glide/menu.uib")];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_committed_blob_matches_its_source() {
    for (source_path, blob_path) in DOCUMENTS {
        let source = std::fs::read_to_string(repo_root().join(source_path))
            .unwrap_or_else(|error| panic!("{source_path} must be readable: {error}"));
        let blob = std::fs::read(repo_root().join(blob_path))
            .unwrap_or_else(|error| panic!("{blob_path} must be readable: {error}"));
        let compiled = renew_cli::ui_compile::compile(&source)
            .unwrap_or_else(|refusal| panic!("{source_path} must compile: {refusal}"));
        assert_eq!(
            compiled.bytes, blob,
            "{blob_path} is not the compile of {source_path} — recompile it with \
             `renew ui-compile` and commit both together"
        );
    }
}
