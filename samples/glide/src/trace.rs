//! The built-in traces headless mode replays.
//!
//! Committed files, read by the shared loader — the format on disk is
//! the same format a recording produces, so each file is its own
//! golden: re-recording a loaded trace reproduces it byte for byte,
//! and the test that proves it lives beside the recorder's caller.

use renew_event::WindowEvent;

use crate::SampleError;

/// A stored trace's identity and the committed file holding it.
struct Scripted {
    name: &'static str,
    summary: &'static str,
    /// Embedded at compile time: a sample whose behaviour depended on
    /// its working directory would not be a pure function of its
    /// command line.
    text: &'static str,
}

const SCRIPTED: &[Scripted] = &[
    Scripted {
        name: "soar",
        summary: "spaced flaps that clear several pipes before falling",
        text: include_str!("../traces/soar.trace"),
    },
    Scripted {
        name: "sink",
        summary: "no input at all — gravity wins",
        text: include_str!("../traces/sink.trace"),
    },
];

/// A loaded trace: events by tick from zero, exactly as stored.
#[derive(Debug)]
pub struct Trace {
    pub name: &'static str,
    pub events: Vec<(u64, WindowEvent)>,
}

/// The trace by that name.
///
/// # Errors
///
/// [`SampleError::Usage`] naming every trace that does exist, or
/// [`SampleError::Failed`] if a committed file does not parse — that is
/// a repository defect, not the caller's command line, and it is
/// blamed as one.
pub fn by_name(name: &str) -> Result<Trace, SampleError> {
    let found = SCRIPTED
        .iter()
        .find(|scripted| scripted.name == name)
        .ok_or_else(|| SampleError::Usage(format!("no trace named `{name}`; {}", names())))?;
    let events = renew_replay::events(found.name, found.text)
        .map_err(|error| SampleError::Failed(format!("built-in trace: {error}")))?;
    Ok(Trace {
        name: found.name,
        events,
    })
}

/// Every built-in trace, named and summarised, for usage messages.
#[must_use]
pub fn names() -> String {
    let described: Vec<String> = SCRIPTED
        .iter()
        .map(|scripted| format!("`{}` ({})", scripted.name, scripted.summary))
        .collect();
    format!("built-in traces: {}", described.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_committed_trace_parses() {
        for scripted in SCRIPTED {
            let trace = by_name(scripted.name).expect("committed traces are repository facts");
            assert_eq!(trace.name, scripted.name);
        }
    }

    #[test]
    fn an_unknown_name_lists_what_exists() {
        let refused = by_name("swim");
        assert!(
            matches!(&refused, Err(SampleError::Usage(message))
                if message.contains("soar") && message.contains("sink")),
            "an unknown trace is a usage error listing what exists: {refused:?}"
        );
    }
}
