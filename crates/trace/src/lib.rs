//! The input-trace codec: a line-oriented text format for recorded input,
//! a reader that refuses everything it does not understand, and a writer
//! that is its exact inverse.
//!
//! A run of this engine is reproducible from a build, a seed and its
//! input. The build is pinned and the seed is a flag; a trace is what
//! makes the third one a file. With one, a bug can be reproduced from the
//! input that caused it, a play session can become a regression test, one
//! input can be diffed across two builds, and a sample can be driven in
//! automation by input no programmer wrote.
//!
//! ```
//! use renew_trace::{Trace, TraceEvent, TraceHeader, TraceKey, parse, write};
//!
//! let header = TraceHeader::new("input_echo", 10, 16_666_667, 5)?.with_key("seed", "0")?;
//! let trace = Trace::new(
//!     header,
//!     vec![
//!         (5, TraceEvent::Key { code: TraceKey::ArrowRight, pressed: true, repeat: false }),
//!         // A tick equal to the run's length means "after the final
//!         // step", and that is where a terminating event usually lands.
//!         (10, TraceEvent::CloseRequested),
//!     ],
//! )?;
//!
//! let text = write(&trace);
//! assert_eq!(
//!     text,
//!     "renew-trace 0 sample=input_echo ticks=10 timestep_ns=16666667 budget=5 seed=0\n\
//!      e 5 key arrow-right down\n\
//!      e 10 close\n"
//! );
//! assert_eq!(parse(&text), Ok(trace));
//! # Ok::<(), renew_trace::TraceError>(())
//! ```
//!
//! # What a trace reproduces
//!
//! The simulation: the state a run reaches, and the exact interleaving of
//! events with steps. Not how many frames carried those steps, not how
//! many steps were dropped, and not which driver supplied the input —
//! those are facts about the schedule that carried the input rather than
//! about the input.
//!
//! Recording the frame timeline as well was tried and abandoned on a
//! measurement: a sample that presents nothing free-runs at about 19,000
//! frames per simulation tick, so ten seconds of it is roughly 11.5
//! million frame entries — some 81 MB of text, to reproduce a hash of
//! polling noise. Where a frame timeline is affordable it is also
//! redundant, because a headless run executes exactly one step per frame
//! by construction. If schedule reproduction is ever wanted, the format
//! grows an optional timeline section for frame-bounded drivers.
//!
//! # Events are indexed by tick, not by frame
//!
//! A frame may run no steps or several, so *the event on frame 40* is not
//! a point in simulation time. Tick *k* means the event is delivered
//! before the step whose tick is *k*; ticks are 0-based; and a tick equal
//! to the header's `ticks` is legal and means *after the final step*. That
//! last case is the common one, not an edge case — with thousands of
//! frames per tick, a terminating event almost always arrives during a
//! frame that runs no step at all.
//!
//! This rests on a property the simulation must have: its state may depend
//! only on the tick index and the events delivered before that tick —
//! never on frame boundaries, on the accumulator's remainder, or on
//! whether a step was dropped. A simulation that reacted to a stall would
//! not be reproducible from a tick index by anything.
//!
//! # Contract
//!
//! - **No input is trusted.** Every rule rejects rather than repairs, and
//!   nothing is skipped: an unknown keyword is an error, because skipping
//!   is how a format silently forks. Every refusal names the line it is
//!   about and what was expected there.
//! - **No file is ever opened.** The reader takes text and the writer
//!   returns it. That is what makes the codec testable and fuzzable with
//!   no filesystem, and it puts the bound on how much untrusted data is
//!   held where it can be enforced — at the seam that reads, which can
//!   refuse an oversized file before a byte reaches a parser. The crate's
//!   lint configuration bans the filesystem calls, the file handles and
//!   the path types, which makes the rule hard to break by accident; it
//!   is a tripwire rather than a proof, and it is written down as one.
//! - **Two things the caller must check, because this crate cannot.**
//!   Invalid UTF-8 never arrives here, so the caller has to read with
//!   something that refuses it rather than something that replaces bad
//!   bytes — lossy repair before the parser sees the file is still lossy
//!   repair. And `timestep_ns` or `budget` of zero parse quite happily,
//!   because the codec does not interpret the schedule it is storing; the
//!   caller turning a header into a real schedule is the one that has to
//!   refuse a zero, and it will have to, because the types that carry
//!   those two numbers cannot hold one.
//! - **Writing and reading are inverses.** [`parse`] of [`write()`] is the
//!   trace it started from, for every trace that can be built — no
//!   exceptions, no configurations, nothing lossy. The reverse holds for
//!   every file the writer could have produced: a hand-written file may
//!   spell a number with leading zeros, and writing it back out spells it
//!   canonically.
//! - **The codec interprets nothing it does not own.** It knows four
//!   header fields. `sample`, any caller key, and what the events *mean*
//!   are the caller's: it preserves them verbatim and checks nothing but
//!   uniqueness. A codec that guessed what a sample name implies would be
//!   wrong differently for every caller.
//! - **Order is never repaired.** Ticks must not decrease, but equal ticks
//!   are allowed and their recorded order is part of the trace: two keys
//!   going down on one tick were seen in one order, and sorting or
//!   deduplicating them would change the input while looking like tidying.
//! - **Nothing here allocates on a hot path, logs, reads a clock, or
//!   spawns anything.** A trace is tens of lines; the codec is a pure
//!   function of its input.
//!
//! # Why text
//!
//! Diffable in version control, hand-writable in a test, readable in a
//! build log, and parseable with no dependency. Binary wins only on
//! compactness, which — with no frame timeline in the file — is a
//! difference between a small file and a slightly smaller one.
//!
//! Every field is an integer or a keyword. Floats are written as their
//! IEEE-754 bit patterns in hexadecimal, so no decimal float is ever
//! parsed and every value round-trips exactly: a float value has two zeros
//! and no equality for `NaN`, while a bit pattern is an integer and
//! compares exactly. Non-finite patterns cannot be written and are refused
//! on read.
//!
//! # The grammar
//!
//! ```text
//! renew-trace <version> sample=<name> ticks=<u64> timestep_ns=<u64> budget=<u32> [key=value…]
//! e <tick> key <name> <down|up> [repeat]
//! e <tick> pointer <hex-f64> <hex-f64>
//! e <tick> button <name|other:<u16>> <down|up>
//! e <tick> wheel <hex-f32> <hex-f32>
//! e <tick> focus <in|out>
//! e <tick> resize <u32> <u32>
//! e <tick> scale <hex-f64>
//! e <tick> redraw
//! e <tick> close
//! ```
//!
//! The version is positional and first, because a reader has to know how
//! to read the rest of a line before it reads it. A reader accepts its own
//! version and every older one. Fields are separated by exactly one space.
//! Numbers are ASCII digits with no sign and no underscores. Bit patterns
//! are `0x` and exactly the width of their type, in lowercase. A trailing
//! carriage return is stripped; a byte order mark is an error that says
//! so, because it is invisible on screen and every other message would
//! blame the wrong thing.
//!
//! # Extension points
//!
//! None. No trait, no `dyn`, no runtime polymorphism. The event vocabulary
//! is this crate's own rather than the windowing layer's, which is what
//! lets the crate depend on nothing: a codec naming another crate's event
//! enum would pull that crate's whole windowing stack into every build
//! that merely reads a file, and would make the meaning of an
//! already-written file hostage to that enum's growth. The conversion
//! between the two vocabularies lives in the application that owns both.

// This crate is a codec; it does not print, and it does not do arithmetic
// on floats — every float it handles is an integer bit pattern from the
// moment it arrives to the moment it leaves.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::float_arithmetic)]

mod error;
mod event;
mod grammar;
mod parse;
mod trace;
mod write;

pub use error::{TraceError, TraceErrorKind};
pub use event::{FiniteF32, FiniteF64, TraceButton, TraceEvent, TraceKey};
pub use grammar::FORMAT_VERSION;
pub use parse::parse;
pub use trace::{Trace, TraceHeader};
pub use write::write;
