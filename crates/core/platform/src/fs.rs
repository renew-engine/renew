//! Whole-file filesystem operations. Streaming I/O arrives with the
//! asset pipeline, not speculatively.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::ErrorKind;

/// Why a filesystem operation failed — always carrying the path.
#[derive(Debug)]
#[non_exhaustive]
pub enum FsError {
    NotFound {
        path: PathBuf,
    },
    PermissionDenied {
        path: PathBuf,
    },
    /// A bounded read was asked for and the file is larger than the
    /// caller allowed (only [`read_to_string_bounded`] produces this).
    TooLarge {
        path: PathBuf,
        limit: usize,
    },
    /// Text was requested and the content is not UTF-8
    /// (only [`read_to_string`] produces this).
    InvalidUtf8 {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        kind: ErrorKind,
    },
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { path } => write!(f, "not found: {}", path.display()),
            Self::PermissionDenied { path } => {
                write!(f, "permission denied: {}", path.display())
            }
            Self::TooLarge { path, limit } => {
                write!(f, "larger than the {limit}-byte limit: {}", path.display())
            }
            Self::InvalidUtf8 { path } => {
                write!(f, "not valid UTF-8: {}", path.display())
            }
            Self::Io { path, kind } => write!(f, "{kind}: {}", path.display()),
        }
    }
}

impl std::error::Error for FsError {}

/// Classification for byte-level operations: `InvalidData` here is NOT a
/// text-encoding problem, so it stays a plain [`FsError::Io`].
fn classify(path: &Path, error: &std::io::Error) -> FsError {
    let path = path.to_path_buf();
    match error.kind() {
        ErrorKind::NotFound => FsError::NotFound { path },
        ErrorKind::PermissionDenied => FsError::PermissionDenied { path },
        kind => FsError::Io { path, kind },
    }
}

/// Classification for text reads, where `InvalidData` means exactly
/// "the bytes are not UTF-8".
fn classify_text(path: &Path, error: &std::io::Error) -> FsError {
    if error.kind() == ErrorKind::InvalidData {
        FsError::InvalidUtf8 {
            path: path.to_path_buf(),
        }
    } else {
        classify(path, error)
    }
}

/// Read a whole file.
///
/// # Errors
///
/// [`FsError`] naming the path, classified by kind.
pub fn read(path: &Path) -> Result<Vec<u8>, FsError> {
    std::fs::read(path).map_err(|error| classify(path, &error))
}

/// Read a whole file as UTF-8 text.
///
/// # Errors
///
/// [`FsError`] naming the path; non-UTF-8 content is
/// [`FsError::InvalidUtf8`].
pub fn read_to_string(path: &Path) -> Result<String, FsError> {
    std::fs::read_to_string(path).map_err(|error| classify_text(path, &error))
}

/// Read a whole file as UTF-8 text, refusing anything past `limit`
/// bytes.
///
/// [`read_to_string`] allocates whatever the file holds, which is fine
/// for content the engine wrote and wrong for content it did not. A
/// parser can validate a hostile file only once it is in memory, so the
/// only place a size limit can actually be enforced is here, before the
/// allocation happens; a bound declared inside a parser is decorative.
///
/// The limit is applied to the bytes actually read, not to the size the
/// filesystem reports. Reported sizes can be stale, absent, or a lie
/// about a growing or synthetic file, so this reads at most one byte
/// past the limit and refuses if that byte exists — which costs nothing
/// and cannot be fooled.
///
/// # Errors
///
/// [`FsError::TooLarge`] naming the limit when the file exceeds it;
/// otherwise as [`read_to_string`].
pub fn read_to_string_bounded(path: &Path, limit: usize) -> Result<String, FsError> {
    use std::io::Read as _;

    let file = std::fs::File::open(path).map_err(|error| classify(path, &error))?;
    // One byte past the limit is enough to tell "at the limit" from
    // "over it" without reading a byte more than that.
    let mut text = String::new();
    let read = file
        .take(limit as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|error| classify_text(path, &error))?;
    if read > limit {
        return Err(FsError::TooLarge {
            path: path.to_path_buf(),
            limit,
        });
    }
    Ok(text)
}

/// Write a whole file, replacing any existing content.
///
/// # Errors
///
/// [`FsError`] naming the path, classified by kind.
pub fn write(path: &Path, contents: &[u8]) -> Result<(), FsError> {
    std::fs::write(path, contents).map_err(|error| classify(path, &error))
}

