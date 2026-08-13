//! Loading a stored trace into engine events.
//!
//! The parsing itself belongs to `renew-trace`; what this adds is the
//! translation into the engine vocabulary and an error that names which
//! trace refused. Events come back indexed by **tick from zero** — the
//! format's own meaning. Any frame numbering is the caller's convention
//! to apply, not this crate's to impose.

use renew_event::WindowEvent;

use crate::convert;

/// A stored trace that would not load.
///
/// Carries the name the caller knows the trace by and the parse error's
/// own display, which names a line. Whose *fault* the failure is — a
/// user's file, a repository fixture — is blame the caller assigns; this
/// crate only knows what refused.
#[derive(Debug)]
pub struct LoadError {
    pub name: String,
    pub detail: String,
}

impl core::fmt::Display for LoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "reading the `{}` trace: {}", self.name, self.detail)
    }
}

/// The events of a stored trace, indexed by tick from zero.
///
/// Only the event lines are read. The header is provenance — the run the
/// file was captured from — and callers that want it own that decision;
/// the one caller that replays a header-owned run reads it through
/// `renew-trace` directly.
///
/// # Errors
///
/// [`LoadError`] when the text does not parse, naming `name` and the
/// parser's line-bearing message.
pub fn events(name: &str, text: &str) -> Result<Vec<(u64, WindowEvent)>, LoadError> {
    let parsed = renew_trace::parse(text).map_err(|error| LoadError {
        name: name.to_string(),
        detail: error.to_string(),
    })?;
    Ok(parsed
        .events()
        .iter()
        .map(|(tick, event)| (*tick, convert::from_trace(*event)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trace_that_does_not_parse_names_itself_and_a_line() {
        let error = events("broken", "not a trace at all\n").expect_err("nonsense must refuse");
        let shown = error.to_string();
        assert!(shown.contains("`broken`"), "{shown}");
        assert!(
            shown.contains("line"),
            "the parser's message names a line: {shown}"
        );
    }

    #[test]
    fn events_come_back_tick_indexed_from_zero() {
        let text = "renew-trace 1 sample=t ticks=2 timestep_ns=1 budget=1\ne 0 close\n";
        let events = events("t", text).expect("a minimal trace loads");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].0, 0,
            "tick zero stays tick zero — no frame shift here"
        );
    }
}
