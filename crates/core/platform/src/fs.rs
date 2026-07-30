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

    #[test]
    fn every_error_variant_displays_its_path() {
        let path = PathBuf::from("some/asset.bin");
        let variants = [
            FsError::NotFound { path: path.clone() },
            FsError::PermissionDenied { path: path.clone() },
            FsError::InvalidUtf8 { path: path.clone() },
            FsError::Io {
                path: path.clone(),
                kind: ErrorKind::TimedOut,
            },
        ];
        for variant in &variants {
            let text = variant.to_string();
            assert!(
                text.contains("some/asset.bin") || text.contains("some\\asset.bin"),
                "path missing from: {text}"
            );
        }
    }

    #[test]
    fn byte_level_classification_maps_the_documented_kinds() {
        let path = Path::new("p");
        assert!(matches!(
            classify(path, &std::io::Error::from(ErrorKind::NotFound)),
            FsError::NotFound { .. }
        ));
        assert!(matches!(
            classify(path, &std::io::Error::from(ErrorKind::PermissionDenied)),
            FsError::PermissionDenied { .. }
        ));
        // Byte-level InvalidData is NOT a text problem: plain Io.
        assert!(matches!(
            classify(path, &std::io::Error::from(ErrorKind::InvalidData)),
            FsError::Io {
                kind: ErrorKind::InvalidData,
                ..
            }
        ));
        assert!(matches!(
            classify(path, &std::io::Error::from(ErrorKind::WouldBlock)),
            FsError::Io {
                kind: ErrorKind::WouldBlock,
                ..
            }
        ));
    }

    #[test]
    fn text_classification_reserves_invalid_data_for_utf8() {
        let path = Path::new("p");
        assert!(matches!(
            classify_text(path, &std::io::Error::from(ErrorKind::InvalidData)),
            FsError::InvalidUtf8 { .. }
        ));
        assert!(matches!(
            classify_text(path, &std::io::Error::from(ErrorKind::NotFound)),
            FsError::NotFound { .. }
        ));
    }

    #[test]
    fn reading_a_directory_fails_with_the_path_reported() {
        // The kind differs per operating system; the contract is that
        // SOME classified error comes back carrying exactly this path.
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
        let error = read(directory).expect_err("directories are not files");
        let (FsError::NotFound { path }
        | FsError::PermissionDenied { path }
        | FsError::InvalidUtf8 { path }
        | FsError::Io { path, .. }) = &error;
        assert_eq!(path, directory);
    }
}