/// Whether the path exists right now. Honest about failure: an
/// inaccessible parent is an error, not `false`. Like every existence
/// check, the answer can be stale by the time it is used — open the
/// file and handle the error where that matters.
///
/// # Errors
///
/// [`FsError`] when existence cannot be determined.
pub fn exists(path: &Path) -> Result<bool, FsError> {
    path.try_exists().map_err(|error| classify(path, &error))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One error of every variant, all naming the same path.
    fn all_variants(path: &Path) -> [FsError; 5] {
        [
            FsError::NotFound {
                path: path.to_path_buf(),
            },
            FsError::PermissionDenied {
                path: path.to_path_buf(),
            },
            FsError::InvalidUtf8 {
                path: path.to_path_buf(),
            },
            FsError::TooLarge {
                path: path.to_path_buf(),
                limit: 16,
            },
            FsError::Io {
                path: path.to_path_buf(),
                kind: ErrorKind::TimedOut,
            },
        ]
    }

    /// The path an error carries, whichever variant it is. The
    /// or-pattern is exhaustive by construction: a future variant that
    /// forgot its path would not compile here.
    fn reported_path(error: &FsError) -> &Path {
        let (FsError::NotFound { path }
        | FsError::PermissionDenied { path }
        | FsError::InvalidUtf8 { path }
        | FsError::TooLarge { path, .. }
        | FsError::Io { path, .. }) = error;
        path
    }

    /// Which error an [`FsError`] is, as a comparable value — with the
    /// kind, where the variant carries one. Classification is then
    /// asserted with `assert_eq!`, which reports the variant it actually
    /// got, rather than with a pattern whose failing arm no passing run
    /// ever reaches.
    #[derive(Debug, PartialEq, Eq)]
    enum Variant {
        NotFound,
        PermissionDenied,
        InvalidUtf8,
        TooLarge,
        Io(ErrorKind),
    }

    fn variant(error: &FsError) -> Variant {
        match error {
            FsError::NotFound { .. } => Variant::NotFound,
            FsError::PermissionDenied { .. } => Variant::PermissionDenied,
            FsError::InvalidUtf8 { .. } => Variant::InvalidUtf8,
            FsError::TooLarge { .. } => Variant::TooLarge,
            FsError::Io { kind, .. } => Variant::Io(*kind),
        }
    }

    /// No two variants collapse onto the same classification. The
    /// classifier tests below only ever see errors the classifier itself
    /// builds, so a variant constructed elsewhere — as the size refusal
    /// is — would otherwise never be mapped by anything, and an arm
    /// returning the wrong answer would sit unnoticed.
    #[test]
    fn every_variant_maps_to_a_classification_of_its_own() {
        let path = Path::new("p");
        let seen: Vec<Variant> = all_variants(path).iter().map(variant).collect();
        assert_eq!(seen.len(), all_variants(path).len());
        for (index, one) in seen.iter().enumerate() {
            for other in &seen[index + 1..] {
                assert_ne!(one, other, "two variants share a classification");
            }
        }
        assert!(seen.contains(&Variant::TooLarge), "{seen:?}");
    }

    #[test]
    fn every_error_variant_carries_its_path_in_the_field_and_the_message() {
        let path = PathBuf::from("some/asset.bin");
        let shown = path.display().to_string();
        for error in &all_variants(&path) {
            assert_eq!(reported_path(error), path, "wrong path in {error:?}");
            let text = error.to_string();
            assert!(text.contains(&shown), "path missing from: {text}");
        }
    }

    #[test]
    fn byte_level_classification_maps_the_documented_kinds() {
        let path = Path::new("p");
        let classified = |kind: ErrorKind| variant(&classify(path, &std::io::Error::from(kind)));
        assert_eq!(classified(ErrorKind::NotFound), Variant::NotFound);
        assert_eq!(
            classified(ErrorKind::PermissionDenied),
            Variant::PermissionDenied
        );
        // Byte-level InvalidData is NOT a text problem: plain Io.
        assert_eq!(
            classified(ErrorKind::InvalidData),
            Variant::Io(ErrorKind::InvalidData)
        );
        assert_eq!(
            classified(ErrorKind::WouldBlock),
            Variant::Io(ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn text_classification_reserves_invalid_data_for_utf8() {
        let path = Path::new("p");
        let classified =
            |kind: ErrorKind| variant(&classify_text(path, &std::io::Error::from(kind)));
        assert_eq!(classified(ErrorKind::InvalidData), Variant::InvalidUtf8);
        // Everything else falls through to the byte-level rules.
        assert_eq!(classified(ErrorKind::NotFound), Variant::NotFound);
        assert_eq!(
            classified(ErrorKind::WouldBlock),
            Variant::Io(ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn reading_a_directory_fails_with_the_path_reported() {
        // The kind differs per operating system; the contract is that
        // SOME classified error comes back carrying exactly this path.
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
        let error = read(directory).expect_err("directories are not files");
        assert_eq!(reported_path(&error), directory, "wrong path in {error:?}");
    }

    #[test]
    fn writing_and_probing_report_their_failures_against_the_same_path() {
        // A NUL byte never reaches an OS filesystem call: every platform
        // rejects the name itself, so both seams fail without depending
        // on any filesystem state.
        let path = Path::new("renew\0invalid");
        let write_error = write(path, b"never lands").expect_err("a NUL path cannot be written");
        assert_eq!(
            reported_path(&write_error),
            path,
            "wrong path in {write_error:?}"
        );
        let exists_error = exists(path).expect_err("a NUL path's existence is undeterminable");
        assert_eq!(
            reported_path(&exists_error),
            path,
            "wrong path in {exists_error:?}"
        );
    }
}
