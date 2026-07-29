//! Diagnostics core: log records, severity levels, and the sink interface.
//!
//! This crate is the reporting seam for the engine. Code emits records
//! through the level macros ([`error!`], [`warn!`], [`info!`], [`debug!`],
//! [`trace!`]); a single [`Sink`], installed once at startup, receives
//! them. Concrete sinks live outside this crate and own their formatting,
//! buffering, timestamps, and output.
//!
//! # Contract
//!
//! - **The emit path performs no heap allocation.** Records borrow their
//!   message as [`core::fmt::Arguments`]; formatting happens in the sink,
//!   into storage the sink owns.
//! - **This crate never reads a clock and never touches the filesystem.**
//!   Records carry no timestamp; sinks stamp records on write from their
//!   own time source.
//! - **[`install`] is called at most once**, during process startup before
//!   engine work begins. A second call is a contract violation (fatal in
//!   dev builds; the first installation stands in release).
//! - **Without an installed sink, emitting is a silent no-op.**
//!   Diagnostics must never become a crash source.

// Diagnostics go through sinks; the standard output macros are banned in
// this crate by construction, not convention.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use core::fmt;
use std::sync::OnceLock;

/// Severity of a record. Ordered from most severe (`Error`, which compares
/// least) to least severe (`Trace`, which compares greatest), so a filter
/// reads `record.level() <= Level::Info`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    /// Upper-case name, fixed width friendly.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad`, not `write_str`: sinks lay levels out in columns, so
        // width and alignment flags must be honored.
        f.pad(self.as_str())
    }
}

/// A single diagnostic record, borrowed for the duration of one sink call.
///
/// Records carry no timestamp: this crate never reads a clock; sinks stamp
/// records on write.
#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    level: Level,
    target: &'static str,
    message: fmt::Arguments<'a>,
}

impl<'a> Record<'a> {
    /// Assemble a record. Usually called through the level macros, which
    /// default `target` to the caller's module path.
    #[must_use]
    pub fn new(level: Level, target: &'static str, message: fmt::Arguments<'a>) -> Self {
        Self {
            level,
            target,
            message,
        }
    }

    #[must_use]
    pub fn level(&self) -> Level {
        self.level
    }

    #[must_use]
    pub fn target(&self) -> &'static str {
        self.target
    }

    #[must_use]
    pub fn message(&self) -> fmt::Arguments<'a> {
        self.message
    }
}

/// Where records go. Implementations own their formatting, buffering,
/// timestamps, and output, and must be callable from any thread.
///
/// This is the crate's designed extension point.
pub trait Sink: Sync {
    fn write(&self, record: &Record<'_>);
}

static SINK: OnceLock<&'static dyn Sink> = OnceLock::new();

/// Install the process-wide sink.
///
/// Called once at startup, before engine work begins. A second call is a
/// contract violation: fatal in dev builds; in release the first
/// installation stands and the call is ignored.
pub fn install(sink: &'static dyn Sink) {
    if SINK.set(sink).is_err() {
        debug_assert!(
            false,
            "diag sink installed twice; installation happens once at startup"
        );
    }
}

/// Hand a record to the installed sink; a silent no-op when none is
/// installed.
pub fn emit(record: &Record<'_>) {
    if let Some(sink) = SINK.get() {
        sink.write(record);
    }
}

/// Emit a record at an explicit [`Level`]. The level macros are the usual
/// front door; this is the common back end.
#[macro_export]
macro_rules! log {
    (target: $target:expr, $level:expr, $($arg:tt)+) => {
        $crate::emit(&$crate::Record::new(
            $level,
            $target,
            ::core::format_args!($($arg)+),
        ))
    };
    ($level:expr, $($arg:tt)+) => {
        $crate::log!(target: ::core::module_path!(), $level, $($arg)+)
    };
}

/// Emit at [`Level::Error`].
#[macro_export]
macro_rules! error {
    (target: $t:expr, $($arg:tt)+) => { $crate::log!(target: $t, $crate::Level::Error, $($arg)+) };
    ($($arg:tt)+) => { $crate::log!($crate::Level::Error, $($arg)+) };
}

/// Emit at [`Level::Warn`].
#[macro_export]
macro_rules! warn {
    (target: $t:expr, $($arg:tt)+) => { $crate::log!(target: $t, $crate::Level::Warn, $($arg)+) };
    ($($arg:tt)+) => { $crate::log!($crate::Level::Warn, $($arg)+) };
}

/// Emit at [`Level::Info`].
#[macro_export]
macro_rules! info {
    (target: $t:expr, $($arg:tt)+) => { $crate::log!(target: $t, $crate::Level::Info, $($arg)+) };
    ($($arg:tt)+) => { $crate::log!($crate::Level::Info, $($arg)+) };
}

/// Emit at [`Level::Debug`].
#[macro_export]
macro_rules! debug {
    (target: $t:expr, $($arg:tt)+) => { $crate::log!(target: $t, $crate::Level::Debug, $($arg)+) };
    ($($arg:tt)+) => { $crate::log!($crate::Level::Debug, $($arg)+) };
}

/// Emit at [`Level::Trace`].
#[macro_export]
macro_rules! trace {
    (target: $t:expr, $($arg:tt)+) => { $crate::log!(target: $t, $crate::Level::Trace, $($arg)+) };
    ($($arg:tt)+) => { $crate::log!($crate::Level::Trace, $($arg)+) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_orders_error_first() {
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
        assert!(Level::Debug < Level::Trace);
    }

    #[test]
    fn level_names_are_uppercase_and_display_matches() {
        for (level, name) in [
            (Level::Error, "ERROR"),
            (Level::Warn, "WARN"),
            (Level::Info, "INFO"),
            (Level::Debug, "DEBUG"),
            (Level::Trace, "TRACE"),
        ] {
            assert_eq!(level.as_str(), name);
            assert_eq!(level.to_string(), name);
        }
    }

    #[test]
    fn display_honors_width_and_alignment() {
        assert_eq!(format!("{:>7}", Level::Warn), "   WARN");
        assert_eq!(format!("{:<7}|", Level::Error), "ERROR  |");
    }

    fn check_record(record: &Record<'_>) {
        assert_eq!(record.level(), Level::Info);
        assert_eq!(record.target(), "unit");
        assert_eq!(format!("{}", record.message()), "value 42");
    }

    #[test]
    fn records_expose_their_parts() {
        check_record(&Record::new(
            Level::Info,
            "unit",
            format_args!("value {}", 42),
        ));
    }
}
