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
//! glb    a four-byte magic, so it is certain too
//! gltf   a parse, because a JSON document has no magic at all
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
//! **The glTF document is the one arm that parses.** It has no magic
//! and no keyword: it is JSON, and "starts with `{`" would claim every
//! configuration file ever written. The question asked instead is the
//! one the format answers -- an `asset` object carrying a `version`
//! string, which the specification requires of every document and which
//! nothing else here has. It costs a parse before the reader parses
//! again, and that is the right price: before this arm existed a `.gltf`
//! fell through to the fallback and came back refused as a truncated
//! STL, which is a confident answer about the wrong format.
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
use crate::{Mesh, blob, glb, gltf, mtl, obj, ply, stl};

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
    /// A glTF document, on its own rather than in a container.
    ///
    /// Its geometry travels as payloads embedded in the document, or in
    /// files beside it that this crate does not open -- **this arm is
    /// the format, not the self-contained subset of it**, so a document
    /// naming a second file is detected here and refused by the reader
    /// rather than being detected as something else.
    Gltf,

    /// The binary glTF container.
    ///
    /// **Read for geometry now**, which this arm was not when it was
    /// added. It earned its place before that: until it existed a binary
    /// glTF fell through to the fallback and was refused as a truncated
    /// STL, which is a confident answer about the wrong format — the
    /// same defect the PLY magic check was tightened to cure.
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
            Self::Gltf => "gltf",
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
            // **This arm said the geometry had no reader until it did.**
            // It now reads one, and the refusal it can return carries
            // the whole layered answer: which of the container, the
            // document, an accessor or the geometry was at fault, and
            // that layer's own numbers.
            //
            // A caller wanting the unwrapped answer calls `gltf::read`
            // directly; this arm exists so that a caller who found the
            // format by detection gets geometry the same way it does for
            // every other format here.
            Self::Glb | Self::Gltf => {
                Some(gltf::read(bytes).map_err(|refusal| MeshError::Gltf(Box::new(refusal))))
            }
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
    // **Before the keyword checks and after the magic ones.** A
    // conformant document is JSON, so it cannot be an OBJ or an MTL, and
    // PLY announces itself with a magic word -- so this arm cannot take
    // another format's files, and asking it early keeps a document from
    // reaching the fallback.
    if gltf::looks_like(bytes) {
        return Format::Gltf;
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
