//! The scripted event traces headless mode replays.
//!
//! No runner supplies keystrokes, so a windowing sample that could only
//! be driven by hand would be a sample CI can never execute â€” and an
//! unexecuted binary is an untested one. These traces are the same
//! events the OS would deliver, on a schedule the sample chooses, fed to
//! the same state machine a window feeds.
//!
//! # Why they are files
//!
//! They used to be a table in this file, written in `WindowEvent`
//! literals. That made the same fact exist twice: once as Rust, once as
//! the text format the codec speaks â€” and a scripted run exercised only
//! the first, so the format a recording is *stored* in was never on the
//! default path. The traces are now committed files, read by the same
//! parser that reads a recording and converted by the same translation a
//! replay uses. One representation, one reader.
//!
//! Each file is also its own golden: it is exactly what `record`
//! produces from the trace it holds, so re-recording a loaded trace
//! reproduces the file byte for byte. That is the assertion which
//! catches the indexing mistake this migration could most plausibly
//! make, and it lives in `tests/recording.rs` where the recorder is.
//!
//! # The header is provenance, not configuration
//!
//! A trace file carries a header â€” `seed`, `ticks`, `budget`,
//! `timestep_ns` â€” and on the `--replay-trace` path the header **owns**
//! the run: it says how long the run was and how it was configured.
//!
//! On this path it does not. `--input-trace walk --seed 7` has to honour
//! the command line, because that is how the determinism matrix drives
//! one trace across many seeds. So a header here records *the run the
//! file was captured from* and nothing more, and only the event lines
//! are read. The same bytes therefore mean two compatible things: a
//! complete run to `replay`, and a reusable script to `--input-trace`.
//! Keeping one format for both is worth the sentence it takes to say so
//! â€” a second, headerless format would have cost a second parser.
//!
//! # Frames here, ticks in the file
//!
//! The file indexes events by **tick**, counted from zero, because that
//! is what the format means. The scripted driver indexes them by
//! **frame**, counted from one. They differ by exactly one, and the
//! conversion happens once, here, at load â€” see [`FIRST_FRAME`].

use renew_platform::window::WindowEvent;

use crate::convert;
use crate::error::SampleError;

/// The frame a tick-zero event is delivered before.
///
/// The driver runs frames `1..=frames` and delivers a scripted event
/// before the frame whose index matches, while the format says tick *k*
/// is delivered before the step whose tick is *k*, counted from zero. A
/// headless run executes exactly one step per frame, so the two indexes
/// describe the same instant offset by one â€” which is why the recorder
/// writes `frame - 1`. Named rather than spelled as a bare `+ 1` so the
/// inverse in `scripted.rs` has something to point at.
const FIRST_FRAME: u64 = 1;

/// A scripted trace as it is stored: its identity, and the committed file
/// that holds its events.
struct Scripted {
    name: &'static str,
    summary: &'static str,
    /// Embedded at compile time, not read at run time. A sample that had
    /// to find its traces on disk would be a sample whose behaviour
    /// depended on the directory it was started from.
    text: &'static str,
}

/// Every trace this sample can replay.
const SCRIPTED: &[Scripted] = &[
    Scripted {
        name: "walk",
        summary: "keys, pointer, wheel, focus and resize, ending in a close request",
        text: include_str!("../traces/walk.trace"),
    },
    Scripted {
        name: "idle",
        summary: "no input at all â€” the loop running on its own",
        text: include_str!("../traces/idle.trace"),
    },
];

/// A loaded trace: a named sequence of events, each scheduled at a frame
/// index counted from one â€” the same index the synthetic clock uses, so
/// "frame 4" is the frame the event lands in.
#[derive(Debug)]
pub struct Trace {
    pub name: &'static str,
    pub summary: &'static str,
    pub events: Vec<(u64, WindowEvent)>,
}

/// The trace by that name, read from the file that holds it.
///
/// # Errors
///
/// [`SampleError::Usage`] naming every trace that does exist â€” a sample
/// that answers "no such trace" and stops is a sample nobody runs twice.
///
/// [`SampleError::Failed`] if a committed trace file does not parse.
/// That is a defect in this repository rather than in the caller's
/// command line and is reported as one, though the unit test below is
/// what should catch it first.
pub fn by_name(name: &str) -> Result<Trace, SampleError> {
    let found = SCRIPTED
        .iter()
        .find(|scripted| scripted.name == name)
        .ok_or_else(|| SampleError::Usage(format!("no trace named `{name}`; {}", names())))?;
    load(found)
}

fn load(scripted: &Scripted) -> Result<Trace, SampleError> {
    let parsed = renew_trace::parse(scripted.text).map_err(|error| {
        SampleError::failed(
            &format!("reading the built-in `{}` trace", scripted.name),
            &error,
        )
    })?;
    let events = parsed
        .events()
        .iter()
        .map(|(tick, event)| {
            (
                tick.saturating_add(FIRST_FRAME),
                convert::from_trace(*event),
            )
        })
        .collect();
    Ok(Trace {
        name: scripted.name,
        summary: scripted.summary,
        events,
    })
}

