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

/// A bounded read accepts a file exactly at the limit and refuses the
/// same file one byte longer. The boundary is the whole point: an
/// off-by-one here either rejects legitimate content or admits the
/// unbounded allocation the limit exists to prevent.
#[test]
fn a_bounded_read_accepts_the_limit_and_refuses_one_byte_past_it() {
    let file = scratch("bounded.txt");
    let exactly = "0123456789";
    fs::write(&file.0, exactly.as_bytes()).expect("write succeeds");
    assert_eq!(
        fs::read_to_string_bounded(&file.0, exactly.len()).expect("at the limit is allowed"),
        exactly
    );

    fs::write(&file.0, format!("{exactly}x").as_bytes()).expect("write succeeds");
    match fs::read_to_string_bounded(&file.0, exactly.len()) {
        Err(fs::FsError::TooLarge { path, limit }) => {
            assert_eq!(path, file.0);
            assert_eq!(limit, exactly.len());
        }
        other => panic!("expected a refusal naming the limit, got {other:?}"),
    }
}

/// The refusal says how big the caller said was acceptable, because the
/// first question anyone asks of this error is "how big is too big".
#[test]
fn the_refusal_names_the_limit_in_its_message() {
    let file = scratch("bounded-message.txt");
    fs::write(&file.0, b"far too much content for this").expect("write succeeds");
    let error = fs::read_to_string_bounded(&file.0, 4).expect_err("must refuse");
    let shown = error.to_string();
    assert!(shown.contains('4'), "{shown}");
    assert!(shown.contains("bounded-message"), "{shown}");
}

/// A zero limit is a real request, not a degenerate one: it accepts an
/// empty file and refuses everything else.
#[test]
fn a_zero_limit_admits_only_an_empty_file() {
    let file = scratch("bounded-empty.txt");
    fs::write(&file.0, b"").expect("write succeeds");
    assert_eq!(
        fs::read_to_string_bounded(&file.0, 0).expect("empty is within a zero limit"),
        ""
    );
    fs::write(&file.0, b"x").expect("write succeeds");
    assert!(matches!(
        fs::read_to_string_bounded(&file.0, 0),
        Err(fs::FsError::TooLarge { .. })
    ));
}

/// An oversized file is reported as oversized even when the limit cuts a
/// character in half.
///
/// The bound reads one byte past the limit and then decodes, so a cut
/// landing inside a multi-byte character makes the decode fail before the
/// size check is reached. The file is refused either way — nothing unsafe
/// — but "this file is not valid text" sends a reader hunting for an
/// encoding problem in a file whose only fault is being too big. Size is
/// the more fundamental fact, so it has to win.
#[test]
fn an_oversized_file_is_too_large_even_when_the_cut_splits_a_character() {
    let file = scratch("bounded-split-char.txt");
    // Four ASCII bytes, then a two-byte character, then more: with a
    // limit of four, the byte read one past the limit is the first half
    // of that character.
    fs::write(&file.0, "aaaa\u{e9}bbbb".as_bytes()).expect("write succeeds");
    match fs::read_to_string_bounded(&file.0, 4) {
        Err(fs::FsError::TooLarge { limit, .. }) => assert_eq!(limit, 4),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

/// Bounding does not weaken the other classifications: a missing file
/// and non-UTF-8 content report what they always did.
#[test]
fn a_bounded_read_still_classifies_missing_and_non_utf8() {
    let missing = scratch("bounded-missing.txt");
    assert!(matches!(
        fs::read_to_string_bounded(&missing.0, 64),
        Err(fs::FsError::NotFound { .. })
    ));

    let invalid = scratch("bounded-invalid.txt");
    fs::write(&invalid.0, &[0xF0, 0x28, 0x8C, 0x28]).expect("write succeeds");
    assert!(matches!(
        fs::read_to_string_bounded(&invalid.0, 64),
        Err(fs::FsError::InvalidUtf8 { .. })
    ));
}
