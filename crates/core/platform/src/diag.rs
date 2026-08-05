//! A diagnostics sink that writes to a file.
//!
//! The reporting crate defines the [`Sink`](renew_diag::Sink) trait and
//! forbids filesystem access in its own lint configuration, on the
//! grounds that sinks own their I/O. This crate owns the filesystem. So
//! the file-writing sink lives here, and nowhere else in the engine has
//! to grow a filesystem exception to get one.
//!
//! # Mechanism, not policy
//!
//! **Nothing here reads the environment, and nothing here installs
//! itself.** That is this crate's contract — it is a doorway to the
//! operating system, not a source of ambient configuration — and it is
//! also the more useful split: a binary decides *whether* to log and
//! *where*, because a binary is the thing that owns its command line,
//! its environment and its files. This module only knows how to turn a
//! record into a line and put it on the end of a file.
//!
//! # Why a whole file per record
//!
//! [`FileSink`] opens, appends and closes on every record rather than
//! holding a handle. A held handle needs an owner, a close, and a story
//! about what happens when the process dies badly — and the records this
//! is built for are emitted when something has *already* gone wrong,
//! where a syscall costs nothing anybody will measure. A run that logs
//! nothing pays nothing, and a run that crashes has its last line on
//! disk rather than in a buffer that died with it.
//!
//! That last property is the whole reason this is not a buffered writer.

use std::path::PathBuf;

use renew_diag::{Record, Sink};

/// A sink that appends one line per record to a file.
///
/// Lines are `LEVEL target: message`, newline-terminated — the level
/// first because a reader scanning a log is looking for the severe ones,
/// and the target second because it says which part of the engine spoke.
///
/// # Failure is silent, and that is deliberate
///
/// A sink that cannot write has nowhere to report it: the reporting
/// channel is the thing that is broken, and a panic here would turn a
/// diagnostic into the fault. So a failed write is dropped. The caller
/// who wants to know whether logging works should check the path itself
/// after the run — the file is created on the first record, so its
/// absence and its emptiness mean the same thing, which is that nothing
/// was reported.
pub struct FileSink {
    path: PathBuf,
}

impl FileSink {
    /// A sink appending to `path`.
    ///
    /// The file is neither created nor truncated here: it is opened on
    /// the first record and appended to thereafter, so several runs
    /// pointed at one path accumulate rather than overwrite each other.
    /// A caller who wants one run per file chooses a fresh path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The path records are appended to.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Format one record exactly as [`Sink::write`] does.
    ///
    /// Split out so the formatting is testable without a filesystem —
    /// the line's shape is the part a reader depends on, and it should
    /// not need a temporary directory to pin.
    #[must_use]
    pub fn line(record: &Record<'_>) -> String {
        format!(
            "{} {}: {}\n",
            record.level().as_str(),
            record.target(),
            record.message()
        )
    }
}

impl Sink for FileSink {
    fn write(&self, record: &Record<'_>) {
        // Dropped rather than reported: see the type's contract. There is
        // no channel left to complain through.
        let _ = crate::fs::append(&self.path, Self::line(record).as_bytes());
    }
}

impl core::fmt::Debug for FileSink {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FileSink")
            .field("path", &self.path)
            .finish()
    }
}

/// Send this process's diagnostics, and any panic, to `path`.
///
/// Installs a [`FileSink`] as the process-wide sink and chains a panic
/// hook that records the panic before the default one runs — so a run
/// that dies leaves its reason on disk rather than only on a console
/// nobody was capturing.
///
/// **The path is a parameter, not something read from the environment.**
/// This crate is a doorway to the operating system and deliberately not
/// a source of ambient configuration; the binary decides whether to log
/// and where, because the binary owns its command line and environment.
///
/// # The leak, which is deliberate
///
/// The sink is leaked, because the reporting crate takes a `'static`
/// reference and a process-wide sink lives until the process ends. One
/// allocation, once, at startup — the alternative is a global slot that
/// exists only to avoid the word "leak" while having the same lifetime.
///
/// # Panics
///
/// Never for a bad path — a sink that cannot write drops its records
/// silently, as its own contract states. Calling this twice is the
/// reporting crate's contract violation, not this function's.
pub fn log_to_file(path: impl Into<PathBuf>) {
    let sink: &'static FileSink = Box::leak(Box::new(FileSink::new(path)));
    renew_diag::install(sink);

    // Chained rather than replaced: the default hook prints to the
    // console, which is what a developer watching a terminal wants, and
    // this adds the copy that survives the terminal closing.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // `{info}` already carries the location and the payload — the
        // same text the default hook prints. Formatting the location
        // separately duplicated it, and left an arm for a location that
        // is absent, which no real panic produces.
        renew_diag::error!(target: "panic", "{info}");
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use renew_diag::Level;

    /// The line's shape, pinned without touching a filesystem.
    #[test]
    fn a_record_becomes_one_terminated_line() {
        let line = FileSink::line(&Record::new(
            Level::Error,
            "renew-rhi",
            format_args!("device lost at {}", 12),
        ));
        assert_eq!(line, "ERROR renew-rhi: device lost at 12\n");
    }

    /// Every level reaches the line, so a reader filtering by severity
    /// sees the word the level crate defines rather than a local
    /// spelling.
    #[test]
    fn every_level_prints_its_own_name() {
        for level in [Level::Error, Level::Warn, Level::Info, Level::Debug] {
            let line = FileSink::line(&Record::new(level, "t", format_args!("m")));
            assert!(
                line.starts_with(level.as_str()),
                "{level:?} produced {line:?}"
            );
        }
    }

    /// Records really do accumulate: a second write appends rather than
    /// replacing, which is what makes a log of a crashing run readable.
    #[test]
    fn records_accumulate_in_order() {
        let dir = std::env::temp_dir().join("renew-filesink-accumulate");
        let _ = std::fs::remove_file(&dir);
        let sink = FileSink::new(&dir);
        sink.write(&Record::new(Level::Warn, "a", format_args!("first")));
        sink.write(&Record::new(Level::Error, "b", format_args!("second")));
        let text = std::fs::read_to_string(&dir).expect("the sink created the file");
        assert_eq!(text, "WARN a: first\nERROR b: second\n");
        let _ = std::fs::remove_file(&dir);
    }

    /// The path is reported back, so a caller that built the sink from a
    /// value can say where records went without keeping its own copy.
    #[test]
    fn the_sink_reports_its_own_path() {
        let path = std::env::temp_dir().join("renew-filesink-path");
        let sink = FileSink::new(&path);
        assert_eq!(sink.path(), path);
    }

    /// The debug form names the file and nothing else — a sink has no
    /// counts, and the path is the only thing a reader wants.
    #[test]
    fn the_debug_form_names_the_file() {
        let sink = FileSink::new("somewhere.log");
        let shown = format!("{sink:?}");
        assert!(shown.starts_with("FileSink"), "{shown}");
        assert!(shown.contains("somewhere.log"), "{shown}");
    }

    /// A path that cannot be written is dropped rather than fatal — the
    /// reporting channel being broken must not become the failure.
    #[test]
    fn an_unwritable_path_is_survived() {
        // A directory component that is not a directory: the open fails
        // on every supported platform.
        let sink = FileSink::new(
            std::env::temp_dir()
                .join("renew-filesink-missing")
                .join("nested")
                .join("log.txt"),
        );
        sink.write(&Record::new(Level::Error, "t", format_args!("m")));
    }
}