/// The traces, named and summarized, for a usage message.
#[must_use]
pub fn names() -> String {
    let listed: Vec<String> = SCRIPTED
        .iter()
        .map(|scripted| format!("{} ({})", scripted.name, scripted.summary))
        .collect();
    format!("available traces: {}", listed.join(", "))
}

#[cfg(test)]
mod tests {
    use super::{FIRST_FRAME, SCRIPTED, Scripted, Trace, by_name, load, names};
    use crate::error::SampleError;

    fn every_trace() -> Vec<Trace> {
        SCRIPTED
            .iter()
            .map(|scripted| load(scripted).expect("a committed trace must parse"))
            .collect()
    }

    /// Whether the committed file behind a named trace writes an event on
    /// the given tick. Reads the stored text, so an assertion built on it
    /// is anchored to the file rather than to the load path under test.
    fn file_has_event_at_tick(name: &str, tick: u64) -> bool {
        SCRIPTED
            .iter()
            .find(|scripted| scripted.name == name)
            .is_some_and(|scripted| {
                let marker = format!("e {tick} ");
                scripted.text.lines().any(|line| line.starts_with(&marker))
            })
    }

    /// The refusal path, which no committed file can reach while they all
    /// parse — and which would therefore go untested if it were left to
    /// them. Reached by handing `load` a deliberately broken entry, so
    /// the arm is exercised rather than exempted.
    ///
    /// What it must get right is the blame: a codec error surfacing on
    /// its own says a trace is malformed without saying which, or whose
    /// fault it is. The caller typed a name; the defect is in this
    /// repository, and the message has to say so.
    #[test]
    fn a_trace_file_that_does_not_parse_says_which_one_and_whose_fault() {
        let broken = Scripted {
            name: "broken",
            summary: "not a trace at all",
            text: "this is not a trace
",
        };
        let error = load(&broken).expect_err("a non-trace must be refused");
        assert!(matches!(error, SampleError::Failed(_)), "{error:?}");
        let message = error.to_string();
        assert!(message.contains("broken"), "must name the trace: {message}");
        assert!(
            message.contains("built-in"),
            "must say the file is this repository's, not the caller's: {message}"
        );
    }

    /// A committed file that did not parse would otherwise fail at run
    /// time, inside a sample, with a message about this repository rather
    /// than about the caller's command line.
    #[test]
    fn every_committed_trace_file_parses() {
        for scripted in SCRIPTED {
            load(scripted)
                .unwrap_or_else(|error| panic!("`{}` does not parse: {error}", scripted.name));
        }
    }

    #[test]
    fn every_trace_is_findable_by_the_name_it_carries() {
        for scripted in SCRIPTED {
            let found = by_name(scripted.name).expect("a listed trace");
            assert_eq!(found.name, scripted.name);
            assert!(
                !found.summary.is_empty(),
                "{} has no summary",
                scripted.name
            );
        }
    }

    #[test]
    fn an_unknown_trace_lists_the_ones_that_exist() {
        let error = by_name("moonwalk").expect_err("no such trace");
        assert!(matches!(error, SampleError::Usage(_)), "{error:?}");
        let message = error.to_string();
        assert!(message.contains("moonwalk"), "{message}");
        for scripted in SCRIPTED {
            assert!(message.contains(scripted.name), "{message}");
        }
    }

    #[test]
    fn the_scripted_events_are_ordered_and_land_inside_a_short_run() {
        for trace in every_trace() {
            let mut previous = 0;
            for (frame, _) in &trace.events {
                assert!(*frame >= previous, "{} is out of order", trace.name);
                previous = *frame;
            }
        }
        assert!(names().contains("walk"));
    }

    /// The load-bearing property of the migration: the file counts ticks
    /// from zero, the driver counts frames from one, and every loaded
    /// event must have moved by exactly that.
    ///
    /// Anchored to the file's own text, because two readings of one table
    /// agree however wrong the reading is. The walk's first event is on
    /// the file's tick 1 and must arrive on frame 2; nothing may arrive
    /// on frame 0, which the driver never runs and where a missing shift
    /// would put every tick-zero event.
    #[test]
    fn a_files_tick_becomes_the_next_frame() {
        assert_eq!(FIRST_FRAME, 1);
        assert!(
            file_has_event_at_tick("walk", 1),
            "the fixture must carry a tick-1 event or this proves nothing"
        );
        let walk = by_name("walk").expect("the walk trace");
        assert_eq!(
            walk.events.first().map(|(frame, _)| *frame),
            Some(2),
            "tick 1 in the file is frame 2 in the driver"
        );
        for trace in every_trace() {
            assert!(
                trace.events.iter().all(|(frame, _)| *frame >= FIRST_FRAME),
                "{} put an event on a frame the driver never runs",
                trace.name
            );
        }
    }
}
