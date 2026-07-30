//! Named thread creation — the only legal home of thread spawning in
//! the engine. The job system builds on these primitives.

use std::fmt;

use crate::ErrorKind;

/// Why a thread operation failed — always naming the thread.
#[derive(Debug)]
#[non_exhaustive]
pub enum ThreadError {
    /// The requested name cannot be used (interior NUL byte).
    InvalidName { name: String },
    /// The operating system refused to create the thread.
    SpawnFailed { name: String, kind: ErrorKind },
    /// The thread panicked — a defect in every engine build profile,
    /// surfaced here instead of unwinding across the join.
    Panicked { name: String },
}

impl fmt::Display for ThreadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { name } => {
                write!(
                    f,
                    "invalid thread name (interior NUL): `{}`",
                    name.escape_debug()
                )
            }
            Self::SpawnFailed { name, kind } => {
                write!(f, "spawning thread `{name}` failed: {kind}")
            }
            Self::Panicked { name } => write!(f, "thread `{name}` panicked"),
        }
    }
}

impl std::error::Error for ThreadError {}

/// A running named thread.
///
/// **Dropping this handle detaches the thread**: it keeps running, and a
/// panic inside it is reported only through the process's panic output —
/// it never surfaces through the engine's error model. Join to observe
/// the result. Detach-on-drop is the deliberate v0 contract; the job
/// system owns richer thread lifecycles when it arrives.
///
/// The handle is `Send` and (structurally) `Sync`: ownership of the
/// join can move across threads, and shared references are harmless
/// because `&self` offers only the name — `join` consumes the handle,
/// so only one owner can ever observe the result.
#[must_use = "dropping a ThreadHandle detaches the thread; join it to observe its result"]
pub struct ThreadHandle<T> {
    name: String,
    handle: std::thread::JoinHandle<T>,
}

impl<T> ThreadHandle<T> {
    /// The thread's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Wait for the thread and return its result.
    ///
    /// # Errors
    ///
    /// [`ThreadError::Panicked`] when the thread panicked.
    pub fn join(self) -> Result<T, ThreadError> {
        self.handle
            .join()
            .map_err(|_panic_payload| ThreadError::Panicked { name: self.name })
    }
}

/// Spawn a named thread. Every engine thread is named — diagnostics
/// discipline from day one. (Some operating systems truncate the name
/// they display — Linux keeps 15 bytes — while [`ThreadHandle::name`]
/// and the in-thread view keep the full string.)
///
/// # Errors
///
/// [`ThreadError::InvalidName`] for names with interior NUL bytes;
/// [`ThreadError::SpawnFailed`] when the operating system refuses.
pub fn spawn_named<T, F>(name: &str, body: F) -> Result<ThreadHandle<T>, ThreadError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    if name.contains('\0') {
        // std's Builder panics on interior NULs; the seam's contract is
        // Result, so the check lives here.
        return Err(ThreadError::InvalidName {
            name: name.to_string(),
        });
    }
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(body)
        .map(|handle| ThreadHandle {
            name: name.to_string(),
            handle,
        })
        .map_err(|error| ThreadError::SpawnFailed {
            name: name.to_string(),
            kind: error.kind(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawned_threads_carry_their_name_and_return_values() {
        let handle = spawn_named("renew-test-worker", || {
            std::thread::current().name().map(str::to_string)
        })
        .expect("spawn succeeds");
        assert_eq!(handle.name(), "renew-test-worker");
        let observed = handle.join().expect("no panic");
        assert_eq!(observed.as_deref(), Some("renew-test-worker"));
    }

    /// A thread body with nothing to do. A named function rather than a
    /// closure so the rejected and the accepted spawn below reach the
    /// same `spawn_named` instantiation: the rejection is measured
    /// against exactly the code the success path runs.
    fn nothing() {}

    #[test]
    fn interior_nul_names_are_an_error_not_a_panic() {
        // `err` up front: the diagnostic must be built eagerly, or the
        // formatting is code no passing run ever executes.
        let error = spawn_named("bad\0name", nothing).err();
        assert!(
            matches!(&error, Some(ThreadError::InvalidName { name }) if name == "bad\0name"),
            "expected InvalidName naming the offending string, got {error:?}"
        );
        // The same body under a legal name spawns and joins: the name
        // was the only reason the call above failed.
        let joined = spawn_named("renew-test-legal-name", nothing).and_then(ThreadHandle::join);
        assert!(
            joined.is_ok(),
            "the same body must spawn under a legal name, got {joined:?}"
        );
    }

    #[test]
    fn error_variants_display_their_thread_name() {
        let spawn_failed = ThreadError::SpawnFailed {
            name: "renew-io".to_string(),
            kind: ErrorKind::OutOfMemory,
        };
        assert!(spawn_failed.to_string().contains("renew-io"));
        assert!(matches!(
            spawn_failed,
            ThreadError::SpawnFailed {
                kind: ErrorKind::OutOfMemory,
                ..
            }
        ));
        let panicked = ThreadError::Panicked {
            name: "renew-sim".to_string(),
        };
        assert!(panicked.to_string().contains("renew-sim"));
        let invalid = ThreadError::InvalidName {
            name: "a\0b".to_string(),
        };
        assert!(invalid.to_string().contains("a\\0b"));
    }

    #[test]
    fn a_panicking_thread_surfaces_as_an_error_naming_it() {
        let handle = spawn_named("renew-test-doomed", || panic!("deliberate")).expect("spawn");
        let error = handle.join().expect_err("panic surfaces");
        assert!(
            matches!(&error, ThreadError::Panicked { name } if name == "renew-test-doomed"),
            "expected Panicked naming the thread, got {error}"
        );
    }
}
