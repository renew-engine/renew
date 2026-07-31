//! The words the format is made of.
//!
//! One spelling of each, in one place. The writer emits these and the
//! reader compares against these, so the two halves of the codec cannot
//! drift apart in the one way that would be hardest to notice: a file that
//! is written correctly, read back correctly, and understood by nothing
//! else.

/// The word every trace begins with — the crate's own name, deliberately.
/// A file that says what wrote it costs eleven bytes and settles an
/// argument.
pub(crate) const MAGIC: &str = "renew-trace";

/// The version this crate writes, and the newest one it reads.
///
/// A reader accepts its own version and every older one.
///
/// **A new caller-owned header key does not move this number.** Those
/// keys were never the codec's to interpret, so a reader meeting one it
/// has never seen keeps it verbatim and reads the file.
///
/// **Everything else in the vocabulary does.** A new event kind, a new
/// line keyword, a new [`TraceKey`](crate::TraceKey) name, a new
/// [`TraceButton`](crate::TraceButton) name — every one of them is a word
/// an existing reader does not know, and this format refuses words it
/// does not know rather than skipping them. So adding one makes every
/// reader already in the world reject the whole file, which is a new
/// format however small the addition looked, and the number has to move
/// with it. The alternative is a reader that blames its own table for a
/// file it should simply have been told it was too old to read.
pub const FORMAT_VERSION: u32 = 0;

/// Fields are separated by exactly one space. Not by whitespace: a tab or
/// a run of spaces is a different file from the one someone meant to
/// write, and a format that shrugs at the difference cannot be compared
/// byte for byte.
pub(crate) const SEPARATOR: char = ' ';

/// A header field is a key, this, and a value. A value may contain one
/// too: the split is at the first, so `extent=640=480` is a value of
/// `640=480` rather than a refusal.
pub(crate) const ASSIGN: char = '=';

/// The header's own fields, in the order they are written.
pub(crate) const SAMPLE: &str = "sample";
pub(crate) const TICKS: &str = "ticks";
pub(crate) const TIMESTEP_NS: &str = "timestep_ns";
pub(crate) const BUDGET: &str = "budget";

/// The four keys the codec owns. Everything else in a header belongs to
/// the caller.
pub(crate) const RESERVED_KEYS: &[&str] = &[SAMPLE, TICKS, TIMESTEP_NS, BUDGET];

/// The keyword every event line begins with.
pub(crate) const EVENT: &str = "e";

/// The nine kinds of event, one per line shape.
pub(crate) const KEY: &str = "key";
pub(crate) const POINTER: &str = "pointer";
pub(crate) const BUTTON: &str = "button";
pub(crate) const WHEEL: &str = "wheel";
pub(crate) const FOCUS: &str = "focus";
pub(crate) const RESIZE: &str = "resize";
pub(crate) const SCALE: &str = "scale";
pub(crate) const REDRAW: &str = "redraw";
pub(crate) const CLOSE: &str = "close";

/// A key or a button is `down` or `up` — never `pressed`, `true` or `1`,
/// which are three ways to say the same thing and therefore two ways too
/// many.
pub(crate) const DOWN: &str = "down";
pub(crate) const UP: &str = "up";

/// Focus arrives and leaves; it is not pressed.
pub(crate) const FOCUS_IN: &str = "in";
pub(crate) const FOCUS_OUT: &str = "out";

/// The operating system's auto-repeat, written only when it is set.
pub(crate) const REPEAT: &str = "repeat";

/// The prefix a native pointer button's index is written behind.
pub(crate) const OTHER_BUTTON: &str = "other:";

/// Every float is a bit pattern behind this prefix, in lowercase, at
/// exactly the width of its type.
pub(crate) const HEX_PREFIX: &str = "0x";

/// The header is line 1 of every trace.
pub(crate) const HEADER_LINE: usize = 1;

/// The first event is line 2, and each event after it is one line later.
/// That is what lets one line number mean the same thing whether a trace
/// was read from text or assembled in memory.
pub(crate) const FIRST_EVENT_LINE: usize = 2;
