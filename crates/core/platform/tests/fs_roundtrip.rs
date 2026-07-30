//! Filesystem seam round-trips against a real scratch directory. The
//! directory comes from cargo at compile time; the process id suffix
//! keeps parallel runs apart (test code — the engine's ambient-state
//! rules bind the library, not its tests). Scratch files are removed by
//! a drop guard so failing assertions don't strand them.

use std::path::{Path, PathBuf};

use renew_platform::fs;

/// A scratch path that deletes its file on drop — including during the
/// unwind of a failed assertion.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let mut path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    path.push(format!("{name}-{}", std::process::id()));
    Scratch(path)
}

#[test]
fn write_exists_read_round_trip() {
    let file = scratch("roundtrip.bin");
    fs::write(&file.0, b"renew \xF0\x9F\x94\xA7").expect("write succeeds");
    assert!(fs::exists(&file.0).expect("determinable"));
    let bytes = fs::read(&file.0).expect("read succeeds");
    assert_eq!(bytes, b"renew \xF0\x9F\x94\xA7");
    let text = fs::read_to_string(&file.0).expect("valid utf-8");
    assert_eq!(text, "renew 🔧");
}

#[test]
fn missing_files_name_the_path_in_a_not_found_error() {
    let file = scratch("never-created.txt");
    match fs::read(&file.0) {
        Err(fs::FsError::NotFound { path: reported }) => assert_eq!(reported, file.0),
        other => panic!("expected NotFound, got {other:?}"),
    }
    assert!(!fs::exists(&file.0).expect("determinable"));
}

#[test]
fn non_utf8_content_is_a_distinct_error() {
    let file = scratch("binary.bin");
    fs::write(&file.0, &[0xFF, 0xFE, 0x00, 0x80]).expect("write succeeds");
    match fs::read_to_string(&file.0) {
        Err(fs::FsError::InvalidUtf8 { path: reported }) => assert_eq!(reported, file.0),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
}

#[test]
fn write_replaces_existing_content() {
    let file = scratch("replace.txt");
    fs::write(&file.0, b"first, much longer content").expect("write succeeds");
    fs::write(&file.0, b"second").expect("rewrite succeeds");
    assert_eq!(fs::read(&file.0).expect("read"), b"second");
}

#[test]
fn reading_a_directory_fails_with_the_path_reported() {
    // The error kind differs per platform; the contract under test is
    // that SOME classified error comes back carrying exactly this path.
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let error = fs::read(directory).expect_err("directories are not files");
    let reported = match &error {
        fs::FsError::NotFound { path }
        | fs::FsError::PermissionDenied { path }
        | fs::FsError::InvalidUtf8 { path }
        | fs::FsError::Io { path, .. } => path.as_path(),
        other => panic!("unexpected variant: {other:?}"),
    };
    assert_eq!(reported, directory);
}
