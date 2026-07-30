//! Filesystem seam round-trips against a real scratch directory. The
//! directory comes from cargo at compile time; the process id suffix
//! keeps parallel runs apart (test code — the engine's ambient-state
//! rules bind the library, not its tests).

use std::path::PathBuf;

use renew_platform::fs;

fn scratch(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    path.push(format!("{name}-{}", std::process::id()));
    path
}

#[test]
fn write_exists_read_round_trip() {
    let path = scratch("roundtrip.bin");
    fs::write(&path, b"renew \xF0\x9F\x94\xA7").expect("write succeeds");
    assert!(fs::exists(&path).expect("determinable"));
    let bytes = fs::read(&path).expect("read succeeds");
    assert_eq!(bytes, b"renew \xF0\x9F\x94\xA7");
    let text = fs::read_to_string(&path).expect("valid utf-8");
    assert_eq!(text, "renew 🔧");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_files_name_the_path_in_a_not_found_error() {
    let path = scratch("never-created.txt");
    match fs::read(&path) {
        Err(fs::FsError::NotFound { path: reported }) => assert_eq!(reported, path),
        other => panic!("expected NotFound, got {other:?}"),
    }
    assert!(!fs::exists(&path).expect("determinable"));
}

#[test]
fn non_utf8_content_is_a_distinct_error() {
    let path = scratch("binary.bin");
    fs::write(&path, &[0xFF, 0xFE, 0x00, 0x80]).expect("write succeeds");
    match fs::read_to_string(&path) {
        Err(fs::FsError::InvalidUtf8 { path: reported }) => assert_eq!(reported, path),
        other => panic!("expected InvalidUtf8, got {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn write_replaces_existing_content() {
    let path = scratch("replace.txt");
    fs::write(&path, b"first, much longer content").expect("write succeeds");
    fs::write(&path, b"second").expect("rewrite succeeds");
    assert_eq!(fs::read(&path).expect("read"), b"second");
    let _ = std::fs::remove_file(&path);
}
