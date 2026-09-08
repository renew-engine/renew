//! Which format a byte string is, and reading it without knowing.
//!
//! Every reader here answers for the format it owns. This module answers
//! the question that comes before that one — **which reader** — so a
//! caller holding bytes from somewhere does not have to decide, and so
//! the decision is written down once instead of once per caller.
//!
//! # The order is the substance, and it is not arbitrary
//!
//! ```text
//! blob   an eight-byte magic, so it is certain
//! ply    a magic word, so it is nearly certain
//! obj    keywords only it uses, so it is a good guess
//! mtl    the same, and checked here so a material library is not
//!        reported as a broken mesh
//! stl    everything left
//! ```
//!
//! **STL is last because it cannot answer for itself.** The format has
//! no magic number, so "these are not STL bytes" and "these are STL
//! bytes cut short" are the same observation — which is why the
//! `NotThisFormat` refusal that PLY and the blob make has no STL
//! counterpart, and why that reader's refusal census says so rather
//! than leaving the gap to be noticed. Its own dispatch between
//! its two dialects is arithmetic for the same reason. Putting it last
//! makes it the fallback rather than a competitor, and means a truncated
//! STL still reaches the reader whose refusals describe it.
//!
//! **MTL is checked before that fallback and not after.** A material
//! library would otherwise reach the STL reader and be refused as a
//! truncated mesh: true, and no help to whoever pointed a tool at the
//! wrong half of an export.
//!
//! # Why this is here rather than in whatever is asking
//!
//! The order above is a fact about *these formats*, not about any one
//! caller. A tool, a loader and an editor would each otherwise
//! rediscover it, and the first of them to get it wrong would be wrong
//! quietly. Keeping it beside the readers also means a change to what a
//! reader accepts is a change this module's own tests see.

use crate::error::MeshError;
use crate::{Mesh, blob, glb, mtl, obj, ply, stl};

/// A format this crate can identify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Wavefront OBJ geometry.
    Obj,
    /// A Wavefront material library, which carries no geometry.
    Mtl,
    /// Stereolithography, either dialect.
    Stl,
    /// Polygon File Format, any of its three encodings.
    Ply,
    /// This crate's own canonical form.
    Blob,
    /// The binary glTF container.
    ///
    /// Identified here and **not yet read for geometry**. That is worth
    /// the arm on its own: until this existed, a binary glTF fell
    /// through to the fallback and was refused as a truncated STL, which
    /// is a confident answer about the wrong format — the same defect
    /// the PLY magic check was tightened to cure.
    Glb,
}

impl Format {
    /// The name, lowercase, stable, and safe for a machine to key on.
    ///
    /// **A name is for a program.** Prose about a format is free to
    /// improve; these are part of the surface.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Obj => "obj",
            Self::Mtl => "mtl",
            Self::Stl => "stl",
            Self::Ply => "ply",
            Self::Blob => "blob",
            Self::Glb => "glb",
        }
    }

    /// Whether this format carries geometry at all.
    ///
    /// A material library does not, and that is a fact about the format
    /// rather than a failure of a file.
    #[must_use]
    pub const fn carries_geometry(self) -> bool {
        !matches!(self, Self::Mtl)
    }

    /// Read this format's geometry, or `None` if it carries none.
    ///
    /// `None` is not an error: it is the answer for a material library,
    /// and a caller that wanted geometry knows what to say about it far
    /// better than this crate does.
    #[must_use]
    pub fn read(self, bytes: &[u8]) -> Option<Result<Mesh, MeshError>> {
        match self {
            Self::Obj => Some(obj::read(bytes)),
            Self::Stl => Some(stl::read(bytes)),
            Self::Ply => Some(ply::read(bytes)),
            Self::Blob => Some(blob::read(bytes)),
            // **The same answer whatever the container says, and that is
            // deliberate.** There is no reader here for the geometry
            // inside a binary glTF, which is true of a well-formed one
            // and a corrupt one alike, so validating the framing first
            // would only let this arm report a fault it is not in a
            // position to do anything about.
            //
            // The first draft did validate, and returned "not this
            // format" when the framing was wrong — about a file whose
            // magic had just matched, which is how it reached this arm.
            // A caller that wants the container's own refusals, with the
            // chunk and the numbers, calls `glb::read`, which is where
            // they live.
            //
            // `Unsupported` is the refusal for a file this crate cannot
            // turn into geometry though nothing is wrong with it, which
            // is exactly the case until a glTF reader exists.
            Self::Glb => Some(Err(MeshError::Unsupported {
                wanted: "a reader for the geometry inside a binary glTF",
            })),
            Self::Mtl => None,
        }
    }
}

/// Which format these bytes are.
///
/// Always answers. There is no "unknown", because [`Format::Stl`] is the
/// fallback and the STL reader's refusals are the ones that describe a
/// byte string that is not any of these — see this module's own
/// documentation for why that is the honest arrangement rather than a
/// shortcut.
#[must_use]
pub fn detect(bytes: &[u8]) -> Format {
    if bytes.starts_with(&blob::MAGIC) {
        return Format::Blob;
    }
    // Two whole magic numbers, four bytes each and eight, that cannot
    // collide: `RENEWMS\0` and `glTF`. The order between them is free,
    // and both come before anything decided by a keyword.
    if glb::looks_like(bytes) {
        return Format::Glb;
    }
    if ply::looks_like(bytes) {
        return Format::Ply;
    }
    if obj::looks_like(bytes) {
        return Format::Obj;
    }
    if mtl::looks_like(bytes) {
        return Format::Mtl;
    }
    Format::Stl
}
