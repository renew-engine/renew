//! Recording a run's input as it happens.
//!
//! The recorder sits beside the world, not inside it: it watches the
//! same events the simulation consumes and notes which tick each was
//! delivered before. That is the whole contract — a trace is the
//! external stimulus indexed by simulation tick, and the tick an event
//! carries is the number of steps that had already run when it arrived.
//!
//! It records **every** event the world was given, not the subset that
//! looks like user input. The world counts what it is told, including
//! repaint requests it ignores, and that count reaches the state digest;
//! a recording that quietly filtered would replay into a different
//! world and the digests would disagree with nothing to explain it.

use renew_platform::window::WindowEvent;
use renew_trace::{Trace, TraceError, TraceHeader};

use crate::convert::{Unencodable, to_trace};

/// Collects events against the tick they were delivered before.
#[derive(Debug, Default)]
pub struct Recorder {
    events: Vec<(u64, renew_trace::TraceEvent)>,
    /// The first event this build could not write down. Kept rather than
    /// returned immediately so a recording fails once, at the end, with
    /// the run intact — a refusal mid-frame would abandon a session the
    /// person was in the middle of.
    refused: Option<Unencodable>,
}

impl Recorder {
    /// Note one event, delivered before the step numbered `tick`.
    pub fn event(&mut self, tick: u64, event: WindowEvent) {
        match to_trace(event) {
            Ok(encoded) => self.events.push((tick, encoded)),
            Err(refusal) => {
                self.refused.get_or_insert(refusal);
            }
        }
    }

    /// How many events have been recorded so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether nothing has been recorded yet.
    ///
    /// A run that ends with an empty recording is legal — a session in
    /// which nobody touched anything is a real session, and its trace
    /// replays to a world that also did nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Close the recording.
    ///
    /// # Errors
    ///
    /// Returns the trace crate's own refusal when the header or the
    /// event sequence does not satisfy the format — the same check a
    /// reader would apply, run here so a recorder can never write a file
    /// its own reader rejects.
    pub fn finish(self, header: TraceHeader) -> Result<Trace, RecordError> {
        if let Some(refusal) = self.refused {
            return Err(RecordError::Unencodable(refusal));
        }
        Trace::new(header, self.events).map_err(RecordError::Malformed)
    }
}

/// Why a recording could not be closed.
#[derive(Debug)]
pub enum RecordError {
    /// An event arrived that this build has no encoding for.
    Unencodable(Unencodable),
    /// The recorded sequence does not satisfy the format.
    Malformed(TraceError),
}

impl core::fmt::Display for RecordError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unencodable(refusal) => write!(f, "{refusal}"),
            Self::Malformed(error) => write!(f, "the recording is not a valid trace: {error}"),
        }
    }
}
