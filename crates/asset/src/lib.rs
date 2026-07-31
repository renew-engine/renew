//! The runtime asset pack: a content-addressed container.
//!
//! A pack is a single file holding many named blobs, each with a digest
//! of its own contents. [`PackBuilder`] writes one; [`Pack`] reads one.
//!
//! # Contract
//!
//! - **The same inputs produce byte-identical output.** Entries are
//!   sorted by name before writing, so the order they were added in — and
//!   therefore the order a directory happened to be walked in — never
//!   reaches the bytes. Nothing here reads a clock, a path, or the
//!   environment.
//! - **Names are unique and sorted.** Both are enforced when writing and
//!   re-checked when reading, because a reader must not trust that the
//!   file in front of it was produced by this writer.
//! - **A pack is read, never trusted.** Every length is validated against
//!   the bytes actually present before it is used; the four regions must
//!   account for the file exactly, so both truncation and appended bytes
//!   are refused. A malformed pack costs no allocation: everything
//!   [`Pack`] returns borrows the caller's buffer.
//! - **The crate never touches the filesystem.** It takes bytes and
//!   returns bytes. The size bound on reading an untrusted file belongs
//!   at the seam that can refuse an oversized one, which is the caller.
//!
//! # What this is not
//!
//! Not compression, not encryption, not streaming, and **not an
//! importer**. There is no PNG decoder here and no audio decoder: those
//! need dependencies that are the owner's to approve, and a subcommand
//! that only copied bytes would be worse than its absence. v0 is the
//! container, which is the part that has to be right before anything is
//! stored in it.
//!
//! The digest is FNV-1a-64 — fast, dependency-free, and **not
//! collision-resistant**. It answers "are these the same bytes", which is
//! what change detection and deduplication need. It is not integrity
//! against someone choosing the bytes.
//!
//! # Example
//!
//! ```
//! use renew_asset::{Pack, PackBuilder};
//!
//! let mut builder = PackBuilder::new();
//! builder.insert("shader/triangle", b"spv...")?;
//! builder.insert("mesh/hero", b"verts...")?;
//! let bytes = builder.finish()?;
//!
//! let pack = Pack::read(&bytes)?;
//! assert_eq!(pack.len(), 2);
//! // Sorted, whatever order they went in.
//! assert_eq!(pack.entries().next().map(|e| e.name), Some("mesh/hero"));
//! assert!(pack.mismatched().is_empty());
//! # Ok::<(), Box<dyn core::error::Error>>(())
//! ```

// A container format writes bytes for a caller and returns refusals as
// values; anything it printed would reach a stream it does not own.
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod error;
mod hash;
mod layout;
mod read;
mod write;

pub use error::{BuildError, PackError};
pub use hash::fnv1a64;
pub use layout::{FORMAT, MAX_NAME_BYTES};
pub use read::{EntryRef, Pack};
pub use write::PackBuilder;
